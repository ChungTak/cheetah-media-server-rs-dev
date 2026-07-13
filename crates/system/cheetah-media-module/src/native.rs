use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_media_api::command::{MediaQuery, RecordFileQuery, RecordTaskQuery, SessionQuery};
use cheetah_media_api::ids::{MediaKey, SessionId};
use cheetah_media_api::model::CloseReason;
use cheetah_media_api::port::{MediaControlApi, MediaRequestContext};
use cheetah_sdk::{
    ConfigEffect, EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
    SdkError,
};

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
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/media".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/media/:vhost/:app/:stream".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/media/:vhost/:app/:stream/online".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/media/:vhost/:app/:stream/close".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/media/:vhost/:app/:stream/keyframe".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/sessions".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/sessions/:session_id/kick".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/record/tasks".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/record/files".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/proxies/pull".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/rtp/sessions".to_string(),
            },
        ]
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
    fn control(&self) -> Result<&Arc<dyn MediaControlApi>, AdapterError> {
        self.ctx.media_services.control.as_ref().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "media control not available",
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
        let query: MediaQuery = if req.body.is_empty() {
            MediaQuery::default()
        } else {
            serde_json::from_slice(&req.body)?
        };
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
        let query: SessionQuery = if req.body.is_empty() {
            SessionQuery::default()
        } else {
            serde_json::from_slice(&req.body)?
        };
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
        let record_api = self.ctx.media_services.record.as_ref().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let query: RecordTaskQuery = if req.body.is_empty() {
            RecordTaskQuery::default()
        } else {
            serde_json::from_slice(&req.body)?
        };
        let page = record_api.query_record_tasks(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn record_files(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.ctx.media_services.record.as_ref().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "record not available",
            ))
        })?;
        let query: RecordFileQuery = if req.body.is_empty() {
            RecordFileQuery::default()
        } else {
            serde_json::from_slice(&req.body)?
        };
        let page = record_api.query_record_files(&ctx, query).await?;
        Ok(json_response(&page))
    }

    async fn proxies_pull(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("proxy"),
        ))
    }

    async fn rtp_sessions(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Err(AdapterError::Media(
            cheetah_media_api::error::MediaError::unsupported_capability("rtp"),
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
            (HttpMethod::Get, "/record/tasks") => self.record_tasks(req).await,
            (HttpMethod::Get, "/record/files") => self.record_files(req).await,
            (HttpMethod::Get, "/proxies/pull") => self.proxies_pull(req).await,
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
    s.replace("%20", " ")
        .replace("%2F", "/")
        .replace("%2f", "/")
        .replace("%23", "#")
}
