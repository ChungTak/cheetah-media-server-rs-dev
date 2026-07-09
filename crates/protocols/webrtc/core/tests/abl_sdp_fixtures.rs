//! ABL-style SDP fixture regression tests.
//!
//! Phase 05 Task 03: cover ABL interop edge cases that must not trigger
//! panics, infinite loops, or unbounded allocation in the SDP parsing
//! and offer payload extraction pipeline.
//!
//! Fixtures:
//! - Mixed-case codec names (h264, H264, Opus, OPUS, h265, HEVC)
//! - Non-contiguous payload type numbers (gaps in PT space)
//! - Video-only offer (missing opus)
//! - Audio-only offer (missing video codec)

use cheetah_webrtc_core::{extract_offer_payloads, preprocess_remote_sdp, OfferCodec};

const ABL_MIXED_CASE: &str = include_str!("fixtures/abl_mixed_case_codecs.sdp");
const ABL_NONCONTIGUOUS_PT: &str = include_str!("fixtures/abl_noncontiguous_pt.sdp");
const ABL_VIDEO_ONLY: &str = include_str!("fixtures/abl_video_only.sdp");
const ABL_AUDIO_ONLY: &str = include_str!("fixtures/abl_audio_only.sdp");

// --- Mixed-case codec names ---

#[test]
fn abl_mixed_case_codecs_does_not_panic() {
    let (sanitized, _report) = preprocess_remote_sdp(ABL_MIXED_CASE);
    assert!(!sanitized.is_empty());
    let _ = extract_offer_payloads(&sanitized);
}

#[test]
fn abl_mixed_case_h264_lowercase_is_recognized() {
    // The fixture uses "h264/90000" (lowercase)
    let payloads = extract_offer_payloads(ABL_MIXED_CASE);
    assert_eq!(payloads.h264, Some(96), "lowercase h264 must be recognized");
}

#[test]
fn abl_mixed_case_h265_uppercase_is_recognized() {
    let payloads = extract_offer_payloads(ABL_MIXED_CASE);
    assert_eq!(payloads.h265, Some(97), "uppercase H265 must be recognized");
}

#[test]
fn abl_mixed_case_opus_titlecase_is_recognized() {
    // The fixture uses "Opus/48000/2" (title case)
    let payloads = extract_offer_payloads(ABL_MIXED_CASE);
    assert_eq!(
        payloads.opus,
        Some(111),
        "title-case Opus must be recognized"
    );
}

// --- Non-contiguous payload type numbers ---

#[test]
fn abl_noncontiguous_pt_does_not_panic() {
    let (sanitized, _report) = preprocess_remote_sdp(ABL_NONCONTIGUOUS_PT);
    assert!(!sanitized.is_empty());
    let _ = extract_offer_payloads(&sanitized);
}

#[test]
fn abl_noncontiguous_pt_extracts_correct_values() {
    // PT 35 for H264, PT 63 for opus — non-standard, non-contiguous
    let payloads = extract_offer_payloads(ABL_NONCONTIGUOUS_PT);
    assert_eq!(
        payloads.h264,
        Some(35),
        "non-contiguous PT 35 for H264 must be extracted"
    );
    assert_eq!(
        payloads.opus,
        Some(63),
        "non-contiguous PT 63 for opus must be extracted"
    );
}

// --- Video-only offer (missing opus) ---

#[test]
fn abl_video_only_offer_does_not_panic() {
    let (sanitized, _report) = preprocess_remote_sdp(ABL_VIDEO_ONLY);
    assert!(!sanitized.is_empty());
    let payloads = extract_offer_payloads(&sanitized);
    assert_eq!(payloads.h264, Some(96));
    assert_eq!(payloads.opus, None, "video-only offer has no opus");
}

#[test]
fn abl_video_only_require_opus_returns_error() {
    let payloads = extract_offer_payloads(ABL_VIDEO_ONLY);
    let err = payloads.require(&[OfferCodec::Opus]).unwrap_err();
    assert_eq!(err.missing, vec![OfferCodec::Opus]);
}

#[test]
fn abl_video_only_require_h264_succeeds() {
    let payloads = extract_offer_payloads(ABL_VIDEO_ONLY);
    payloads.require(&[OfferCodec::H264]).unwrap();
}

// --- Audio-only offer (missing video codec) ---

#[test]
fn abl_audio_only_offer_does_not_panic() {
    let (sanitized, _report) = preprocess_remote_sdp(ABL_AUDIO_ONLY);
    assert!(!sanitized.is_empty());
    let payloads = extract_offer_payloads(&sanitized);
    assert_eq!(payloads.opus, Some(111));
    assert_eq!(payloads.h264, None, "audio-only offer has no H264");
    assert_eq!(payloads.h265, None, "audio-only offer has no H265");
}

#[test]
fn abl_audio_only_require_h264_returns_error() {
    let payloads = extract_offer_payloads(ABL_AUDIO_ONLY);
    let err = payloads.require(&[OfferCodec::H264]).unwrap_err();
    assert_eq!(err.missing, vec![OfferCodec::H264]);
}

#[test]
fn abl_audio_only_require_opus_succeeds() {
    let payloads = extract_offer_payloads(ABL_AUDIO_ONLY);
    payloads.require(&[OfferCodec::Opus]).unwrap();
}

// --- Comprehensive fixture: does not panic on any ABL fixture ---

#[test]
fn abl_offer_fixture_does_not_panic() {
    // All ABL fixtures must survive preprocessing + payload extraction
    // without panicking or triggering unbounded allocation.
    let fixtures = [
        ABL_MIXED_CASE,
        ABL_NONCONTIGUOUS_PT,
        ABL_VIDEO_ONLY,
        ABL_AUDIO_ONLY,
    ];
    for fixture in fixtures {
        let (sanitized, _) = preprocess_remote_sdp(fixture);
        let _ = extract_offer_payloads(&sanitized);
        let _ = extract_offer_payloads(fixture);
    }
}

// --- Edge cases: synthetic malformed inputs ---

#[test]
fn empty_sdp_does_not_panic() {
    let (sanitized, _) = preprocess_remote_sdp("");
    let payloads = extract_offer_payloads(&sanitized);
    assert_eq!(payloads.h264, None);
    assert_eq!(payloads.h265, None);
    assert_eq!(payloads.opus, None);
}

#[test]
fn sdp_with_only_rtpmap_no_media_line_does_not_panic() {
    let sdp = "v=0\r\na=rtpmap:96 H264/90000\r\n";
    let payloads = extract_offer_payloads(sdp);
    // Still extracts — the parser is line-based, not section-aware
    assert_eq!(payloads.h264, Some(96));
}

#[test]
fn sdp_with_pt_at_boundary_values() {
    // PT 0 and PT 127 are valid dynamic range boundaries
    let sdp = concat!(
        "v=0\r\n",
        "a=rtpmap:0 H264/90000\r\n",
        "a=rtpmap:127 opus/48000/2\r\n",
    );
    let payloads = extract_offer_payloads(sdp);
    assert_eq!(payloads.h264, Some(0));
    assert_eq!(payloads.opus, Some(127));
}

#[test]
fn sdp_with_very_long_codec_name_does_not_panic() {
    let long_name = "X".repeat(1000);
    let sdp = format!("v=0\r\na=rtpmap:96 {long_name}/90000\r\n");
    let payloads = extract_offer_payloads(&sdp);
    assert_eq!(payloads.h264, None);
}

#[test]
fn sdp_with_many_rtpmap_lines_does_not_allocate_unbounded() {
    // 256 rtpmap lines — should complete quickly without OOM
    let mut sdp = String::from("v=0\r\n");
    for pt in 0..=255u16 {
        sdp.push_str(&format!("a=rtpmap:{pt} H264/90000\r\n"));
    }
    let payloads = extract_offer_payloads(&sdp);
    // First match wins — PT 0
    assert_eq!(payloads.h264, Some(0));
}
