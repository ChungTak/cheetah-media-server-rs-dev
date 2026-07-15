use std::sync::{Arc, RwLock};

use crate::adapter_config::{extract_zlm_config, load_zlm_config, ZlmAdapterConfig};
use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::audit::{AuditApi, AuditEvent, AuditResult};
use cheetah_media_api::command::{
    DeleteRecordRequest, MediaQuery, RecordFileQuery, RecordPlaybackCommand, RecordTaskQuery,
    SessionQuery, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{MediaKey, RecordFileId, SessionId};
use cheetah_media_api::model::{CloseReason, RecordTaskState};
use cheetah_media_api::port::{
    ControlAuthApi, MediaControlApi, MediaRequestContext, ProxyApi, RecordApi, RtpApi, SnapshotApi,
};
use cheetah_media_api::{AuthCredentials, MediaScope, Principal};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};

use crate::error::{zlm_error_response, AdapterError};

mod control;
mod proxy;
mod routes;
mod rtp;
mod snapshot;

const MODULE_ID: &str = "media-http-zlm";

/// Factory for the ZLMediaKit-compatible HTTP adapter module.
///
/// ZLMediaKit 兼容 HTTP adapter 模块工厂。
pub struct ZlmMediaModuleFactory;

impl ModuleFactory for ZlmMediaModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "ZLM Media HTTP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "media.zlm".to_string(),
            routes_prefix: "/index".to_string(),
            capabilities: vec![ModuleCapability::HttpApi],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(ZlmMediaModule::new())
    }
}

/// ZLMediaKit-compatible media HTTP adapter module.
///
/// ZLMediaKit 兼容媒体 HTTP adapter 模块。
pub struct ZlmMediaModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    config: Arc<RwLock<ZlmAdapterConfig>>,
}

impl ZlmMediaModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            config: Arc::new(RwLock::new(ZlmAdapterConfig::default())),
        }
    }
}

impl Default for ZlmMediaModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ZlmMediaModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "ZLM Media HTTP Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let cfg = load_zlm_config(&ctx.engine.config_provider.global());
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
        let next = extract_zlm_config(next);
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
        routes::zlm_http_routes()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        if !self.config.read().unwrap().enabled {
            return None;
        }
        Some(Arc::new(ZlmMediaHttpService {
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

pub(crate) struct ZlmMediaHttpService {
    ctx: EngineContext,
    config: Arc<RwLock<ZlmAdapterConfig>>,
}

impl ZlmMediaHttpService {
    fn control(&self) -> Result<Arc<dyn MediaControlApi>, AdapterError> {
        self.ctx.media_services.control().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "media control not available",
            ))
        })
    }

    pub(crate) fn rtp(&self) -> Result<Arc<dyn RtpApi>, AdapterError> {
        self.ctx.media_services.rtp().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "rtp not available",
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

    pub(crate) fn snapshot(&self) -> Result<Arc<dyn SnapshotApi>, AdapterError> {
        self.ctx.media_services.snapshot().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "snapshot not available",
            ))
        })
    }

    pub(crate) fn proxy(&self) -> Result<Arc<dyn ProxyApi>, AdapterError> {
        self.ctx.media_services.proxy().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "proxy not available",
            ))
        })
    }

    fn auth(&self) -> Result<Arc<dyn ControlAuthApi>, AdapterError> {
        Ok(self.ctx.control_auth_api.clone())
    }

    fn authenticate(
        &self,
        req: &HttpRequest,
        cfg: &ZlmAdapterConfig,
    ) -> Result<Principal, AdapterError> {
        use cheetah_media_api::error::{MediaError, MediaErrorCode};
        match cfg.auth.mode.as_str() {
            "none" => Ok(Principal {
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
            }),
            "secret" => {
                let expected = cfg.secret.as_deref().ok_or_else(|| {
                    AdapterError::Media(MediaError::new(
                        MediaErrorCode::Unauthenticated,
                        "zlm secret not configured",
                    ))
                })?;
                use subtle::{Choice, ConstantTimeEq};
                // Only accept the secret from headers; never from the URL query string
                // to avoid exposure in access logs or Referer headers.
                let provided = secret_from_header(req);
                let provided_bytes = provided.as_bytes();
                let expected_bytes = expected.as_bytes();
                let len_eq = (provided_bytes.len() as u64).ct_eq(&(expected_bytes.len() as u64));
                let mut content_eq = Choice::from(1u8);
                for (i, p) in provided_bytes.iter().enumerate() {
                    let e = expected_bytes.get(i).copied().unwrap_or(0);
                    content_eq &= u8::ct_eq(p, &e);
                }
                let valid = bool::from(len_eq & content_eq);
                if !valid {
                    return Err(AdapterError::Media(MediaError::new(
                        MediaErrorCode::Unauthenticated,
                        "invalid zlm secret",
                    )));
                }
                Ok(Principal {
                    identity: "zlm".to_string(),
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
                })
            }
            _ => {
                let credentials = AuthCredentials {
                    authorization_header: header_value(&req.headers, "authorization")
                        .map(|s| s.to_string()),
                    mtls_identity: header_value(&req.headers, "x-mtls-identity")
                        .map(|s| s.to_string()),
                    deployment_token: header_value(&req.headers, "x-deployment-token")
                        .map(|s| s.to_string()),
                };
                self.auth()?
                    .authenticate(&credentials)
                    .map_err(AdapterError::Media)
            }
        }
    }

    pub(crate) fn request_context(
        &self,
        req: &HttpRequest,
    ) -> Result<MediaRequestContext, AdapterError> {
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
            source_adapter: "zlm".to_string(),
            trace_context: header_value(&req.headers, "x-trace-context").map(|s| s.to_string()),
            deadline,
            idempotency_key: header_value(&req.headers, "idempotency-key").map(|s| s.to_string()),
        })
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
        routes::zlm_required_scope(method, path)
    }

    fn audit(&self) -> Result<Arc<dyn AuditApi>, AdapterError> {
        Ok(self.ctx.audit_api.clone())
    }

    fn audit_operation(&self, method: HttpMethod, path: &str) -> Option<&'static str> {
        match (method, path) {
            (HttpMethod::Post, "/api/close_stream") | (HttpMethod::Post, "/api/close_streams") => {
                Some("media.close")
            }
            (HttpMethod::Post, "/api/kick_session") | (HttpMethod::Post, "/api/kick_sessions") => {
                Some("session.kick")
            }
            (HttpMethod::Post, "/api/addStreamProxy") => Some("proxy.pull.create"),
            (HttpMethod::Post, "/api/delStreamProxy") => Some("proxy.pull.delete"),
            (HttpMethod::Post, "/api/addStreamPusherProxy") => Some("proxy.push.create"),
            (HttpMethod::Post, "/api/delStreamPusherProxy") => Some("proxy.push.delete"),
            (HttpMethod::Post, "/api/startRecord") | (HttpMethod::Post, "/api/startRecordTask") => {
                Some("record.start")
            }
            (HttpMethod::Post, "/api/stopRecord") => Some("record.stop"),
            (HttpMethod::Post, "/api/deleteRecordDirectory") => Some("file.delete"),
            (HttpMethod::Post, "/api/openRtpServer") => Some("rtp.receiver.open"),
            (HttpMethod::Post, "/api/connectRtpServer") => Some("rtp.receiver.connect"),
            (HttpMethod::Post, "/api/closeRtpServer") => Some("rtp.receiver.close"),
            (HttpMethod::Post, "/api/startSendRtp")
            | (HttpMethod::Post, "/api/startSendRtpPassive")
            | (HttpMethod::Post, "/api/startSendRtpTalk") => Some("rtp.sender.open"),
            (HttpMethod::Post, "/api/stopSendRtp") => Some("rtp.sender.close"),
            (HttpMethod::Post, "/api/setRecordSpeed")
            | (HttpMethod::Post, "/api/seekRecordStamp")
            | (HttpMethod::Post, "/api/controlRecordPlay")
            | (HttpMethod::Post, "/api/loadMP4File") => Some("record.playback_control"),
            (HttpMethod::Get, "/api/getSnap") => Some("snapshot.create"),
            (HttpMethod::Post, "/api/deleteSnapDirectory") => Some("snapshot.directory.delete"),
            (HttpMethod::Get, "/api/downloadFile") => Some("file.download"),
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

    pub(crate) fn extract_params(
        &self,
        req: &HttpRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        match req.method {
            HttpMethod::Get => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ if req.body.is_empty() => Ok(crate::util::query_to_json(req.query.as_deref())),
            _ => Ok(serde_json::from_slice(&req.body)?),
        }
    }

    pub(crate) fn parse_media_key(
        &self,
        params: &serde_json::Value,
    ) -> Result<MediaKey, AdapterError> {
        let vhost = params["vhost"].as_str().unwrap_or("__defaultVhost__");
        let app = params["app"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("app is required".to_string()))?;
        let stream = params["stream"]
            .as_str()
            .or_else(|| params["stream_id"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("stream is required".to_string()))?;
        MediaKey::new(vhost, app, stream, None).map_err(AdapterError::Media)
    }

    async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let mut query = MediaQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            schema: params["schema"].as_str().map(String::from),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = self.control()?.get_media_list(ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let online = self.control()?.is_media_online(ctx, &key).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "online": online == cheetah_media_api::model::OnlineState::Online }),
        ))
    }

    async fn get_media_info(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let info = self.control()?.get_media(ctx, &key).await?;
        Ok(zlm_response(0, "success", info))
    }

    async fn get_all_session(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let mut query = SessionQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = self.control()?.list_sessions(ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn close_stream(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let _ = self
            .control()?
            .kick_stream(ctx, &key, CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn kick_session(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let id = params["id"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("id is required".to_string()))?;
        self.control()?
            .kick_session(ctx, &SessionId(id.to_string()), CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn record_start(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let request = StartRecordRequest {
            media_key: key,
            format: format.clone(),
            template: cheetah_media_api::model::RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: cheetah_media_api::model::StoragePolicy::default(),
            idempotency_key: ctx
                .idempotency_key
                .clone()
                .map(cheetah_media_api::ids::IdempotencyKey),
        };
        let task = record_api.start_record(ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true, "taskId": task.task_id.0}),
        ))
    }

    async fn record_stop(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let mut query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordTaskQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_tasks(ctx, query).await?;
        let task = page
            .items
            .into_iter()
            .find(|t| {
                t.format == format
                    && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
            })
            .ok_or_else(|| {
                AdapterError::Media(cheetah_media_api::error::MediaError::not_found(
                    "record task",
                ))
            })?;
        record_api
            .stop_record(
                ctx,
                StopRecordRequest {
                    task_id: task.task_id,
                },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn is_recording(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let mut query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordTaskQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_tasks(ctx, query).await?;
        let recording = page.items.iter().any(|t| {
            t.format == format
                && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
        });
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"status": recording}),
        ))
    }

    async fn get_mp4_files(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            format: Some("mp4".to_string()),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_files(ctx, query).await?;
        let paths: Vec<String> = page.items.iter().map(|f| f.path_handle.0.clone()).collect();
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"paths": paths, "rootPath": ""}),
        ))
    }

    async fn delete_record_directory(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordFileQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let mut total_deleted = 0usize;
        let mut total_failed = 0usize;
        loop {
            let page = record_api.query_record_files(ctx, query.clone()).await?;
            if page.items.is_empty() {
                break;
            }
            let mut page_deleted = 0usize;
            for f in &page.items {
                match record_api
                    .delete_record_file(
                        ctx,
                        DeleteRecordRequest {
                            file_id: f.file_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => {
                        page_deleted += 1;
                        total_deleted += 1;
                    }
                    Err(_) => {
                        total_failed += 1;
                    }
                }
            }
            if (page.items.len() as u64) < query.page_size || page_deleted == 0 {
                break;
            }
        }
        let result = total_failed == 0;
        let data = serde_json::json!({
            "result": result,
            "deleted": total_deleted,
            "failed": total_failed,
        });
        Ok(zlm_response(
            0,
            if result { "success" } else { "partial success" },
            data,
        ))
    }

    async fn set_record_speed(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["speed", "scale", "value"])?;
        record_api
            .control_record_playback(
                ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Scale { value },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn seek_record_stamp(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["stamp", "seek", "value"])?;
        record_api
            .control_record_playback(
                ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Seek {
                    value: value as i64,
                },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn control_record_play(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let command = parse_zlm_playback_command(&params)?;
        record_api
            .control_record_playback(ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn load_mp4_file(
        &self,
        _ctx: &MediaRequestContext,
        _req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("vod"),
        ))
    }
}

#[async_trait]
impl ModuleHttpService for ZlmMediaHttpService {
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
                (HttpMethod::Get, "/api/getMediaList") => self.get_media_list(&ctx, req).await,
                (HttpMethod::Get, "/api/isMediaOnline") => self.is_media_online(&ctx, req).await,
                (HttpMethod::Get, "/api/getMediaPlayerList") => {
                    self.get_media_player_list(&ctx, req).await
                }
                (HttpMethod::Get, "/api/getMediaInfo") => self.get_media_info(&ctx, req).await,
                (HttpMethod::Post, "/api/close_stream") => self.close_stream(&ctx, req).await,
                (HttpMethod::Post, "/api/close_streams") => self.close_streams(&ctx, req).await,
                (HttpMethod::Get, "/api/getAllSession") => self.get_all_session(&ctx, req).await,
                (HttpMethod::Post, "/api/kick_session") => self.kick_session(&ctx, req).await,
                (HttpMethod::Post, "/api/kick_sessions") => self.kick_sessions(&ctx, req).await,
                (HttpMethod::Post, "/api/addStreamProxy") => self.add_stream_proxy(&ctx, req).await,
                (HttpMethod::Post, "/api/delStreamProxy") => self.del_stream_proxy(&ctx, req).await,
                (HttpMethod::Get, "/api/listStreamProxy") => {
                    self.list_stream_proxy(&ctx, req).await
                }
                (HttpMethod::Get, "/api/getProxyInfo") => self.get_proxy_info(&ctx, req).await,
                (HttpMethod::Post, "/api/addStreamPusherProxy") => {
                    self.add_stream_pusher_proxy(&ctx, req).await
                }
                (HttpMethod::Post, "/api/delStreamPusherProxy") => {
                    self.del_stream_pusher_proxy(&ctx, req).await
                }
                (HttpMethod::Get, "/api/listStreamPusherProxy") => {
                    self.list_stream_pusher_proxy(&ctx, req).await
                }
                (HttpMethod::Get, "/api/getProxyPusherInfo") => {
                    self.get_proxy_pusher_info(&ctx, req).await
                }
                (HttpMethod::Post, "/api/startRecord") => self.record_start(&ctx, req).await,
                (HttpMethod::Post, "/api/startRecordTask") => {
                    self.start_record_task(&ctx, req).await
                }
                (HttpMethod::Post, "/api/stopRecord") => self.record_stop(&ctx, req).await,
                (HttpMethod::Get, "/api/isRecording") => self.is_recording(&ctx, req).await,
                (HttpMethod::Get, "/api/getMP4RecordFile") => self.get_mp4_files(&ctx, req).await,
                (HttpMethod::Post, "/api/deleteRecordDirectory") => {
                    self.delete_record_directory(&ctx, req).await
                }
                (HttpMethod::Post, "/api/openRtpServer") => self.open_rtp_server(&ctx, req).await,
                (HttpMethod::Post, "/api/connectRtpServer") => {
                    self.connect_rtp_server(&ctx, req).await
                }
                (HttpMethod::Post, "/api/closeRtpServer") => self.close_rtp_server(&ctx, req).await,
                (HttpMethod::Get, "/api/listRtpServer") => self.list_rtp_server(&ctx, req).await,
                (HttpMethod::Post, "/api/startSendRtp") => self.start_send_rtp(&ctx, req).await,
                (HttpMethod::Post, "/api/startSendRtpPassive") => {
                    self.start_send_rtp_passive(&ctx, req).await
                }
                (HttpMethod::Post, "/api/startSendRtpTalk") => {
                    self.start_send_rtp_talk(&ctx, req).await
                }
                (HttpMethod::Post, "/api/stopSendRtp") => self.stop_send_rtp(&ctx, req).await,
                (HttpMethod::Get, "/api/listRtpSender") => self.list_rtp_sender(&ctx, req).await,
                (HttpMethod::Get, "/api/getRtpInfo") => self.get_rtp_info(&ctx, req).await,
                (HttpMethod::Post, "/api/setRecordSpeed") => self.set_record_speed(&ctx, req).await,
                (HttpMethod::Post, "/api/seekRecordStamp") => {
                    self.seek_record_stamp(&ctx, req).await
                }
                (HttpMethod::Post, "/api/controlRecordPlay") => {
                    self.control_record_play(&ctx, req).await
                }
                (HttpMethod::Post, "/api/loadMP4File") => self.load_mp4_file(&ctx, req).await,
                (HttpMethod::Get, "/api/getSnap") => self.get_snap(&ctx, req).await,
                (HttpMethod::Post, "/api/deleteSnapDirectory") => {
                    self.delete_snap_directory(&ctx, req).await
                }
                (HttpMethod::Get, "/api/downloadFile") => self.download_file(&ctx, req).await,
                _ => {
                    if routes::is_zlm_catalog_route(req.method, req.path.as_str()) {
                        Err(AdapterError::Media(
                            cheetah_media_api::error::MediaError::unsupported_capability(&format!(
                                "{:?} {}",
                                req.method, req.path
                            )),
                        ))
                    } else {
                        Err(AdapterError::InvalidRequest("not found".to_string()))
                    }
                }
            };

            self.record_audit(&ctx, &audit_req, &response).await;

            response
        }
        .await;

        let mut response = match result {
            Ok(resp) => resp,
            Err(AdapterError::Media(err)) => {
                let body = zlm_error_response(&err);
                zlm_json_response(body)
            }
            Err(AdapterError::InvalidRequest(msg)) => {
                let body = zlm_error_response(
                    &cheetah_media_api::error::MediaError::invalid_argument(msg),
                );
                zlm_json_response(body)
            }
            Err(AdapterError::Serialization(msg)) => {
                let body = zlm_error_response(&cheetah_media_api::error::MediaError::internal(msg));
                zlm_json_response(body)
            }
        };
        crate::util::set_response_request_id_header(&mut response, &request_id);
        Ok(response)
    }
}

pub(crate) fn zlm_record_format(value: &serde_json::Value) -> Result<String, AdapterError> {
    if value.is_null() {
        return Ok("mp4".to_string());
    }
    if let Some(num) = parse_json_u64(value) {
        let format = match num {
            0 => "mp4",
            1 => "hls",
            2 => "hls",
            3 => "fmp4",
            other => {
                return Err(AdapterError::InvalidRequest(format!(
                    "unsupported numeric record type {other}"
                )))
            }
        };
        return Ok(format.to_string());
    }
    if let Some(s) = value.as_str() {
        if s.trim().is_empty() {
            return Ok("mp4".to_string());
        }
        return Ok(s.to_lowercase());
    }
    Ok("mp4".to_string())
}

fn parse_json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

fn parse_json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

fn parse_zlm_file_id(params: &serde_json::Value) -> Result<String, AdapterError> {
    params["file_id"]
        .as_str()
        .or_else(|| params["fileId"].as_str())
        .or_else(|| params["file_path"].as_str())
        .map(String::from)
        .ok_or_else(|| AdapterError::InvalidRequest("file_id is required".to_string()))
}

fn parse_zlm_playback_value(
    params: &serde_json::Value,
    aliases: &[&str],
) -> Result<f64, AdapterError> {
    for alias in aliases {
        if let Some(v) = parse_json_f64(&params[*alias]) {
            return Ok(v);
        }
    }
    Err(AdapterError::InvalidRequest(
        "playback value is required".to_string(),
    ))
}

fn parse_zlm_playback_command(
    params: &serde_json::Value,
) -> Result<RecordPlaybackCommand, AdapterError> {
    let command = params["command"]
        .as_str()
        .ok_or_else(|| AdapterError::InvalidRequest("command is required".to_string()))?
        .to_lowercase();
    match command.as_str() {
        "pause" => Ok(RecordPlaybackCommand::Pause),
        "resume" => Ok(RecordPlaybackCommand::Resume),
        "scale" | "speed" => {
            let value = parse_zlm_playback_value(params, &["value", "speed", "scale"])?;
            Ok(RecordPlaybackCommand::Scale { value })
        }
        "seek" | "stamp" => {
            let value = parse_zlm_playback_value(params, &["value", "stamp", "seek"])?;
            Ok(RecordPlaybackCommand::Seek {
                value: value as i64,
            })
        }
        _ => Err(AdapterError::InvalidRequest(format!(
            "unsupported playback command {command}"
        ))),
    }
}

pub(crate) fn zlm_response<T: serde::Serialize>(code: i32, msg: &str, data: T) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "code": code,
                "msg": msg,
                "data": data,
            }))
            .unwrap_or_default(),
        ),
    }
}

fn zlm_json_response(params: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(serde_json::to_vec(&params).unwrap_or_default()),
    }
}

fn header_value<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// Extract the ZLM shared secret from a header.
///
/// Only `x-zlm-secret` and `Authorization: Bearer ...` are accepted; the URL
/// query string is intentionally ignored so the secret does not appear in access
/// logs or Referer headers.
///
/// 仅从 HTTP 头读取 ZLM 共享密钥，URL query string 被忽略，避免密钥泄露到
/// access log 或 Referer 头中。
fn secret_from_header(req: &HttpRequest) -> String {
    if let Some(header) = header_value(&req.headers, "x-zlm-secret") {
        return header.to_string();
    }
    if let Some(header) = header_value(&req.headers, "authorization") {
        let header = header.trim();
        let stripped = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .or_else(|| header.strip_prefix("BEARER "));
        return stripped.unwrap_or(header).trim().to_string();
    }
    String::new()
}

pub(crate) fn page_from_params(params: &serde_json::Value) -> u64 {
    params["page"].as_u64().unwrap_or(0)
}

pub(crate) fn page_size_from_params(params: &serde_json::Value) -> u64 {
    params["pageSize"]
        .as_u64()
        .or_else(|| params["page_size"].as_u64())
        .unwrap_or(20)
}
