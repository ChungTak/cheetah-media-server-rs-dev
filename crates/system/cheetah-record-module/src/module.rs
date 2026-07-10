//! Record module factory + lifecycle integration with `cheetah-sdk`.
//!
//! Wires the registry, REST API, and a real `RecordExecutor` (subscribes to
//! engine streams and drives `cheetah_codec::record` writers to disk).

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, SdkError,
};

use crate::api::{
    FileDeleteRequest, FileQueryRequest, RecordApi, StartRecordRequest, StopRecordRequest,
};
use crate::config::RecordModuleConfig;
use crate::executor::RecordExecutor;
use crate::registry::RecordRegistry;
use crate::task::TaskExecutor;
use crate::zlm_compat::{
    ZlmDeleteDirectory, ZlmGetMp4Files, ZlmIsRecording, ZlmRecordCompat, ZlmStartRecord,
    ZlmStopRecord,
};

const MODULE_ID: &str = "record";

/// `RecordModuleFactory` data structure.
/// `RecordModuleFactory` 数据结构。
pub struct RecordModuleFactory;

impl ModuleFactory for RecordModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Record Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "record".to_string(),
            routes_prefix: "/api/v1/record".to_string(),
            capabilities: vec![
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(RecordModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "record-module".to_string(),
            default_value: RecordModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg =
                    RecordModuleConfig::from_value(value.clone()).map_err(|e| e.to_string())?;
                cfg.validate()
            })),
        })
    }
}

/// Record module instance.
pub struct RecordModule {
    state: ModuleState,
    config: RecordModuleConfig,
    ctx: Option<EngineContext>,
    registry: Arc<RecordRegistry>,
    executor: Option<Arc<RecordExecutor>>,
    api: Option<Arc<RecordApi>>,
}

impl RecordModule {
    /// Creates a new `RecordModule` instance.
    /// 创建新的 `RecordModule` 实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: RecordModuleConfig::default(),
            ctx: None,
            registry: Arc::new(RecordRegistry::new(0)),
            executor: None,
            api: None,
        }
    }
}

impl Default for RecordModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for RecordModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Record Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = RecordModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine.clone());
        self.registry = Arc::new(RecordRegistry::new(self.config.max_tasks));
        let executor = Arc::new(RecordExecutor::new(
            ctx.engine.clone(),
            self.config.clone(),
            self.registry.clone(),
        ));
        let executor_dyn: Arc<dyn TaskExecutor> = executor.clone();
        self.executor = Some(executor);
        self.api = Some(Arc::new(RecordApi::new(
            self.registry.clone(),
            executor_dyn,
        )));
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
        // The module manager spawns each module's `start()` to completion,
        // so returning immediately keeps the engine startup pipeline moving
        // (other modules and the control plane can come up). Background
        // record tasks are spawned on demand by `RecordExecutor` via
        // `runtime_api.spawn`; cancellation is driven through `stop()` and
        // each task's own per-task cancel token, so the supplied parent
        // `cancel` token is not retained here.
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(executor) = self.executor.as_ref() {
            executor.shutdown().await;
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = RecordModuleConfig::from_value(change.next)
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
                path: "/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/list".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/query".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/file/query".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/file/delete".to_string(),
            },
            // ZLMediaKit-compatible endpoints. The engine HTTP wrapper mounts
            // them under the same module routes_prefix; in practice clients
            // hit `/api/v1/record/zlm/<route>` to reach them. Keeping them
            // co-located with the cheetah-style routes lets one HTTP service
            // serve both API surfaces.
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/startRecord".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/stopRecord".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/zlm/isRecording".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/zlm/getMP4RecordFile".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm/deleteRecordDirectory".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let api = self.api.as_ref()?.clone();
        let zlm = ZlmRecordCompat::new(api.clone());
        Some(Arc::new(RecordHttpService { api, zlm }))
    }
}

struct RecordHttpService {
    api: Arc<RecordApi>,
    zlm: ZlmRecordCompat,
}

#[async_trait]
impl ModuleHttpService for RecordHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let response = match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/start") => {
                let body: StartRecordRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .start(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Post, "/stop") => {
                let body: StopRecordRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let resp = self
                    .api
                    .stop(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Get, "/list") | (HttpMethod::Get, "/query") => {
                let resp = self.api.list();
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Get, "/file/query") => {
                let q: FileQueryRequest = if req.body.is_empty() {
                    FileQueryRequest::default()
                } else {
                    serde_json::from_slice(&req.body).unwrap_or_default()
                };
                let resp = self
                    .api
                    .query_files(q)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&resp).unwrap())
            }
            (HttpMethod::Post, "/file/delete") => {
                let body: FileDeleteRequest = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                self.api
                    .delete_file(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                let body = serde_json::json!({"code": 200, "msg": "success"});
                HttpResponse::ok_json(serde_json::to_vec(&body).unwrap())
            }
            // ZLMediaKit compat endpoints
            (HttpMethod::Post, "/zlm/startRecord") => {
                let body: ZlmStartRecord = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .start_record(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Post, "/zlm/stopRecord") => {
                let body: ZlmStopRecord = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid zlm body: {e}")))?;
                let value = self
                    .zlm
                    .stop_record(body)
                    .await
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Get, "/zlm/isRecording") => {
                let body: ZlmIsRecording = if req.body.is_empty() {
                    return Ok(HttpResponse {
                        status: 400,
                        body: bytes::Bytes::from_static(b"{\"code\":-1,\"msg\":\"missing body\"}"),
                        headers: vec![],
                    });
                } else {
                    serde_json::from_slice(&req.body)
                        .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?
                };
                let value = self
                    .zlm
                    .is_recording(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Get, "/zlm/getMP4RecordFile") => {
                let body: ZlmGetMp4Files = if req.body.is_empty() {
                    return Ok(HttpResponse {
                        status: 400,
                        body: bytes::Bytes::from_static(b"{\"code\":-1,\"msg\":\"missing body\"}"),
                        headers: vec![],
                    });
                } else {
                    serde_json::from_slice(&req.body)
                        .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?
                };
                let value = self
                    .zlm
                    .get_mp4_files(body)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                HttpResponse::ok_json(serde_json::to_vec(&value).unwrap())
            }
            (HttpMethod::Post, "/zlm/deleteRecordDirectory") => {
                let body: ZlmDeleteDirectory = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid body: {e}")))?;
                let value = self
                    .zlm
                    .delete_record_directory(body)
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
