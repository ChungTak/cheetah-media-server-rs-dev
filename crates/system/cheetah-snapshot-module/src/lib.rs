//! Snapshot feature module for the Cheetah media engine.
//!
//! Captures a still image from a live stream, writes it atomically to managed
//! storage, and publishes a `SnapshotCompleted` media event.
//!
//! 为 Cheetah 媒体引擎提供截图能力。从在线视频流捕获静止图像，原子写入
//! 受管存储并发布 `SnapshotCompleted` 媒体事件。

pub mod config;
pub mod media_provider;
pub mod module;
pub mod registry;

mod executor;

pub use module::SnapshotModuleFactory;
