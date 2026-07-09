//! Integration tests for SimpleMediaServer-shipped SDP fixtures.
//!
//! Counterpart of `zlm_sdp_fixtures.rs`: feeds the SMS reference offers
//! (under `vendor-ref/simple-media-server/Src/Webrtc/SdpExample/`)
//! through the SDP compatibility preprocessor and `WebRtcCore`'s
//! `AcceptOffer` flow. Each fixture asserts the same end-to-end
//! contract (no panic, valid answer, full lifecycle).
//!
//! Phase 05 §"互操作 fixture" called out that the SMS h265 / janus /
//! offer-simulcast fixtures had been copied into the repo but were
//! not yet hooked into a smoke run; this module is that hook-up.

use std::time::Instant;

use cheetah_webrtc_core::{
    preprocess_remote_sdp, WebRtcCodecProfile, WebRtcCore, WebRtcCoreCommand, WebRtcCoreConfig,
    WebRtcCoreEvent, WebRtcCoreInput, WebRtcCoreOutput, WebRtcLocalDescriptionKind,
    WebRtcSessionId, WebRtcSessionLifecycle, WebRtcSessionRole,
};

const SMS_PUBLISH_OFFER: &str = include_str!(
    "../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/publish-offer-sms.sdp"
);
const SMS_PUBLISH_OFFER_VANILLA: &str = include_str!(
    "../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/publish-offer.sdp"
);
const SMS_OFFER: &str =
    include_str!("../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/offer.sdp");
const SMS_OFFER_SIMULCAST: &str = include_str!(
    "../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/offer-simulcast.sdp"
);
const SMS_H265_OFFER: &str = include_str!(
    "../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/h265-offer.sdp"
);
const SMS_JANUS_OFFER: &str = include_str!(
    "../../../../../vendor-ref/simple-media-server/Src/Webrtc/SdpExample/janus_offer.sdp"
);

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
            // SMS fixtures advertise H264/H265/G711 — run the device
            // profile so codec acceptance does not silently filter
            // them.
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
fn sms_publish_offer_is_accepted() {
    accept_offer_and_assert_answer(SMS_PUBLISH_OFFER, "publish-offer-sms.sdp");
}

#[test]
fn sms_publish_offer_vanilla_is_accepted() {
    accept_offer_and_assert_answer(SMS_PUBLISH_OFFER_VANILLA, "publish-offer.sdp");
}

#[test]
fn sms_offer_is_accepted() {
    accept_offer_and_assert_answer(SMS_OFFER, "offer.sdp");
}

#[test]
fn sms_offer_simulcast_is_accepted() {
    accept_offer_and_assert_answer(SMS_OFFER_SIMULCAST, "offer-simulcast.sdp");
}

#[test]
fn sms_h265_offer_is_accepted() {
    accept_offer_and_assert_answer(SMS_H265_OFFER, "h265-offer.sdp");
}

#[test]
fn sms_janus_offer_is_accepted() {
    accept_offer_and_assert_answer(SMS_JANUS_OFFER, "janus_offer.sdp");
}

/// Sanity check that the simulcast fixture itself still describes
/// simulcast; if upstream changes the file we want the test to fail
/// loudly rather than silently regress on real simulcast negotiation.
#[test]
fn sms_offer_simulcast_advertises_rid_layers() {
    assert!(SMS_OFFER_SIMULCAST.contains("a=simulcast:"));
    assert!(SMS_OFFER_SIMULCAST.contains("a=rid:"));
}

/// Sanity check that the H265 fixture still advertises H265.
#[test]
fn sms_h265_offer_advertises_h265() {
    assert!(SMS_H265_OFFER.to_ascii_lowercase().contains("h265"));
}
