use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{MediaQuery, SessionQuery};
use cheetah_media_api::ids::{MediaKey, SessionId, StreamKeyBridge};
use cheetah_media_api::model::CloseReason;
use cheetah_media_api::port::{
    MediaControlApi, MediaRequestContext, ProxyApi, RecordApi, RtpApi, ServerAdminApi, SnapshotApi,
};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};

use crate::error::{zlm_error_response, AdapterError};

mod proxy;
mod record;
mod routes;
mod rtp;
mod server;
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
}

impl ZlmMediaModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
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
        self::routes::http_routes()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(ZlmMediaHttpService {
            ctx: self.ctx.clone()?,
        }))
    }
}

pub(crate) struct ZlmMediaHttpService {
    ctx: EngineContext,
}

impl ZlmMediaHttpService {
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

    pub(crate) fn snapshot(&self) -> Result<Arc<dyn SnapshotApi>, AdapterError> {
        self.ctx.media_services.snapshot().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "snapshot not available",
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

    pub(crate) fn rtp(&self) -> Result<Arc<dyn RtpApi>, AdapterError> {
        self.ctx.media_services.rtp().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "rtp not available",
            ))
        })
    }

    fn server_admin(&self) -> Result<Arc<dyn ServerAdminApi>, AdapterError> {
        self.ctx.media_services.server_admin().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("server_admin"),
            )
        })
    }

    pub(crate) fn require_principal(&self, ctx: &MediaRequestContext) -> Result<(), AdapterError> {
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
            Some(token) if crate::util::constant_time_eq(token, expected) => Ok(()),
            _ => Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::unauthenticated(
                    "server admin requires valid authentication",
                ),
            )),
        }
    }

    pub(crate) fn request_context(&self, req: &HttpRequest) -> MediaRequestContext {
        let principal = req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("authorization"))
            .and_then(|h| {
                let value = h.value.trim();
                value.strip_prefix("Bearer ").map(|t| t.trim().to_string())
            });
        MediaRequestContext {
            request_id: cheetah_media_api::ids::RequestId("".to_string()),
            correlation_id: None,
            principal,
            source_adapter: "zlm".to_string(),
            trace_context: None,
            deadline: None,
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

    async fn get_media_list(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let query = MediaQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            schema: params["schema"].as_str().map(String::from),
            ..Default::default()
        };
        let page = self.control()?.get_media_list(&ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn is_media_online(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let online = self.control()?.is_media_online(&ctx, &key).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "online": online == cheetah_media_api::model::OnlineState::Online }),
        ))
    }

    async fn get_media_info(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let info = self.control()?.get_media(&ctx, &key).await?;
        Ok(zlm_response(0, "success", info))
    }

    async fn get_all_session(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let query = SessionQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            ..Default::default()
        };
        let page = self.control()?.list_sessions(&ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    async fn close_stream(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let _ = self
            .control()?
            .kick_stream(&ctx, &key, CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    async fn kick_session(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let id = params["id"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("id is required".to_string()))?;
        self.control()?
            .kick_session(&ctx, &SessionId(id.to_string()), CloseReason::Kicked)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }
}

#[async_trait]
impl ModuleHttpService for ZlmMediaHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let result = match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/api/getMediaList") => self.get_media_list(req).await,
            (HttpMethod::Get, "/api/isMediaOnline") => self.is_media_online(req).await,
            (HttpMethod::Get, "/api/getMediaInfo") => self.get_media_info(req).await,
            (HttpMethod::Get, "/api/getAllSession") => self.get_all_session(req).await,
            (HttpMethod::Post, "/api/close_stream") => self.close_stream(req).await,
            (HttpMethod::Post, "/api/kick_session") => self.kick_session(req).await,
            (HttpMethod::Post, "/api/startRecord") => self.record_start(req).await,
            (HttpMethod::Post, "/api/stopRecord") => self.record_stop(req).await,
            (HttpMethod::Get, "/api/isRecording") => self.is_recording(req).await,
            (HttpMethod::Get, "/api/getMP4RecordFile") => self.get_mp4_files(req).await,
            (HttpMethod::Post, "/api/deleteRecordDirectory") => {
                self.delete_record_directory(req).await
            }
            (HttpMethod::Post, "/api/openRtpServer") => self.open_rtp_server(req).await,
            (HttpMethod::Post, "/api/closeRtpServer") => self.close_rtp_server(req).await,
            (HttpMethod::Post, "/api/startSendRtp") => self.start_send_rtp(req).await,
            (HttpMethod::Post, "/api/stopSendRtp") => self.stop_send_rtp(req).await,
            (HttpMethod::Get, "/api/getRtpInfo") => self.get_rtp_info(req).await,
            (HttpMethod::Get, "/api/getSnap") => self.get_snap(req).await,
            (HttpMethod::Post, "/api/deleteSnapDirectory") => self.delete_snap_directory(req).await,
            (HttpMethod::Post, "/api/addStreamProxy") => self.add_stream_proxy(req).await,
            (HttpMethod::Post, "/api/delStreamProxy") => self.del_stream_proxy(req).await,
            (HttpMethod::Get, "/api/getAllStreamProxy") => self.get_all_stream_proxy(req).await,
            (HttpMethod::Post, "/api/addFFmpegSource") => self.add_ffmpeg_source(req).await,
            (HttpMethod::Post, "/api/delFFmpegSource") => self.del_ffmpeg_source(req).await,
            (HttpMethod::Get, "/api/getServerLoad") => self.get_server_load(req).await,
            (HttpMethod::Get, "/api/getWorkThreadsLoad") => self.get_work_threads_load(req).await,
            (HttpMethod::Get, "/api/getServerConfig") => self.get_server_config(req).await,
            (HttpMethod::Post, "/api/setServerConfig") => self.set_server_config(req).await,
            (HttpMethod::Post, "/api/restartServer") => self.restart_server(req).await,
            (HttpMethod::Post, "/api/shutdownServer") => self.shutdown_server(req).await,
            _ => Err(AdapterError::InvalidRequest("not found".to_string())),
        };

        match result {
            Ok(resp) => Ok(resp),
            Err(AdapterError::Media(err)) => {
                let body = zlm_error_response(&err);
                Ok(zlm_json_response(body))
            }
            Err(AdapterError::InvalidRequest(msg)) => {
                let body = zlm_error_response(
                    &cheetah_media_api::error::MediaError::invalid_argument(msg),
                );
                Ok(zlm_json_response(body))
            }
            Err(AdapterError::Serialization(msg)) => {
                let body = zlm_error_response(&cheetah_media_api::error::MediaError::internal(msg));
                Ok(zlm_json_response(body))
            }
        }
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

pub(crate) fn zlm_key_string(key: &MediaKey) -> String {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
    format!("{namespace}/{path}")
}
