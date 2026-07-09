//! Configuration values consumed by [`crate::WebRtcCore`].
//!
//! All fields here are pure data; the core never reads environment variables
//! or files. Driver and module layers populate the config from their
//! respective configuration models and pass it in.

use serde::{Deserialize, Serialize};

use crate::types::WebRtcCodecProfile;

/// ICE transport policy filter applied at candidate gathering time.
///
/// Mirrors the W3C `RTCIceTransportPolicy` enum. The driver layer is
/// responsible for applying the filter when it adds local candidates;
/// the core stores the policy so observability surfaces report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebRtcIceTransportPolicy {
    /// All candidate types are gathered (default).
    #[default]
    All,
    /// Only relay (TURN) candidates are gathered.
    RelayOnly,
    /// Only host + reflexive candidates are gathered (no TURN).
    P2pOnly,
}

/// Bounds on the WebRTC core to keep all caches and queues finite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcCoreLimits {
    /// Maximum number of concurrent sessions the core will accept.
    pub max_sessions: usize,
    /// Maximum number of pending output items per session that we buffer
    /// internally between `pump_output` calls.
    pub max_pending_outputs_per_session: usize,
    /// Maximum allowed remote SDP size in bytes.
    ///
    /// Rejects requests larger than this with [`crate::WebRtcCoreError::SdpTooLarge`]
    /// instead of leaving the bound to upstream HTTP layers.
    pub max_remote_sdp_bytes: usize,
    /// Maximum number of remote ICE candidates accepted per session.
    pub max_remote_candidates_per_session: usize,
    /// Maximum DataChannel message size in bytes the core will accept
    /// from the boundary `WebRtcCoreCommand::SendDataChannel`. Larger
    /// payloads emit a [`crate::WebRtcCoreDiagnosticKind::PendingOutputDropped`]
    /// diagnostic and are silently dropped instead of overflowing
    /// `str0m`'s SCTP buffer or crashing the peer.
    ///
    /// ZLM clamps to 256 KiB by default; we follow suit.
    pub max_data_channel_message_bytes: usize,
}

impl Default for WebRtcCoreLimits {
    fn default() -> Self {
        Self {
            max_sessions: 4096,
            max_pending_outputs_per_session: 4096,
            max_remote_sdp_bytes: 64 * 1024,
            max_remote_candidates_per_session: 256,
            max_data_channel_message_bytes: 256 * 1024,
        }
    }
}

/// Static configuration applied when a [`crate::WebRtcCore`] is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcCoreConfig {
    /// Whether to run as ICE-lite. Defaults to `false` to match SMS behaviour.
    pub ice_lite: bool,
    /// ICE candidate policy filter. Mirrors the WebRTC W3C
    /// `RTCIceTransportPolicy` semantics:
    ///
    /// * `All`: announce all candidates (host + reflexive + relay).
    /// * `RelayOnly`: only announce relay (TURN) candidates.
    /// * `P2pOnly`: only announce host + reflexive (no relay).
    ///
    /// The actual filtering is performed by the driver layer when it
    /// adds local candidates to the core; the core stores the policy
    /// for diagnostic surfacing only.
    pub ice_transport_policy: WebRtcIceTransportPolicy,
    /// Negotiation profile for codec selection.
    pub codec_profile: WebRtcCodecProfile,
    /// Enable Transport Wide Congestion Control / Bandwidth Estimation.
    pub enable_bwe: bool,
    /// Initial estimate when BWE is enabled.
    pub bwe_initial_bitrate_bps: Option<u64>,
    /// Whether the server allows simulcast media in offers/answers.
    pub enable_simulcast: bool,
    /// Send-side RTX cache packet count.
    pub rtx_cache_packets: usize,
    /// Send-side RTX cache age in milliseconds.
    pub rtx_cache_age_ms: u64,
    /// Optional cap on RTX retransmission ratio (0..1].
    pub rtx_ratio_cap: Option<f32>,
    /// Reorder window for video receive streams.
    pub video_reorder_packets: usize,
    /// Reorder window for audio receive streams.
    pub audio_reorder_packets: usize,
    /// Whether to expose RTP-mode I/O (raw RTP bypass of `str0m`'s
    /// packetizer/depacketizer). Phase 01 keeps this off by default.
    pub enable_rtp_mode: bool,
    /// Hard limits, see [`WebRtcCoreLimits`].
    pub limits: WebRtcCoreLimits,
}

impl Default for WebRtcCoreConfig {
    fn default() -> Self {
        Self {
            ice_lite: false,
            ice_transport_policy: WebRtcIceTransportPolicy::default(),
            codec_profile: WebRtcCodecProfile::Browser,
            enable_bwe: true,
            bwe_initial_bitrate_bps: Some(1_200_000),
            enable_simulcast: true,
            rtx_cache_packets: 1024,
            rtx_cache_age_ms: 3_000,
            rtx_ratio_cap: Some(0.15),
            video_reorder_packets: 30,
            audio_reorder_packets: 10,
            enable_rtp_mode: false,
            limits: WebRtcCoreLimits::default(),
        }
    }
}
