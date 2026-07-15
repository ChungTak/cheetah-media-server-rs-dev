use std::sync::{Arc, RwLock};

use crate::adapter_config::{extract_native_config, load_native_config, NativeAdapterConfig};
use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::audit::{AuditApi, AuditEvent, AuditResult};
use cheetah_media_api::command::{
    DeleteRecordRequest, DeleteSnapshotRequest, MediaQuery, RecordFileQuery, RecordPlaybackCommand,
    RecordTaskQuery, RtpQuery, RtpReceiverRequest, RtpSenderRequest, SessionQuery, SnapshotQuery,
    SnapshotRequest, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{
    FileHandle, MediaKey, RecordFileId, RecordTaskId, RtpSessionId, SessionId,
};
use cheetah_media_api::model::CloseReason;
use cheetah_media_api::port::{
    ControlAuthApi, MediaControlApi, MediaRequestContext, RecordApi, RtpApi, SnapshotApi,
};
use cheetah_media_api::{AuthCredentials, FileRange, MediaFileStoreApi, MediaScope, Principal};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};
use serde::de::DeserializeOwned;

use crate::error::{native_error_response, AdapterError};

const MODULE_ID: &str = "media-http-native";

/// Factory for the native media HTTP adapter module.
///
/// native 媒体 HTTP adapter 模块工厂。
pub struct NativeMediaModuleFactory;

impl ModuleFactory for NativeMediaModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Native Media HTTP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "media.native".to_string(),
            routes_prefix: "/api/v1".to_string(),
            capabilities: vec![ModuleCapability::HttpApi],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(NativeMediaModule::new())
    }
}

/// Native media HTTP adapter module.
///
/// native 媒体 HTTP adapter 模块。
pub struct NativeMediaModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    config: Arc<RwLock<NativeAdapterConfig>>,
}

impl NativeMediaModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            config: Arc::new(RwLock::new(NativeAdapterConfig::default())),
        }
    }
}

impl Default for NativeMediaModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for NativeMediaModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Native Media HTTP Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let cfg = load_native_config(&ctx.engine.config_provider.global());
        *self.config.write().unwrap() = cfg;
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: cheetah_sdk::CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = change.next_global.as_ref().unwrap_or(&change.next);
        let next = extract_native_config(next);
        let previous = self.config.read().unwrap().clone();
        if previous.enabled != next.enabled || previous.path_prefix != next.path_prefix {
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        *self.config.write().unwrap() = next;
        Ok(ConfigEffect::Immediate)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        if !self.config.read().unwrap().enabled {
            return Vec::new();
        }
        // `cheetah-control` now supports `{name}` path templates, so the native
        // module declares its full route catalog. Unknown paths are rejected with
        // 404 and wrong-method requests with 405 by the control-plane dispatcher.
        crate::native_routes::native_http_routes()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        if !self.config.read().unwrap().enabled {
            return None;
        }
        Some(Arc::new(NativeMediaHttpService {
            ctx: self.ctx.clone()?,
            config: self.config.clone(),
        }))
    }

    fn http_mount_prefix(&self) -> Option<String> {
        Some(self.config.read().unwrap().path_prefix.clone())
    }

    fn http_max_body_bytes(&self) -> usize {
        self.config.read().unwrap().max_body_bytes
    }

    fn http_request_timeout_ms(&self) -> Option<u64> {
        Some(self.config.read().unwrap().request_timeout_ms)
    }
}

struct NativeMediaHttpService {
    ctx: EngineContext,
    config: Arc<RwLock<NativeAdapterConfig>>,
}

impl NativeMediaHttpService {
    fn control(&self) -> Result<Arc<dyn MediaControlApi>, AdapterError> {
        self.ctx.media_services.control().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "media control not available",
            ))
        })
    }

    fn record(&self) -> Result<Arc<dyn RecordApi>, AdapterError> {
        self.ctx.media_services.record().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })
    }

    fn rtp(&self) -> Result<Arc<dyn RtpApi>, AdapterError> {
        self.ctx.media_services.rtp().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "rtp not available",
            ))
        })
    }

    fn snapshot(&self) -> Result<Arc<dyn SnapshotApi>, AdapterError> {
        self.ctx.media_services.snapshot().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "snapshot not available",
            ))
        })
    }

    fn file_store(&self) -> Arc<dyn MediaFileStoreApi> {
        self.ctx.media_file_store.clone()
    }

    fn auth(&self) -> Result<Arc<dyn ControlAuthApi>, AdapterError> {
        Ok(self.ctx.control_auth_api.clone())
    }

    fn authenticate(
        &self,
        req: &HttpRequest,
        cfg: &NativeAdapterConfig,
    ) -> Result<Principal, AdapterError> {
        if cfg.auth.mode == "none" {
            return Ok(Principal {
                identity: "anonymous".to_string(),
                scopes: vec![
                    MediaScope::MediaRead,
                    MediaScope::MediaControl,
                    MediaScope::MediaPublish,
                    MediaScope::MediaConsume,
                    MediaScope::RecordManage,
                    MediaScope::FileRead,
                    MediaScope::FileDelete,
                    MediaScope::ServerAdmin,
                ],
            });
        }
        let credentials = AuthCredentials {
            authorization_header: header_value(&req.headers, "authorization")
                .map(|s| s.to_string()),
            mtls_identity: header_value(&req.headers, "x-mtls-identity").map(|s| s.to_string()),
            deployment_token: header_value(&req.headers, "x-deployment-token")
                .map(|s| s.to_string()),
        };
        self.auth()?
            .authenticate(&credentials)
            .map_err(AdapterError::Media)
    }

    fn require_scope(
        &self,
        ctx: &MediaRequestContext,
        scope: &MediaScope,
    ) -> Result<(), AdapterError> {
        let has = ctx
            .principal
            .as_ref()
            .map(|p| p.has_scope(scope))
            .unwrap_or(false);
        if !has {
            return Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::new(
                    cheetah_media_api::error::MediaErrorCode::PermissionDenied,
                    format!("missing scope: {scope}"),
                ),
            ));
        }
        Ok(())
    }

    fn required_scope(&self, method: HttpMethod, path: &str) -> Option<MediaScope> {
        crate::native_routes::native_required_scope(method, path)
    }

    fn audit(&self) -> Result<Arc<dyn AuditApi>, AdapterError> {
        Ok(self.ctx.audit_api.clone())
    }

    fn audit_operation(&self, method: HttpMethod, path: &str) -> Option<&'static str> {
        match (method, path) {
            (HttpMethod::Post, path) if path.starts_with("/media/") && path.ends_with("/close") => {
                Some("media.close")
            }
            (HttpMethod::Post, path)
                if path.starts_with("/sessions/") && path.ends_with("/kick") =>
            {
                Some("session.kick")
            }
            (HttpMethod::Post, "/record/tasks") => Some("record.start"),
            (HttpMethod::Post, path)
                if path.starts_with("/record/tasks/") && path.ends_with("/stop") =>
            {
                Some("record.stop")
            }
            (HttpMethod::Delete, path) if path.starts_with("/record/files/") => Some("file.delete"),
            (HttpMethod::Post, path)
                if path.starts_with("/record/playback/") && path.ends_with("/control") =>
            {
                Some("record.playback_control")
            }
            (HttpMethod::Post, "/snapshots") => Some("snapshot.create"),
            (HttpMethod::Delete, "/snapshots/directories") => Some("snapshot.directory.delete"),
            (HttpMethod::Post, "/rtp/receivers") => Some("rtp.receiver.open"),
            (HttpMethod::Post, "/rtp/senders") => Some("rtp.sender.open"),
            (HttpMethod::Post, path)
                if path.starts_with("/rtp/sessions/") && path.ends_with("/stop") =>
            {
                Some("rtp.session.stop")
            }
            _ => None,
        }
    }

    async fn record_audit(
        &self,
        ctx: &MediaRequestContext,
        req: &HttpRequest,
        result: &Result<HttpResponse, AdapterError>,
    ) {
        let Some(operation) = self.audit_operation(req.method, &req.path) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let (audit_result, summary) = match result {
            Ok(_) => (AuditResult::Success, "ok".to_string()),
            Err(AdapterError::Media(err)) => (
                AuditResult::Failure {
                    code: err.code.to_string(),
                    message: err.message.to_string(),
                },
                "failed".to_string(),
            ),
            Err(AdapterError::InvalidRequest(msg)) => (
                AuditResult::Denied {
                    reason: msg.clone(),
                },
                "invalid".to_string(),
            ),
            Err(AdapterError::Serialization(msg)) => (
                AuditResult::Failure {
                    code: "serialization".to_string(),
                    message: msg.clone(),
                },
                "serialization failed".to_string(),
            ),
        };
        let event = AuditEvent {
            timestamp_ms: now,
            request_id: ctx.request_id.0.clone(),
            correlation_id: ctx.correlation_id.clone(),
            principal: ctx.principal.as_ref().map(|p| p.identity.clone()),
            operation: operation.to_string(),
            resource: req.path.clone(),
            result: audit_result,
            summary,
        };
        if let Ok(api) = self.audit() {
            api.record(ctx, event).await;
        }
    }

    fn request_context(&self, req: &HttpRequest) -> Result<MediaRequestContext, AdapterError> {
        let request_id = cheetah_media_api::ids::RequestId(
            header_value(&req.headers, "x-request-id")
                .map(|v| v.to_string())
                .unwrap_or_else(crate::util::generate_request_id),
        );
        let client_deadline =
            header_value(&req.headers, "x-deadline").and_then(|v| v.parse::<i64>().ok());
        let deadline = crate::util::request_deadline(client_deadline, 60_000);
        let cfg = self.config.read().unwrap();
        let principal = Some(self.authenticate(req, &cfg)?);
        drop(cfg);
        Ok(MediaRequestContext {
            request_id,
            correlation_id: header_value(&req.headers, "x-correlation-id").map(|s| s.to_string()),
            principal,
            source_adapter: "native".to_string(),
            trace_context: header_value(&req.headers, "x-trace-context").map(|s| s.to_string()),
            deadline,
            idempotency_key: header_value(&req.headers, "idempotency-key").map(|s| s.to_string()),
        })
    }

    fn parse_media_key(&self, path: &str, prefix: &str) -> Result<MediaKey, AdapterError> {
        let rest = path.strip_prefix(prefix).unwrap_or(path);
        let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() < 3 {
            return Err(AdapterError::InvalidRequest(
                "media path must contain vhost/app/stream".to_string(),
            ));
        }
        let vhost = percent_decode(parts[0]);
        let app = percent_decode(parts[1]);
        let stream = percent_decode(parts[2]);
        MediaKey::new(vhost, app, stream, None).map_err(AdapterError::Media)
    }

    async fn media_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let mut query: MediaQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = self.control()?.get_media_list(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn media_detail(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let key = self.parse_media_key(&req.path, "/media/")?;
        let info = self.control()?.get_media(ctx, &key).await?;
        Ok(json_response(&info))
    }

    async fn media_online(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let key = self.parse_media_key(&req.path, "/media/")?;
        let online = self.control()?.is_media_online(ctx, &key).await?;
        Ok(json_response(&serde_json::json!({ "online": online })))
    }

    async fn media_close(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let key = self.parse_media_key(&req.path, "/media/")?;
        let report = self
            .control()?
            .kick_stream(ctx, &key, CloseReason::Kicked)
            .await?;
        Ok(json_response(&report))
    }

    async fn media_keyframe(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let key = self.parse_media_key(&req.path, "/media/")?;
        self.control()?.request_keyframe(ctx, &key).await?;
        Ok(json_response(&serde_json::json!({ "requested": true })))
    }

    async fn session_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let mut query: SessionQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = self.control()?.list_sessions(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn session_kick(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
        let session_id = parts
            .get(parts.len().saturating_sub(2))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AdapterError::InvalidRequest("missing session_id".to_string()))?;
        self.control()?
            .kick_session(ctx, &SessionId(session_id.to_string()), CloseReason::Kicked)
            .await?;
        Ok(json_response(&serde_json::json!({ "kicked": true })))
    }

    async fn record_tasks(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let mut query: RecordTaskQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_tasks(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn record_files(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let mut query: RecordFileQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_files(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn record_start(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let mut request: StartRecordRequest = parse_body(&req)?;
        if request.idempotency_key.is_none() {
            request.idempotency_key = ctx
                .idempotency_key
                .clone()
                .map(cheetah_media_api::ids::IdempotencyKey);
        }
        let task = record_api.start_record(ctx, request).await?;
        Ok(json_response(&task))
    }

    async fn record_stop(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/tasks/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing task_id".to_string()))?;
        let request = StopRecordRequest {
            task_id: RecordTaskId(id),
        };
        let task = record_api.stop_record(ctx, request).await?;
        Ok(json_response(&task))
    }

    async fn record_file_delete(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/files/", "")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        record_api
            .delete_record_file(
                ctx,
                DeleteRecordRequest {
                    file_id: RecordFileId(id),
                },
            )
            .await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn record_playback_control(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let file_id = record_id_from_path(&req.path, "/record/playback/", "/control")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        let command: RecordPlaybackCommand = parse_body(&req)?;
        record_api
            .control_record_playback(ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(json_response(&serde_json::json!({ "controlled": true })))
    }

    async fn rtp_receivers(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let request: RtpReceiverRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_receiver(ctx, request).await?;
        Ok(json_response(&session))
    }

    async fn rtp_senders(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let request: RtpSenderRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_sender(ctx, request).await?;
        Ok(json_response(&session))
    }

    async fn rtp_session_stop(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let id = rtp_id_from_path(&req.path, "/rtp/sessions/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing session_id".to_string()))?;
        rtp_api.stop_rtp_session(ctx, &RtpSessionId(id)).await?;
        Ok(json_response(&serde_json::json!({ "stopped": true })))
    }

    async fn rtp_sessions(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let mut query: RtpQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn snapshot_create(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let request: SnapshotRequest = parse_body(&req)?;
        let handle = snapshot_api.take_snapshot(ctx, request).await?;
        Ok(json_response(&handle))
    }

    async fn snapshot_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let mut query: SnapshotQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = snapshot_api.query_snapshots(ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn snapshot_delete_directory(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let request: DeleteSnapshotRequest = parse_body(&req)?;
        snapshot_api.delete_snapshot_directory(ctx, request).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn file_download(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let handle = file_id_from_download_path(&req.path).ok_or_else(|| {
            AdapterError::InvalidRequest("invalid file download path".to_string())
        })?;
        let filename = query_param(&req, "filename");
        let range = parse_range_header(&req.headers).map(http_range_to_file_range);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let download = self
            .file_store()
            .resolve_download(ctx, &FileHandle(handle), range, filename, now_ms)
            .map_err(AdapterError::Media)?;

        Ok(download_response(download))
    }

    async fn proxies_pull(
        &self,
        _ctx: &MediaRequestContext,
        _req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("proxy"),
        ))
    }

    async fn capabilities(
        &self,
        _ctx: &MediaRequestContext,
        _req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let caps = self.ctx.media_services.capabilities();
        Ok(json_response(&caps))
    }
}

#[async_trait]
impl ModuleHttpService for NativeMediaHttpService {
    async fn handle(&self, mut req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let max_body_bytes = self.config.read().unwrap().max_body_bytes;
        if req.body.len() > max_body_bytes {
            return Err(SdkError::InvalidArgument(
                "request body too large".to_string(),
            ));
        }
        let audit_req = req.clone();
        let request_id = header_value(&req.headers, "x-request-id")
            .map(|v| v.to_string())
            .unwrap_or_else(crate::util::generate_request_id);
        crate::util::set_request_id_header(&mut req, &request_id);

        let result: Result<HttpResponse, AdapterError> = async {
            let ctx = self.request_context(&req)?;
            let Some(scope) = self.required_scope(req.method, &req.path) else {
                return Err(AdapterError::Media(
                    cheetah_media_api::error::MediaError::new(
                        cheetah_media_api::error::MediaErrorCode::NotFound,
                        "not found",
                    ),
                ));
            };
            if let Err(ref auth_err) = self.require_scope(&ctx, &scope) {
                let err = Err(auth_err.clone());
                self.record_audit(&ctx, &audit_req, &err).await;
                return err;
            }
            let response = match (req.method, req.path.as_str()) {
                (HttpMethod::Get, "/media/capabilities") => self.capabilities(&ctx, req).await,
                (HttpMethod::Get, "/media") => self.media_list(&ctx, req).await,
                (HttpMethod::Get, path)
                    if path.starts_with("/media/") && path.ends_with("/online") =>
                {
                    self.media_online(&ctx, req).await
                }
                (HttpMethod::Get, path)
                    if path.starts_with("/media/") && path.ends_with("/close") =>
                {
                    Err(AdapterError::InvalidRequest(
                        "use POST for close".to_string(),
                    ))
                }
                (HttpMethod::Post, path)
                    if path.starts_with("/media/") && path.ends_with("/close") =>
                {
                    self.media_close(&ctx, req).await
                }
                (HttpMethod::Post, path)
                    if path.starts_with("/media/") && path.ends_with("/keyframe") =>
                {
                    self.media_keyframe(&ctx, req).await
                }
                (HttpMethod::Get, path) if path.starts_with("/media/") => {
                    self.media_detail(&ctx, req).await
                }
                (HttpMethod::Get, "/sessions") => self.session_list(&ctx, req).await,
                (HttpMethod::Post, path)
                    if path.starts_with("/sessions/") && path.ends_with("/kick") =>
                {
                    self.session_kick(&ctx, req).await
                }
                (HttpMethod::Post, "/record/tasks") => self.record_start(&ctx, req).await,
                (HttpMethod::Post, path)
                    if path.starts_with("/record/tasks/") && path.ends_with("/stop") =>
                {
                    self.record_stop(&ctx, req).await
                }
                (HttpMethod::Get, "/record/tasks") => self.record_tasks(&ctx, req).await,
                (HttpMethod::Get, "/record/files") => self.record_files(&ctx, req).await,
                (HttpMethod::Delete, path) if path.starts_with("/record/files/") => {
                    self.record_file_delete(&ctx, req).await
                }
                (HttpMethod::Post, path)
                    if path.starts_with("/record/playback/") && path.ends_with("/control") =>
                {
                    self.record_playback_control(&ctx, req).await
                }
                (HttpMethod::Post, "/snapshots") => self.snapshot_create(&ctx, req).await,
                (HttpMethod::Get, "/snapshots") => self.snapshot_list(&ctx, req).await,
                (HttpMethod::Delete, "/snapshots/directories") => {
                    self.snapshot_delete_directory(&ctx, req).await
                }
                (HttpMethod::Get, path)
                    if path.starts_with("/files/") && path.ends_with("/download") =>
                {
                    self.file_download(&ctx, req).await
                }
                (HttpMethod::Get, "/proxies/pull") => self.proxies_pull(&ctx, req).await,
                (HttpMethod::Post, "/rtp/receivers") => self.rtp_receivers(&ctx, req).await,
                (HttpMethod::Post, "/rtp/senders") => self.rtp_senders(&ctx, req).await,
                (HttpMethod::Post, path)
                    if path.starts_with("/rtp/sessions/") && path.ends_with("/stop") =>
                {
                    self.rtp_session_stop(&ctx, req).await
                }
                (HttpMethod::Get, "/rtp/sessions") => self.rtp_sessions(&ctx, req).await,
                _ => Err(AdapterError::InvalidRequest("not found".to_string())),
            };

            self.record_audit(&ctx, &audit_req, &response).await;

            response
        }
        .await;

        let mut response = match result {
            Ok(resp) => resp,
            Err(AdapterError::Media(err)) => {
                let err_request_id = err.request_id.clone();
                let (status, body) = native_error_response(&err, err_request_id.as_deref());
                HttpResponse {
                    status,
                    headers: vec![HttpHeader {
                        name: "content-type".to_string(),
                        value: "application/json".to_string(),
                    }],
                    body: Bytes::from(serde_json::to_vec(&body).unwrap_or_default()),
                }
            }
            Err(AdapterError::InvalidRequest(msg)) => HttpResponse {
                status: 400,
                headers: vec![HttpHeader {
                    name: "content-type".to_string(),
                    value: "application/json".to_string(),
                }],
                body: Bytes::from(
                    serde_json::to_vec(&serde_json::json!({
                        "error": { "code": "invalid_argument", "message": msg }
                    }))
                    .unwrap_or_default(),
                ),
            },
            Err(AdapterError::Serialization(msg)) => HttpResponse {
                status: 500,
                headers: vec![HttpHeader {
                    name: "content-type".to_string(),
                    value: "application/json".to_string(),
                }],
                body: Bytes::from(
                    serde_json::to_vec(&serde_json::json!({
                        "error": { "code": "internal", "message": msg }
                    }))
                    .unwrap_or_default(),
                ),
            },
        };
        crate::util::set_response_request_id_header(&mut response, &request_id);
        Ok(response)
    }
}

fn header_value<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn json_response<T: serde::Serialize>(value: &T) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(serde_json::to_vec(value).unwrap_or_default()),
    }
}

fn percent_decode(s: &str) -> String {
    crate::util::percent_decode(s)
}

/// Extract the record id from a path like /record/tasks/{id}/stop.
///
/// 从 `/record/tasks/{id}/stop` 这类路径中提取记录 ID。
fn record_id_from_path(path: &str, prefix: &str, suffix: &str) -> Option<String> {
    rtp_id_from_path(path, prefix, suffix)
}

/// Extract the RTP session id from a path like /rtp/sessions/{id}/stop.
///
/// 从 `/rtp/sessions/{id}/stop` 这类路径中提取 RTP session ID。
fn rtp_id_from_path(path: &str, prefix: &str, suffix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    let id = if suffix.is_empty() {
        rest
    } else {
        rest.strip_suffix(suffix)?
    };
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

/// Parse a request body (JSON) or URL query string into the target query type.
///
/// 将请求 body（JSON）或 URL query 字符串解析为目标查询类型。
fn parse_query<T: DeserializeOwned + Default>(req: &HttpRequest) -> Result<T, AdapterError> {
    if !req.body.is_empty() {
        return Ok(serde_json::from_slice(&req.body)?);
    }
    if let Some(qs) = req.query.as_deref().filter(|q| !q.is_empty()) {
        let qs = qs.strip_prefix('?').unwrap_or(qs);
        return serde_urlencoded::from_str(qs)
            .map_err(|e| AdapterError::Serialization(e.to_string()));
    }
    Ok(T::default())
}

/// Parse a JSON request body.
///
/// 解析 JSON 请求体。
fn parse_body<T: DeserializeOwned>(req: &HttpRequest) -> Result<T, AdapterError> {
    if req.body.is_empty() {
        return Err(AdapterError::InvalidRequest(
            "request body is required".to_string(),
        ));
    }
    serde_json::from_slice(&req.body).map_err(|e| AdapterError::Serialization(e.to_string()))
}

/// Extract the file handle from a path like `/files/{handle}/download`.
///
/// 从 `/files/{handle}/download` 路径中提取文件句柄。
fn file_id_from_download_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/files/")?;
    let id = rest.strip_suffix("/download")?;
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return None;
    }
    Some(id.to_string())
}

/// Return the value of a URL query parameter, if any.
///
/// 返回 URL 查询参数值。
fn query_param(req: &HttpRequest, name: &str) -> Option<String> {
    let qs = req.query.as_deref()?;
    let qs = qs.strip_prefix('?').unwrap_or(qs);
    for pair in qs.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(crate::util::percent_decode(v));
            }
        } else if pair == name {
            return Some(String::new());
        }
    }
    None
}

/// A parsed HTTP `Range` request, distinct from the SDK `FileRange` so
/// suffix and explicit ranges can be told apart before the file size is known.
///
/// 解析后的 HTTP `Range` 请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpRange {
    From(u64),
    Bounded(u64, u64),
    Suffix(u64),
}

/// Parse a `Range: bytes=start-end` header.
///
/// Supports `bytes=start-end`, `bytes=start-` and `bytes=-suffix`.
///
/// 解析 `Range: bytes=start-end` 头。
fn parse_range_header(headers: &[HttpHeader]) -> Option<HttpRange> {
    let value = header_value(headers, "range")?;
    let value = value.trim();
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let start = start.trim();
    let end = end.trim();

    if start.is_empty() && end.is_empty() {
        return None;
    }

    if start.is_empty() {
        // Suffix range: bytes=-N means the last N bytes.
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        Some(HttpRange::Suffix(suffix))
    } else if end.is_empty() {
        let start = start.parse::<u64>().ok()?;
        Some(HttpRange::From(start))
    } else {
        let start = start.parse::<u64>().ok()?;
        let end = end.parse::<u64>().ok()?;
        if start > end {
            return None;
        }
        Some(HttpRange::Bounded(start, end))
    }
}

/// Convert a parsed HTTP range into a `FileRange` for the file store.
///
/// Suffix ranges keep their length in `start` with `is_suffix` set.
///
/// 将解析后的 HTTP range 转换为文件存储使用的 `FileRange`。
fn http_range_to_file_range(range: HttpRange) -> FileRange {
    match range {
        HttpRange::From(start) => FileRange::from(start),
        HttpRange::Bounded(start, end) => FileRange::bounded(start, end),
        HttpRange::Suffix(n) => FileRange::suffix(n),
    }
}

/// Build an HTTP response from a `FileDownload`.
///
/// 从 `FileDownload` 构建 HTTP 响应。
fn download_response(download: cheetah_media_api::FileDownload) -> HttpResponse {
    let total = download.total_size;
    let mut headers = vec![
        HttpHeader {
            name: "content-type".to_string(),
            value: download.content_type,
        },
        HttpHeader {
            name: "content-length".to_string(),
            value: download.body.len().to_string(),
        },
        HttpHeader {
            name: "content-disposition".to_string(),
            value: format!("attachment; filename=\"{}\"", download.filename),
        },
    ];

    let status = if let Some(r) = download.range {
        let end = r.end.unwrap_or(total.saturating_sub(1));
        headers.push(HttpHeader {
            name: "content-range".to_string(),
            value: format!("bytes {}-{}/{}", r.start, end, total),
        });
        206
    } else {
        200
    };

    HttpResponse {
        status,
        headers,
        body: download.body,
    }
}
