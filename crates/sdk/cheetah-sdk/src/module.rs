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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    Created,
    Initialized,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

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

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Bytes,
}

impl HttpResponse {
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

#[derive(Clone)]
pub struct HttpRouteMount {
    pub module_id: ModuleId,
    pub prefix: String,
    pub routes: Vec<HttpRouteDescriptor>,
    pub service: Arc<dyn ModuleHttpService>,
}

#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub module_id: ModuleId,
    pub display_name: String,
    pub state: ModuleState,
}

#[derive(Debug, Clone)]
pub enum ModuleCapability {
    Publish,
    Subscribe,
    HttpApi,
    BackgroundJob,
    Room,
}

#[derive(Debug, Clone)]
pub struct ModuleManifest {
    pub module_id: ModuleId,
    pub display_name: String,
    pub dependencies: Vec<ModuleId>,
    pub config_namespace: String,
    pub routes_prefix: String,
    pub capabilities: Vec<ModuleCapability>,
}

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

#[derive(Clone)]
pub struct ModuleInitContext {
    pub manifest: ModuleManifest,
    pub engine: EngineContext,
    pub initial_config: serde_json::Value,
}

#[async_trait]
pub trait Module: Send + Sync {
    fn info(&self) -> ModuleInfo;
    fn state(&self) -> ModuleState;

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError>;

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError>;

    async fn stop(&mut self) -> Result<(), SdkError>;

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError>;

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        Vec::new()
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        None
    }
}

pub trait ModuleFactory: Send + Sync {
    fn manifest(&self) -> ModuleManifest;
    fn create(&self) -> Box<dyn Module>;

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        None
    }
}
