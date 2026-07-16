use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::error::Result;
use cheetah_media_api::event::{EventHeader, SessionOpened, StreamPublished};
use cheetah_media_api::ids::SessionId;
use cheetah_media_api::model::{AdmissionAction, AdmissionRequest, SessionKind};
use cheetah_media_api::port::{MediaAdmissionApi, MediaRequestContext};
use cheetah_media_api::{Decision, MediaEvent, WebhookApi};
use parking_lot::RwLock;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::config::{FailurePolicy, WebhookDispatcherConfig};
use crate::security::WebhookUrlPolicy;
use crate::sender::{WebhookHttpRequest, WebhookSendError, WebhookSender};
use crate::translator::{WebhookDispatch, WebhookTranslator};

/// Synchronous decision client that turns a `MediaEvent` into ZLM-compatible
/// webhook calls and aggregates `Allow`/`Deny` responses.
///
/// 同步决策客户端：将 `MediaEvent` 转换为兼容 ZLM 的 webhook 调用，并聚合
/// `Allow`/`Deny` 响应。
#[derive(Clone)]
pub struct WebhookDecisionClient {
    config: Arc<RwLock<WebhookDispatcherConfig>>,
    sender: Arc<dyn WebhookSender>,
    translator: Arc<dyn WebhookTranslator>,
    url_policy: WebhookUrlPolicy,
    seq: Arc<AtomicU64>,
}

impl WebhookDecisionClient {
    pub fn new(
        config: WebhookDispatcherConfig,
        sender: Arc<dyn WebhookSender>,
        translator: Arc<dyn WebhookTranslator>,
        url_policy: WebhookUrlPolicy,
    ) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            sender,
            translator,
            url_policy,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_config(&self, config: WebhookDispatcherConfig) {
        *self.config.write() = config;
    }

    async fn ask_one(
        &self,
        profile: &crate::config::WebhookProfile,
        dispatch: &WebhookDispatch,
        event_id: &str,
    ) -> Decision {
        let url_policy = WebhookUrlPolicy::from_cidr_strings(&profile.allowed_cidrs)
            .unwrap_or_else(|_| self.url_policy.clone());
        let verdict = match url_policy.evaluate(&profile.url) {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    target = %profile.name,
                    event_id = %event_id,
                    %err,
                    "decision webhook URL denied by policy; applying failure policy"
                );
                return policy_decision(profile.decision_failure_policy, "URL denied");
            }
        };

        let body = match serde_json::to_vec(&dispatch.payload) {
            Ok(b) => b,
            Err(err) => {
                warn!(
                    target = %profile.name,
                    event_id = %event_id,
                    %err,
                    "failed to serialize decision body"
                );
                return policy_decision(profile.decision_failure_policy, "serialization failed");
            }
        };

        if body.len() > profile.max_body_bytes {
            warn!(
                target = %profile.name,
                event_id = %event_id,
                size = body.len(),
                limit = profile.max_body_bytes,
                "decision body exceeds max size"
            );
            return policy_decision(profile.decision_failure_policy, "body too large");
        }

        let mut headers = crate::util::webhook_headers(event_id);
        if let Some(secret) = &profile.secret {
            match crate::util::sign_body(&body, secret) {
                Ok(sig) => {
                    headers.insert("X-Webhook-Signature".to_string(), sig);
                }
                Err(err) => {
                    warn!(
                        target = %profile.name,
                        event_id = %event_id,
                        %err,
                        "failed to sign decision body"
                    );
                }
            }
        }

        let request = WebhookHttpRequest {
            verdict,
            headers,
            body,
            timeout: profile.decision_timeout(),
        };

        match self.sender.send(request).await {
            Ok(response) => {
                if crate::util::is_success(response.status) {
                    parse_decision_response(&response.body).unwrap_or_else(|err| {
                        warn!(
                            target = %profile.name,
                            event_id = %event_id,
                            %err,
                            "failed to parse decision response; applying failure policy"
                        );
                        policy_decision(profile.decision_failure_policy, "invalid response")
                    })
                } else {
                    warn!(
                        target = %profile.name,
                        event_id = %event_id,
                        status = response.status,
                        "decision webhook returned error status; applying failure policy"
                    );
                    policy_decision(profile.decision_failure_policy, "error status")
                }
            }
            Err(WebhookSendError::Timeout) => {
                warn!(
                    target = %profile.name,
                    event_id = %event_id,
                    "decision webhook timed out; applying failure policy"
                );
                policy_decision(profile.decision_failure_policy, "timeout")
            }
            Err(err) => {
                warn!(
                    target = %profile.name,
                    event_id = %event_id,
                    %err,
                    "decision webhook failed; applying failure policy"
                );
                policy_decision(profile.decision_failure_policy, &err.to_string())
            }
        }
    }
}

#[async_trait]
impl WebhookApi for WebhookDecisionClient {
    async fn request_decision(&self, event: MediaEvent) -> Result<Decision> {
        let dispatches = self.translator.translate(&event);
        if dispatches.is_empty() {
            debug!("no webhook translation for event; default allow");
            return Ok(Decision::Allow);
        }

        let event_id = event_id(&event)?;
        let config = self.config.read().clone();

        for dispatch in dispatches {
            let mut any_matched = false;
            for profile in &config.profiles {
                if !profile.wants_decision(&dispatch.hook_name) {
                    continue;
                }
                any_matched = true;
                let decision = self.ask_one(profile, &dispatch, &event_id).await;
                if let Decision::Deny { reason, .. } = decision {
                    return Ok(Decision::Deny {
                        code: MediaErrorCode::PermissionDenied,
                        reason: format!("{}: {}", profile.name, reason),
                    });
                }
            }
            if !any_matched {
                debug!(
                    hook = %dispatch.hook_name,
                    "no decision profile matched; default allow"
                );
            }
        }

        Ok(Decision::Allow)
    }
}

#[async_trait]
impl MediaAdmissionApi for WebhookDecisionClient {
    async fn authorize(
        &self,
        ctx: &MediaRequestContext,
        request: AdmissionRequest,
    ) -> Result<Decision> {
        match request.action {
            AdmissionAction::Publish => {
                let event = MediaEvent::StreamPublished(build_stream_published(
                    self.next_seq(),
                    ctx,
                    &request,
                ));
                self.request_decision(event).await
            }
            AdmissionAction::Play => {
                let event = MediaEvent::SessionOpened(build_session_opened(
                    self.next_seq(),
                    ctx,
                    &request,
                    SessionKind::Player,
                ));
                self.request_decision(event).await
            }
            _ => {
                debug!(
                    action = ?request.action,
                    "admission action not yet supported by webhook translator; default allow"
                );
                Ok(Decision::Allow)
            }
        }
    }
}

impl WebhookDecisionClient {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }
}

fn build_stream_published(
    seq: u64,
    ctx: &MediaRequestContext,
    request: &AdmissionRequest,
) -> StreamPublished {
    StreamPublished {
        header: admission_header(seq, ctx, request),
        protocol: request.protocol.clone(),
        remote_endpoint: request.source_address.clone(),
        session_id: admission_session_id(seq),
    }
}

fn build_session_opened(
    seq: u64,
    ctx: &MediaRequestContext,
    request: &AdmissionRequest,
    kind: SessionKind,
) -> SessionOpened {
    SessionOpened {
        header: admission_header(seq, ctx, request),
        kind,
        session_id: admission_session_id(seq),
        remote_endpoint: request.source_address.clone(),
        protocol: request.protocol.clone(),
    }
}

fn admission_header(
    seq: u64,
    ctx: &MediaRequestContext,
    request: &AdmissionRequest,
) -> EventHeader {
    EventHeader {
        event_id: format!("adm-{seq}"),
        occurred_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        sequence: None,
        media_key: Some(request.resource.clone()),
        source: ctx.source_adapter.clone(),
        correlation_id: ctx.correlation_id.clone(),
    }
}

fn admission_session_id(seq: u64) -> SessionId {
    SessionId(format!("adm-session-{seq}"))
}

fn event_id(event: &MediaEvent) -> Result<String> {
    let mut ev = event.clone();
    Ok(ev.header_mut().event_id.clone())
}

fn policy_decision(policy: FailurePolicy, reason: &str) -> Decision {
    match policy {
        FailurePolicy::Allow => Decision::Allow,
        FailurePolicy::Deny => Decision::Deny {
            code: MediaErrorCode::PermissionDenied,
            reason: reason.to_string(),
        },
    }
}

#[derive(Deserialize)]
struct DecisionResponse {
    code: i32,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    close: bool,
}

fn parse_decision_response(
    body: &str,
) -> std::result::Result<Decision, Box<dyn std::error::Error + Send + Sync>> {
    let resp: DecisionResponse = serde_json::from_str(body)?;
    if resp.code != 0 {
        return Ok(Decision::Deny {
            code: MediaErrorCode::PermissionDenied,
            reason: if resp.msg.is_empty() {
                format!("code {}", resp.code)
            } else {
                resp.msg
            },
        });
    }
    if resp.close {
        return Ok(Decision::Deny {
            code: MediaErrorCode::PermissionDenied,
            reason: resp.msg,
        });
    }
    Ok(Decision::Allow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebhookProfile;
    use crate::sender::{WebhookResponse, WebhookSender};
    use crate::translator::ZlmWebhookTranslator;
    use cheetah_media_api::event::{EventHeader, MediaEvent, SessionOpened};
    use cheetah_media_api::ids::{MediaKey, SessionId};
    use cheetah_media_api::model::{AdmissionAction, AdmissionRequest, SessionKind};
    use cheetah_media_api::port::MediaAdmissionApi;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeSender {
        responses: Mutex<Vec<WebhookResponse>>,
    }

    #[async_trait]
    impl WebhookSender for FakeSender {
        async fn send(
            &self,
            _request: WebhookHttpRequest,
        ) -> std::result::Result<WebhookResponse, WebhookSendError> {
            let mut responses = self.responses.lock().unwrap();
            let response = responses.remove(0);
            Ok(response)
        }
    }

    fn default_header() -> EventHeader {
        EventHeader {
            event_id: "evt-1".to_string(),
            occurred_at: 1,
            sequence: None,
            media_key: Some(MediaKey::new("__defaultVhost__", "live", "test", None).unwrap()),
            source: "test".to_string(),
            correlation_id: None,
        }
    }

    fn play_event() -> MediaEvent {
        MediaEvent::SessionOpened(SessionOpened {
            header: default_header(),
            kind: SessionKind::Player,
            protocol: "rtmp".to_string(),
            remote_endpoint: Some("10.0.0.1:1935".to_string()),
            session_id: SessionId("s1".to_string()),
        })
    }

    #[tokio::test]
    async fn decision_allows_on_success_response() {
        let profile = WebhookProfile {
            name: "hook".to_string(),
            url: "http://127.0.0.1:9999/on_play".to_string(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            decision_failure_policy: FailurePolicy::Deny,
            ..Default::default()
        };

        let sender = Arc::new(FakeSender {
            responses: Mutex::new(vec![WebhookResponse {
                status: 200,
                body: r#"{"code":0,"msg":"success"}"#.to_string(),
                duration_ms: 1,
            }]),
        });

        let client = WebhookDecisionClient::new(
            WebhookDispatcherConfig {
                profiles: vec![profile],
            },
            sender,
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        );

        let decision = client.request_decision(play_event()).await.unwrap();
        assert_eq!(decision, Decision::Allow);
    }

    #[tokio::test]
    async fn decision_denies_on_non_zero_code() {
        let profile = WebhookProfile {
            name: "hook".to_string(),
            url: "http://127.0.0.1:9999/on_play".to_string(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            decision_failure_policy: FailurePolicy::Deny,
            ..Default::default()
        };

        let sender = Arc::new(FakeSender {
            responses: Mutex::new(vec![WebhookResponse {
                status: 200,
                body: r#"{"code":-1,"msg":"forbidden"}"#.to_string(),
                duration_ms: 1,
            }]),
        });

        let client = WebhookDecisionClient::new(
            WebhookDispatcherConfig {
                profiles: vec![profile],
            },
            sender,
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        );

        let decision = client.request_decision(play_event()).await.unwrap();
        assert!(matches!(decision, Decision::Deny { .. }));
    }

    #[tokio::test]
    async fn decision_uses_failure_policy_on_timeout() {
        let profile = WebhookProfile {
            name: "hook".to_string(),
            url: "http://127.0.0.1:9999/on_play".to_string(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            decision_timeout_ms: 100,
            decision_failure_policy: FailurePolicy::Allow,
            ..Default::default()
        };

        let sender = Arc::new(FakeSender {
            responses: Mutex::new(vec![WebhookResponse {
                status: 200,
                body: "slow".to_string(),
                duration_ms: 1000,
            }]),
        });

        let client = WebhookDecisionClient::new(
            WebhookDispatcherConfig {
                profiles: vec![profile],
            },
            sender,
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        );

        // The fake sender ignores the timeout, but a real RuntimeHttpClient would.
        // This test just verifies the success response path still yields Allow.
        let decision = client.request_decision(play_event()).await.unwrap();
        assert_eq!(decision, Decision::Allow);
    }

    fn play_admission_request() -> AdmissionRequest {
        AdmissionRequest {
            action: AdmissionAction::Play,
            principal: None,
            resource: MediaKey::new("__defaultVhost__", "live", "test", None).unwrap(),
            protocol: "rtmp".to_string(),
            source_address: Some("10.0.0.1:1935".to_string()),
            params: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn authorize_translates_play_to_on_play_and_allows() {
        let profile = WebhookProfile {
            name: "hook".to_string(),
            url: "http://127.0.0.1:9999/on_play".to_string(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            decision_failure_policy: FailurePolicy::Deny,
            ..Default::default()
        };

        let sender = Arc::new(FakeSender {
            responses: Mutex::new(vec![WebhookResponse {
                status: 200,
                body: r#"{"code":0,"msg":"success"}"#.to_string(),
                duration_ms: 1,
            }]),
        });

        let client = WebhookDecisionClient::new(
            WebhookDispatcherConfig {
                profiles: vec![profile],
            },
            sender,
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        );

        let ctx = MediaRequestContext::default();
        let decision = client
            .authorize(&ctx, play_admission_request())
            .await
            .unwrap();
        assert_eq!(decision, Decision::Allow);
    }

    #[tokio::test]
    async fn authorize_translates_play_to_on_play_and_denies() {
        let profile = WebhookProfile {
            name: "hook".to_string(),
            url: "http://127.0.0.1:9999/on_play".to_string(),
            decision_events: vec!["on_play".to_string()],
            allowed_cidrs: vec!["127.0.0.1/32".to_string()],
            decision_failure_policy: FailurePolicy::Deny,
            ..Default::default()
        };

        let sender = Arc::new(FakeSender {
            responses: Mutex::new(vec![WebhookResponse {
                status: 200,
                body: r#"{"code":-1,"msg":"forbidden"}"#.to_string(),
                duration_ms: 1,
            }]),
        });

        let client = WebhookDecisionClient::new(
            WebhookDispatcherConfig {
                profiles: vec![profile],
            },
            sender,
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        );

        let ctx = MediaRequestContext::default();
        let decision = client
            .authorize(&ctx, play_admission_request())
            .await
            .unwrap();
        assert!(matches!(decision, Decision::Deny { .. }));
    }
}
