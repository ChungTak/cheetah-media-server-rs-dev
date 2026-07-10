use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::config::{ConfigEffect, ModuleConfigChange};
use crate::ids::{ModuleId, RoomId, StreamKey};
use crate::module::{HttpRouteMount, ModuleState};
use crate::SdkError;

#[derive(Debug, Clone)]
pub struct ModuleConfigApplyReport {
    pub module_id: ModuleId,
    pub effect: ConfigEffect,
}

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

#[derive(Debug, Clone)]
pub struct RoomParticipant {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub room_id: RoomId,
    pub name: String,
    pub participants: Vec<RoomParticipant>,
    pub bound_streams: Vec<StreamKey>,
}

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

pub trait MetricsApi: Send + Sync {
    fn render(&self) -> String;
}

pub trait HealthApi: Send + Sync {
    fn is_live(&self) -> bool;
    fn is_ready(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    pub name: String,
    pub endpoint: String,
    pub metadata: BTreeMap<String, String>,
}

pub trait ServiceRegistry: Send + Sync {
    fn register(&self, service: ServiceDescriptor) -> Result<(), SdkError>;
    fn get(&self, name: &str) -> Option<ServiceDescriptor>;
    fn unregister(&self, name: &str) -> Result<(), SdkError>;
    fn list_services(&self) -> Vec<ServiceDescriptor>;
}

pub trait DatabaseApi: Send + Sync {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), SdkError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SdkError>;
    fn delete(&self, key: &str) -> Result<(), SdkError>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>, SdkError>;
}

#[derive(Debug, Clone)]
pub struct ProxyRoute {
    pub path_prefix: String,
    pub target: String,
}

pub trait ProxyManager: Send + Sync {
    fn register_route(&self, route: ProxyRoute) -> Result<(), SdkError>;
    fn remove_route(&self, path_prefix: &str) -> Result<(), SdkError>;
    fn list_routes(&self) -> Vec<ProxyRoute>;
}

#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub node_id: String,
    pub endpoint: String,
}

pub trait ClusterApi: Send + Sync {
    fn set_local_node(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn upsert_peer(&self, node: ClusterNode) -> Result<(), SdkError>;
    fn remove_peer(&self, node_id: &str) -> Result<(), SdkError>;
    fn list_nodes(&self) -> Vec<ClusterNode>;
}

#[derive(Debug, Clone)]
pub struct FfmpegJob {
    pub job_id: String,
    pub command: String,
}

pub trait FfmpegApi: Send + Sync {
    fn submit_job(&self, job: FfmpegJob) -> Result<(), SdkError>;
    fn cancel_job(&self, job_id: &str) -> Result<(), SdkError>;
    fn list_jobs(&self) -> Vec<FfmpegJob>;
}
