use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::config::{ConfigEffect, ModuleConfigChange};
use crate::ids::{ModuleId, RoomId, StreamKey};
use crate::module::{HttpRouteMount, ModuleState};
use crate::SdkError;

/// `ModuleConfigApplyReport` data structure.
/// `ModuleConfigApplyReport` 数据结构.
#[derive(Debug, Clone)]
pub struct ModuleConfigApplyReport {
    /// `module_id` field of type `ModuleId`.
    /// `module_id` 字段，类型为 `ModuleId`.
    pub module_id: ModuleId,
    /// `effect` field of type `ConfigEffect`.
    /// `effect` 字段，类型为 `ConfigEffect`.
    pub effect: ConfigEffect,
}

/// `ModuleManagerApi` trait.
/// `ModuleManagerApi` trait.
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

/// `RoomParticipant` data structure.
/// `RoomParticipant` 数据结构.
#[derive(Debug, Clone)]
pub struct RoomParticipant {
    /// `id` field of type `String`.
    /// `id` 字段，类型为 `String`.
    pub id: String,
}

/// `RoomSnapshot` data structure.
/// `RoomSnapshot` 数据结构.
#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    /// `room_id` field of type `RoomId`.
    /// `room_id` 字段，类型为 `RoomId`.
    pub room_id: RoomId,
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `participants` field.
    /// `participants` 字段.
    pub participants: Vec<RoomParticipant>,
    /// `bound_streams` field.
    /// `bound_streams` 字段.
    pub bound_streams: Vec<StreamKey>,
}

/// `RoomServiceApi` trait.
/// `RoomServiceApi` trait.
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

/// `MetricsApi` trait.
/// `MetricsApi` trait.
pub trait MetricsApi: Send + Sync {
    fn render(&self) -> String;
}

/// `HealthApi` trait.
/// `HealthApi` trait.
pub trait HealthApi: Send + Sync {
    fn is_live(&self) -> bool;
    fn is_ready(&self) -> bool;
}

/// `ServiceDescriptor` data structure.
/// `ServiceDescriptor` 数据结构.
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    /// `name` field of type `String`.
    /// `name` 字段，类型为 `String`.
    pub name: String,
    /// `endpoint` field of type `String`.
    /// `endpoint` 字段，类型为 `String`.
    pub endpoint: String,
    /// `metadata` field.
    /// `metadata` 字段.
    pub metadata: BTreeMap<String, String>,
}

/// `ServiceRegistry` trait.
/// `ServiceRegistry` trait.
pub trait ServiceRegistry: Send + Sync {
    fn register(&self, service: ServiceDescriptor) -> Result<(), SdkError>;
    fn get(&self, name: &str) -> Option<ServiceDescriptor>;
    fn unregister(&self, name: &str) -> Result<(), SdkError>;
    fn list_services(&self) -> Vec<ServiceDescriptor>;
}

/// `DatabaseApi` trait.
/// `DatabaseApi` trait.
pub trait DatabaseApi: Send + Sync {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), SdkError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SdkError>;
    fn delete(&self, key: &str) -> Result<(), SdkError>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>, SdkError>;
}

/// `ProxyRoute` data structure.
/// `ProxyRoute` 数据结构.
#[derive(Debug, Clone)]
pub struct ProxyRoute {
    /// `path_prefix` field of type `String`.
    /// `path_prefix` 字段，类型为 `String`.
    pub path_prefix: String,
    /// `target` field of type `String`.
    /// `target` 字段，类型为 `String`.
    pub target: String,
}

/// `ProxyManager` trait.
/// `ProxyManager` trait.
pub trait ProxyManager: Send + Sync {
    fn register_route(&self, route: ProxyRoute) -> Result<(), SdkError>;
    fn remove_route(&self, path_prefix: &str) -> Result<(), SdkError>;
    fn list_routes(&self) -> Vec<ProxyRoute>;
}

/// `ClusterNode` data structure.
/// `ClusterNode` 数据结构.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    /// `node_id` field of type `String`.
    /// `node_id` 字段，类型为 `String`.
    pub node_id: String,
    /// `endpoint` field of type `String`.
    /// `endpoint` 字段，类型为 `String`.
    pub endpoint: String,
}

/// `ClusterApi` trait.
/// `ClusterApi` trait.
pub trait ClusterApi: Send + Sync {
    fn set_local_node(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn upsert_peer(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn remove_peer(&self, node_id: &str) -> Result<(), SdkError>;
    fn list_nodes(&self) -> Vec<ClusterNode>;
}

/// `FfmpegJob` data structure.
/// `FfmpegJob` 数据结构.
#[derive(Debug, Clone)]
pub struct FfmpegJob {
    /// `job_id` field of type `String`.
    /// `job_id` 字段，类型为 `String`.
    pub job_id: String,
    /// `command` field of type `String`.
    /// `command` 字段，类型为 `String`.
    pub command: String,
}

/// `FfmpegApi` trait.
/// `FfmpegApi` trait.
pub trait FfmpegApi: Send + Sync {
    fn submit_job(&self, job: FfmpegJob) -> Result<(), SdkError>;
    fn cancel_job(&self, job_id: &str) -> Result<(), SdkError>;
    fn list_jobs(&self) -> Vec<FfmpegJob>;
}
