use crate::error::SdkError;
use crate::ids::TaskId;
pub use cheetah_runtime_api::CancellationToken;
use serde::{Deserialize, Serialize};

/// Classification of a task in the task system.
///
/// 任务系统中任务的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Task,
    Job,
    Work,
    Channel,
}

/// Runtime state of a task.
///
/// 任务的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Running,
    Stopping,
    Stopped,
    Succeeded,
    Failed,
}

/// Final outcome reported when a task finishes.
///
/// 任务完成时报告的最终结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOutcome {
    Succeeded,
    Failed(String),
    Cancelled(Option<String>),
}

/// Simplified terminal state used in snapshots.
///
/// 快照中使用的简化终端状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskTerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// Snapshot of a task's runtime state.
///
/// 任务运行时状态快照。
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

/// API for creating, cancelling, finishing, and monitoring tasks.
///
/// 创建、取消、完成和监控任务的 API。
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
