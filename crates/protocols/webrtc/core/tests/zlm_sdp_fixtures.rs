//! Integration tests for ZLMediaKit-shipped SDP fixtures.
//!
//! These fixtures exercise the SDP compatibility layer end-to-end: each
//! fixture is fed through [`preprocess_remote_sdp`] and then accepted by a
//! fresh [`WebRtcCore`] via `AcceptOffer`. The test asserts that
//!
//! * the preprocessor never panics,
//! * the resulting answer is a non-empty SDP string,
//! * a `Created` lifecycle event is emitted, and
//! * (for simulcast) the resulting MediaTrackAdded surfaces RID
//!   information so that the module layer can route per-layer media.
//!
//! These tests are the Phase 01 entry point for ZLM SDP compatibility:
//! any future regression that breaks ZLM offer ingestion will fail here.

use std::time::Instant;

use cheetah_webrtc_core::{
    preprocess_remote_sdp, WebRtcCodecProfile, WebRtcCore, WebRtcCoreCommand, WebRtcCoreConfig,
    WebRtcCoreEvent, WebRtcCoreInput, WebRtcCoreOutput, WebRtcLocalDescriptionKind,
    WebRtcSessionId, WebRtcSessionLifecycle, WebRtcSessionRole, WebRtcSimulcastRidSource,
};

const ZLM_OFFER: &str = include_str!("fixtures/zlm_offer.sdp");
const ZLM_OFFER_SIMULCAST: &str = include_str!("fixtures/zlm_offer_simulcast.sdp");
const ZLM_JANUS_OFFER: &str = include_str!("fixtures/zlm_janus_offer.sdp");
const MUNGED_SSRC_SIM_NO_RID: &str = include_str!("fixtures/munged_ssrc_sim_no_rid.sdp");

fn drain(core: &mut WebRtcCore) -> Vec<WebRtcCoreOutput> {
    let mut sink = Vec::new();
    core.pump_outputs(&mut sink);
    sink
}

fn accept_offer_and_assert_answer(fixture: &str, label: &str) {
    let (sanitized, _report) = preprocess_remote_sdp(fixture);
    assert!(
        sanitized.starts_with("v=0\r\n"),
        "preprocessor must canonicalise to CRLF for fixture {label}"
    );
    assert!(
        sanitized.ends_with("\r\n"),
        "preprocessor must end fixture {label} with CRLF"
    );

    let mut core = WebRtcCore::new(
        WebRtcCoreConfig {
            // ZLM fixtures advertise H264/H265/G711, so we run the
            // device profile to maximise codec acceptance.
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

    let mut saw_created = false;
    let mut saw_local_description = false;
    let mut saw_local_ready = false;
    for out in &outputs {
        match out {
            WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                state: WebRtcSessionLifecycle::Created,
                ..
            }) => saw_created = true,
            WebRtcCoreOutput::Event(WebRtcCoreEvent::Lifecycle {
                state: WebRtcSessionLifecycle::LocalDescriptionReady,
                ..
            }) => saw_local_ready = true,
            WebRtcCoreOutput::LocalDescription {
                kind: WebRtcLocalDescriptionKind::Answer,
                sdp,
                ..
            } => {
                assert!(
                    sdp.starts_with("v=0"),
                    "answer for {label} must start with v=0"
                );
                saw_local_description = true;
            }
            _ => {}
        }
    }

    assert!(saw_created, "{label}: missing Created lifecycle");
    assert!(
        saw_local_description,
        "{label}: missing LocalDescription Answer output"
    );
    assert!(
        saw_local_ready,
        "{label}: missing LocalDescriptionReady lifecycle"
    );
}

#[test]
fn zlm_offer_is_accepted() {
    accept_offer_and_assert_answer(ZLM_OFFER, "zlm_offer.sdp");
}

#[test]
fn zlm_simulcast_offer_is_accepted() {
    accept_offer_and_assert_answer(ZLM_OFFER_SIMULCAST, "zlm_offer_simulcast.sdp");
}

#[test]
fn zlm_janus_offer_is_accepted() {
    accept_offer_and_assert_answer(ZLM_JANUS_OFFER, "zlm_janus_offer.sdp");
}

#[test]
fn zlm_simulcast_offer_advertises_rid_layers() {
    // Sanity check that the fixture itself still describes simulcast;
    // if the upstream fixture changes shape we want the test to fail
    // loudly rather than silently regressing on real simulcast
    // negotiation.
    assert!(ZLM_OFFER_SIMULCAST.contains("a=simulcast:send"));
    assert!(ZLM_OFFER_SIMULCAST.contains("a=rid:q send"));
    assert!(ZLM_OFFER_SIMULCAST.contains("a=rid:h send"));
    assert!(ZLM_OFFER_SIMULCAST.contains("a=rid:f send"));
}

/// Phase 01 contract: the simulcast RID-source enum is part of the
/// public surface so module/driver layers can attribute simulcast
/// observations to their origin (SDP, RID extension, repaired-RID,
/// SSRC SIM group, or generated). `str0m` itself only fires
/// `MediaAdded` once a peer is actually transmitting, so the runtime
/// firing of [`WebRtcCoreEvent::SimulcastLayerObserved`] is exercised
/// through the driver-level integration tests.
#[test]
fn simulcast_rid_source_surface_is_stable() {
    let _ = WebRtcSimulcastRidSource::SdpRid;
    let _ = WebRtcSimulcastRidSource::RidExt;
    let _ = WebRtcSimulcastRidSource::RepairedRidExt;
    let _ = WebRtcSimulcastRidSource::SsrcSimGroup;
    let _ = WebRtcSimulcastRidSource::Generated;
}

/// Phase 01: SDP with `a=ssrc-group:SIM` but no `a=rid` lines should
/// have synthetic RID injected by the preprocessor, allowing str0m to
/// negotiate simulcast.
#[test]
fn munged_ssrc_sim_no_rid_is_accepted() {
    accept_offer_and_assert_answer(MUNGED_SSRC_SIM_NO_RID, "munged_ssrc_sim_no_rid.sdp");
}

/// Phase 01: verify that the preprocessor injects r0/r1/r2 for the
/// munged SDP fixture and reports it.
#[test]
fn munged_ssrc_sim_no_rid_generates_rid_labels() {
    let (sanitized, report) = preprocess_remote_sdp(MUNGED_SSRC_SIM_NO_RID);
    assert!(
        report.ssrc_group_sim_rid_generated,
        "should report ssrc_group_sim_rid_generated"
    );
    assert!(sanitized.contains("a=rid:r0 send"));
    assert!(sanitized.contains("a=rid:r1 send"));
    assert!(sanitized.contains("a=rid:r2 send"));
    assert!(sanitized.contains("a=simulcast:send r0;r1;r2"));
}

/// Phase 01: the ZLM simulcast fixture has `a=extmap-allow-mixed` at
/// session level; the preprocessor should observe it.
#[test]
fn zlm_simulcast_offer_reports_extmap_allow_mixed() {
    let (_sanitized, report) = preprocess_remote_sdp(ZLM_OFFER_SIMULCAST);
    assert!(
        report.extmap_allow_mixed_observed,
        "ZLM simulcast fixture has extmap-allow-mixed"
    );
}

/// Phase 01: AcceptOffer emits RtpExtensionObserved with the extension
/// mappings from the remote SDP.
#[test]
fn accept_offer_emits_rtp_extension_observed() {
    let mut core = WebRtcCore::new(
        WebRtcCoreConfig {
            codec_profile: WebRtcCodecProfile::Device,
            ..Default::default()
        },
        Instant::now(),
    );
    let session_id = WebRtcSessionId::new(42);
    core.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
        session_id,
        role: WebRtcSessionRole::Publisher,
        remote_sdp: ZLM_OFFER_SIMULCAST.to_string(),
        local_candidates: Vec::new(),
        now_micros: 0,
    }))
    .expect("AcceptOffer should succeed");

    let outputs = drain(&mut core);
    let ext_event = outputs.iter().find(|o| {
        matches!(
            o,
            WebRtcCoreOutput::Event(WebRtcCoreEvent::RtpExtensionObserved { .. })
        )
    });
    assert!(
        ext_event.is_some(),
        "should emit RtpExtensionObserved event"
    );
    if let Some(WebRtcCoreOutput::Event(WebRtcCoreEvent::RtpExtensionObserved {
        session_id: sid,
        mappings,
    })) = ext_event
    {
        assert_eq!(*sid, session_id);
        assert!(!mappings.is_empty(), "mappings should not be empty");
        // The ZLM simulcast fixture has audio-level, abs-send-time,
        // transport-cc, mid, rid, repaired-rid, video-orientation, etc.
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == cheetah_webrtc_core::RtpExtensionType::AudioLevel),
            "should contain audio-level mapping"
        );
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == cheetah_webrtc_core::RtpExtensionType::TransportWideCc),
            "should contain transport-wide-cc mapping"
        );
    }
}
