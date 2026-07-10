//! Per-task state and command channel.

use cheetah_codec::RecordFormat;

pub use crate::metadata::RecordTaskState;

/// Template parameters for a record task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordTaskTemplate {
    pub format: RecordFormat,
    pub app: String,
    pub stream: String,
    pub source_stream_key: String,
    pub duration_limit_ms: u64,
    pub segment_duration_ms: u64,
    pub segment_count_limit: u32,
}

/// Commands accepted by an active record task.
#[derive(Debug, Clone)]
pub enum RecordTaskCommand {
    /// Stop and finalize the task.
    Stop,
    /// Update the segment policy at runtime.
    UpdateSegments {
        segment_duration_ms: u64,
        segment_count_limit: u32,
    },
}

/// Logical handle to a running record task.
#[derive(Debug, Clone)]
pub struct RecordTask {
    pub task_id: String,
    pub template: RecordTaskTemplate,
}

/// Trait the record module uses to drive the runtime portion of a task.
///
/// The trait is runtime-neutral: a Tokio implementation lives in the engine
/// host and consumes a `SubscriberSource` to drive the writer chain.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Spawn a record task. Returns the task id once the executor has
    /// established a subscription and writer.
    async fn spawn(&self, task: RecordTask) -> Result<(), TaskExecutorError>;

    /// Stop and finalize a task.
    async fn stop(&self, task_id: &str) -> Result<(), TaskExecutorError>;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TaskExecutorError {
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("task creation failed: {0}")]
    SpawnFailed(String),
    #[error("internal: {0}")]
    Internal(String),
}
