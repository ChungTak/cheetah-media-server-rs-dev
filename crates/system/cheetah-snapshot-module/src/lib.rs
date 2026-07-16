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
//! Encoding: MJPEG frames are stored as JPEG. Other codecs store the keyframe
//! payload as `application/octet-stream` with the requested extension when a
//! real image encoder is unavailable, returning `Unsupported` only when no
//! video keyframe arrives before the deadline.

mod config;
mod image_encode;
mod media_provider;
mod module;
mod registry;

pub use config::SnapshotModuleConfig;
pub use image_encode::ImageEncoderBackend;
pub use media_provider::SnapshotMediaProvider;
pub use module::{SnapshotModule, SnapshotModuleFactory};
pub use registry::SnapshotRegistry;
