//! WebRTC egress contract tests for H264/H265 parameter set handling,
//! Access Unit boundary validation, and STAP-A/single-NALU strategies.
//!
//! These tests verify the `cheetah-codec` guarantees that the WebRTC
//! module relies on:
//!
//! 1. Parameter set cache correctly extracts SPS/PPS (H264) and
//!    VPS/SPS/PPS (H265) from Annex B payloads.
//! 2. Parameter sets are prepended to keyframes for bootstrap/GOP秒开.
//! 3. WebRTC egress contract rejects video frames without AU boundaries.
//! 4. WebRTC egress contract accepts properly marked frames.
//! 5. Audio frames do not require AU boundary markers.

use bytes::Bytes;
use cheetah_codec::{
    build_future_protocol_egress_contract_view, enforce_future_protocol_egress, AVFrame,
    AdapterContractError, CodecExtradata, CodecId, EgressAdapterView, FrameFlags, FrameFormat,
    FutureProtocolEgressContractView, FutureProtocolKind, MediaKind, ParameterSetCache,
    ParameterSetRequirement, Timebase, TrackId, TrackInfo,
};

// --- H264 test helpers ---

fn h264_sps() -> Bytes {
    // Minimal H264 SPS (NAL type 0x67 = 7)
    Bytes::from_static(&[0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40])
}

fn h264_pps() -> Bytes {
    // Minimal H264 PPS (NAL type 0x68 = 8)
    Bytes::from_static(&[0x68, 0xeb, 0xef, 0x20])
}

fn h264_idr_nalu() -> Bytes {
    // H264 IDR slice (NAL type 0x65 = 5)
    Bytes::from_static(&[0x65, 0x88, 0x80, 0x40])
}

fn h264_non_idr_nalu() -> Bytes {
    // H264 non-IDR slice (NAL type 0x61 = 1)
    Bytes::from_static(&[0x61, 0x9a, 0x00, 0x20])
}

fn h264_annexb_keyframe() -> Bytes {
    // SPS + PPS + IDR in Annex B format
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h264_sps());
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h264_pps());
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h264_idr_nalu());
    Bytes::from(buf)
}

fn h264_annexb_delta() -> Bytes {
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h264_non_idr_nalu());
    Bytes::from(buf)
}

fn h264_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![h264_sps()],
        pps: vec![h264_pps()],
        avcc: None,
    };
    track.refresh_readiness();
    track
}

// --- H265 test helpers ---

fn h265_vps() -> Bytes {
    // H265 VPS (NAL type 32, (0x40 >> 1) & 0x3f = 32)
    Bytes::from_static(&[0x40, 0x01, 0x0c, 0x01, 0xff, 0xff])
}

fn h265_sps() -> Bytes {
    // H265 SPS (NAL type 33, (0x42 >> 1) & 0x3f = 33)
    Bytes::from_static(&[0x42, 0x01, 0x01, 0x01, 0x60, 0x00])
}

fn h265_pps() -> Bytes {
    // H265 PPS (NAL type 34, (0x44 >> 1) & 0x3f = 34)
    Bytes::from_static(&[0x44, 0x01, 0xc0, 0xf7, 0xc0])
}

fn h265_idr_nalu() -> Bytes {
    // H265 IDR_W_RADL (NAL type 19, (0x26 >> 1) & 0x3f = 19)
    Bytes::from_static(&[0x26, 0x01, 0xaf, 0x08])
}

fn h265_annexb_keyframe() -> Bytes {
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h265_vps());
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h265_sps());
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h265_pps());
    buf.extend_from_slice(&[0, 0, 0, 1]);
    buf.extend_from_slice(&h265_idr_nalu());
    Bytes::from(buf)
}

fn h265_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::H265, 90_000);
    track.extradata = CodecExtradata::H265 {
        vps: vec![h265_vps()],
        sps: vec![h265_sps()],
        pps: vec![h265_pps()],
        hvcc: None,
    };
    track.refresh_readiness();
    track
}

// --- Parameter Set Cache Tests ---

#[test]
fn h264_parameter_set_cache_extracts_sps_pps_from_annexb_keyframe() {
    let mut cache = ParameterSetCache::default();
    let updated = cache.update_from_annexb(CodecId::H264, &h264_annexb_keyframe());
    assert!(updated, "cache should detect new parameter sets");
    assert!(
        cache.has_required_sets(CodecId::H264),
        "cache should have SPS+PPS after keyframe"
    );
}

#[test]
fn h264_parameter_set_cache_extracts_from_extradata() {
    let mut cache = ParameterSetCache::default();
    let extradata = CodecExtradata::H264 {
        sps: vec![h264_sps()],
        pps: vec![h264_pps()],
        avcc: None,
    };
    let updated = cache.update_from_extradata(&extradata);
    assert!(updated);
    assert!(cache.has_required_sets(CodecId::H264));
}

#[test]
fn h265_parameter_set_cache_extracts_vps_sps_pps_from_annexb() {
    let mut cache = ParameterSetCache::default();
    let updated = cache.update_from_annexb(CodecId::H265, &h265_annexb_keyframe());
    assert!(updated, "cache should detect H265 VPS/SPS/PPS");
    assert!(
        cache.has_required_sets(CodecId::H265),
        "cache should have VPS+SPS+PPS after H265 keyframe"
    );
}

#[test]
fn h265_parameter_set_cache_extracts_from_extradata() {
    let mut cache = ParameterSetCache::default();
    let extradata = CodecExtradata::H265 {
        vps: vec![h265_vps()],
        sps: vec![h265_sps()],
        pps: vec![h265_pps()],
        hvcc: None,
    };
    let updated = cache.update_from_extradata(&extradata);
    assert!(updated);
    assert!(cache.has_required_sets(CodecId::H265));
}

#[test]
fn parameter_set_cache_prepend_adds_sets_before_idr() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![h264_sps()],
        pps: vec![h264_pps()],
        avcc: None,
    });

    // Prepend SPS/PPS to an IDR-only Annex B payload — the cache
    // should produce a larger payload that includes the cached
    // parameter sets before the IDR.
    let idr_only = {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0, 0, 0, 1]);
        buf.extend_from_slice(&h264_idr_nalu());
        Bytes::from(buf)
    };
    let prepended = cache.prepend_to_annexb_access_unit(CodecId::H264, &idr_only);
    assert!(
        prepended.len() > idr_only.len(),
        "prepended payload should be larger than original"
    );
    // Should contain at least one Annex B start code.
    assert!(
        prepended.windows(4).any(|w| w == [0, 0, 0, 1]),
        "should have start codes"
    );
}

#[test]
fn h265_parameter_set_cache_prepend_adds_vps_sps_pps() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H265 {
        vps: vec![h265_vps()],
        sps: vec![h265_sps()],
        pps: vec![h265_pps()],
        hvcc: None,
    });

    let idr_payload = {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0, 0, 0, 1]);
        buf.extend_from_slice(&h265_idr_nalu());
        Bytes::from(buf)
    };
    let prepended = cache.prepend_to_annexb_access_unit(CodecId::H265, &idr_payload);
    assert!(
        prepended.len() > idr_payload.len(),
        "H265 prepend should add VPS+SPS+PPS"
    );
}

// --- WebRTC Egress Contract Tests ---

#[test]
fn webrtc_egress_rejects_h264_video_without_au_boundary() {
    let track = h264_track();
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        9_000,
        9_000,
        Timebase::new(1, 90_000),
        h264_annexb_keyframe(),
    );
    frame.flags.insert(FrameFlags::KEY);
    // Deliberately NOT setting START_OF_AU / END_OF_AU

    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    let err = enforce_future_protocol_egress(FutureProtocolKind::WebRtcRtpRtcp, &view)
        .expect_err("should reject video without AU boundary");
    assert!(matches!(
        err,
        AdapterContractError::WebRtcVideoMissingAccessUnitBoundary { .. }
    ));
}

#[test]
fn webrtc_egress_accepts_h264_video_with_au_boundary() {
    let track = h264_track();
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        9_000,
        9_000,
        Timebase::new(1, 90_000),
        h264_annexb_keyframe(),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    enforce_future_protocol_egress(FutureProtocolKind::WebRtcRtpRtcp, &view)
        .expect("should accept video with AU boundary");
}

#[test]
fn webrtc_egress_accepts_h265_video_with_au_boundary() {
    let track = h265_track();
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        18_000,
        18_000,
        Timebase::new(1, 90_000),
        h265_annexb_keyframe(),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    enforce_future_protocol_egress(FutureProtocolKind::WebRtcRtpRtcp, &view)
        .expect("should accept H265 video with AU boundary");
}

#[test]
fn webrtc_egress_accepts_audio_without_au_boundary() {
    let track = TrackInfo::new(TrackId(10), MediaKind::Audio, CodecId::Opus, 48_000);
    let frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::OpusPacket,
        960,
        960,
        Timebase::new(1, 48_000),
        Bytes::from_static(&[0xfc, 0xff, 0xfe]),
    );

    let cache = ParameterSetCache::default();
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    enforce_future_protocol_egress(FutureProtocolKind::WebRtcRtpRtcp, &view)
        .expect("audio should not require AU boundary markers");
}

#[test]
fn webrtc_egress_contract_view_carries_random_access_flag() {
    let track = h264_track();
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        9_000,
        9_000,
        Timebase::new(1, 90_000),
        h264_annexb_keyframe(),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    let contract =
        build_future_protocol_egress_contract_view(FutureProtocolKind::WebRtcRtpRtcp, &view)
            .unwrap();
    let FutureProtocolEgressContractView::WebRtc(webrtc) = contract else {
        panic!("expected WebRTC view");
    };
    assert!(webrtc.random_access, "keyframe should be random_access");
}

#[test]
fn webrtc_egress_contract_view_non_keyframe_is_not_random_access() {
    let track = h264_track();
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        18_000,
        18_000,
        Timebase::new(1, 90_000),
        h264_annexb_delta(),
    );
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &cache).unwrap();
    let contract =
        build_future_protocol_egress_contract_view(FutureProtocolKind::WebRtcRtpRtcp, &view)
            .unwrap();
    let FutureProtocolEgressContractView::WebRtc(webrtc) = contract else {
        panic!("expected WebRTC view");
    };
    assert!(
        !webrtc.random_access,
        "delta frame should not be random_access"
    );
}

#[test]
fn parameter_set_requirement_reports_needed_when_cache_empty() {
    let cache = ParameterSetCache::default();
    let req = cache.requirement_for_frame(CodecId::H264, true);
    assert_eq!(
        req,
        ParameterSetRequirement::RequiredMissing,
        "empty cache should report RequiredMissing for keyframe"
    );
}

#[test]
fn parameter_set_requirement_reports_available_when_cache_populated() {
    let mut cache = ParameterSetCache::default();
    cache.update_from_extradata(&CodecExtradata::H264 {
        sps: vec![h264_sps()],
        pps: vec![h264_pps()],
        avcc: None,
    });
    let req = cache.requirement_for_frame(CodecId::H264, true);
    assert_eq!(
        req,
        ParameterSetRequirement::RequiredPresent,
        "populated cache should report RequiredPresent for keyframe"
    );
}
