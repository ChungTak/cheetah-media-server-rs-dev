use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::config::{
    ConfigApplyApi, ConfigEffect, ConfigProvider, ModuleConfigChange, ModuleSchemaRegistration,
};
use crate::error::SdkError;
use crate::ids::ModuleId;
use crate::service::{
    ClusterApi, DatabaseApi, FfmpegApi, HealthApi, MetricsApi, ModuleManagerApi, ProxyManager,
    RoomServiceApi, ServiceRegistry,
};
use crate::stream::{CoreAdaptersApi, PublisherApi, StreamManagerApi, SubscriberApi};
use crate::task::{CancellationToken, TaskSystemApi};
use crate::EventBus;
use cheetah_runtime_api::RuntimeApi;

/// Lifecycle state of a module inside the engine.
///
/// 引擎内模块的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    Created,
    Initialized,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// HTTP methods exposed by module HTTP routes.
///
/// 模块 HTTP 路由暴露的 HTTP 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

/// Name/value pair for an HTTP header.
///
/// HTTP 头部的名称/值对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// Descriptor for a single HTTP route registered by a module.
///
/// 模块注册的单个 HTTP 路由描述符。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteDescriptor {
    pub method: HttpMethod,
    /// Route path relative to the module mount prefix.
    ///
    /// A path beginning with `//` is matched at the HTTP server root
    /// instead of under `ModuleManifest::routes_prefix`; the module still
    /// receives the normalized single-slash path in `HttpRequest::path`.
    pub path: String,
}

/// Incoming HTTP request delivered to a module HTTP service.
///
/// 传递给模块 HTTP 服务的入站 HTTP 请求。
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

/// Outgoing HTTP response produced by a module HTTP service.
///
/// 模块 HTTP 服务产生的出站 HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

impl HttpResponse {
    /// Build a 200 OK JSON response with the correct content-type header.
    ///
    /// 构造一个带正确 content-type 头的 200 OK JSON 响应。
    pub fn ok_json(body: impl Into<Bytes>) -> Self {
        Self {
            status: 200,
            headers: vec![HttpHeader {
                name: "content-type".to_string(),
                value: "application/json".to_string(),
            }],
            body: body.into(),
        }
    }
}

#[async_trait]
pub trait ModuleHttpService: Send + Sync {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError>;
}

/// A mounted module HTTP service with its routes and prefix.
///
/// 带有路由和前缀的已挂载模块 HTTP 服务。
#[derive(Clone)]
pub struct HttpRouteMount {
    pub module_id: ModuleId,
    pub prefix: String,
    pub routes: Vec<HttpRouteDescriptor>,
    pub service: Arc<dyn ModuleHttpService>,
}

/// Runtime metadata for a module instance.
///
/// 模块实例的运行时元数据。
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub module_id: ModuleId,
    pub display_name: String,
    pub state: ModuleState,
}

/// Capabilities that a module can advertise to the engine.
///
/// 模块可向引擎声明的能力。
#[derive(Debug, Clone)]
pub enum ModuleCapability {
    Publish,
    Subscribe,
    HttpApi,
    BackgroundJob,
    Room,
}

/// Module registration manifest provided by a module factory.
///
/// The manifest declares dependencies, configuration namespace, HTTP route prefix
/// and the capabilities the module contributes to the engine.
///
/// 模块工厂提供的模块注册清单。
///
/// 清单声明依赖、配置命名空间、HTTP 路由前缀以及模块为引擎贡献的能力。
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    pub module_id: ModuleId,
    pub display_name: String,
    pub dependencies: Vec<ModuleId>,
    pub config_namespace: String,
    pub routes_prefix: String,
    pub capabilities: Vec<ModuleCapability>,
}

/// Bundle of engine APIs injected into every module during initialization.
///
/// Modules use these APIs to publish/subscribe frames, manage streams, schedule tasks,
/// access configuration, and interact with runtime-neutral services.
///
/// 初始化期间注入到每个模块的引擎 API 集合。
///
/// 模块使用这些 API 发布/订阅帧、管理流、调度任务、访问配置，
/// 并与运行时无关的服务交互。
#[derive(Clone)]
pub struct EngineContext {
    pub runtime_api: Arc<dyn RuntimeApi>,
    pub publisher_api: Arc<dyn PublisherApi>,
    pub subscriber_api: Arc<dyn SubscriberApi>,
    pub core_adapters_api: Arc<dyn CoreAdaptersApi>,
    pub stream_manager_api: Arc<dyn StreamManagerApi>,
    pub task_system_api: Arc<dyn TaskSystemApi>,
    pub event_bus: Arc<dyn EventBus>,
    pub config_provider: Arc<dyn ConfigProvider>,
    pub config_apply_api: Arc<dyn ConfigApplyApi>,
    pub module_manager_api: Weak<dyn ModuleManagerApi>,
    pub room_service_api: Arc<dyn RoomServiceApi>,
    pub metrics_api: Arc<dyn MetricsApi>,
    pub health_api: Arc<dyn HealthApi>,
    pub service_registry: Arc<dyn ServiceRegistry>,
    pub database_api: Arc<dyn DatabaseApi>,
    pub proxy_manager: Arc<dyn ProxyManager>,
    pub cluster_api: Arc<dyn ClusterApi>,
    pub ffmpeg_api: Arc<dyn FfmpegApi>,
}

/// Context passed to `Module::init` containing the manifest and engine access.
///
/// `Module::init` 调用时传入的上下文，包含清单和引擎访问。
#[derive(Clone)]
pub struct ModuleInitContext {
    pub manifest: ModuleManifest,
    pub engine: EngineContext,
    pub initial_config: serde_json::Value,
}

/// Module lifecycle trait implemented by every protocol/feature module.
///
/// The engine calls `init`, `start`, `stop` and `apply_config` in order and routes
/// matching HTTP requests to the module's `http_service`.
///
/// 每个协议/功能模块实现的生命周期 trait。
///
/// 引擎按顺序调用 `init`、`start`、`stop` 和 `apply_config`，
/// 并将匹配的 HTTP 请求路由到模块的 `http_service`。
#[async_trait]
pub trait Module: Send + Sync {
    /// Return runtime metadata for this module.
    ///
    /// 返回此模块的运行时元数据。
    fn info(&self) -> ModuleInfo;
    /// Return the current lifecycle state.
    ///
    /// 返回当前生命周期状态。
    fn state(&self) -> ModuleState;

    /// Initialize the module with its manifest and engine APIs.
    ///
    /// 使用清单和引擎 API 初始化模块。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError>;

    /// Start the module's main loop or background tasks.
    ///
    /// The module should stop when `cancel` is triggered.
    ///
    /// 启动模块的主循环或后台任务。
    ///
    /// 当 `cancel` 被触发时，模块应停止。
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError>;

    /// Stop the module and release its resources.
    ///
    /// 停止模块并释放其资源。
    async fn stop(&mut self) -> Result<(), SdkError>;

    /// Apply a runtime configuration change and return the resulting effect.
    ///
    /// 应用运行时配置更改并返回结果效果。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError>;

    /// Return the HTTP routes exposed by this module.
    ///
    /// 返回此模块暴露的 HTTP 路由。
    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        Vec::new()
    }

    /// Return the HTTP service that handles requests for `http_routes`.
    ///
    /// 返回处理 `http_routes` 请求的 HTTP 服务。
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        None
    }
}

/// Factory trait for creating module instances and registering their metadata.
///
/// 用于创建模块实例并注册其元数据的工厂 trait。
pub trait ModuleFactory: Send + Sync {
    /// Return the module manifest used for registration.
    ///
    /// 返回用于注册的模块清单。
    fn manifest(&self) -> ModuleManifest;

    /// Create a new module instance.
    ///
    /// 创建新的模块实例。
    fn create(&self) -> Box<dyn Module>;

    /// Return an optional configuration schema for the module.
    ///
    /// 返回模块的可选配置模式。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        None
    }
}
