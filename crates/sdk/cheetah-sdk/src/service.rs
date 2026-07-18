use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::config::{ConfigEffect, ModuleConfigChange};
use crate::ids::{ModuleId, RoomId, StreamKey};
use crate::module::{HttpRouteMount, ModuleState};
use crate::SdkError;

/// Report returned after applying a config change to a module.
///
/// 对模块应用配置变更后返回的报告。
#[derive(Debug, Clone)]
pub struct ModuleConfigApplyReport {
    pub module_id: ModuleId,
    pub effect: ConfigEffect,
}

/// API for module lifecycle management and HTTP route mounting.
///
/// 模块生命周期管理和 HTTP 路由挂载的 API。
#[async_trait]
pub trait ModuleManagerApi: Send + Sync {
    fn modules(&self) -> Vec<(ModuleId, ModuleState)>;

    fn http_mounts(&self) -> Vec<HttpRouteMount>;

    async fn apply_module_config_change(
        &self,
        change: ModuleConfigChange,
    ) -> Result<ModuleConfigApplyReport, SdkError>;

    async fn apply_module_config_changes(
        &self,
        changes: Vec<ModuleConfigChange>,
    ) -> Result<Vec<ModuleConfigApplyReport>, SdkError>;

    async fn restart_module(&self, module_id: &ModuleId) -> Result<(), SdkError>;

    async fn restart_modules(&self, module_ids: Vec<ModuleId>) -> Result<(), SdkError>;
}

/// Participant in a room.
///
/// 房间中的参与者。
#[derive(Debug, Clone)]
pub struct RoomParticipant {
    pub id: String,
}

/// Snapshot of a room and its participants/streams.
///
/// 房间及其参与者/流的快照。
#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub room_id: RoomId,
    pub name: String,
    pub participants: Vec<RoomParticipant>,
    pub bound_streams: Vec<StreamKey>,
}

/// API for room lifecycle and stream binding.
///
/// 房间生命周期和流绑定的 API。
pub trait RoomServiceApi: Send + Sync {
    fn create_room(&self, name: &str) -> Result<RoomId, SdkError>;
    fn delete_room(&self, room_id: RoomId) -> Result<(), SdkError>;
    fn join_room(&self, room_id: RoomId, participant_id: &str) -> Result<(), SdkError>;
    fn leave_room(&self, room_id: RoomId, participant_id: &str) -> Result<(), SdkError>;
    fn bind_stream(&self, room_id: RoomId, stream_key: StreamKey) -> Result<(), SdkError>;
    fn unbind_stream(&self, room_id: RoomId, stream_key: &StreamKey) -> Result<(), SdkError>;
    fn get_room(&self, room_id: RoomId) -> Result<Option<RoomSnapshot>, SdkError>;
    fn snapshot(&self) -> Vec<RoomSnapshot>;
}

/// API for rendering metrics in text format and recording counters.
///
/// 以文本格式渲染指标并记录计数器的 API。
pub trait MetricsApi: Send + Sync {
    /// Increment a monotonic counter by the given delta.
    ///
    /// 按给定增量递增单调计数器。
    fn inc(&self, key: &str, value: u64);

    fn render(&self) -> String;
}

/// API for liveness/readiness probes.
///
/// 存活/就绪探针的 API。
pub trait HealthApi: Send + Sync {
    fn is_live(&self) -> bool;
    fn is_ready(&self) -> bool;
}

/// Service registration descriptor for the service registry.
///
/// 服务注册表的服务注册描述符。
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    pub name: String,
    pub endpoint: String,
    pub metadata: BTreeMap<String, String>,
}

/// API for service registration and lookup.
///
/// 服务注册与查找的 API。
pub trait ServiceRegistry: Send + Sync {
    fn register(&self, service: ServiceDescriptor) -> Result<(), SdkError>;
    fn get(&self, name: &str) -> Option<ServiceDescriptor>;
    fn unregister(&self, name: &str) -> Result<(), SdkError>;
    fn list_services(&self) -> Vec<ServiceDescriptor>;
}

/// Simple key/value database API.
///
/// 简单键值数据库 API。
pub trait DatabaseApi: Send + Sync {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), SdkError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SdkError>;
    fn delete(&self, key: &str) -> Result<(), SdkError>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>, SdkError>;
}

/// HTTP proxy route mapping.
///
/// HTTP 代理路由映射。
#[derive(Debug, Clone)]
pub struct ProxyRoute {
    pub path_prefix: String,
    pub target: String,
}

/// API for registering/removing HTTP proxy routes.
///
/// 注册/移除 HTTP 代理路由的 API。
pub trait ProxyManager: Send + Sync {
    fn register_route(&self, route: ProxyRoute) -> Result<(), SdkError>;
    fn remove_route(&self, path_prefix: &str) -> Result<(), SdkError>;
    fn list_routes(&self) -> Vec<ProxyRoute>;
}

/// Cluster node metadata.
///
/// 集群节点元数据。
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub node_id: String,
    pub endpoint: String,
}

/// API for managing cluster membership.
///
/// 管理集群成员关系的 API。
pub trait ClusterApi: Send + Sync {
    fn set_local_node(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn upsert_peer(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn remove_peer(&self, node_id: &str) -> Result<(), SdkError>;
    fn list_nodes(&self) -> Vec<ClusterNode>;
}
