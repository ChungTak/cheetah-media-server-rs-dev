use crate::error::SdkError;
use crate::ids::TaskId;
pub use cheetah_runtime_api::CancellationToken;
use serde::{Deserialize, Serialize};

/// Kind of `Task`.
/// `Task` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Task,
    Job,
    Work,
    Channel,
}

/// State used by `Task`.
/// `Task` 使用的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Running,
    Stopping,
    Stopped,
    Succeeded,
    Failed,
}

/// `TaskOutcome` enumeration.
/// `TaskOutcome` 枚举。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOutcome {
    Succeeded,
    Failed(String),
    Cancelled(Option<String>),
}

/// `TaskTerminalOutcome` enumeration.
/// `TaskTerminalOutcome` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskTerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// `TaskSnapshot` data structure.
/// `TaskSnapshot` 数据结构。
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub parent_id: Option<TaskId>,
    pub kind: TaskKind,
    pub state: TaskState,
    pub terminal_outcome: Option<TaskTerminalOutcome>,
    pub owner: String,
    pub label: String,
    pub level: u8,
    pub child_ids: Vec<TaskId>,
    pub started_unix_millis: u64,
    pub updated_unix_millis: u64,
    pub finished_unix_millis: Option<u64>,
    pub cancel_reason: Option<String>,
    pub finish_message: Option<String>,
    pub spawn_site: String,
}

/// API surface for `Task System`.
/// `Task System` 的 API 接口。
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
