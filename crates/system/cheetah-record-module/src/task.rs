//! Per-task state and command channel.
//!
//! Defines the task template, commands, and the runtime-neutral executor trait
//! used by the record module to spawn and stop recording tasks.
//!
//! 每个任务的状态与命令通道。
//!
//! 定义任务模板、命令以及录制模块用于启动和停止录制任务的运行时无关执行器 trait。

use cheetah_codec::RecordFormat;

pub use crate::metadata::RecordTaskState;

/// Template parameters for a record task.
///
/// Describes what to record (app/stream), from which stream key, and the
/// segmentation/duration limits. This is the static request side of a task.
///
/// 录制任务的模板参数。
///
/// 描述要录制的内容（app/stream）、源流键以及分片/时长限制。
/// 这是任务请求侧的静态部分。
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
///
/// Currently `Stop` and `UpdateSegments` are defined; the V1 executor only
/// handles `Stop`.
///
/// 活跃录制任务可接受的命令。
///
/// 当前定义了 `Stop` 与 `UpdateSegments`；V1 执行器仅处理 `Stop`。
#[derive(Debug, Clone)]
pub enum RecordTaskCommand {
    /// Stop and finalize the task.
    ///
    /// 停止并结束任务。
    Stop,
    /// Update the segment policy at runtime.
    ///
    /// 在运行时更新分片策略。
    UpdateSegments {
        segment_duration_ms: u64,
        segment_count_limit: u32,
    },
}

/// Logical handle to a running record task.
///
/// Carries the resolved task id and the concrete template. The executor uses
/// this to open a subscriber and drive the container writer.
///
/// 运行中录制任务的逻辑句柄。
///
/// 携带已解析的任务 ID 与具体模板。执行器用它打开订阅者并驱动容器写入器。
#[derive(Debug, Clone)]
pub struct RecordTask {
    pub task_id: String,
    pub template: RecordTaskTemplate,
}

/// Trait the record module uses to drive the runtime portion of a task.
///
/// The trait is runtime-neutral: a Tokio implementation lives in the engine
/// host and consumes a `SubscriberSource` to drive the writer chain.
///
/// 录制模块用于驱动任务运行时部分的 trait。
///
/// 该 trait 运行时无关：Tokio 实现位于引擎宿主中，通过消费 `SubscriberSource`
/// 来驱动写入器链。
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Spawn a record task. Returns once the executor has accepted the task.
    ///
    /// 启动一个录制任务。在执行器接受任务后返回。
    async fn spawn(&self, task: RecordTask) -> Result<(), TaskExecutorError>;

    /// Stop and finalize a task.
    ///
    /// 停止并结束一个任务。
    async fn stop(&self, task_id: &str) -> Result<(), TaskExecutorError>;
}

/// Errors returned by the task executor.
///
/// 任务执行器返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TaskExecutorError {
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("task creation failed: {0}")]
    SpawnFailed(String),
    #[error("internal: {0}")]
    Internal(String),
}
