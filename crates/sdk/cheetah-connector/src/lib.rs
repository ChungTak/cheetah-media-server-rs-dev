//! High-level protocol connector facade for external Cheetah integrators.
//!
//! `cheetah-connector` exposes a small, stable API that lets callers pull and push
//! media streams over the first-party protocol set without depending on the
//! engine internals. All public types are runtime-neutral.
//!
//! 面向外部 Cheetah 集成者的高层协议 connector facade。
//!
//! `cheetah-connector` 暴露一个精简、稳定的 API，允许调用方通过官方协议集
//! 拉/推媒体流，而无需依赖引擎内部。所有公开类型都是 runtime 中立的。
//!
//! # Capability matrix
//!
//! | Protocol | Pull | Push |
//! | --- | --- | --- |
//! | RTSP | no (adapter pending) | no |
//! | HTTP-FLV | yes | no |
//! | RTMP | no | yes |
//! | WebRTC | no | no (adapter pending) |
//!
//! Unlisted protocol/direction pairs return [`ConnectorError::UnsupportedProtocol`].
//!
//! `rtsp` pull and `webrtc` push adapters are declared but not yet wired in this
//! build; the `rtsp` and `webrtc` features enable the underlying modules but
//! `supports()` returns `false` for those pairs until the connector adapters are
//! completed.
//!
//! # Metadata contract
//!
//! The cross-protocol loopback (RTMP push → HTTP-FLV pull) preserves the fields
//! listed in [`WIRE_METADATA_MUST_PRESERVED`]. It does **not** preserve the fields
//! listed in [`WIRE_METADATA_NOT_PRESERVED`]; those fields are dropped or reset
//! by the protocol ingress/egress layers.
//!
//! # Feature flags
//!
//! - `rtsp` — enable the RTSP module (connector pull adapter pending).
//! - `http-flv` — enable HTTP-FLV pull.
//! - `rtmp` — enable RTMP push.
//! - `webrtc` — enable the WebRTC module and the in-process media loopback fixture
//!   (connector push adapter pending).
//! - `loopback` — enable `RuntimeConnector::open_in_memory_loopback` (implies `rtmp` + `http-flv`).
//! - `full` — enable all of the above.

pub mod error;
#[cfg(feature = "loopback")]
mod loopback;
pub mod options;
mod protocol;

#[cfg(feature = "http-flv")]
mod pull;
#[cfg(feature = "rtmp")]
mod push;

mod connector;
mod engine_bootstrap;
mod handles;

pub use connector::{EngineConnector, RuntimeConnector};
pub use engine_bootstrap::ConnectorBuilder;
pub use error::{CloseReason, ConnectorError, Operation};
pub use handles::{LoopbackPair, PullHandle, PushHandle};
#[cfg(feature = "rtmp")]
pub use options::RtmpPushExtras;
pub use options::{
    ConnectorPullOptions, ConnectorPushOptions, LoopbackLayer, LoopbackOptions, LoopbackTopology,
    ProtocolPullExtras, ProtocolPushExtras,
};
pub use protocol::{Direction, Protocol};

// Re-export the cancellation token that the public API already accepts.
pub use cheetah_runtime_api::CancellationToken;

use crate::protocol::supports as inner_supports;

/// Returns whether the connector first-party capability matrix supports this pair.
///
/// 返回 connector 官方能力矩阵是否支持该协议/方向组合。
///
/// See the crate-level capability matrix for the current status.
pub fn supports(protocol: Protocol, direction: Direction) -> bool {
    inner_supports(protocol, direction)
}

/// Fields that `cheetah-connector` guarantees to preserve across the RTMP push →
/// HTTP-FLV pull wire path.
///
/// 保证在 RTMP 推 → HTTP-FLV 拉 wire 路径中被保留的字段。
pub const WIRE_METADATA_MUST_PRESERVED: &[&str] = &[
    "track_id",
    "media_kind",
    "codec",
    "format",
    "pts",
    "dts",
    "timebase",
    "pts_us",
    "dts_us",
    "keyframe",
    "payload",
];

/// Fields that `cheetah-connector` explicitly does **not** preserve across the
/// RTMP push → HTTP-FLV pull wire path.
///
/// 明确不保证在 RTMP 推 → HTTP-FLV 拉 wire 路径中被保留的字段。
pub const WIRE_METADATA_NOT_PRESERVED: &[&str] = &[
    "duration",
    "origin",
    "side_data.full_fidelity",
    "flags.non_key_video_extended",
];
