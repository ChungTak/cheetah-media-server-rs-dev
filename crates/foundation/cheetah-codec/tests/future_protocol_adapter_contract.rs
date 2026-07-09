use bytes::Bytes;
use cheetah_codec::{
    build_future_protocol_egress_contract_view, enforce_future_protocol_egress,
    enforce_future_protocol_ingress, AVFrame, AdapterContractError, CodecConfigRequirement,
    CodecExtradata, CodecId, EgressAdapterView, FrameFlags, FrameFormat,
    FutureProtocolEgressContractView, FutureProtocolKind, IngressAdapterFrame, MediaKind,
    ParameterSetCache, SourceTimestamp, Timebase, TimestampNormalizeOutput, TrackId, TrackInfo,
};

fn h264_track(track_id: u32) -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(track_id), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x64, 0x00, 0x1f])],
        pps: vec![Bytes::from_static(&[0x68, 0xeb, 0xef, 0x20])],
        avcc: None,
    };
    track.refresh_readiness();
    track
}

fn normalized_output(pts: i64, dts: i64, discontinuity: bool) -> TimestampNormalizeOutput {
    TimestampNormalizeOutput {
        pts,
        dts,
        pts_us: pts * 1000,
        dts_us: dts * 1000,
        discontinuity,
        alerts: Default::default(),
    }
}

#[test]
fn srt_ingress_requires_normalized_timeline_source() {
    let track = h264_track(1);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        100,
        100,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65]),
    );
    frame.flags.insert(FrameFlags::KEY);

    let passthrough = IngressAdapterFrame::from_passthrough(track.clone(), frame.clone())
        .expect("passthrough frame should be accepted for generic protocols");
    let err = enforce_future_protocol_ingress(FutureProtocolKind::SrtTransport, &passthrough)
        .expect_err("srt ingress must reject bypassed normalization");
    assert!(matches!(
        err,
        AdapterContractError::SrtBypassedMediaNormalization
    ));

    let normalized =
        IngressAdapterFrame::from_normalized(track, frame, &normalized_output(100, 100, false))
            .expect("normalized ingress frame should be accepted");
    enforce_future_protocol_ingress(FutureProtocolKind::SrtTransport, &normalized)
        .expect("srt ingress should accept normalizer-driven timeline");
}

#[test]
fn normalized_ingress_rejects_timestamp_mismatch() {
    let track = h264_track(2);
    let frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        200,
        180,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x61]),
    );
    let err =
        IngressAdapterFrame::from_normalized(track, frame, &normalized_output(201, 180, false))
            .expect_err("mismatched normalized timestamps must fail");
    assert!(matches!(
        err,
        AdapterContractError::NormalizedTimestampMismatch { .. }
    ));
}

#[test]
fn egress_view_includes_required_export_fields() {
    let track = h264_track(3);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        9_000,
        9_000,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88]),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let mut parameter_sets = ParameterSetCache::default();
    parameter_sets.update_from_extradata(&track.extradata);

    let view = EgressAdapterView::build(&track, &frame, &parameter_sets)
        .expect("egress export view should be created");

    assert_eq!(view.codec(), CodecId::H264);
    assert!(view.fragment_boundary().start_of_access_unit);
    assert!(view.fragment_boundary().end_of_access_unit);
    assert_eq!(view.encapsulation_timestamps().rtmp_timestamp_ms, 100);
    assert!(!view.parameter_set_replay().units.is_empty());
    assert_eq!(
        view.codec_config().requirement,
        CodecConfigRequirement::Required
    );
}

#[test]
fn webrtc_egress_requires_access_unit_boundaries_for_video() {
    let track = h264_track(4);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        100,
        100,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65]),
    );
    frame.flags.insert(FrameFlags::KEY);

    let mut parameter_sets = ParameterSetCache::default();
    parameter_sets.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &parameter_sets)
        .expect("view should build before protocol-specific validation");

    let err = enforce_future_protocol_egress(FutureProtocolKind::WebRtcRtpRtcp, &view)
        .expect_err("webrtc contract must reject video without AU boundary markers");
    assert!(matches!(
        err,
        AdapterContractError::WebRtcVideoMissingAccessUnitBoundary { .. }
    ));
}

#[test]
fn webrtc_ingress_requires_normalized_timeline_source() {
    let track = h264_track(5);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        120,
        120,
        Timebase::new(1, 1_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65]),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);

    let passthrough = IngressAdapterFrame::from_passthrough(track.clone(), frame.clone())
        .expect("passthrough frame should be accepted for generic protocols");
    let err = enforce_future_protocol_ingress(FutureProtocolKind::WebRtcRtpRtcp, &passthrough)
        .expect_err("webrtc ingress must reject bypassed normalization");
    assert!(matches!(
        err,
        AdapterContractError::WebRtcBypassedMediaNormalization
    ));

    let normalized =
        IngressAdapterFrame::from_normalized(track, frame, &normalized_output(120, 120, false))
            .expect("normalized ingress frame should be accepted");
    enforce_future_protocol_ingress(FutureProtocolKind::WebRtcRtpRtcp, &normalized)
        .expect("webrtc ingress should accept normalizer-driven timeline");
}

#[test]
fn srt_egress_contract_view_uses_canonical_timeline_and_codec_config() {
    let track = h264_track(6);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        4_560,
        4_500,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88]),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);
    frame.set_source_timestamp(SourceTimestamp::Rtp(cheetah_codec::RtpTimestamp::new(
        3_895_818_000,
        3_895_818_000,
    )));

    let mut parameter_sets = ParameterSetCache::default();
    parameter_sets.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &parameter_sets).expect("build view");
    let contract =
        build_future_protocol_egress_contract_view(FutureProtocolKind::SrtTransport, &view)
            .expect("build srt contract view");

    let FutureProtocolEgressContractView::Srt(srt) = contract else {
        panic!("expected SRT contract view");
    };
    assert_eq!(srt.track_id, track.track_id);
    assert_eq!(srt.codec, CodecId::H264);
    assert_eq!(srt.dts_ms, 50);
    assert_eq!(srt.composition_time_ms, 1);
    assert_eq!(
        srt.codec_config.requirement,
        CodecConfigRequirement::Required
    );
    assert!(!srt.parameter_set_replay.units.is_empty());
}

#[test]
fn webrtc_egress_contract_view_uses_exported_rtp_timestamp_only() {
    let track = h264_track(7);
    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        FrameFormat::CanonicalH26x,
        9_900,
        9_000,
        Timebase::new(1, 90_000),
        Bytes::from_static(&[0, 0, 0, 1, 0x65]),
    );
    frame.flags.insert(FrameFlags::KEY);
    frame.flags.insert(FrameFlags::START_OF_AU);
    frame.flags.insert(FrameFlags::END_OF_AU);
    frame.flags.insert(FrameFlags::DISCONTINUITY);
    frame.set_source_timestamp(SourceTimestamp::Rtp(cheetah_codec::RtpTimestamp::new(
        123_456_789,
        123_456_789,
    )));

    let mut parameter_sets = ParameterSetCache::default();
    parameter_sets.update_from_extradata(&track.extradata);
    let view = EgressAdapterView::build(&track, &frame, &parameter_sets).expect("build view");
    let contract =
        build_future_protocol_egress_contract_view(FutureProtocolKind::WebRtcRtpRtcp, &view)
            .expect("build webrtc contract view");
    let FutureProtocolEgressContractView::WebRtc(webrtc) = contract else {
        panic!("expected WebRTC contract view");
    };

    assert_eq!(webrtc.track_id, track.track_id);
    assert_eq!(webrtc.rtp_timestamp_ticks, 9_900);
    assert!(webrtc.random_access);
    assert!(webrtc.discontinuity);
    assert!(webrtc.fragment_boundary.start_of_access_unit);
    assert!(webrtc.fragment_boundary.end_of_access_unit);
}
