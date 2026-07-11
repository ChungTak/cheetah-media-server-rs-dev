//! High-level protocol connector facade for external Cheetah integrators.
//!
//! `cheetah-connector` is the only public composition layer for integrators. It
//! wraps `cheetah-engine`, `cheetah-sdk`, and the protocol `*-module` crates
//! behind a stable `(protocol, url, options) -> PullHandle / PushHandle` API.
//!
//! 面向外部 Cheetah 集成者的高层协议 connector facade。
//!
//! `cheetah-connector` 是集成者唯一的公共组合层。它在稳定的
//! `(protocol, url, options) -> PullHandle / PushHandle` API 后包装
//! `cheetah-engine`、`cheetah-sdk` 与协议 `*-module` crate。
//!
//! # Capability matrix
//!
//! | Protocol | Pull | Push |
//! | --- | --- | --- |
//! | RTSP | yes | no |
//! | HTTP-FLV | yes | no |
//! | RTMP | no | yes |
//! | WebRTC | no | yes |
//!
//! Unlisted protocol/direction pairs return [`ConnectorError::UnsupportedProtocol`].
//!
//! # Feature flags
//!
//! - `rtsp` — enable RTSP pull.
//! - `http-flv` — enable HTTP-FLV pull.
//! - `rtmp` — enable RTMP push.
//! - `webrtc` — enable WebRTC push.
//! - `loopback` — enable `open_in_memory_loopback` (requires `rtmp` + `http-flv`).
//! - `full` — all of the above.

pub mod error;
pub mod options;
pub mod protocol;

mod connector;
mod engine_bootstrap;
mod handles;
mod pull;
mod push;

#[cfg(feature = "loopback")]
mod loopback;

pub use connector::{EngineConnector, RuntimeConnector};
pub use engine_bootstrap::ConnectorBuilder;
pub use error::{CloseReason, ConnectorError, Operation};
pub use handles::{LoopbackPair, PullHandle, PushHandle};

#[cfg(feature = "loopback")]
pub use loopback::open_in_memory_loopback;
pub use options::{
    ConnectorPullOptions, ConnectorPushOptions, LoopbackLayer, LoopbackOptions, LoopbackTopology,
};
pub use protocol::{supports, Direction, Protocol};

// Re-export SDK runtime types needed by callers.
pub use cheetah_runtime_api::CancellationToken;
