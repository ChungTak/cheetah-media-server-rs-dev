//! Snapshot module: capture a random-access video frame from a live stream.
//!
//! Flow:
//! 1. Authorize / resolve MediaKey
//! 2. Open a bounded frame subscriber
//! 3. Wait for a keyframe (or MJPEG frame) within deadline
//! 4. Persist payload under a managed file path
//! 5. Register FileHandle + SnapshotInfo
//! 6. Publish SnapshotCompleted
//!
//! Encoding: H.264/H.265 keyframes and MJPEG frames are decoded and re-encoded
//! as JPEG through the shared `ImageProcessApi` backed by avcodec-rs.

mod config;
mod media_provider;
mod module;
mod registry;

pub use config::SnapshotModuleConfig;
pub use media_provider::SnapshotMediaProvider;
pub use module::{SnapshotModule, SnapshotModuleFactory};
pub use registry::SnapshotRegistry;
