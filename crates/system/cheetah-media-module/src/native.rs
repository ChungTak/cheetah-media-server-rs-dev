use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{
    DeleteRecordRequest, DeleteSnapshotRequest, MediaQuery, RecordFileQuery, RecordPlaybackCommand,
    RecordTaskQuery, RtpQuery, RtpReceiverRequest, RtpSenderRequest, SessionQuery, SnapshotQuery,
    SnapshotRequest, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{MediaKey, RecordFileId, RecordTaskId, RtpSessionId, SessionId};
use cheetah_media_api::model::CloseReason;
use cheetah_media_api::port::{
    MediaControlApi, MediaRequestContext, RecordApi, RtpApi, SnapshotApi,
};
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
}

impl NativeMediaModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
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

    async fn apply_config(
        &mut self,
        _change: ModuleConfigChange,
    ) -> Result<ConfigEffect, SdkError> {
        Ok(ConfigEffect::Immediate)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        // The control-plane dispatcher (`cheetah-control/src/lib.rs`) only does
        // exact path comparison, so parameterized routes like `/media/:vhost/:app/:stream`
        // can never match. Returning an empty route list makes `route_match` treat
        // every request under `/api/v1` as a match, and the `handle` method below
        // performs its own prefix/suffix routing based on the actual path.
        Vec::new()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(NativeMediaHttpService {
            ctx: self.ctx.clone()?,
        }))
    }
}

struct NativeMediaHttpService {
    ctx: EngineContext,
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

    fn request_context(&self, req: &HttpRequest) -> MediaRequestContext {
        let request_id = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("x-request-id"))
            .map(|h| cheetah_media_api::ids::RequestId(h.value.clone()))
            .unwrap_or_else(|| cheetah_media_api::ids::RequestId("".to_string()));
        MediaRequestContext {
            request_id,
            correlation_id: None,
            principal: None,
            source_adapter: "native".to_string(),
            trace_context: None,
            deadline: None,
        }
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

    async fn media_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let mut query: MediaQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = self.control()?.get_media_list(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn media_detail(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let key = self.parse_media_key(&req.path, "/media/")?;
        let info = self.control()?.get_media(&ctx, &key).await?;
        Ok(json_response(&info))
    }

    async fn media_online(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let key = self.parse_media_key(&req.path, "/media/")?;
        let online = self.control()?.is_media_online(&ctx, &key).await?;
        Ok(json_response(&serde_json::json!({ "online": online })))
    }

    async fn media_close(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let key = self.parse_media_key(&req.path, "/media/")?;
        let report = self
            .control()?
            .kick_stream(&ctx, &key, CloseReason::Kicked)
            .await?;
        Ok(json_response(&report))
    }

    async fn media_keyframe(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let key = self.parse_media_key(&req.path, "/media/")?;
        self.control()?.request_keyframe(&ctx, &key).await?;
        Ok(json_response(&serde_json::json!({ "requested": true })))
    }

    async fn session_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let mut query: SessionQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = self.control()?.list_sessions(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn session_kick(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let parts: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
        let session_id = parts
            .get(parts.len().saturating_sub(2))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AdapterError::InvalidRequest("missing session_id".to_string()))?;
        self.control()?
            .kick_session(
                &ctx,
                &SessionId(session_id.to_string()),
                CloseReason::Kicked,
            )
            .await?;
        Ok(json_response(&serde_json::json!({ "kicked": true })))
    }

    async fn record_tasks(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let mut query: RecordTaskQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_tasks(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn record_files(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let mut query: RecordFileQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_files(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn record_start(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let request: StartRecordRequest = parse_body(&req)?;
        let task = record_api.start_record(&ctx, request).await?;
        Ok(json_response(&task))
    }

    async fn record_stop(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/tasks/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing task_id".to_string()))?;
        let request = StopRecordRequest {
            task_id: RecordTaskId(id),
        };
        let task = record_api.stop_record(&ctx, request).await?;
        Ok(json_response(&task))
    }

    async fn record_file_delete(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/files/", "")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        record_api
            .delete_record_file(
                &ctx,
                DeleteRecordRequest {
                    file_id: RecordFileId(id),
                },
            )
            .await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn record_playback_control(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let file_id = record_id_from_path(&req.path, "/record/playback/", "/control")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        let command: RecordPlaybackCommand = parse_body(&req)?;
        record_api
            .control_record_playback(&ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(json_response(&serde_json::json!({ "controlled": true })))
    }

    async fn rtp_receivers(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let request: RtpReceiverRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_receiver(&ctx, request).await?;
        Ok(json_response(&session))
    }

    async fn rtp_senders(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let request: RtpSenderRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_sender(&ctx, request).await?;
        Ok(json_response(&session))
    }

    async fn rtp_session_stop(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let id = rtp_id_from_path(&req.path, "/rtp/sessions/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing session_id".to_string()))?;
        rtp_api.stop_rtp_session(&ctx, &RtpSessionId(id)).await?;
        Ok(json_response(&serde_json::json!({ "stopped": true })))
    }

    async fn rtp_sessions(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let mut query: RtpQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn snapshot_create(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let snapshot_api = self.snapshot()?;
        let request: SnapshotRequest = parse_body(&req)?;
        let handle = snapshot_api.take_snapshot(&ctx, request).await?;
        Ok(json_response(&handle))
    }

    async fn snapshot_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let snapshot_api = self.snapshot()?;
        let mut query: SnapshotQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = snapshot_api.query_snapshots(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn snapshot_delete_directory(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let snapshot_api = self.snapshot()?;
        let request: DeleteSnapshotRequest = parse_body(&req)?;
        snapshot_api
            .delete_snapshot_directory(&ctx, request)
            .await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn file_download(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("file download"),
        ))
    }

    async fn proxies_pull(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("proxy"),
        ))
    }
}

#[async_trait]
impl ModuleHttpService for NativeMediaHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let result = match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/media") => self.media_list(req).await,
            (HttpMethod::Get, path) if path.starts_with("/media/") && path.ends_with("/online") => {
                self.media_online(req).await
            }
            (HttpMethod::Get, path) if path.starts_with("/media/") && path.ends_with("/close") => {
                Err(AdapterError::InvalidRequest(
                    "use POST for close".to_string(),
                ))
            }
            (HttpMethod::Post, path) if path.starts_with("/media/") && path.ends_with("/close") => {
                self.media_close(req).await
            }
            (HttpMethod::Post, path)
                if path.starts_with("/media/") && path.ends_with("/keyframe") =>
            {
                self.media_keyframe(req).await
            }
            (HttpMethod::Get, path) if path.starts_with("/media/") => self.media_detail(req).await,
            (HttpMethod::Get, "/sessions") => self.session_list(req).await,
            (HttpMethod::Post, path)
                if path.starts_with("/sessions/") && path.ends_with("/kick") =>
            {
                self.session_kick(req).await
            }
            (HttpMethod::Post, "/record/tasks") => self.record_start(req).await,
            (HttpMethod::Post, path)
                if path.starts_with("/record/tasks/") && path.ends_with("/stop") =>
            {
                self.record_stop(req).await
            }
            (HttpMethod::Get, "/record/tasks") => self.record_tasks(req).await,
            (HttpMethod::Get, "/record/files") => self.record_files(req).await,
            (HttpMethod::Delete, path) if path.starts_with("/record/files/") => {
                self.record_file_delete(req).await
            }
            (HttpMethod::Post, path)
                if path.starts_with("/record/playback/") && path.ends_with("/control") =>
            {
                self.record_playback_control(req).await
            }
            (HttpMethod::Post, "/snapshots") => self.snapshot_create(req).await,
            (HttpMethod::Get, "/snapshots") => self.snapshot_list(req).await,
            (HttpMethod::Delete, "/snapshots/directories") => {
                self.snapshot_delete_directory(req).await
            }
            (HttpMethod::Get, path)
                if path.starts_with("/files/") && path.ends_with("/download") =>
            {
                self.file_download(req).await
            }
            (HttpMethod::Get, "/proxies/pull") => self.proxies_pull(req).await,
            (HttpMethod::Post, "/rtp/receivers") => self.rtp_receivers(req).await,
            (HttpMethod::Post, "/rtp/senders") => self.rtp_senders(req).await,
            (HttpMethod::Post, path)
                if path.starts_with("/rtp/sessions/") && path.ends_with("/stop") =>
            {
                self.rtp_session_stop(req).await
            }
            (HttpMethod::Get, "/rtp/sessions") => self.rtp_sessions(req).await,
            _ => Err(AdapterError::InvalidRequest("not found".to_string())),
        };

        match result {
            Ok(resp) => Ok(resp),
            Err(AdapterError::Media(err)) => {
                let request_id = err.request_id.clone();
                let (status, body) = native_error_response(&err, request_id.as_deref());
                Ok(HttpResponse {
                    status,
                    headers: vec![HttpHeader {
                        name: "content-type".to_string(),
                        value: "application/json".to_string(),
                    }],
                    body: Bytes::from(serde_json::to_vec(&body).unwrap_or_default()),
                })
            }
            Err(AdapterError::InvalidRequest(msg)) => Ok(HttpResponse {
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
            }),
            Err(AdapterError::Serialization(msg)) => Ok(HttpResponse {
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
            }),
        }
    }
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
