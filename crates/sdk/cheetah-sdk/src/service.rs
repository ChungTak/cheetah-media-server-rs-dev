use std::collections::BTreeMap;

use async_trait::async_trait;

use cheetah_media_api::ids::MediaKey;

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

/// Typed input for an FFmpeg job.
///
/// FFmpeg 任务的类型化输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfmpegInput {
    /// Pull from a URL; the host has already been validated by the caller.
    Url { url: String },
}

/// Typed output for an FFmpeg job.
///
/// FFmpeg 任务的类型化输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfmpegOutput {
    /// Push to a URL; the host has already been validated by the caller.
    Url { url: String },
    /// Write to the engine's media pipeline for `media_key`.
    Engine { media_key: MediaKey },
}

/// Resource limits for an FFmpeg job.
///
/// FFmpeg 任务的资源限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfmpegResourceLimits {
    /// Maximum number of stderr lines retained for diagnostics.
    pub max_stderr_lines: usize,
    /// Maximum runtime in milliseconds before the job is killed.
    pub max_runtime_ms: u64,
}

impl Default for FfmpegResourceLimits {
    fn default() -> Self {
        Self {
            max_stderr_lines: 256,
            max_runtime_ms: 300_000,
        }
    }
}

/// FFmpeg job specification.
///
/// Replaces the previous `command: String` design; callers cannot pass arbitrary
/// executable names or shell strings. The profile is selected from a controlled
/// set of configured profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegJobSpec {
    pub profile_id: String,
    pub input: FfmpegInput,
    pub output: FfmpegOutput,
    pub input_options: Vec<String>,
    pub output_options: Vec<String>,
    pub resource_limits: FfmpegResourceLimits,
}

impl Default for FfmpegJobSpec {
    fn default() -> Self {
        Self {
            profile_id: "default".to_string(),
            input: FfmpegInput::Url { url: String::new() },
            output: FfmpegOutput::Url { url: String::new() },
            input_options: Vec::new(),
            output_options: Vec::new(),
            resource_limits: FfmpegResourceLimits::default(),
        }
    }
}

/// Lifecycle state of an FFmpeg job.
///
/// FFmpeg 任务生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegJobState {
    Pending,
    Running,
    Exited,
    Failed,
    Cancelled,
}

impl FfmpegJobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            FfmpegJobState::Exited | FfmpegJobState::Failed | FfmpegJobState::Cancelled
        )
    }
}

/// Snapshot of an FFmpeg job's status.
///
/// FFmpeg 任务状态快照。
#[derive(Debug, Clone)]
pub struct FfmpegJobStatus {
    pub job_id: String,
    pub state: FfmpegJobState,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub exit_summary: String,
    pub pid: Option<u32>,
}

/// Handle returned by `FfmpegApi::submit`.
///
/// `FfmpegApi::submit` 返回的句柄。
#[derive(Debug, Clone)]
pub struct FfmpegJobHandle {
    pub job_id: String,
    pub status: FfmpegJobStatus,
}

/// API for submitting and managing FFmpeg jobs.
///
/// 提交与管理 FFmpeg 任务的 API。
#[async_trait]
pub trait FfmpegApi: Send + Sync {
    async fn submit(
        &self,
        job_id: String,
        spec: FfmpegJobSpec,
    ) -> Result<FfmpegJobHandle, SdkError>;
    async fn get(&self, job_id: &str) -> Result<FfmpegJobStatus, SdkError>;
    async fn list(&self) -> Vec<FfmpegJobStatus>;
    async fn wait(&self, job_id: &str) -> Result<FfmpegJobStatus, SdkError>;
    async fn cancel(&self, job_id: &str) -> Result<(), SdkError>;
    /// Remove a finished job from the registry, releasing its memory.
    ///
    /// 从注册表中移除已结束的任务，释放其内存。
    async fn remove(&self, job_id: &str) -> Result<(), SdkError>;
    /// Whether the executor/provider is actually configured and ready to run
    /// FFmpeg jobs. Capability reports should not advertise ffmpeg-related
    /// operations when this returns false.
    ///
    /// executor/provider 是否已配置并可运行 FFmpeg 任务。返回 false 时，
    /// 能力报告不应宣告与 ffmpeg 相关的操作。
    fn is_available(&self) -> bool {
        false
    }
}
