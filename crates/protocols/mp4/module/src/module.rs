//! MP4 VOD module — `cheetah-sdk::Module` integration & HTTP routing.
//!
//! MP4 VOD 模块 —— `cheetah-sdk::Module` 集成与 HTTP 路由。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, ProviderRegistration, SdkError,
};
use serde::Serialize;

use crate::api::{ControlVodRequest, StartVodRequest, StopVodRequest, VodApi};
use crate::config::Mp4ModuleConfig;
use crate::media_provider::Mp4PlaybackProvider;
use crate::session_registry::VodSessionRegistry;
use crate::zlm_compat::{ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat};

const MODULE_ID: &str = "mp4";

/// Factory for creating `Mp4Module` instances.
///
/// 创建 `Mp4Module` 实例的工厂。
pub struct Mp4ModuleFactory;

/// `ModuleFactory` implementation for MP4 VOD.
///
/// MP4 VOD 的 `ModuleFactory` 实现。
impl ModuleFactory for Mp4ModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "MP4 VOD Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "mp4".to_string(),
            routes_prefix: "/api/v1/vod".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(Mp4Module::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "mp4-module".to_string(),
            default_value: Mp4ModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg = Mp4ModuleConfig::from_value(value.clone()).map_err(|e| e.to_string())?;
                cfg.validate()
            })),
        })
    }
}

/// MP4 VOD module state.
///
/// MP4 VOD 模块状态。
pub struct Mp4Module {
    state: ModuleState,
    config: Mp4ModuleConfig,
    ctx: Option<EngineContext>,
    registry: Arc<VodSessionRegistry>,
    api: Option<Arc<VodApi>>,
    playback_provider: Option<Arc<Mp4PlaybackProvider>>,
    media_services_registration: Option<ProviderRegistration>,
}

/// `Mp4Module` constructor and helpers.
///
/// `Mp4Module` 构造与辅助。
impl Mp4Module {
    /// Create a new MP4 module instance.
    ///
    /// 创建新的 MP4 模块实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Mp4ModuleConfig::default(),
            ctx: None,
            registry: Arc::new(VodSessionRegistry::new(0)),
            api: None,
            playback_provider: None,
            media_services_registration: None,
        }
    }
}

/// `Default` delegates to `new()`.
///
/// `Default` 委托给 `new()`。
impl Default for Mp4Module {
    fn default() -> Self {
        Self::new()
    }
}

/// `Module` lifecycle and HTTP routing for MP4 VOD.
///
/// MP4 VOD 的 `Module` 生命周期与 HTTP 路由。
#[async_trait]
impl Module for Mp4Module {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "MP4 VOD Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = Mp4ModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine.clone());
        self.registry = Arc::new(VodSessionRegistry::new(self.config.max_sessions));
        let api = Arc::new(VodApi::with_engine_bridge(
            self.registry.clone(),
            Arc::new(self.config.clone()),
            ctx.engine.core_adapters_api.clone(),
            ctx.engine.runtime_api.clone(),
        ));
        self.api = Some(api.clone());
        if self.config.enabled {
            let provider = Arc::new(Mp4PlaybackProvider::new(
                api,
                ctx.engine.media_file_store.clone(),
            ));
            let mut capabilities = cheetah_media_api::MediaCapabilitySet::empty();
            capabilities.add(cheetah_media_api::MediaCapability::Playback, 1);
            self.media_services_registration = Some(
                ctx.engine
                    .media_services
                    .register_playback_with_capabilities(provider.clone(), capabilities),
            );
            self.playback_provider = Some(provider);
        }
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        if !self.config.enabled {
            self.state = ModuleState::Running;
            cancel.cancelled().await;
            return Ok(());
        }
        self.state = ModuleState::Running;
        cancel.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(provider) = self.playback_provider.take() {
            provider.shutdown_all();
        }
        if let Some(reg) = self.media_services_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = Mp4ModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_cfg != self.config {
            self.config = new_cfg;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }

    /// Register HTTP routes for start/control/stop and ZLM compat endpoints.
    ///
    /// 注册 start/control/stop 及 ZLM 兼容端点的 HTTP 路由。
    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/start".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/control".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/stop".to_string(),
            },
            // ZLM compat
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/loadMP4File".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/seekRecordStamp".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/setRecordSpeed".to_string(),
            },
        ]
    }

    /// Return the HTTP service handler for the module.
    ///
    /// 返回模块的 HTTP 服务处理器。
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let api = self.api.as_ref()?.clone();
        let zlm = ZlmVodCompat::new(api.clone());
        Some(Arc::new(VodHttpService { api, zlm }))
    }
}

/// HTTP service implementation for VOD routes.
///
/// VOD 路由的 HTTP 服务实现。
struct VodHttpService {
    api: Arc<VodApi>,
    zlm: ZlmVodCompat,
}

/// Route incoming HTTP requests to the appropriate `VodApi` or `ZlmVodCompat` handler.
///
/// 将入站 HTTP 请求路由到对应的 `VodApi` 或 `ZlmVodCompat` 处理器。
#[async_trait]
impl ModuleHttpService for VodHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let response = match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/start") => {
                let body: StartVodRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .start(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&resp)?
            }
            (HttpMethod::Post, "/control") => {
                let body: ControlVodRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .control(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&resp)?
            }
            (HttpMethod::Post, "/stop") => {
                let body: StopVodRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .stop(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&resp)?
            }
            (HttpMethod::Post, "/zlm/loadMP4File") => {
                let body: ZlmLoadMp4 = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .load_mp4(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&value)?
            }
            (HttpMethod::Post, "/zlm/seekRecordStamp") => {
                let body: ZlmSeekRecord = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .seek_record(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&value)?
            }
            (HttpMethod::Post, "/zlm/setRecordSpeed") => {
                let body: ZlmSetSpeed = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .set_speed(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                json_response(&value)?
            }
            _ => HttpResponse {
                status: 404,
                body: bytes::Bytes::from(b"{\"code\":404,\"msg\":\"not found\"}".as_slice()),
                headers: vec![],
            },
        };
        Ok(response)
    }
}

fn json_response<T: Serialize>(value: &T) -> Result<HttpResponse, SdkError> {
    let body = serde_json::to_vec(value)
        .map_err(|e| SdkError::Internal(format!("failed to serialize response: {e}")))?;
    Ok(HttpResponse::ok_json(body))
}
