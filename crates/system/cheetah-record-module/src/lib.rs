//! Unified multi-format record module.
//!
//! Manages record tasks for the engine and dispatches per-format writers
//! exposed by `cheetah-codec::record`. The HTTP API surface is SMS-compatible
//! (`/api/v1/record/*`).
//!
//! The crate is intentionally framework-neutral — the runtime hosting the
//! HTTP service is the engine's HTTP module wrapper, not a hard-coded Axum
//! dependency. Concrete disk I/O is performed by the runtime layer.

/// `api` module.
/// `api` 模块.
pub mod api;
/// `config` module.
/// `config` 模块.
pub mod config;
/// `executor` module.
/// `executor` 模块.
pub mod executor;
/// `metadata` module.
/// `metadata` 模块.
pub mod metadata;
/// `module` module.
/// `模块` 模块.
pub mod module;
/// `registry` module.
/// `registry` 模块.
pub mod registry;
/// `task` module.
/// `task` 模块.
pub mod task;
/// `zlm_compat` module.
/// `zlm_compat` 模块.
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
