//! Events emitted by the core for the driver and module layers.
//!
//! Only a small forward-compatible subset of `str0m::Event` is mapped today;
//! later phases extend this enum with media-frame, RTP, RTCP and BWE
//! variants. The mapping is conservative: variants we do not understand
//! become diagnostic records rather than corrupting the event stream.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::types::{DataChannelId, MidLabel, WebRtcSessionId};

/// All boundary-visible events for a session.
#[derive(Debug, Clone)]
pub enum WebRtcCoreEvent {
    Lifecycle {
        session_id: WebRtcSessionId,
        state: WebRtcSessionLifecycle,
    },
    Ice {
        session_id: WebRtcSessionId,
        state: WebRtcIceState,
    },
    MediaTrackAdded {
        session_id: WebRtcSessionId,
        track: WebRtcMediaTrack,
    },
    Media {
        session_id: WebRtcSessionId,
        event: WebRtcMediaEvent,
    },
    DataChannel {
        session_id: WebRtcSessionId,
        event: WebRtcDataChannelEvent,
    },
    Stats {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcSessionStats,
    },
    Bwe {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcBweStats,
    },
    RtcpFeedback {
        session_id: WebRtcSessionId,
        feedback: WebRtcRtcpFeedback,
    },
    /// Simulcast layer surfaced by SDP negotiation or RID extension
    /// observation. Emitted once per RID per direction at track-add
    /// time so the module can pre-allocate per-layer routing state.
    SimulcastLayerObserved {
        session_id: WebRtcSessionId,
        observation: WebRtcSimulcastLayerObservation,
    },
    /// RTP extension mappings observed during SDP negotiation. Emitted
    /// once per session after `AcceptOffer` / `CreateOffer` so the
    /// module can track which extensions are active and their id/type
    /// mapping without re-parsing SDP.
    RtpExtensionObserved {
        session_id: WebRtcSessionId,
        mappings: Vec<crate::sdp_compat::RtpExtensionMapping>,
    },
    /// Payload type numbers extracted from the remote SDP offer. Emitted
    /// once per session after `AcceptOffer` so the module layer knows
    /// which dynamic payload types the browser assigned to each codec.
    /// The answer SDP uses these negotiated values — never hardcoded
    /// constants.
    OfferPayloadNegotiated {
        session_id: WebRtcSessionId,
        payloads: crate::offer_payload::OfferPayloads,
    },
}

/// `WebRtcSessionLifecycle` enumeration.
/// `WebRtcSessionLifecycle` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcSessionLifecycle {
    /// Session has been created in the core.
    Created,
    /// Local SDP description is ready (offer or answer).
    LocalDescriptionReady,
    /// ICE+DTLS+SRTP up.
    Connected,
    /// ICE temporarily disconnected, may recover.
    Disconnected,
    /// Session has been closed.
    Closed,
    /// Session ended because of an unrecoverable error.
    Failed,
}

/// State used by `Web Rtc ICE`.
/// `Web Rtc ICE` 使用的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcIceState {
    New,
    Checking,
    Connected,
    Disconnected,
    Closed,
}

/// A media track newly negotiated for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcMediaTrack {
    pub mid: MidLabel,
    pub kind: WebRtcMediaKind,
    pub direction: WebRtcMediaDirection,
    /// Simulcast layer RIDs negotiated for this track, if any.
    pub simulcast_send: Vec<String>,
    pub simulcast_recv: Vec<String>,
}

/// Kind of `Web Rtc Media`.
/// `Web Rtc Media` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaKind {
    Audio,
    Video,
}

/// `WebRtcMediaDirection` enumeration.
/// `WebRtcMediaDirection` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaDirection {
    SendOnly,
    RecvOnly,
    SendRecv,
    Inactive,
}

/// Media-level events surfaced from `str0m`.
///
/// Carries enough metadata for the module to push frames into engine
/// ingestion: codec, clock rate, RID for simulcast and the canonical
/// media time numerator / denominator.
#[derive(Debug, Clone)]
pub enum WebRtcMediaEvent {
    /// A frame arrived on the named track.
    Frame {
        mid: MidLabel,
        rid: Option<String>,
        codec: WebRtcCodecKind,
        clock_rate: u32,
        random_access: bool,
        rtp_timestamp_ticks: u32,
        rtp_timestamp_denom: u32,
        payload: Bytes,
        network_time_micros: u64,
        /// RTP header / packet metadata for codec-side adapters.
        ///
        /// The boundary uses an explicit metadata struct rather than
        /// boolean / option overloads on the carrier so codec adapters
        /// stay decoupled from the str0m-level RTP stream surface.
        meta: WebRtcFrameMeta,
    },
    /// The remote requested a keyframe via PLI.
    PliReceived { mid: MidLabel },
    /// The remote requested a keyframe via FIR.
    FirReceived { mid: MidLabel },
}

/// Per-frame metadata carried alongside [`WebRtcMediaEvent::Frame`].
///
/// Each field maps onto a well-known RTP header extension that
/// `str0m::media::MediaData` exposes:
///
/// * `audio_level_dbov` — `urn:ietf:params:rtp-hdrext:ssrc-audio-level`
///   (RFC 6464). Negative dBOV; 0 is loudest, -127 is silence.
/// * `voice_activity` — companion bit in the audio-level extension.
/// * `video_orientation` — CVO extension (RFC 7742) bit-packed
///   `(rotation, flip)` byte.
/// * `sequence_number` — first RTP sequence number that contributed
///   to this access unit; usable as the canonical
///   `WebRtcIngressContractView::sequence_number`.
/// * `contiguous` — false when str0m's reorder buffer detected a gap
///   relative to the previously emitted frame on the same track. The
///   module forwards this to `cheetah-codec` so the timestamp
///   normalizer can mark the frame as discontinuous.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcFrameMeta {
    pub audio_level_dbov: Option<i8>,
    pub voice_activity: Option<bool>,
    pub video_orientation: Option<u8>,
    pub sequence_number: Option<u16>,
    pub contiguous: bool,
}

/// Codec carried in [`WebRtcMediaEvent::Frame`].
///
/// We intentionally keep this in `cheetah-webrtc-core` rather than
/// re-using `cheetah-codec::CodecId` so that core stays decoupled from the
/// fuller codec model. The module layer maps this to `CodecId` when
/// pushing into engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcCodecKind {
    Opus,
    Pcma,
    Pcmu,
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
    Unknown,
}

/// Events produced by the `Web Rtc Data Channel` subsystem.
/// `Web Rtc Data Channel` 子系统产生的事件。
#[derive(Debug, Clone)]
pub enum WebRtcDataChannelEvent {
    Opened {
        id: DataChannelId,
        label: String,
    },
    Message {
        id: DataChannelId,
        payload: Bytes,
        binary: bool,
    },
    Closed {
        id: DataChannelId,
    },
}

/// `WebRtcRtcpFeedback` enumeration.
/// `WebRtcRtcpFeedback` 枚举。
#[derive(Debug, Clone)]
pub enum WebRtcRtcpFeedback {
    SenderReport,
    ReceiverReport,
    Pli {
        mid: Option<MidLabel>,
    },
    Fir {
        mid: Option<MidLabel>,
    },
    Nack {
        mid: Option<MidLabel>,
        count: u32,
    },
    Twcc,
    /// Receiver Estimated Maximum Bitrate, surfaced from `str0m`'s BWE
    /// subsystem (`Event::EgressBitrateEstimate(BweKind::Remb)`). The
    /// `mid` identifies which media this estimate applies to and the
    /// `bitrate_bps` is the raw estimate in bits per second.
    Remb {
        mid: Option<MidLabel>,
        bitrate_bps: u64,
    },
    /// Remote endpoint terminated the session via RTCP BYE. We surface
    /// this as a hint to module observability — the actual session
    /// teardown is driven by `Lifecycle::Closed`.
    Bye,
}

/// Observation of a simulcast layer becoming active or inactive.
///
/// Phase 04 will use these to drive layer selection. Phase 01 only emits
/// them on simulcast track addition for visibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcSimulcastLayerObservation {
    pub mid: MidLabel,
    pub rid: String,
    pub source: WebRtcSimulcastRidSource,
}

/// Where the RID label was derived from.
///
/// Mirrors ZLMediaKit's RID fallback chain in `RtpExtContext`: peers may
/// signal the RID via the `rid` extension, the `repaired-rid` extension
/// for retransmissions, the SSRC group `SIM` map, or — when the offer
/// has been munged to drop RID lines — the receiving side has to
/// generate a stable label from SSRC ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcSimulcastRidSource {
    /// `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id`.
    RidExt,
    /// `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id`.
    RepairedRidExt,
    /// `a=ssrc-group:SIM` mapping.
    SsrcSimGroup,
    /// SDP munging stripped RID, label generated from SSRC order.
    Generated,
    /// Negotiated through `a=rid` lines and surfaced by `str0m`.
    SdpRid,
}
