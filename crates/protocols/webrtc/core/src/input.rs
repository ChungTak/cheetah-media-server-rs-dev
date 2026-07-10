//! Input model for the core state machine.
//!
//! Drivers convert their runtime-specific inputs (UDP packets, timer ticks,
//! HTTP commands) into [`WebRtcCoreInput`] values and feed them into
//! [`crate::WebRtcCore::handle_input`].
//!
//! All time values are `u64` microseconds anchored at the `start_micros`
//! provided to [`crate::WebRtcCore::new`]. The core never reads the system
//! clock.

use std::net::SocketAddr;

use bytes::Bytes;

use crate::types::{DataChannelId, MidLabel, WebRtcSessionId, WebRtcSessionRole};

/// Top-level input fed into [`crate::WebRtcCore::handle_input`].
#[derive(Debug, Clone)]
pub enum WebRtcCoreInput {
    /// `Command` variant.
    /// `Command` 变体.
    Command(WebRtcCoreCommand),
    /// `Network` variant.
    /// `Network` 变体.
    Network(WebRtcNetworkInput),
    /// `Timeout` variant.
    /// `Timeout` 变体.
    Timeout {
        session_id: WebRtcSessionId,
        now_micros: u64,
    },
    /// Wall-clock advancement without an associated network input.
    ///
    /// Used by drivers to ensure forward progress even when no packets are
    /// arriving. Internally translated into `Input::Timeout` for `str0m`.
    Tick { now_micros: u64 },
}

/// User-driven commands.
///
/// The variant data deliberately keeps types small and `Clone`-able so that
/// drivers can fan-out commands across shards if needed.
#[derive(Debug, Clone)]
pub enum WebRtcCoreCommand {
    /// Create a new session and accept a remote SDP offer, producing an
    /// SDP answer for delivery back to the peer.
    AcceptOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        remote_sdp: String,
        local_candidates: Vec<String>,
        now_micros: u64,
    },
    /// Create a new session in offering mode and produce a local SDP
    /// offer. The caller is expected to deliver the offer to a remote
    /// peer and feed back the answer via `ApplyAnswer`.
    ///
    /// Phase 05 scope: this is the foundation for client pull/push and
    /// P2P, where Cheetah is the offerer.
    CreateOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        spec: WebRtcOfferSpec,
        local_candidates: Vec<String>,
        now_micros: u64,
    },
    /// Apply a remote SDP answer to a previously created offering session.
    ApplyAnswer {
        session_id: WebRtcSessionId,
        remote_sdp: String,
        now_micros: u64,
    },
    /// Trickle a remote ICE candidate into an existing session.
    AddRemoteCandidate {
        session_id: WebRtcSessionId,
        candidate: String,
        now_micros: u64,
    },
    /// Trigger an ICE restart on an existing session.
    ///
    /// Calls `SdpApi::ice_restart` on the wrapped `str0m` session,
    /// producing a fresh local offer with rotated ICE credentials.
    /// The offer is delivered through the same `LocalDescription`
    /// path as `CreateOffer`, so the driver surfaces it via
    /// `WebRtcDriverEvent::OfferReady`.
    ///
    /// `keep_local_candidates=true` retains the previously gathered
    /// host candidates; `false` clears them so the caller can add
    /// fresh ones (typically used when the local network has changed
    /// and the previous candidates are no longer reachable).
    IceRestart {
        session_id: WebRtcSessionId,
        keep_local_candidates: bool,
        now_micros: u64,
    },
    /// Send DataChannel data on a previously opened channel.
    SendDataChannel(WebRtcDataChannelOut),
    /// Write a media frame to the remote peer (player role only).
    SendFrame(Box<WebRtcSendFrame>),
    /// Ask the local sender to emit a fresh keyframe for the given track.
    RequestKeyframe {
        session_id: WebRtcSessionId,
        mid: MidLabel,
        kind: WebRtcRequestKeyframeKind,
        now_micros: u64,
    },
    /// Close the session and emit a lifecycle event.
    Close {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
}

/// Network packet routed to a specific session by the driver.
#[derive(Debug, Clone)]
pub struct WebRtcNetworkInput {
    /// `session_id` field of type `WebRtcSessionId`.
    /// `session_id` 字段，类型为 `WebRtcSessionId`.
    pub session_id: WebRtcSessionId,
    /// `source` field of type `SocketAddr`.
    /// `source` 字段，类型为 `SocketAddr`.
    pub source: SocketAddr,
    /// `destination` field of type `SocketAddr`.
    /// `destination` 字段，类型为 `SocketAddr`.
    pub destination: SocketAddr,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
    /// `now_micros` field of type `u64`.
    /// `now_micros` 字段，类型为 `u64`.
    pub now_micros: u64,
}

/// DataChannel write request.
#[derive(Debug, Clone)]
pub struct WebRtcDataChannelOut {
    /// `session_id` field of type `WebRtcSessionId`.
    /// `session_id` 字段，类型为 `WebRtcSessionId`.
    pub session_id: WebRtcSessionId,
    /// `channel` field of type `DataChannelId`.
    /// `channel` 字段，类型为 `DataChannelId`.
    pub channel: DataChannelId,
    /// `payload` field of type `Bytes`.
    /// `payload` 字段，类型为 `Bytes`.
    pub payload: Bytes,
    /// `binary` field of type `bool`.
    /// `binary` 字段，类型为 `bool`.
    pub binary: bool,
}

/// Direction request for [`WebRtcCoreCommand::CreateOffer`].
///
/// Driven by the module: a publish-style client pull session would
/// request `RecvOnly` audio+video while a push-style session would
/// request `SendOnly`.
#[derive(Debug, Clone, Default)]
pub struct WebRtcOfferSpec {
    /// `video_direction` field.
    /// `video_direction` 字段.
    pub video_direction: Option<WebRtcOfferDirection>,
    /// `audio_direction` field.
    /// `audio_direction` 字段.
    pub audio_direction: Option<WebRtcOfferDirection>,
    /// `data_channel` field of type `bool`.
    /// `data_channel` 字段，类型为 `bool`.
    pub data_channel: bool,
}

/// `WebRtcOfferDirection` enumeration.
/// `WebRtcOfferDirection` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcOfferDirection {
    /// `SendOnly` variant.
    /// `SendOnly` 变体.
    SendOnly,
    /// `RecvOnly` variant.
    /// `RecvOnly` 变体.
    RecvOnly,
    /// `SendRecv` variant.
    /// `SendRecv` 变体.
    SendRecv,
}

/// Outbound media frame written to the remote peer.
///
/// The driver layer constructs these from engine `AVFrame`s. The core
/// converts the boundary representation back into the `str0m::Writer`
/// API. `mid` must match a previously-negotiated send-direction track.
#[derive(Debug, Clone)]
pub struct WebRtcSendFrame {
    /// `session_id` field of type `WebRtcSessionId`.
    /// `session_id` 字段，类型为 `WebRtcSessionId`.
    pub session_id: WebRtcSessionId,
    /// `mid` field.
    /// `mid` 字段.
    pub mid: crate::types::MidLabel,
    /// `codec` field.
    /// `codec` 字段.
    pub codec: crate::event::WebRtcCodecKind,
    /// `clock_rate` field of type `u32`.
    /// `clock_rate` 字段，类型为 `u32`.
    pub clock_rate: u32,
    /// `rtp_timestamp_ticks` field of type `u32`.
    /// `rtp_timestamp_ticks` 字段，类型为 `u32`.
    pub rtp_timestamp_ticks: u32,
    /// `rtp_timestamp_denom` field of type `u32`.
    /// `rtp_timestamp_denom` 字段，类型为 `u32`.
    pub rtp_timestamp_denom: u32,
    /// `random_access` field of type `bool`.
    /// `random_access` 字段，类型为 `bool`.
    pub random_access: bool,
    /// `payload` field of type `Bytes`.
    /// `payload` 字段，类型为 `Bytes`.
    pub payload: Bytes,
    /// `network_time_micros` field of type `u64`.
    /// `network_time_micros` 字段，类型为 `u64`.
    pub network_time_micros: u64,
}

/// `WebRtcRequestKeyframeKind` enumeration.
/// `WebRtcRequestKeyframeKind` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcRequestKeyframeKind {
    /// Picture Loss Indication.
    Pli,
    /// Full Intra Request.
    Fir,
}

/// `WebRtcCloseReason` enumeration.
/// `WebRtcCloseReason` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcCloseReason {
    /// `Normal` variant.
    /// `Normal` 变体.
    Normal,
    /// `HandshakeTimeout` variant.
    /// `HandshakeTimeout` 变体.
    HandshakeTimeout,
    /// `Idle` variant.
    /// `Idle` 变体.
    Idle,
    /// `PeerClosed` variant.
    /// `PeerClosed` 变体.
    PeerClosed,
    /// `Internal` variant.
    /// `Internal` 变体.
    Internal(String),
}
