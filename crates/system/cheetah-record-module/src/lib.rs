//! Unified multi-format record module.
//!
//! Manages record tasks for the engine and dispatches per-format writers
//! exposed by `cheetah-codec::record`. The HTTP API surface is SMS-compatible
//! (`/api/v1/record/*`).
//!
//! The crate is intentionally framework-neutral — the runtime hosting the
//! HTTP service is the engine's HTTP module wrapper, not a hard-coded Axum
//! dependency. Concrete disk I/O is performed by the runtime layer.

/// Module for `api`.
/// `api` 相关模块。
pub mod api;
/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `executor`.
/// `executor` 相关模块。
pub mod executor;
/// Module for `metadata`.
/// `metadata` 相关模块。
pub mod metadata;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;
/// Module for `registry`.
/// `registry` 相关模块。
pub mod registry;
/// Module for `task`.
/// `task` 相关模块。
pub mod task;
/// Module for `zlm_compat`.
/// `zlm_compat` 相关模块。
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
