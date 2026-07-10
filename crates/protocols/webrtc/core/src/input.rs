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
    Command(WebRtcCoreCommand),
    Network(WebRtcNetworkInput),
    Timeout {
        session_id: WebRtcSessionId,
        now_micros: u64,
    },
    /// Wall-clock advancement without an associated network input.
    ///
    /// Used by drivers to ensure forward progress even when no packets are
    /// arriving. Internally translated into `Input::Timeout` for `str0m`.
    Tick {
        now_micros: u64,
    },
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
    pub session_id: WebRtcSessionId,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub data: Bytes,
    pub now_micros: u64,
}

/// DataChannel write request.
#[derive(Debug, Clone)]
pub struct WebRtcDataChannelOut {
    pub session_id: WebRtcSessionId,
    pub channel: DataChannelId,
    pub payload: Bytes,
    pub binary: bool,
}

/// Direction request for [`WebRtcCoreCommand::CreateOffer`].
///
/// Driven by the module: a publish-style client pull session would
/// request `RecvOnly` audio+video while a push-style session would
/// request `SendOnly`.
#[derive(Debug, Clone, Default)]
pub struct WebRtcOfferSpec {
    pub video_direction: Option<WebRtcOfferDirection>,
    pub audio_direction: Option<WebRtcOfferDirection>,
    pub data_channel: bool,
}

/// `WebRtcOfferDirection` enumeration.
/// `WebRtcOfferDirection` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcOfferDirection {
    SendOnly,
    RecvOnly,
    SendRecv,
}

/// Outbound media frame written to the remote peer.
///
/// The driver layer constructs these from engine `AVFrame`s. The core
/// converts the boundary representation back into the `str0m::Writer`
/// API. `mid` must match a previously-negotiated send-direction track.
#[derive(Debug, Clone)]
pub struct WebRtcSendFrame {
    pub session_id: WebRtcSessionId,
    pub mid: crate::types::MidLabel,
    pub codec: crate::event::WebRtcCodecKind,
    pub clock_rate: u32,
    pub rtp_timestamp_ticks: u32,
    pub rtp_timestamp_denom: u32,
    pub random_access: bool,
    pub payload: Bytes,
    pub network_time_micros: u64,
}

/// Kind of `Web Rtc Request Keyframe`.
/// `Web Rtc Request Keyframe` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcRequestKeyframeKind {
    /// Picture Loss Indication.
    Pli,
    /// Full Intra Request.
    Fir,
}

/// `WebRtcCloseReason` enumeration.
/// `WebRtcCloseReason` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcCloseReason {
    Normal,
    HandshakeTimeout,
    Idle,
    PeerClosed,
    Internal(String),
}
