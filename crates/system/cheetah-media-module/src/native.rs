use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{
    DeleteRecordRequest, FfmpegProxyRequest, MediaQuery, ProxyQuery, PullProxyRequest,
    PushProxyRequest, RecordFileQuery, RecordPlaybackCommand, RecordTaskQuery, RtpQuery,
    RtpReceiverRequest, RtpSenderRequest, ServerConfigUpdate, SessionQuery, StartRecordRequest,
    StopRecordRequest, WebRtcRoomQuery, WebRtcRoomRequest, WhepRequest, WhipRequest,
};
use cheetah_media_api::ids::{
    MediaKey, ProxyId, RecordFileId, RecordTaskId, RtpSessionId, SessionId, WebRtcRoomId,
};
use cheetah_media_api::model::CloseReason;
use cheetah_media_api::port::{
    MediaControlApi, MediaRequestContext, ProxyApi, RtpApi, ServerAdminApi, WebRtcApi,
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

    fn record(&self) -> Result<Arc<dyn cheetah_media_api::port::RecordApi>, AdapterError> {
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

    fn proxy(&self) -> Result<Arc<dyn ProxyApi>, AdapterError> {
        self.ctx.media_services.proxy().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("proxy"),
            )
        })
    }

    fn server_admin(&self) -> Result<Arc<dyn ServerAdminApi>, AdapterError> {
        self.ctx.media_services.server_admin().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("server_admin"),
            )
        })
    }

    fn require_principal(&self, ctx: &MediaRequestContext) -> Result<(), AdapterError> {
        let global = self.ctx.config_provider.global();
        let Some(expected) = global
            .get("media")
            .and_then(|m| m.get("api_secret"))
            .and_then(serde_json::Value::as_str)
        else {
            return Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::unauthenticated(
                    "server admin authentication not configured",
                ),
            ));
        };
        match ctx.principal.as_deref() {
            Some(token) if token == expected => Ok(()),
            _ => Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::unauthenticated(
                    "server admin requires valid authentication",
                ),
            )),
        }
    }

    fn webrtc(&self) -> Result<Arc<dyn WebRtcApi>, AdapterError> {
        self.ctx.media_services.webrtc().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("webrtc"),
            )
        })
    }

    fn request_context(&self, req: &HttpRequest) -> MediaRequestContext {
        let request_id = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("x-request-id"))
            .map(|h| cheetah_media_api::ids::RequestId(h.value.clone()))
            .unwrap_or_else(|| cheetah_media_api::ids::RequestId("".to_string()));
        let principal = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("authorization"))
            .and_then(|h| {
                let value = h.value.trim();
                value.strip_prefix("Bearer ").map(|t| t.trim().to_string())
            });
        MediaRequestContext {
            request_id,
            correlation_id: None,
            principal,
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

    async fn proxies_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let mut query: ProxyQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = proxy_api.list_proxies(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn proxies_pull(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let request: PullProxyRequest = parse_body(&req)?;
        let info = proxy_api.create_pull_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }

    async fn proxies_push(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let request: PushProxyRequest = parse_body(&req)?;
        let info = proxy_api.create_push_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }

    async fn proxies_ffmpeg(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let mut request: FfmpegProxyRequest = parse_body(&req)?;
        crate::util::validate_ffmpeg_options(&request.input_options)?;
        crate::util::validate_ffmpeg_options(&request.output_options)?;
        // For a native API call, the user should not be able to pass a raw command string.
        request.source_url = request.source_url.trim().to_string();
        let info = proxy_api.create_ffmpeg_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }

    async fn proxies_pull_delete(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/pull")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_pull_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn proxies_push_delete(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/push")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_push_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn proxies_ffmpeg_delete(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/ffmpeg")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_ffmpeg_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }

    async fn server_info(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        Ok(json_response(&info))
    }

    async fn server_config(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let config = api.server_config(&ctx).await?;
        Ok(json_response(&config))
    }

    async fn server_config_update(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let update: ServerConfigUpdate = parse_body(&req)?;
        let config = cheetah_media_api::model::ServerConfig {
            values: update.values,
        };
        api.set_server_config(&ctx, config).await?;
        if update.restart {
            api.restart_server(&ctx).await?;
        }
        Ok(json_response(&serde_json::json!({ "updated": true })))
    }

    async fn server_restart(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.restart_server(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "restarting": true })))
    }

    async fn server_shutdown(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.shutdown_server(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "shutting_down": true })))
    }

    async fn server_ports(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let ports = api.list_ports(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "ports": ports })))
    }

    async fn webrtc_whip(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let request: WhipRequest = parse_body(&req)?;
        let response = api.whip_publish(&ctx, request).await?;
        Ok(json_response(&response))
    }

    async fn webrtc_whep(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let request: WhepRequest = parse_body(&req)?;
        let response = api.whep_subscribe(&ctx, request).await?;
        Ok(json_response(&response))
    }

    async fn webrtc_rooms_create(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let request: WebRtcRoomRequest = parse_body(&req)?;
        let room = api.create_room(&ctx, request).await?;
        Ok(json_response(&room))
    }

    async fn webrtc_rooms_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let mut query: WebRtcRoomQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = api.list_rooms(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn webrtc_room_delete(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let id = room_id_from_path(&req.path, "/webrtc/rooms/", "")
            .ok_or_else(|| AdapterError::InvalidRequest("missing room_id".to_string()))?;
        api.delete_room(&ctx, &WebRtcRoomId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
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
            (HttpMethod::Get, "/proxies") => self.proxies_list(req).await,
            (HttpMethod::Post, "/proxies/pull") => self.proxies_pull(req).await,
            (HttpMethod::Post, "/proxies/push") => self.proxies_push(req).await,
            (HttpMethod::Post, "/proxies/ffmpeg") => self.proxies_ffmpeg(req).await,
            (HttpMethod::Delete, path)
                if path.starts_with("/proxies/") && path.ends_with("/pull") =>
            {
                self.proxies_pull_delete(req).await
            }
            (HttpMethod::Delete, path)
                if path.starts_with("/proxies/") && path.ends_with("/push") =>
            {
                self.proxies_push_delete(req).await
            }
            (HttpMethod::Delete, path)
                if path.starts_with("/proxies/") && path.ends_with("/ffmpeg") =>
            {
                self.proxies_ffmpeg_delete(req).await
            }
            (HttpMethod::Post, "/rtp/receivers") => self.rtp_receivers(req).await,
            (HttpMethod::Post, "/rtp/senders") => self.rtp_senders(req).await,
            (HttpMethod::Post, path)
                if path.starts_with("/rtp/sessions/") && path.ends_with("/stop") =>
            {
                self.rtp_session_stop(req).await
            }
            (HttpMethod::Get, "/rtp/sessions") => self.rtp_sessions(req).await,
            (HttpMethod::Get, "/server/info") => self.server_info(req).await,
            (HttpMethod::Get, "/server/config") => self.server_config(req).await,
            (HttpMethod::Post, "/server/config") => self.server_config_update(req).await,
            (HttpMethod::Post, "/server/restart") => self.server_restart(req).await,
            (HttpMethod::Post, "/server/shutdown") => self.server_shutdown(req).await,
            (HttpMethod::Get, "/server/ports") => self.server_ports(req).await,
            (HttpMethod::Post, "/webrtc/whip") => self.webrtc_whip(req).await,
            (HttpMethod::Post, "/webrtc/whep") => self.webrtc_whep(req).await,
            (HttpMethod::Post, "/webrtc/rooms") => self.webrtc_rooms_create(req).await,
            (HttpMethod::Get, "/webrtc/rooms") => self.webrtc_rooms_list(req).await,
            (HttpMethod::Delete, path) if path.starts_with("/webrtc/rooms/") => {
                self.webrtc_room_delete(req).await
            }
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

/// Extract the proxy id from a path like /proxies/{id}/pull.
fn proxy_id_from_path(path: &str, prefix: &str, suffix: &str) -> Option<String> {
    rtp_id_from_path(path, prefix, suffix)
}

/// Extract the WebRTC room id from a path like /webrtc/rooms/{id}.
fn room_id_from_path(path: &str, prefix: &str, suffix: &str) -> Option<String> {
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
