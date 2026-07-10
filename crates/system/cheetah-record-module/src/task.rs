//! Per-task state and command channel.

use cheetah_codec::RecordFormat;

pub use crate::metadata::RecordTaskState;

/// Template parameters for a record task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordTaskTemplate {
    /// `format` field of type `RecordFormat`.
    /// `format` 字段，类型为 `RecordFormat`.
    pub format: RecordFormat,
    /// `app` field of type `String`.
    /// `app` 字段，类型为 `String`.
    pub app: String,
    /// `stream` field of type `String`.
    /// `stream` 字段，类型为 `String`.
    pub stream: String,
    /// `source_stream_key` field of type `String`.
    /// `source_stream_key` 字段，类型为 `String`.
    pub source_stream_key: String,
    /// `duration_limit_ms` field of type `u64`.
    /// `duration_limit_ms` 字段，类型为 `u64`.
    pub duration_limit_ms: u64,
    /// `segment_duration_ms` field of type `u64`.
    /// `segment_duration_ms` 字段，类型为 `u64`.
    pub segment_duration_ms: u64,
    /// `segment_count_limit` field of type `u32`.
    /// `segment_count_limit` 字段，类型为 `u32`.
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
    /// `task_id` field of type `String`.
    /// `task_id` 字段，类型为 `String`.
    pub task_id: String,
    /// `template` field of type `RecordTaskTemplate`.
    /// `template` 字段，类型为 `RecordTaskTemplate`.
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

/// `TaskExecutorError` enumeration.
/// `TaskExecutorError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TaskExecutorError {
    /// `NotFound` variant.
    /// `NotFound` 变体.
    #[error("task not found: {0}")]
    NotFound(String),
    /// `SpawnFailed` variant.
    /// `SpawnFailed` 变体.
    #[error("task creation failed: {0}")]
    SpawnFailed(String),
    /// `Internal` variant.
    /// `Internal` 变体.
    #[error("internal: {0}")]
    Internal(String),
}
