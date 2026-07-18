//! Proxy module: manages pull, push, and FFmpeg stream proxies.
//!
//! Provides a real [`ProxyApi`] implementation registered with
//! [`cheetah_sdk::MediaServices`] so that the engine facade can delegate ZLM
//! and native proxy routes to a single provider.
//!
//! Data-plane behaviour is feature-gated:
//! - `rtsp` — RTSP pull publishes into the destination engine stream
//! - `http-flv` — HTTP-FLV pull bridges frames into an engine publisher
//! - `rtmp` — RTMP push bridges an engine subscriber to a remote publish URL
//! - `processing` — pull streams can be routed through `ProcessingPolicy` derived
//!   transcode jobs when the source codec is not compatible with the destination

pub mod config;
pub mod media_provider;
pub mod module;
#[allow(dead_code)]
pub(crate) mod processing;
pub mod registry;
pub(crate) mod ssrf;
pub mod task;

pub use config::ProxyModuleConfig;
pub use media_provider::ProxyMediaProvider;
pub use module::{ProxyModule, ProxyModuleFactory};
pub use registry::{ProxyEntry, ProxyRegistry};
