//! Record module factory + lifecycle integration with `cheetah-sdk`.
//!
//! Wires the registry, REST API, and a real `RecordExecutor` (subscribes to
//! engine streams and drives `cheetah_codec::record` writers to disk).
//!
//! 录制模块工厂及其与 `cheetah-sdk` 的生命周期集成。
//!
//! 连接注册表、REST API 与真实 `RecordExecutor`（订阅引擎流并将
//! `cheetah_codec::record` 写入器输出到磁盘）。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, ProviderRegistration, SdkError,
};

use crate::api::{
    FileDeleteRequest, FileQueryRequest, RecordApi, StartRecordRequest, StopRecordRequest,
};
use crate::config::RecordModuleConfig;
use crate::executor::RecordExecutor;
use crate::registry::RecordRegistry;
use crate::task::TaskExecutor;

const MODULE_ID: &str = "record";

/// Factory for creating `RecordModule` instances and registering metadata.
///
/// The factory declares module id, HTTP route prefix, and config schema so the
/// engine can mount the module without hard-coding its internals.
///
/// 创建 `RecordModule` 实例并注册元数据的工厂。
///
/// 工厂声明模块 ID、HTTP 路由前缀与配置 schema，使引擎无需硬编码内部即可挂载模块。
pub struct RecordModuleFactory;

impl ModuleFactory for RecordModuleFactory {
    /// Return the module manifest: id, capabilities, and config namespace.
    ///
    /// 返回模块 manifest：ID、能力与配置命名空间。
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

    /// Create a new `RecordModule` instance.
    ///
    /// 创建新的 `RecordModule` 实例。
    fn create(&self) -> Box<dyn Module> {
        Box::new(RecordModule::new())
    }

    /// Return the JSON schema registration for the engine config provider.
    ///
    /// 返回引擎配置提供方使用的 JSON schema 注册。
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
///
/// Holds the lifecycle state, config, registry, executor, and API surface.
/// The module integrates with the SDK lifecycle (`init -> start -> stop ->
/// apply_config`) and exposes the HTTP service through `RecordHttpService`.
///
/// 录制模块实例。
///
/// 保存生命周期状态、配置、注册表、执行器与 API 层。模块遵循 SDK 生命周期
/// (`init -> start -> stop -> apply_config`)，并通过 `RecordHttpService` 暴露 HTTP 服务。
pub struct RecordModule {
    state: ModuleState,
    config: RecordModuleConfig,
    ctx: Option<EngineContext>,
    registry: Arc<RecordRegistry>,
    executor: Option<Arc<RecordExecutor>>,
    api: Option<Arc<RecordApi>>,
    media_services_registration: Option<ProviderRegistration>,
}

impl RecordModule {
    /// Create a new module in the `Created` state.
    ///
    /// The registry is created with capacity 0 before configuration is loaded.
    ///
    /// 在 `Created` 状态下创建新模块。
    ///
    /// 在配置加载前，注册表以 0 容量创建。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: RecordModuleConfig::default(),
            ctx: None,
            registry: Arc::new(RecordRegistry::new(0)),
            executor: None,
            api: None,
            media_services_registration: None,
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
    /// Return module metadata including the current state.
    ///
    /// 返回包含当前状态的模块元数据。
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Record Module".to_string(),
            state: self.state,
        }
    }

    /// Return the current lifecycle state.
    ///
    /// 返回当前生命周期状态。
    fn state(&self) -> ModuleState {
        self.state
    }

    /// Initialize the module with engine context and initial config.
    ///
    /// Parses the record namespace, creates a registry sized by `max_tasks`,
    /// builds the executor, and exposes the API handle.
    ///
    /// 使用引擎上下文与初始配置初始化模块。
    ///
    /// 解析 record 命名空间，根据 `max_tasks` 创建注册表，构建执行器并暴露 API 句柄。
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
        let record_api = Arc::new(RecordApi::new(
            self.registry.clone(),
            executor_dyn,
            ctx.engine.media_event_bus.clone(),
        ));
        self.api = Some(record_api.clone());
        let record_capabilities = {
            let mut set = cheetah_media_api::MediaCapabilitySet::empty();
            set.add(cheetah_media_api::MediaCapability::Record, 1);
            set
        };
        self.media_services_registration =
            Some(ctx.engine.media_services.register_record_with_capabilities(
                Arc::new(crate::media_provider::RecordMediaProvider::new(
                    record_api,
                    ctx.engine.media_file_store.clone(),
                    ctx.engine.media_services.downgrade(),
                )),
                record_capabilities,
            ));
        self.state = ModuleState::Initialized;
        Ok(())
    }

    /// Start the module. The actual background work is spawned on demand.
    ///
    /// The module manager spawns each module's `start()` to completion, so
    /// returning immediately keeps the engine startup pipeline moving.
    /// Background record tasks are spawned on demand by `RecordExecutor` via
    /// `runtime_api.spawn`; cancellation is driven through `stop()` and each
    /// task's own per-task cancel token.
    ///
    /// 启动模块。实际后台工作按需派生。
    ///
    /// 模块管理器将每个模块的 `start()` 派生到完成，因此立即返回以保持引擎启动流水线。
    /// 后台录制任务由 `RecordExecutor` 通过 `runtime_api.spawn` 按需派生；
    /// 取消由 `stop()` 与每个任务自身的取消 token 驱动。
    async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        Ok(())
    }

    /// Stop the module and cancel all running record tasks.
    ///
    /// 停止模块并取消所有运行中的录制任务。
    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(executor) = self.executor.as_ref() {
            executor.shutdown().await;
        }
        if let Some(reg) = self.media_services_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    /// Apply a runtime config change. Non-trivial changes require a restart.
    ///
    /// 应用运行时配置变更。非平凡变更需要重启。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = RecordModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_cfg != self.config {
            self.config = new_cfg;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }

    /// List HTTP routes exposed by the module.
    ///
    /// Includes the SMS-style endpoints (`/start`, `/stop`, `/list`, etc.) mounted under the
    /// module's `routes_prefix`.
    ///
    /// 列出模块暴露的 HTTP 路由。
    ///
    /// 包含 SMS 风格端点（`/start`、`/stop`、`/list` 等），挂载在模块的 `routes_prefix` 下。
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
        ]
    }

    /// Return the HTTP service handler that routes requests to the API.
    ///
    /// 返回将请求路由到 API 的 HTTP 服务处理器。
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let api = self.api.as_ref()?.clone();
        Some(Arc::new(RecordHttpService { api }))
    }
}

/// HTTP service implementation that dispatches to `RecordApi`.
///
/// HTTP 服务实现，将请求分派到 `RecordApi`。
struct RecordHttpService {
    api: Arc<RecordApi>,
}

#[async_trait]
impl ModuleHttpService for RecordHttpService {
    /// Route an HTTP request to the correct API handler.
    ///
    /// 将 HTTP 请求路由到正确的 API 处理器。
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
                let result = self
                    .api
                    .query_files(q)
                    .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
                let resp = crate::api::FileQueryResponse {
                    code: 200,
                    msg: "success".to_string(),
                    data: result.files,
                };
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
            _ => HttpResponse {
                status: 404,
                body: bytes::Bytes::from(b"{\"code\":404,\"msg\":\"not found\"}".as_slice()),
                headers: vec![],
            },
        };
        Ok(response)
    }
}
