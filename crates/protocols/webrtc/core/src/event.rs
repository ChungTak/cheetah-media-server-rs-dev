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
    /// `Lifecycle` variant.
    /// `Lifecycle` 变体.
    Lifecycle {
        session_id: WebRtcSessionId,
        state: WebRtcSessionLifecycle,
    },
    /// `Ice` variant.
    /// `Ice` 变体.
    Ice {
        session_id: WebRtcSessionId,
        state: WebRtcIceState,
    },
    /// `MediaTrackAdded` variant.
    /// `MediaTrackAdded` 变体.
    MediaTrackAdded {
        session_id: WebRtcSessionId,
        track: WebRtcMediaTrack,
    },
    /// `Media` variant.
    /// `Media` 变体.
    Media {
        session_id: WebRtcSessionId,
        event: WebRtcMediaEvent,
    },
    /// `DataChannel` variant.
    /// `DataChannel` 变体.
    DataChannel {
        session_id: WebRtcSessionId,
        event: WebRtcDataChannelEvent,
    },
    /// `Stats` variant.
    /// `Stats` 变体.
    Stats {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcSessionStats,
    },
    /// `Bwe` variant.
    /// `Bwe` 变体.
    Bwe {
        session_id: WebRtcSessionId,
        snapshot: crate::stats::WebRtcBweStats,
    },
    /// `RtcpFeedback` variant.
    /// `RtcpFeedback` 变体.
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
/// `WebRtcSessionLifecycle` 枚举.
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

/// `WebRtcIceState` enumeration.
/// `WebRtcIceState` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcIceState {
    /// `New` variant.
    /// `New` 变体.
    New,
    /// `Checking` variant.
    /// `Checking` 变体.
    Checking,
    /// `Connected` variant.
    /// `Connected` 变体.
    Connected,
    /// `Disconnected` variant.
    /// `Disconnected` 变体.
    Disconnected,
    /// `Closed` variant.
    /// `Closed` 变体.
    Closed,
}

/// A media track newly negotiated for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcMediaTrack {
    /// `mid` field of type `MidLabel`.
    /// `mid` 字段，类型为 `MidLabel`.
    pub mid: MidLabel,
    /// `kind` field of type `WebRtcMediaKind`.
    /// `kind` 字段，类型为 `WebRtcMediaKind`.
    pub kind: WebRtcMediaKind,
    /// `direction` field of type `WebRtcMediaDirection`.
    /// `direction` 字段，类型为 `WebRtcMediaDirection`.
    pub direction: WebRtcMediaDirection,
    /// Simulcast layer RIDs negotiated for this track, if any.
    pub simulcast_send: Vec<String>,
    /// `simulcast_recv` field.
    /// `simulcast_recv` 字段.
    pub simulcast_recv: Vec<String>,
}

/// `WebRtcMediaKind` enumeration.
/// `WebRtcMediaKind` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaKind {
    /// `Audio` variant.
    /// `Audio` 变体.
    Audio,
    /// `Video` variant.
    /// `Video` 变体.
    Video,
}

/// `WebRtcMediaDirection` enumeration.
/// `WebRtcMediaDirection` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcMediaDirection {
    /// `SendOnly` variant.
    /// `SendOnly` 变体.
    SendOnly,
    /// `RecvOnly` variant.
    /// `RecvOnly` 变体.
    RecvOnly,
    /// `SendRecv` variant.
    /// `SendRecv` 变体.
    SendRecv,
    /// `Inactive` variant.
    /// `Inactive` 变体.
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
    /// `audio_level_dbov` field.
    /// `audio_level_dbov` 字段.
    pub audio_level_dbov: Option<i8>,
    /// `voice_activity` field.
    /// `voice_activity` 字段.
    pub voice_activity: Option<bool>,
    /// `video_orientation` field.
    /// `video_orientation` 字段.
    pub video_orientation: Option<u8>,
    /// `sequence_number` field.
    /// `sequence_number` 字段.
    pub sequence_number: Option<u16>,
    /// `contiguous` field of type `bool`.
    /// `contiguous` 字段，类型为 `bool`.
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
    /// `Opus` variant.
    /// `Opus` 变体.
    Opus,
    /// `Pcma` variant.
    /// `Pcma` 变体.
    Pcma,
    /// `Pcmu` variant.
    /// `Pcmu` 变体.
    Pcmu,
    /// `H264` variant.
    /// `H264` 变体.
    H264,
    /// `H265` variant.
    /// `H265` 变体.
    H265,
    /// `Vp8` variant.
    /// `Vp8` 变体.
    Vp8,
    /// `Vp9` variant.
    /// `Vp9` 变体.
    Vp9,
    /// `Av1` variant.
    /// `Av1` 变体.
    Av1,
    /// `Unknown` variant.
    /// `Unknown` 变体.
    Unknown,
}

/// `WebRtcDataChannelEvent` enumeration.
/// `WebRtcDataChannelEvent` 枚举.
#[derive(Debug, Clone)]
pub enum WebRtcDataChannelEvent {
    /// `Opened` variant.
    /// `Opened` 变体.
    Opened { id: DataChannelId, label: String },
    /// `Message` variant.
    /// `Message` 变体.
    Message {
        id: DataChannelId,
        payload: Bytes,
        binary: bool,
    },
    /// `Closed` variant.
    /// `Closed` 变体.
    Closed { id: DataChannelId },
}

/// `WebRtcRtcpFeedback` enumeration.
/// `WebRtcRtcpFeedback` 枚举.
#[derive(Debug, Clone)]
pub enum WebRtcRtcpFeedback {
    /// `SenderReport` variant.
    /// `SenderReport` 变体.
    SenderReport,
    /// `ReceiverReport` variant.
    /// `ReceiverReport` 变体.
    ReceiverReport,
    /// `Pli` variant.
    /// `Pli` 变体.
    Pli { mid: Option<MidLabel> },
    /// `Fir` variant.
    /// `Fir` 变体.
    Fir { mid: Option<MidLabel> },
    /// `Nack` variant.
    /// `Nack` 变体.
    Nack { mid: Option<MidLabel>, count: u32 },
    /// `Twcc` variant.
    /// `Twcc` 变体.
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
    /// `mid` field of type `MidLabel`.
    /// `mid` 字段，类型为 `MidLabel`.
    pub mid: MidLabel,
    /// `rid` field of type `String`.
    /// `rid` 字段，类型为 `String`.
    pub rid: String,
    /// `source` field of type `WebRtcSimulcastRidSource`.
    /// `source` 字段，类型为 `WebRtcSimulcastRidSource`.
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
