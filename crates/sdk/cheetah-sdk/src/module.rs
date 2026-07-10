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

/// Lifecycle state of a module instance.
///
/// Modules move through `Created -> Initialized -> Running -> Stopping -> Stopped`.
/// `Failed` can be reached from any state and usually indicates an unrecoverable error.
///
/// 模块实例的生命周期状态。
///
/// 模块按 `Created -> Initialized -> Running -> Stopping -> Stopped` 迁移。
/// `Failed` 可从任何状态进入，通常表示不可恢复的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    /// Module instance has been created but not yet initialized.
    /// 模块实例已创建，但尚未初始化。
    Created,
    /// `init` has been called and the module has bound to engine resources.
    /// `init` 已调用，模块已绑定到引擎资源。
    Initialized,
    /// `start` has been called and the module is actively serving traffic.
    /// `start` 已调用，模块正在主动处理流量。
    Running,
    /// `stop` has been requested and the module is shutting down.
    /// `stop` 已请求，模块正在关闭。
    Stopping,
    /// The module has stopped and released its resources.
    /// 模块已停止并释放资源。
    Stopped,
    /// The module failed and cannot continue.
    /// 模块失败且无法继续。
    Failed,
}

/// HTTP method supported by a module route.
///
/// 模块路由支持的 HTTP 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    /// HTTP GET.
    /// HTTP GET。
    Get,
    /// HTTP POST.
    /// HTTP POST。
    Post,
    /// HTTP PUT.
    /// HTTP PUT。
    Put,
    /// HTTP PATCH.
    /// HTTP PATCH。
    Patch,
    /// HTTP DELETE.
    /// HTTP DELETE。
    Delete,
    /// HTTP OPTIONS.
    /// HTTP OPTIONS。
    Options,
}

/// Single HTTP header as received or sent by a module.
///
/// 模块接收或发送的单个 HTTP 头。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHeader {
    /// Header name.
    /// 头名称。
    pub name: String,
    /// Header value.
    /// 头值。
    pub value: String,
}

/// Registration of one HTTP route for a module.
///
/// 为模块注册的一个 HTTP 路由。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteDescriptor {
    /// Allowed HTTP method.
    /// 允许的 HTTP 方法。
    pub method: HttpMethod,
    /// Route path relative to the module mount prefix.
    ///
    /// A path beginning with `//` is matched at the HTTP server root
    /// instead of under `ModuleManifest::routes_prefix`; the module still
    /// receives the normalized single-slash path in `HttpRequest::path`.
    ///
    /// 相对于模块挂载前缀的路由路径。
    ///
    /// 以 `//` 开头的路径会在 HTTP 服务器根路径匹配，而非 `ModuleManifest::routes_prefix`
    /// 下；模块在 `HttpRequest::path` 中仍收到规范化后的单斜杠路径。
    pub path: String,
}

/// Request delivered to a module's HTTP handler.
///
/// 传递给模块 HTTP 处理器的请求。
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method.
    /// HTTP 方法。
    pub method: HttpMethod,
    /// Normalized request path.
    /// 规范化后的请求路径。
    pub path: String,
    /// Query string without the leading `?`.
    /// 不带前导 `?` 的查询字符串。
    pub query: Option<String>,
    /// Request headers.
    /// 请求头。
    pub headers: Vec<HttpHeader>,
    /// Request body.
    /// 请求体。
    pub body: Bytes,
}

/// Response returned by a module's HTTP handler.
///
/// 模块 HTTP 处理器返回的响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    /// HTTP 状态码。
    pub status: u16,
    /// Response headers.
    /// 响应头。
    pub headers: Vec<HttpHeader>,
    /// Response body.
    /// 响应体。
    pub body: Bytes,
}

impl HttpResponse {
    /// Build a 200 OK response with `application/json` content type.
    /// 构建带有 `application/json` 内容类型的 200 OK 响应。
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

/// Trait implemented by objects that handle HTTP requests for a module.
///
/// 为模块处理 HTTP 请求的对象所实现的 trait。
#[async_trait]
pub trait ModuleHttpService: Send + Sync {
    /// Handle an incoming HTTP request and return a response.
    /// 处理传入的 HTTP 请求并返回响应。
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError>;
}

/// Mounted HTTP routes for a module.
///
/// The engine uses this to register module routes with the control HTTP server.
///
/// 模块已挂载的 HTTP 路由。
///
/// 引擎用它将模块路由注册到控制 HTTP 服务器。
#[derive(Clone)]
pub struct HttpRouteMount {
    /// Module that owns these routes.
    /// 拥有这些路由的模块。
    pub module_id: ModuleId,
    /// URL prefix under which the routes are mounted.
    /// 路由挂载的 URL 前缀。
    pub prefix: String,
    /// List of route descriptors.
    /// 路由描述符列表。
    pub routes: Vec<HttpRouteDescriptor>,
    /// Handler for incoming requests.
    /// 传入请求的处理器。
    pub service: Arc<dyn ModuleHttpService>,
}

/// Runtime information about a module instance.
///
/// 模块实例的运行时信息。
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// Module identifier.
    /// 模块标识。
    pub module_id: ModuleId,
    /// Human-readable display name.
    /// 人类可读显示名称。
    pub display_name: String,
    /// Current lifecycle state.
    /// 当前生命周期状态。
    pub state: ModuleState,
}

/// Capability advertised by a module.
///
/// Capabilities tell the engine which roles a module can fulfill.
///
/// 模块声明的能力。
///
/// 能力告诉引擎模块可以承担哪些角色。
#[derive(Debug, Clone)]
pub enum ModuleCapability {
    /// Can accept a publishing stream.
    /// 可接受发布流。
    Publish,
    /// Can serve a subscribing stream.
    /// 可服务订阅流。
    Subscribe,
    /// Exposes HTTP API routes.
    /// 暴露 HTTP API 路由。
    HttpApi,
    /// Runs background jobs.
    /// 运行后台任务。
    BackgroundJob,
    /// Manages rooms.
    /// 管理房间。
    Room,
}

/// Static manifest describing a module and its dependencies.
///
/// The manifest is produced by the module factory and is used by the engine to
/// wire up configuration, HTTP routes, and lifecycle callbacks.
///
/// 描述模块及其依赖的静态清单。
///
/// 清单由模块工厂产生，引擎用它连接配置、HTTP 路由和生命周期回调。
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    /// Module identifier.
    /// 模块标识。
    pub module_id: ModuleId,
    /// Human-readable display name.
    /// 人类可读显示名称。
    pub display_name: String,
    /// Other module IDs this module depends on.
    /// 本模块依赖的其他模块 ID。
    pub dependencies: Vec<ModuleId>,
    /// Configuration namespace for this module.
    /// 本模块的配置命名空间。
    pub config_namespace: String,
    /// URL prefix for module HTTP routes.
    /// 模块 HTTP 路由的 URL 前缀。
    pub routes_prefix: String,
    /// Capabilities this module provides.
    /// 本模块提供的能力。
    pub capabilities: Vec<ModuleCapability>,
}

/// Context passed to a module so it can interact with the engine.
///
/// `EngineContext` is the single point through which modules access runtime,
/// stream, task, and service APIs. It intentionally does not expose concrete
/// runtime types so that modules remain runtime-neutral.
///
/// 传递给模块的上下文，使其能够与引擎交互。
///
/// `EngineContext` 是模块访问运行时、流、任务和服务 API 的单一入口。
/// 它有意不暴露具体运行时类型，使模块保持运行时无关。
#[derive(Clone)]
pub struct EngineContext {
    /// Runtime abstraction for spawn, timers, and channels.
    /// 用于 spawn、定时器和通道的运行时抽象。
    pub runtime_api: Arc<dyn RuntimeApi>,
    /// API for publishing media into a stream.
    /// 向流发布媒体的 API。
    pub publisher_api: Arc<dyn PublisherApi>,
    /// API for subscribing to a stream.
    /// 订阅流的 API。
    pub subscriber_api: Arc<dyn SubscriberApi>,
    /// API for accessing codec adapters and converters.
    /// 访问编解码器适配器与转换器的 API。
    pub core_adapters_api: Arc<dyn CoreAdaptersApi>,
    /// API for managing streams, publishers, and subscribers.
    /// 管理流、发布者和订阅者的 API。
    pub stream_manager_api: Arc<dyn StreamManagerApi>,
    /// API for spawning and canceling tasks.
    /// 用于生成和取消任务的 API。
    pub task_system_api: Arc<dyn TaskSystemApi>,
    /// Event bus for inter-module and engine events.
    /// 用于模块间和引擎事件的事件总线。
    pub event_bus: Arc<dyn EventBus>,
    /// Read-only configuration access.
    /// 只读配置访问。
    pub config_provider: Arc<dyn ConfigProvider>,
    /// API for applying configuration changes.
    /// 用于应用配置变更的 API。
    pub config_apply_api: Arc<dyn ConfigApplyApi>,
    /// API for managing other modules. Stored as a weak reference to avoid cycles.
    /// 管理其他模块的 API。使用弱引用以避免循环。
    pub module_manager_api: Weak<dyn ModuleManagerApi>,
    /// API for room-level operations.
    /// 房间级别操作的 API。
    pub room_service_api: Arc<dyn RoomServiceApi>,
    /// API for recording metrics.
    /// 用于记录指标的 API。
    pub metrics_api: Arc<dyn MetricsApi>,
    /// API for health checks.
    /// 用于健康检查的 API。
    pub health_api: Arc<dyn HealthApi>,
    /// Service registry for registering and discovering services.
    /// 用于注册和发现服务的服务注册表。
    pub service_registry: Arc<dyn ServiceRegistry>,
    /// API for database access.
    /// 数据库访问 API。
    pub database_api: Arc<dyn DatabaseApi>,
    /// API for managing proxy connections.
    /// 管理代理连接的 API。
    pub proxy_manager: Arc<dyn ProxyManager>,
    /// API for cluster-wide operations.
    /// 集群范围操作的 API。
    pub cluster_api: Arc<dyn ClusterApi>,
    /// API for FFmpeg invocations.
    /// 用于调用 FFmpeg 的 API。
    pub ffmpeg_api: Arc<dyn FfmpegApi>,
}

/// Context passed to `Module::init`.
///
/// Contains the module's manifest, the engine context, and the initial configuration.
///
/// 传递给 `Module::init` 的上下文。
///
/// 包含模块清单、引擎上下文和初始配置。
#[derive(Clone)]
pub struct ModuleInitContext {
    /// Module manifest as reported by the factory.
    /// 工厂报告的模块清单。
    pub manifest: ModuleManifest,
    /// Engine context with all system APIs.
    /// 包含所有系统 API 的引擎上下文。
    pub engine: EngineContext,
    /// Initial module configuration object.
    /// 初始模块配置对象。
    pub initial_config: serde_json::Value,
}

/// Trait implemented by every `cheetah` module.
///
/// A module is a protocol or feature implementation that plugs into the engine.
/// It advertises a manifest, lifecycle methods, and optional HTTP routes.
///
/// 每个 `cheetah` 模块实现的 trait。
///
/// 模块是插入引擎的协议或功能实现。它声明清单、生命周期方法和可选 HTTP 路由。
#[async_trait]
pub trait Module: Send + Sync {
    /// Return runtime information about the module.
    /// 返回模块的运行时信息。
    fn info(&self) -> ModuleInfo;

    /// Return the current lifecycle state.
    /// 返回当前生命周期状态。
    fn state(&self) -> ModuleState;

    /// Initialize the module with the given context.
    /// 使用给定上下文初始化模块。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError>;

    /// Start the module. The cancellation token is signaled when the engine wants to stop.
    /// 启动模块。当引擎想要停止时，取消令牌会被触发。
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError>;

    /// Stop the module and release its resources.
    /// 停止模块并释放资源。
    async fn stop(&mut self) -> Result<(), SdkError>;

    /// Apply a runtime configuration change and report the required effect.
    /// 应用运行时配置变更并报告所需效果。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError>;

    /// HTTP routes exposed by this module. Empty by default.
    /// 本模块暴露的 HTTP 路由。默认为空。
    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        Vec::new()
    }

    /// HTTP handler for the registered routes. `None` by default.
    /// 已注册路由的 HTTP 处理器。默认为 `None`。
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        None
    }
}

/// Factory that creates module instances and describes their static metadata.
///
/// 创建模块实例并描述其静态元数据的工厂。
pub trait ModuleFactory: Send + Sync {
    /// Return the static manifest for this module.
    /// 返回本模块的静态清单。
    fn manifest(&self) -> ModuleManifest;
    /// Create a new uninitialized module instance.
    /// 创建新的未初始化模块实例。
    fn create(&self) -> Box<dyn Module>;

    /// Optional JSON schema for module configuration. `None` by default.
    /// 可选的模块配置 JSON schema。默认为 `None`。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        None
    }
}
