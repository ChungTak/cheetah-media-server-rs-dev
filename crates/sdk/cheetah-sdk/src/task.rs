use crate::error::SdkError;
use crate::ids::TaskId;
pub use cheetah_runtime_api::CancellationToken;
use serde::{Deserialize, Serialize};

/// `TaskKind` enumeration.
/// `TaskKind` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    /// `Task` variant.
    /// `Task` 变体.
    Task,
    /// `Job` variant.
    /// `Job` 变体.
    Job,
    /// `Work` variant.
    /// `Work` 变体.
    Work,
    /// `Channel` variant.
    /// `Channel` 变体.
    Channel,
}

/// `TaskState` enumeration.
/// `TaskState` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// `Running` variant.
    /// `Running` 变体.
    Running,
    /// `Stopping` variant.
    /// `Stopping` 变体.
    Stopping,
    /// `Stopped` variant.
    /// `Stopped` 变体.
    Stopped,
    /// `Succeeded` variant.
    /// `Succeeded` 变体.
    Succeeded,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed,
}

/// `TaskOutcome` enumeration.
/// `TaskOutcome` 枚举.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOutcome {
    /// `Succeeded` variant.
    /// `Succeeded` 变体.
    Succeeded,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed(String),
    /// `Cancelled` variant.
    /// `Cancelled` 变体.
    Cancelled(Option<String>),
}

/// `TaskTerminalOutcome` enumeration.
/// `TaskTerminalOutcome` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskTerminalOutcome {
    /// `Succeeded` variant.
    /// `Succeeded` 变体.
    Succeeded,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed,
    /// `Cancelled` variant.
    /// `Cancelled` 变体.
    Cancelled,
}

/// `TaskSnapshot` data structure.
/// `TaskSnapshot` 数据结构.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    /// `id` field of type `TaskId`.
    /// `id` 字段，类型为 `TaskId`.
    pub id: TaskId,
    /// `parent_id` field.
    /// `parent_id` 字段.
    pub parent_id: Option<TaskId>,
    /// `kind` field of type `TaskKind`.
    /// `kind` 字段，类型为 `TaskKind`.
    pub kind: TaskKind,
    /// `state` field of type `TaskState`.
    /// `state` 字段，类型为 `TaskState`.
    pub state: TaskState,
    /// `terminal_outcome` field.
    /// `terminal_outcome` 字段.
    pub terminal_outcome: Option<TaskTerminalOutcome>,
    /// `owner` field of type `String`.
    /// `owner` 字段，类型为 `String`.
    pub owner: String,
    /// `label` field of type `String`.
    /// `label` 字段，类型为 `String`.
    pub label: String,
    /// `level` field of type `u8`.
    /// `level` 字段，类型为 `u8`.
    pub level: u8,
    /// `child_ids` field.
    /// `child_ids` 字段.
    pub child_ids: Vec<TaskId>,
    /// `started_unix_millis` field of type `u64`.
    /// `started_unix_millis` 字段，类型为 `u64`.
    pub started_unix_millis: u64,
    /// `updated_unix_millis` field of type `u64`.
    /// `updated_unix_millis` 字段，类型为 `u64`.
    pub updated_unix_millis: u64,
    /// `finished_unix_millis` field.
    /// `finished_unix_millis` 字段.
    pub finished_unix_millis: Option<u64>,
    /// `cancel_reason` field.
    /// `cancel_reason` 字段.
    pub cancel_reason: Option<String>,
    /// `finish_message` field.
    /// `finish_message` 字段.
    pub finish_message: Option<String>,
    /// `spawn_site` field of type `String`.
    /// `spawn_site` 字段，类型为 `String`.
    pub spawn_site: String,
}

/// `TaskSystemApi` trait.
/// `TaskSystemApi` trait.
pub trait TaskSystemApi: Send + Sync {
    #[track_caller]
    fn create_task(
        &self,
        parent_id: Option<TaskId>,
        kind: TaskKind,
        owner: &str,
        label: &str,
    ) -> Result<TaskId, SdkError>;

    fn cancel(&self, task_id: TaskId, reason: Option<&str>) -> Result<(), SdkError>;

    fn finish(&self, task_id: TaskId, outcome: TaskOutcome) -> Result<(), SdkError>;

    fn token(&self, task_id: TaskId) -> Result<CancellationToken, SdkError>;

    fn snapshot(&self) -> Vec<TaskSnapshot>;
}
