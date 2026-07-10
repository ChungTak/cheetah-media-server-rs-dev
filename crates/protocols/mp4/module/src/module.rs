//! MP4 VOD module — `cheetah-sdk::Module` integration & HTTP routing.

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, SdkError,
};

use crate::api::{ControlVodRequest, StartVodRequest, StopVodRequest, VodApi};
use crate::config::Mp4ModuleConfig;
use crate::session_registry::VodSessionRegistry;
use crate::zlm_compat::{ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat};

const MODULE_ID: &str = "mp4";

/// `Mp4ModuleFactory` data structure.
/// `Mp4ModuleFactory` 数据结构.
pub struct Mp4ModuleFactory;

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

/// `Mp4Module` data structure.
/// `Mp4Module` 数据结构.
pub struct Mp4Module {
    /// `state` field of type `ModuleState`.
    /// `state` 字段，类型为 `ModuleState`.
    state: ModuleState,
    /// `config` field of type `Mp4ModuleConfig`.
    /// `config` 字段，类型为 `Mp4ModuleConfig`.
    config: Mp4ModuleConfig,
    /// `ctx` field.
    /// `ctx` 字段.
    ctx: Option<EngineContext>,
    /// `registry` field.
    /// `registry` 字段.
    registry: Arc<VodSessionRegistry>,
    /// `api` field.
    /// `api` 字段.
    api: Option<Arc<VodApi>>,
}

impl Mp4Module {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Mp4ModuleConfig::default(),
            ctx: None,
            registry: Arc::new(VodSessionRegistry::new(0)),
            api: None,
        }
    }
}

impl Default for Mp4Module {
    fn default() -> Self {
        Self::new()
    }
}

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
        let api = VodApi::with_engine_bridge(
            self.registry.clone(),
            Arc::new(self.config.clone()),
            ctx.engine.core_adapters_api.clone(),
            ctx.engine.runtime_api.clone(),
        );
        self.api = Some(Arc::new(api));
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

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let api = self.api.as_ref()?.clone();
        let zlm = ZlmVodCompat::new(api.clone());
        Some(Arc::new(VodHttpService { api, zlm }))
    }
}

struct VodHttpService {
    api: Arc<VodApi>,
    zlm: ZlmVodCompat,
}

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
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Post, "/control") => {
                let body: ControlVodRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .control(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Post, "/stop") => {
                let body: StopVodRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .stop(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Post, "/zlm/loadMP4File") => {
                let body: ZlmLoadMp4 = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .load_mp4(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Post, "/zlm/seekRecordStamp") => {
                let body: ZlmSeekRecord = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .seek_record(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Post, "/zlm/setRecordSpeed") => {
                let body: ZlmSetSpeed = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .set_speed(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
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
