//! Integration tests for browser SDP fixtures (Chrome, Firefox, Safari).
//!
//! These fixtures exercise the SDP compatibility layer with real-world
//! browser offers. Each fixture is fed through [`preprocess_remote_sdp`]
//! and then accepted by a fresh [`WebRtcCore`] via `AcceptOffer`.
//!
//! Phase 01 requirement: browser SDP munging patterns must not cause
//! panics or rejections in the preprocessing + str0m pipeline.

use std::time::Instant;

use cheetah_webrtc_core::{
    preprocess_remote_sdp, WebRtcCodecProfile, WebRtcCore, WebRtcCoreCommand, WebRtcCoreConfig,
    WebRtcCoreEvent, WebRtcCoreInput, WebRtcCoreOutput, WebRtcLocalDescriptionKind,
    WebRtcSessionId, WebRtcSessionLifecycle, WebRtcSessionRole,
};

const CHROME_OFFER: &str = include_str!("fixtures/offer_from_chrome.sdp");
const FIREFOX_OFFER: &str = include_str!("fixtures/offer_from_firefox.sdp");
const SAFARI_OFFER: &str = include_str!("fixtures/offer_from_safari.sdp");

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
            codec_profile: WebRtcCodecProfile::Browser,
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
fn chrome_offer_is_accepted() {
    accept_offer_and_assert_answer(CHROME_OFFER, "offer_from_chrome.sdp");
}

#[test]
fn firefox_offer_is_accepted() {
    accept_offer_and_assert_answer(FIREFOX_OFFER, "offer_from_firefox.sdp");
}

#[test]
fn safari_offer_is_accepted() {
    accept_offer_and_assert_answer(SAFARI_OFFER, "offer_from_safari.sdp");
}

/// Firefox simulcast offer has `a=rid` and `a=simulcast` lines; verify
/// they are preserved through preprocessing.
#[test]
fn firefox_offer_preserves_simulcast_rid() {
    let (sanitized, report) = preprocess_remote_sdp(FIREFOX_OFFER);
    assert!(
        sanitized.contains("a=rid:q send"),
        "Firefox RID q must be preserved"
    );
    assert!(
        sanitized.contains("a=rid:h send"),
        "Firefox RID h must be preserved"
    );
    assert!(
        sanitized.contains("a=simulcast:send q;h"),
        "Firefox simulcast line must be preserved"
    );
    // Firefox already has RID lines, so no injection should happen
    assert!(
        !report.ssrc_group_sim_rid_generated,
        "should not inject RID when already present"
    );
}

/// Chrome offer has `a=extmap-allow-mixed` at session level.
#[test]
fn chrome_offer_reports_extmap_allow_mixed() {
    let (_sanitized, report) = preprocess_remote_sdp(CHROME_OFFER);
    assert!(
        report.extmap_allow_mixed_observed,
        "Chrome fixture has extmap-allow-mixed"
    );
}

/// Safari offer does NOT have `a=extmap-allow-mixed`.
#[test]
fn safari_offer_does_not_have_extmap_allow_mixed() {
    let (_sanitized, report) = preprocess_remote_sdp(SAFARI_OFFER);
    assert!(
        !report.extmap_allow_mixed_observed,
        "Safari fixture should not have extmap-allow-mixed"
    );
}

/// RTP extension mappings are extracted from Chrome offer.
#[test]
fn chrome_offer_rtp_extensions_extracted() {
    let mappings = cheetah_webrtc_core::extract_rtp_extension_mappings(CHROME_OFFER);
    assert!(!mappings.is_empty());
    // Chrome has audio-level, abs-send-time, transport-cc, mid, rid,
    // repaired-rid, video-orientation, playout-delay, etc.
    assert!(
        mappings
            .iter()
            .any(|m| m.ext_type == cheetah_webrtc_core::RtpExtensionType::AudioLevel),
        "Chrome should have audio-level"
    );
    assert!(
        mappings
            .iter()
            .any(|m| m.ext_type == cheetah_webrtc_core::RtpExtensionType::VideoOrientation),
        "Chrome should have video-orientation"
    );
}

/// Firefox uses direction qualifiers on some extmaps (e.g. `/recvonly`).
#[test]
fn firefox_offer_has_direction_qualified_extmaps() {
    let mappings = cheetah_webrtc_core::extract_rtp_extension_mappings(FIREFOX_OFFER);
    let recvonly_exts: Vec<_> = mappings
        .iter()
        .filter(|m| m.direction.as_deref() == Some("recvonly"))
        .collect();
    assert!(
        !recvonly_exts.is_empty(),
        "Firefox should have at least one recvonly extmap"
    );
}
