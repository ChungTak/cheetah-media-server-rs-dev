use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::config::{
    ConfigApplyApi, ConfigEffect, ConfigProvider, ModuleConfigChange, ModuleSchemaRegistration,
};
use crate::error::SdkError;
use crate::ids::ModuleId;
use crate::media_data_plane::MediaDataPlaneApi;
use crate::media_session::MediaSessionDirectoryApi;
use crate::service::{
    ClusterApi, DatabaseApi, FfmpegApi, HealthApi, MetricsApi, ModuleManagerApi, ProxyManager,
    RoomServiceApi, ServiceRegistry,
};
use crate::stream::{CoreAdaptersApi, PublisherApi, StreamManagerApi, SubscriberApi};
use crate::task::{CancellationToken, TaskSystemApi};
use crate::EventBus;
use crate::MediaFileStoreApi;
use cheetah_media_api::event::MediaEventSender;
use cheetah_runtime_api::RuntimeApi;

/// Lifecycle state of a module.
///
/// 模块生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    Created,
    Initialized,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// HTTP methods supported by module HTTP routes.
///
/// 模块 HTTP 路由支持的 HTTP 方法。
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
/// HTTP 头的名称/值对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// HTTP route descriptor for a module's HTTP service.
///
/// HTTP 路由描述符。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteDescriptor {
    pub method: HttpMethod,
    /// Route path relative to the module mount prefix.
    ///
    /// A path beginning with `//` is matched at the HTTP server root
    /// instead of under `ModuleManifest::routes_prefix`; the module still
    /// receives the normalized single-slash path in `HttpRequest::path`.
    ///
    /// 相对于模块挂载前缀的路由路径。
    ///
    /// 以 `//` 开头的路径在 HTTP 服务器根路径匹配，而不是在 `ModuleManifest::routes_prefix`
    /// 下；模块仍会在 `HttpRequest::path` 中收到规范化的单斜杠路径。
    pub path: String,
}

/// HTTP request delivered to a module's `ModuleHttpService`.
///
/// 传递给模块 `ModuleHttpService` 的 HTTP 请求。
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

/// HTTP response returned by a module's `ModuleHttpService`.
///
/// 模块 `ModuleHttpService` 返回的 HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

impl HttpResponse {
    /// Convenience constructor for a 200 OK response with `application/json`.
    ///
    /// 便捷构造 200 OK 并带 `application/json` 的响应。
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

/// Module-provided HTTP request handler.
///
/// 模块提供的 HTTP 请求处理器。
#[async_trait]
pub trait ModuleHttpService: Send + Sync {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError>;
}

/// A mounted HTTP route set, including the module-provided service handler.
///
/// 已挂载的 HTTP 路由集，包含模块提供的服务处理器。
#[derive(Clone)]
pub struct HttpRouteMount {
    pub module_id: ModuleId,
    pub prefix: String,
    pub routes: Vec<HttpRouteDescriptor>,
    pub service: Arc<dyn ModuleHttpService>,
}

/// Runtime summary of a module.
///
/// 模块运行时摘要。
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub module_id: ModuleId,
    pub display_name: String,
    pub state: ModuleState,
}

/// Capabilities advertised by a module in its manifest.
///
/// 模块在其 manifest 中声明的能力。
#[derive(Debug, Clone)]
pub enum ModuleCapability {
    Publish,
    Subscribe,
    HttpApi,
    BackgroundJob,
    Room,
}

/// Module manifest describing metadata, dependencies and capabilities.
///
/// 模块 manifest，描述元数据、依赖和能力。
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    pub module_id: ModuleId,
    pub display_name: String,
    pub dependencies: Vec<ModuleId>,
    pub config_namespace: String,
    pub routes_prefix: String,
    pub capabilities: Vec<ModuleCapability>,
}

pub use crate::media_provider::{MediaServices, ProviderRegistration};

/// Capability injection handle passed to every module during initialization.
///
/// Provides access to runtime, stream management, tasks, events, config, and
/// auxiliary services (rooms, metrics, health, database, etc.).
///
/// 在初始化时传递给每个模块的能力注入句柄。
///
/// 提供对运行时、流管理、任务、事件、配置和辅助服务（房间、指标、健康、数据库等）的访问。
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
    pub media_services: MediaServices,
    pub media_session_directory: Arc<dyn MediaSessionDirectoryApi>,
    pub media_data_plane: Arc<dyn MediaDataPlaneApi>,
    pub media_file_store: Arc<dyn MediaFileStoreApi>,
    pub media_event_sender: Arc<dyn MediaEventSender>,
}

/// Context used to initialize a module: manifest, engine APIs, and initial config.
///
/// 初始化模块时使用的上下文：manifest、引擎 API 和初始配置。
#[derive(Clone)]
pub struct ModuleInitContext {
    pub manifest: ModuleManifest,
    pub engine: EngineContext,
    pub initial_config: serde_json::Value,
}

/// Module lifecycle contract. Implementations are registered with `ModuleFactory`.
///
/// 模块生命周期契约。实现通过 `ModuleFactory` 注册。
#[async_trait]
pub trait Module: Send + Sync {
    /// Return runtime module metadata.
    ///
    /// 返回运行时模块元数据。
    fn info(&self) -> ModuleInfo;
    /// Return the current lifecycle state.
    ///
    /// 返回当前生命周期状态。
    fn state(&self) -> ModuleState;

    /// Initialize the module with engine context and initial config.
    ///
    /// 使用引擎上下文和初始配置初始化模块。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError>;

    /// Start the module's main loop. The module should stop when the token is cancelled.
    ///
    /// 启动模块主循环。当 token 被取消时模块应停止。
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError>;

    /// Gracefully stop the module.
    ///
    /// 优雅停止模块。
    async fn stop(&mut self) -> Result<(), SdkError>;

    /// Apply a runtime configuration change and report the effect level.
    ///
    /// 应用运行时配置变更并报告影响级别。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError>;

    /// List HTTP routes exposed by this module. Empty by default.
    ///
    /// 列出模块暴露的 HTTP 路由。默认空。
    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        Vec::new()
    }

    /// Return the HTTP service handler for this module, if any.
    ///
    /// 返回模块的 HTTP 服务处理器（如有）。
    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        None
    }
}

/// Factory for creating module instances and exposing their manifest/config schema.
///
/// 创建模块实例并暴露其 manifest/配置 schema 的工厂。
pub trait ModuleFactory: Send + Sync {
    /// Return the module's manifest.
    ///
    /// 返回模块 manifest。
    fn manifest(&self) -> ModuleManifest;

    /// Create a new module instance.
    ///
    /// 创建新的模块实例。
    fn create(&self) -> Box<dyn Module>;

    /// Return the config schema registration, if any.
    ///
    /// 返回配置 schema 注册（如有）。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        None
    }
}
