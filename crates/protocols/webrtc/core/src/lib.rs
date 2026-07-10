//! `cheetah-webrtc-core` is the Sans-I/O WebRTC protocol surface for the project.
//!
//! It wraps [`str0m::Rtc`] sessions and exposes deterministic, runtime-neutral
//! input/output types so that the driver layer can drive WebRTC sessions
//! without leaking `tokio`, sockets, or system clock dependencies into the
//! state machine.
//!
//! Boundary invariants enforced by this crate:
//!
//! - No call to [`std::time::Instant::now`] from any state-machine method.
//! - No async fn, no spawned tasks, no internal channels.
//! - Time is provided externally as `u64` microseconds anchored at the
//!   `start_micros` value supplied to [`WebRtcCore::new`].
//! - Network packets are pure data; the driver layer is responsible for I/O.
//!
//! Phase 01 scope: SDP offer/answer plumbing, ICE candidate ingestion, timer
//! and network packet pumping for one or more sessions, and a small bridge
//! between [`str0m::Event`] and [`event::WebRtcCoreEvent`] for downstream
//! phases. Media write paths, RTX/NACK/TWCC policy and DataChannel writes are
//! sketched as commands but only implement the safe subset the rest of the
//! pipeline currently consumes.

/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `error`.
/// `error` 相关模块。
pub mod error;
/// Module for `event`.
/// `event` 相关模块。
pub mod event;
/// Module for `input`.
/// `input` 相关模块。
pub mod input;
/// Module for `offer_payload`.
/// `offer_payload` 相关模块。
pub mod offer_payload;
/// Module for `output`.
/// `output` 相关模块。
pub mod output;
/// Module for `sdp_compat`.
/// `sdp_compat` 相关模块。
pub mod sdp_compat;
/// Module for `session`.
/// `session` 相关模块。
pub mod session;
/// Module for `stats`.
/// `stats` 相关模块。
pub mod stats;
/// Module for `types`.
/// `types` 相关模块。
pub mod types;

pub use config::{WebRtcCoreConfig, WebRtcCoreLimits, WebRtcIceTransportPolicy};
pub use error::{WebRtcCoreDiagnostic, WebRtcCoreError};
pub use event::{
    WebRtcCodecKind, WebRtcCoreEvent, WebRtcDataChannelEvent, WebRtcFrameMeta, WebRtcIceState,
    WebRtcMediaDirection, WebRtcMediaEvent, WebRtcMediaKind, WebRtcMediaTrack, WebRtcRtcpFeedback,
    WebRtcSessionLifecycle, WebRtcSimulcastLayerObservation, WebRtcSimulcastRidSource,
};
pub use input::{
    WebRtcCloseReason, WebRtcCoreCommand, WebRtcCoreInput, WebRtcDataChannelOut,
    WebRtcNetworkInput, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcRequestKeyframeKind,
    WebRtcSendFrame,
};
pub use offer_payload::{extract_offer_payloads, OfferCodec, OfferPayloads, PayloadNotFound};
pub use output::{WebRtcCoreOutput, WebRtcLocalDescriptionKind, WebRtcPacketOut, WebRtcTimer};
pub use sdp_compat::{
    extract_rtp_extension_mappings, inject_rid_from_ssrc_group_sim, preprocess_remote_sdp,
    RtpExtensionMapping, RtpExtensionType, SdpCompatReport,
};
pub use session::WebRtcCore;
pub use stats::{WebRtcBweStats, WebRtcSessionStats};
pub use types::{
    DataChannelId, MidLabel, WebRtcCodecProfile, WebRtcDirection, WebRtcIceRole, WebRtcSessionId,
    WebRtcSessionRole, WebRtcSessionState,
};
