//! Stable boundary types exposed to the driver and module layers.
//!
//! The crate intentionally keeps these types `Copy` or owned-string-based and
//! avoids re-exporting `str0m`-specific identifiers so that downstream
//! crates do not gain transitive coupling to a particular `str0m` version.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Identifier for a WebRTC session managed by the core.
///
/// Driver and module layers create sessions ahead of time and own the id.
/// The core does not allocate ids — keeping allocation outside makes the
/// state machine a pure function over its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WebRtcSessionId(pub u64);

impl WebRtcSessionId {
    /// Creates a new `WebRtcSessionId` instance.
    /// 创建新的 `WebRtcSessionId` 实例。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// `value` function of `WebRtcSessionId`.
    /// `WebRtcSessionId` 的 `value` 函数。
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WebRtcSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "webrtc-session-{}", self.0)
    }
}

/// SDP m-line label as a small owned string.
///
/// Mirrors `str0m::media::Mid` but does not leak the underlying type. The
/// label format is opaque outside this crate — it is produced by the core
/// and consumed by other layers as a stable identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MidLabel(pub String);

impl MidLabel {
    /// Creates a new `MidLabel` instance.
    /// 创建新的 `MidLabel` 实例。
    pub fn new(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    /// `as_str` function of `MidLabel`.
    /// `MidLabel` 的 `as_str` 函数。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MidLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier for a WebRTC DataChannel within a session.
///
/// Wraps the integer id `str0m` returns through `Event::ChannelOpen` so that
/// downstream code does not depend on `str0m::channel::ChannelId` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataChannelId(pub u32);

impl DataChannelId {
    /// Creates a new `DataChannelId` instance.
    /// 创建新的 `DataChannelId` 实例。
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// `value` function of `DataChannelId`.
    /// `DataChannelId` 的 `value` 函数。
    pub const fn value(self) -> u32 {
        self.0
    }
}

/// Direction the local endpoint plays for a given session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcDirection {
    /// Inbound media: remote peer sends, we receive.
    RecvOnly,
    /// Outbound media: we send, remote peer receives.
    SendOnly,
    /// Bi-directional media flow.
    SendRecv,
    /// No media; only DataChannel or signaling.
    Inactive,
}

/// High-level role of a session within the cheetah stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcSessionRole {
    /// Remote peer publishes media into the engine via this session.
    Publisher,
    /// Engine plays media to remote peer via this session.
    Player,
    /// Bi-directional or DataChannel-only session (echo / P2P / control).
    Bidirectional,
}

/// ICE role assignment for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcIceRole {
    /// Local endpoint is the controlling agent.
    Controlling,
    /// Local endpoint is the controlled agent.
    Controlled,
}

/// Session lifecycle state visible at the boundary.
///
/// This intentionally collapses some `str0m::IceConnectionState` values so
/// that downstream code only deals with the transitions it actually needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcSessionState {
    /// Session created but no SDP exchanged yet.
    Created,
    /// SDP applied; ICE/DTLS handshake in progress.
    Connecting,
    /// ICE+DTLS+SRTP up.
    Connected,
    /// Closing was requested; cleanup in progress.
    Closing,
    /// Session has been closed.
    Closed,
    /// Session ended because of an unrecoverable error.
    Failed,
}

/// Codec negotiation profile applied to a session at construction time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcCodecProfile {
    /// Browser-friendly codec set: H264, VP8, VP9, AV1 and Opus.
    #[default]
    Browser,
    /// Device-friendly codec set: also H265, PCMA, PCMU.
    Device,
    /// Pass-through profile for non-browser peers; allows RTP mode.
    Passthrough,
}
