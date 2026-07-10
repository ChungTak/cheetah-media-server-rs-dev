//! Unified multi-format record module.
//!
//! Manages record tasks for the engine and dispatches per-format writers
//! exposed by `cheetah-codec::record`. The HTTP API surface is SMS-compatible
//! (`/api/v1/record/*`).
//!
//! The crate is intentionally framework-neutral — the runtime hosting the
//! HTTP service is the engine's HTTP module wrapper, not a hard-coded Axum
//! dependency. Concrete disk I/O is performed by the runtime layer.
//!
//! 统一的多格式录制模块。
//!
//! 管理引擎录制任务并调度 `cheetah-codec::record` 暴露的各格式写入器。
//! HTTP API 接口与 SMS 兼容（`/api/v1/record/*`）。
//!
//! 本 crate 刻意保持框架无关：HTTP 服务由引擎的 HTTP 模块包装器承载，
//! 而不是硬编码的 Axum 依赖。具体磁盘 I/O 由运行时层执行。

pub mod api;
pub mod config;
pub mod executor;
pub mod metadata;
pub mod module;
pub mod registry;
pub mod task;
pub mod zlm_compat;

pub use api::{RecordApi, RecordApiError};
pub use config::RecordModuleConfig;
pub use metadata::{RecordFileMetadata, RecordFileQuery, RecordTaskMetadata};
pub use module::{RecordModule, RecordModuleFactory};
pub use registry::{RecordRegistry, RegistryError};
pub use task::{RecordTask, RecordTaskCommand, RecordTaskState, RecordTaskTemplate, TaskExecutor};
pub use zlm_compat::{
    ZlmCompatError, ZlmDeleteDirectory, ZlmGetMp4Files, ZlmIsRecording, ZlmRecordCompat,
    ZlmStartRecord, ZlmStopRecord,
};
