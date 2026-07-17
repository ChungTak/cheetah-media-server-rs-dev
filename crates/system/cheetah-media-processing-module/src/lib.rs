//! Media processing module.
//!
//! Provides optional `ImageProcessApi` and `MediaProcessingApi` providers backed by
//! `avcodec-rs` when the corresponding Cargo features are enabled. All blocking
//! codec work is scheduled through `RuntimeApi::spawn_blocking`.
//!
//! `media-processing-cpu` is a convenience feature that enables all single-stream
//! capabilities and selects `avcodec/profile-native-free`. Real profile selection
//! should be explicit; the default feature set is empty so the module does not
//! compile `avcodec` by default.

pub mod config;
pub mod module;

mod provider;

pub use module::{MediaProcessingModule, MediaProcessingModuleFactory};
