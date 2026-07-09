//! OvenMediaEngine-style SDP fixture validation.
//!
//! These fixtures pin the OME ingest shapes used by the module-level
//! WebSocket/WHIP compatibility paths: extmap-allow-mixed, simulcast RID
//! layers, timestamp-related RTP extensions, and H265 payload descriptors.

use std::time::Instant;

use cheetah_webrtc_core::{
    extract_rtp_extension_mappings, preprocess_remote_sdp, RtpExtensionType, WebRtcCodecProfile,
    WebRtcCore, WebRtcCoreCommand, WebRtcCoreConfig, WebRtcCoreEvent, WebRtcCoreInput,
    WebRtcCoreOutput, WebRtcLocalDescriptionKind, WebRtcSessionId, WebRtcSessionLifecycle,
    WebRtcSessionRole,
};

const OME_PUBLISH_SIMULCAST_OFFER: &str = include_str!("fixtures/ome/publish_simulcast_offer.sdp");
const OME_PUBLISH_H265_DESCRIPTOR: &str = include_str!("fixtures/ome/publish_h265_descriptor.sdp");
const OME_PLAY_UDP_OFFER: &str = include_str!("fixtures/ome/play_udp_offer.sdp");
const OME_PLAY_RELAY_RED_ULPFEC_OFFER: &str =
    include_str!("fixtures/ome/play_relay_red_ulpfec_offer.sdp");
const OME_PLAY_H265_LOW_LATENCY_OFFER: &str =
    include_str!("fixtures/ome/play_h265_low_latency_offer.sdp");

fn drain(core: &mut WebRtcCore) -> Vec<WebRtcCoreOutput> {
    let mut sink = Vec::new();
    core.pump_outputs(&mut sink);
    sink
}

fn accept_offer_and_assert_answer(fixture: &str, label: &str) -> Vec<WebRtcCoreOutput> {
    let (sanitized, _report) = preprocess_remote_sdp(fixture);
    assert!(
        sanitized.starts_with("v=0\r\n"),
        "preprocessor must canonicalise to CRLF for fixture {label}"
    );

    let mut core = WebRtcCore::new(
        WebRtcCoreConfig {
            codec_profile: WebRtcCodecProfile::Device,
            ..Default::default()
        },
        Instant::now(),
    );
    let session_id = WebRtcSessionId::new(1);
    core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
        session_id,
        role: WebRtcSessionRole::Publisher,
        remote_sdp: fixture.to_string(),
        local_candidates: Vec::new(),
        now_micros: 0,
    }))
    .unwrap_or_else(|err| panic!("AcceptOffer for {label} failed: {err}"));

    let outputs = drain(&mut core);
    assert!(outputs.iter().any(|out| matches!(
        out,
        WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
            state: WebRtcSessionLifecycle::Created,
            ..
        })
    )));
    assert!(outputs.iter().any(|out| matches!(
        out,
        WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
            state: WebRtcSessionLifecycle::LocalDescriptionReady,
            ..
        })
    )));
    assert!(outputs.iter().any(|out| matches!(
        out,
        WebRtcCoreOutput::LocalDescription {
            kind: WebRtcLocalDescriptionKind::Answer,
            ..
        }
    )));
    outputs
}

#[test]
fn ome_publish_simulcast_offer_is_accepted() {
    accept_offer_and_assert_answer(OME_PUBLISH_SIMULCAST_OFFER, "ome publish simulcast");
}

#[test]
fn ome_publish_simulcast_offer_reports_extmap_and_rids() {
    let (sanitized, report) = preprocess_remote_sdp(OME_PUBLISH_SIMULCAST_OFFER);
    assert!(report.extmap_allow_mixed_observed);
    assert!(sanitized.contains("a=rid:q send"));
    assert!(sanitized.contains("a=rid:h send"));
    assert!(sanitized.contains("a=rid:f send"));
    assert!(sanitized.contains("a=simulcast:send q;h;f"));
}

#[test]
fn ome_publish_offer_exposes_timestamp_extensions() {
    let mappings = extract_rtp_extension_mappings(OME_PUBLISH_SIMULCAST_OFFER);
    assert!(mappings
        .iter()
        .any(|mapping| mapping.ext_type == RtpExtensionType::TransmissionOffset));
    assert!(mappings
        .iter()
        .any(|mapping| mapping.ext_type == RtpExtensionType::VideoTiming));
}

#[test]
fn ome_publish_h265_descriptor_is_canonicalised_for_diagnostics() {
    let (sanitized, report) = preprocess_remote_sdp(OME_PUBLISH_H265_DESCRIPTOR);
    assert!(report.extmap_allow_mixed_observed);
    assert!(sanitized.contains("a=rtpmap:110 H265/90000\r\n"));
    assert!(sanitized.contains("sprop-vps="));
    assert!(sanitized.ends_with("\r\n"));
}

#[test]
fn ome_play_fixtures_are_canonicalised_without_panicking() {
    for fixture in [
        OME_PLAY_UDP_OFFER,
        OME_PLAY_RELAY_RED_ULPFEC_OFFER,
        OME_PLAY_H265_LOW_LATENCY_OFFER,
    ] {
        let (sanitized, report) = preprocess_remote_sdp(fixture);
        assert!(report.extmap_allow_mixed_observed);
        assert!(sanitized.starts_with("v=0\r\n"));
        assert!(sanitized.ends_with("\r\n"));
    }
}
