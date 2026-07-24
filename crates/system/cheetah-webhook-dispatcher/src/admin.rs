//! Webhook profile administration provider.
//!
//! Profiles are persisted through the `DatabaseApi` so they survive module
//! restarts. The store also supports a synthetic `test_profile` call that
//! performs DNS, connect, HTTP and signature checks against a target URL.
//!
//! Webhook 管理 provider。配置通过 `DatabaseApi` 持久化，模块重启后可恢复。
//! 同时支持 `test_profile` 调用来对目标 URL 执行 DNS、连接、HTTP 和签名检查。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::ids::RequestId;
use cheetah_media_api::port::{MediaRequestContext, WebhookAdminApi};
use cheetah_media_api::webhook::{
    CreateWebhookProfileRequest, UpdateWebhookProfileRequest, WebhookProfile, WebhookProfileId,
    WebhookTest, WebhookTestReport,
};
use cheetah_sdk::DatabaseApi;
use tracing::{debug, warn};

use crate::security::{WebhookUrlPolicy, WebhookUrlVerdict};
use crate::sender::{WebhookHttpRequest, WebhookSendError, WebhookSender};
use crate::util;

const PROFILE_KEY_PREFIX: &str = "webhook:profile:";

/// Persistent store and administrative API for webhook profiles.
pub struct WebhookAdminStore {
    db: Arc<dyn DatabaseApi>,
    sender: Arc<dyn WebhookSender>,
    url_policy: WebhookUrlPolicy,
}

impl WebhookAdminStore {
    pub fn new(
        db: Arc<dyn DatabaseApi>,
        sender: Arc<dyn WebhookSender>,
        url_policy: WebhookUrlPolicy,
    ) -> Self {
        Self {
            db,
            sender,
            url_policy,
        }
    }

    fn profile_key(&self, id: &WebhookProfileId) -> String {
        format!("{}{}", PROFILE_KEY_PREFIX, id.0)
    }

    fn load(&self, id: &WebhookProfileId) -> Result<WebhookProfile, MediaError> {
        let key = self.profile_key(id);
        let bytes = self
            .db
            .get(&key)
            .map_err(|e| MediaError::storage_failed(format!("failed to read profile: {e}")))?
            .ok_or_else(|| MediaError::new(MediaErrorCode::NotFound, "profile not found"))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| MediaError::storage_failed(format!("corrupt profile: {e}")))
    }

    fn save(&self, profile: &WebhookProfile) -> Result<(), MediaError> {
        let key = self.profile_key(&profile.id);
        let bytes = serde_json::to_vec(profile)
            .map_err(|e| MediaError::storage_failed(format!("failed to serialize profile: {e}")))?;
        self.db
            .put(&key, &bytes)
            .map_err(|e| MediaError::storage_failed(format!("failed to write profile: {e}")))
    }

    fn delete_key(&self, id: &WebhookProfileId) -> Result<(), MediaError> {
        let key = self.profile_key(id);
        self.db
            .delete(&key)
            .map_err(|e| MediaError::storage_failed(format!("failed to delete profile: {e}")))
    }
}

#[async_trait]
impl WebhookAdminApi for WebhookAdminStore {
    async fn create_profile(
        &self,
        _ctx: &MediaRequestContext,
        mut request: CreateWebhookProfileRequest,
    ) -> Result<WebhookProfile, MediaError> {
        let id = request
            .id
            .take()
            .ok_or_else(|| MediaError::invalid_argument("profile id is required for creation"))?;

        if self.load(&id).is_ok() {
            return Err(MediaError::new(
                MediaErrorCode::AlreadyExists,
                "profile already exists",
            ));
        }

        let mut profile = request.into_profile(id);
        profile.generation = 1;
        self.save(&profile)?;
        debug!(profile_id = %profile.id.0, "created webhook profile");
        Ok(profile)
    }

    async fn get_profile(
        &self,
        _ctx: &MediaRequestContext,
        id: &WebhookProfileId,
    ) -> Result<WebhookProfile, MediaError> {
        self.load(id)
    }

    async fn list_profiles(
        &self,
        _ctx: &MediaRequestContext,
    ) -> Result<Vec<WebhookProfile>, MediaError> {
        let keys = self
            .db
            .list_prefix(PROFILE_KEY_PREFIX)
            .map_err(|e| MediaError::storage_failed(format!("failed to list profiles: {e}")))?;
        let mut profiles = Vec::new();
        for key in keys {
            if let Some(bytes) = self
                .db
                .get(&key)
                .map_err(|e| MediaError::storage_failed(format!("failed to read profile: {e}")))?
            {
                if let Ok(profile) = serde_json::from_slice::<WebhookProfile>(&bytes) {
                    profiles.push(profile);
                } else {
                    warn!(key = %key, "skipping corrupt webhook profile");
                }
            }
        }
        profiles.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        Ok(profiles)
    }

    async fn update_profile(
        &self,
        _ctx: &MediaRequestContext,
        mut request: UpdateWebhookProfileRequest,
    ) -> Result<WebhookProfile, MediaError> {
        let stored = self.load(&request.profile.id)?;
        if stored.generation != request.expected_generation {
            return Err(MediaError::new(
                MediaErrorCode::Conflict,
                "profile generation mismatch",
            ));
        }

        // Preserve the existing secret when the caller does not supply a new one.
        if request.profile.secret.is_empty() {
            request.profile.secret = stored.secret;
        }

        let mut profile = request.profile;
        profile.generation = stored.generation + 1;
        self.save(&profile)?;
        debug!(profile_id = %profile.id.0, generation = profile.generation, "updated webhook profile");
        Ok(profile)
    }

    async fn delete_profile(
        &self,
        _ctx: &MediaRequestContext,
        id: &WebhookProfileId,
    ) -> Result<(), MediaError> {
        // Ensure the profile exists before deleting.
        let _ = self.load(id)?;
        self.delete_key(id)?;
        debug!(profile_id = %id.0, "deleted webhook profile");
        Ok(())
    }

    async fn test_profile(
        &self,
        ctx: &MediaRequestContext,
        id: &WebhookProfileId,
    ) -> Result<WebhookTestReport, MediaError> {
        let profile = self.load(id)?;
        if !profile.enabled {
            return Err(MediaError::new(
                MediaErrorCode::Unavailable,
                "profile is disabled",
            ));
        }

        run_webhook_test(
            ctx,
            &profile.target_url,
            &profile.secret,
            Duration::from_millis(profile.timeout_ms),
            &self.url_policy,
            &self.sender,
        )
        .await
    }
}

/// Run a synthetic webhook test against the given target and return a summary.
async fn run_webhook_test(
    ctx: &MediaRequestContext,
    target_url: &str,
    secret: &str,
    timeout: Duration,
    policy: &WebhookUrlPolicy,
    sender: &Arc<dyn WebhookSender>,
) -> Result<WebhookTestReport, MediaError> {
    let start = Instant::now();
    let mut report = WebhookTestReport {
        dns_resolved: false,
        connected: false,
        http_status: None,
        body_valid: None,
        signature_valid: None,
        latency_ms: 0,
        error: None,
    };

    let (addr, parsed) = match policy.evaluate(target_url) {
        Ok(WebhookUrlVerdict::Allow(addr, parsed)) => {
            report.dns_resolved = true;
            (addr, parsed)
        }
        Ok(WebhookUrlVerdict::Deny(reason)) => {
            report.error = Some(reason);
            report.latency_ms = start.elapsed().as_millis() as u64;
            return Ok(report);
        }
        Err(e) => {
            report.error = Some(e.to_string());
            report.latency_ms = start.elapsed().as_millis() as u64;
            return Ok(report);
        }
    };

    let test = WebhookTest {
        event_id: RequestId(format!("test-{}", ctx.request_id.0)),
        kind: "WebhookTest".to_string(),
        media_key: cheetah_media_api::ids::MediaKey::with_default_vhost("test", "test", None)
            .expect("default media key is valid"),
        payload: "webhook test payload".to_string(),
    };

    let body = serde_json::to_vec(&test)
        .map_err(|e| MediaError::storage_failed(format!("failed to serialize test: {e}")))?;
    let mut headers = crate::util::webhook_headers(&test.event_id.0);

    if !secret.is_empty() {
        match util::sign_body(&body, secret) {
            Ok(sig) => {
                headers.insert("X-Webhook-Signature".to_string(), sig);
            }
            Err(e) => {
                warn!(err = %e, "failed to sign test webhook body");
            }
        }
    }

    let request = WebhookHttpRequest {
        verdict: WebhookUrlVerdict::Allow(addr, parsed),
        headers,
        body,
        timeout,
    };

    match sender.send(request).await {
        Ok(response) => {
            report.connected = true;
            report.http_status = Some(response.status);
            report.body_valid = Some(util::is_success(response.status));
            report.signature_valid = None;
            report.latency_ms = response.duration_ms;
        }
        Err(WebhookSendError::Io(e)) => {
            report.connected = e.kind() != std::io::ErrorKind::NotConnected
                && e.kind() != std::io::ErrorKind::ConnectionRefused;
            report.error = Some(e.to_string());
            report.latency_ms = start.elapsed().as_millis() as u64;
        }
        Err(e) => {
            report.error = Some(e.to_string());
            report.latency_ms = start.elapsed().as_millis() as u64;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io;
    use std::sync::{Arc, Mutex};

    use cheetah_media_api::error::MediaErrorCode;
    use cheetah_media_api::port::{MediaRequestContext, WebhookAdminApi};
    use cheetah_media_api::webhook::{
        CreateWebhookProfileRequest, UpdateWebhookProfileRequest, WebhookFailurePolicy,
        WebhookProfile, WebhookProfileId, WebhookProfileMode, WebhookTestReport,
    };
    use cheetah_sdk::{DatabaseApi, SdkError};

    use crate::admin::WebhookAdminStore;
    use crate::security::WebhookUrlPolicy;
    use crate::sender::{WebhookHttpRequest, WebhookResponse, WebhookSendError, WebhookSender};

    struct FakeDatabase {
        entries: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl FakeDatabase {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::new(HashMap::new()),
            })
        }
    }

    impl DatabaseApi for FakeDatabase {
        fn put(&self, key: &str, value: &[u8]) -> Result<(), SdkError> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        }

        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SdkError> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        fn delete(&self, key: &str) -> Result<(), SdkError> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }

        fn list_prefix(&self, prefix: &str) -> Result<Vec<String>, SdkError> {
            let guard = self.entries.lock().unwrap();
            let mut keys: Vec<String> = guard
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            keys.sort();
            Ok(keys)
        }
    }

    struct FakeSender {
        requests: Mutex<Vec<WebhookHttpRequest>>,
        response: Mutex<WebhookResponse>,
        error: Mutex<Option<WebhookSendError>>,
    }

    impl FakeSender {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(WebhookResponse {
                    status: 200,
                    body: "ok".to_string(),
                    duration_ms: 5,
                }),
                error: Mutex::new(None),
            })
        }

        fn err(e: WebhookSendError) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(WebhookResponse {
                    status: 200,
                    body: "ok".to_string(),
                    duration_ms: 0,
                }),
                error: Mutex::new(Some(e)),
            })
        }

        fn last_request(&self) -> Option<WebhookHttpRequest> {
            self.requests.lock().unwrap().last().cloned()
        }
    }

    #[async_trait::async_trait]
    impl WebhookSender for FakeSender {
        async fn send(
            &self,
            request: WebhookHttpRequest,
        ) -> Result<WebhookResponse, WebhookSendError> {
            self.requests.lock().unwrap().push(request);
            if let Some(err) = self.error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(self.response.lock().unwrap().clone())
        }
    }

    fn test_policy() -> WebhookUrlPolicy {
        WebhookUrlPolicy {
            block_private: false,
            allowed_cidrs: vec!["127.0.0.0/8".parse().unwrap()],
            ..Default::default()
        }
    }

    fn store(db: Arc<dyn DatabaseApi>) -> WebhookAdminStore {
        WebhookAdminStore::new(db, FakeSender::ok(), test_policy())
    }

    fn profile_request(id: &str) -> CreateWebhookProfileRequest {
        CreateWebhookProfileRequest {
            id: Some(WebhookProfileId(id.to_string())),
            enabled: true,
            mode: WebhookProfileMode::NativeDomain,
            target_url: "http://127.0.0.1:9999/hook".to_string(),
            event_filter: vec!["on_play".to_string()],
            admission_actions: vec![],
            failure_policy: WebhookFailurePolicy::FailClosed,
            timeout_ms: 1000,
            secret: "s3cret".to_string(),
        }
    }

    fn ctx() -> MediaRequestContext {
        MediaRequestContext {
            request_id: cheetah_media_api::ids::RequestId("req-1".to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn create_and_get_profile_round_trip() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();
        assert_eq!(created.id.0, "hook-1");
        assert_eq!(created.generation, 1);
        assert_eq!(created.secret, "s3cret");

        let view = created.view();
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("s3cret"), "view must not expose secret");

        let fetched = api.get_profile(&ctx, &created.id).await.unwrap();
        assert_eq!(fetched.secret, "s3cret");
    }

    #[tokio::test]
    async fn list_profiles_returns_sorted_views() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        api.create_profile(&ctx, profile_request("beta"))
            .await
            .unwrap();
        api.create_profile(&ctx, profile_request("alpha"))
            .await
            .unwrap();

        let profiles = api.list_profiles(&ctx).await.unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id.0, "alpha");
        assert_eq!(profiles[1].id.0, "beta");
    }

    #[tokio::test]
    async fn update_with_expected_generation_advances_and_preserves_secret() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();
        let mut profile: WebhookProfile = created.clone();
        profile.target_url = "http://127.0.0.1:9999/hook2".to_string();
        profile.secret = String::new();

        let updated = api
            .update_profile(
                &ctx,
                UpdateWebhookProfileRequest {
                    profile,
                    expected_generation: created.generation,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.generation, 2);
        assert_eq!(updated.target_url, "http://127.0.0.1:9999/hook2");
        assert_eq!(updated.secret, "s3cret");

        let update_conflict = UpdateWebhookProfileRequest {
            profile: updated.clone(),
            expected_generation: 1,
        };
        let err = api.update_profile(&ctx, update_conflict).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Conflict);
    }

    #[tokio::test]
    async fn delete_profile_removes_it() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();
        api.delete_profile(&ctx, &created.id).await.unwrap();

        let err = api.get_profile(&ctx, &created.id).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::NotFound);
    }

    #[tokio::test]
    async fn persistence_survives_store_recreation() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();

        let api2 = WebhookAdminStore::new(db, FakeSender::ok(), test_policy());
        let fetched = api2.get_profile(&ctx, &created.id).await.unwrap();
        assert_eq!(fetched.id.0, "hook-1");
        assert_eq!(fetched.secret, "s3cret");
    }

    #[tokio::test]
    async fn test_profile_sends_signed_request_and_reports_success() {
        let sender = FakeSender::ok();
        let db = FakeDatabase::new();
        let api = WebhookAdminStore::new(db, sender.clone(), test_policy());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();
        let report: WebhookTestReport = api.test_profile(&ctx, &created.id).await.unwrap();

        assert!(report.dns_resolved);
        assert!(report.connected);
        assert_eq!(report.http_status, Some(200));
        assert_eq!(report.body_valid, Some(true));

        let req = sender.last_request().expect("sender received request");
        let sig = req
            .headers
            .get("X-Webhook-Signature")
            .expect("signature header present");
        assert!(!sig.is_empty());
    }

    #[tokio::test]
    async fn test_profile_disabled_is_rejected() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let mut req = profile_request("hook-1");
        req.enabled = false;
        let created = api.create_profile(&ctx, req).await.unwrap();

        let err = api.test_profile(&ctx, &created.id).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unavailable);
    }

    #[tokio::test]
    async fn test_profile_connect_error_is_summarized() {
        let sender = FakeSender::err(WebhookSendError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "refused",
        )));
        let db = FakeDatabase::new();
        let api = WebhookAdminStore::new(db, sender, test_policy());
        let ctx = ctx();

        let created = api
            .create_profile(&ctx, profile_request("hook-1"))
            .await
            .unwrap();
        let report = api.test_profile(&ctx, &created.id).await.unwrap();

        assert!(report.dns_resolved);
        assert!(!report.connected);
        assert!(report.error.is_some());
    }

    #[tokio::test]
    async fn duplicate_create_fails() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        api.create_profile(&ctx, profile_request("dup"))
            .await
            .unwrap();
        let err = api
            .create_profile(&ctx, profile_request("dup"))
            .await
            .unwrap_err();
        assert_eq!(err.code, MediaErrorCode::AlreadyExists);
    }

    #[tokio::test]
    async fn create_without_id_is_rejected() {
        let db = FakeDatabase::new();
        let api = store(db.clone());
        let ctx = ctx();

        let mut req = profile_request("hook-1");
        req.id = None;
        let err = api.create_profile(&ctx, req).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    }
}
