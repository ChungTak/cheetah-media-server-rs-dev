//! PS module tests.
//!
//! PS 模块测试。

use super::{
    encode_pts_dts, PesPacket, PsDemuxDiagnostic, PsDemuxEvent, PsDemuxer, PsDemuxerConfig,
    PsMuxer, PsPacket, PsStreamKind,
};
use crate::frame::{AVFrame, FrameFlags, FrameFormat};
use crate::prelude::*;
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo, TrackReadiness};
use crate::ts_common::crc32_mpeg2;
use bytes::Bytes;

#[test]
fn pes_roundtrip() {
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: Some(89_000),
        payload: Bytes::from_static(b"es"),
    };
    let encoded = pes.encode();
    let (decoded, _) = PesPacket::parse(&encoded).expect("pes parse");
    assert_eq!(decoded.stream_id, 0xE0);
    assert_eq!(decoded.kind, PsStreamKind::Video);
    assert_eq!(decoded.payload, Bytes::from_static(b"es"));
    assert_eq!(decoded.pts, Some(90_000));
    assert_eq!(decoded.dts, Some(89_000));
}

#[test]
fn ps_roundtrip() {
    let ps = PsPacket {
        pes: vec![
            PesPacket {
                stream_id: 0xE0,
                kind: PsStreamKind::Video,
                pts: None,
                dts: None,
                payload: Bytes::from_static(b"v"),
            },
            PesPacket {
                stream_id: 0xC0,
                kind: PsStreamKind::Audio,
                pts: None,
                dts: None,
                payload: Bytes::from_static(b"a"),
            },
        ],
    };
    let encoded = ps.encode();
    let decoded = PsPacket::parse(&encoded);
    assert_eq!(decoded.pes.len(), 2);
    assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"v"));
    assert_eq!(decoded.pes[1].payload, Bytes::from_static(b"a"));
}

#[test]
fn ps_parse_bounded_limits_number_of_pes_packets() {
    let ps = PsPacket {
        pes: vec![
            PesPacket {
                stream_id: 0xE0,
                kind: PsStreamKind::Video,
                pts: None,
                dts: None,
                payload: Bytes::from_static(b"v0"),
            },
            PesPacket {
                stream_id: 0xC0,
                kind: PsStreamKind::Audio,
                pts: None,
                dts: None,
                payload: Bytes::from_static(b"a1"),
            },
            PesPacket {
                stream_id: 0xE0,
                kind: PsStreamKind::Video,
                pts: None,
                dts: None,
                payload: Bytes::from_static(b"v2"),
            },
        ],
    };
    let encoded = ps.encode();
    let decoded = PsPacket::parse_bounded(&encoded, encoded.len(), 2);
    assert_eq!(decoded.pes.len(), 2);
    assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"v0"));
    assert_eq!(decoded.pes[1].payload, Bytes::from_static(b"a1"));
}

#[test]
fn ps_parse_bounded_limits_bytes_for_truncated_rtp_payload() {
    let first = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: Some(89_000),
        payload: Bytes::from_static(b"video-es"),
    };
    let second = PesPacket {
        stream_id: 0xC0,
        kind: PsStreamKind::Audio,
        pts: Some(90_000),
        dts: None,
        payload: Bytes::from_static(b"audio-es"),
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(&[0x55, 0x66, 0x77, 0x88]);
    payload.extend_from_slice(first.encode().as_ref());
    payload.extend_from_slice(second.encode().as_ref());

    let first_len = first.encode().len();
    let truncated_len = 4 + first_len + 4;
    let decoded = PsPacket::parse_bounded(&payload, truncated_len, 16);
    assert_eq!(decoded.pes.len(), 1);
    assert_eq!(decoded.pes[0].kind, PsStreamKind::Video);
    assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"video-es"));
}

#[test]
fn ps_parse_bounded_zero_limits_return_empty() {
    let decoded = PsPacket::parse_bounded(&[0, 0, 1, 0xE0, 0, 3, 0x80, 0, 0], 0, 1);
    assert!(decoded.pes.is_empty());
    let decoded = PsPacket::parse_bounded(&[0, 0, 1, 0xE0, 0, 3, 0x80, 0, 0], 128, 0);
    assert!(decoded.pes.is_empty());
}

#[test]
fn ps_demuxer_and_muxer_roundtrip() {
    let video_track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
    let audio_track = TrackInfo::new(TrackId(0xC0), MediaKind::Audio, CodecId::G711A, 8_000);

    let mut muxer = PsMuxer::new();
    muxer.add_track(video_track.clone());
    muxer.add_track(audio_track.clone());

    // Create random keyframe video AVFrame: AnnexB format [0, 0, 0, 1, 0x67, ...] which triggers keyframe true
    let mut video_payload = vec![0, 0, 0, 1, 0x67, 0x42, 0, 0x0A]; // H264 SPS
    video_payload.extend_from_slice(b"video frame data");
    let mut video_frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000, // pts
        90_000, // dts
        Timebase::new(1, 90_000),
        Bytes::from(video_payload.clone()),
    );
    video_frame.flags.insert(FrameFlags::KEY);

    let audio_frame = AVFrame::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::G711A,
        FrameFormat::G711Packet,
        90_080,
        90_080,
        Timebase::new(1, 8_000),
        Bytes::from_static(b"audio frame data"),
    );

    let muxed_video = muxer.mux(&video_frame).expect("mux video");
    let muxed_audio = muxer.mux(&audio_frame).expect("mux audio");

    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let events1 = demuxer.push(&muxed_video);
    let events2 = demuxer.push(&muxed_audio);
    let events3 = demuxer.flush();

    let mut all_events = Vec::new();
    all_events.extend(events1);
    all_events.extend(events2);
    all_events.extend(events3);

    let mut found_tracks = false;
    let mut found_video_frame = false;
    let mut found_audio_frame = false;

    for event in all_events {
        match event {
            PsDemuxEvent::TrackInfo(tracks) => {
                assert!(tracks.iter().any(|t| t.track_id == TrackId(0xE0)));
                assert!(tracks.iter().any(|t| t.track_id == TrackId(0xC0)));
                found_tracks = true;
            }
            PsDemuxEvent::Frame(frame) => {
                if frame.track_id == TrackId(0xE0) {
                    assert_eq!(frame.pts, 90_000);
                    assert_eq!(frame.payload.as_ref(), video_payload.as_slice());
                    found_video_frame = true;
                } else if frame.track_id == TrackId(0xC0) {
                    assert_eq!(frame.pts, 90_080);
                    assert_eq!(frame.payload.as_ref(), b"audio frame data");
                    found_audio_frame = true;
                }
            }
            _ => {}
        }
    }

    assert!(found_tracks);
    assert!(found_video_frame);
    assert!(found_audio_frame);
}

#[test]
fn ps_demuxer_unbounded_video_pes_does_not_truncate_on_internal_nalu_start_code() {
    // PES_packet_length == 0 is allowed for video PES; the demuxer must scan for the
    // next PS-layer start code, *not* match every internal H.264 Annex-B start code
    // (`00 00 01` / `00 00 00 01`) inside the NAL payload. This regression test feeds
    // a single unbounded-length video PES carrying two NAL units (each with a 4-byte
    // start code) followed by a real PS-layer end-of-stream marker.
    let mut buf = Vec::new();

    // Pack header (mandatory before any PES is parsed at the top level).
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    // 10-byte SCR/mux-rate body and 0 stuffing bytes.
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    // Video PES with PES_packet_length == 0 (unbounded) and PTS only.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xE0]); // start_code + stream_id
    buf.extend_from_slice(&[0x00, 0x00]); // PES_packet_length = 0
    buf.push(0x80); // marker
    buf.push(0x80); // PTS_DTS_flags = 10 -> PTS only
    buf.push(0x05); // header_data_length
    buf.extend_from_slice(&encode_pts_dts(900_000, 0x2));
    // Annex-B NALU 1 with 4-byte start code (`00 00 00 01`).
    buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A]);
    // Annex-B NALU 2 with 3-byte start code (`00 00 01`).
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]);
    // Some additional payload bytes.
    buf.extend_from_slice(b"-extra-payload-");

    // Real next PS-layer packet: program end (`MPEG_program_end_code` = 0xB9).
    // Use a system header instead so it's a recognised PS-layer stream id.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBB, 0x00, 0x06, 0, 0, 0, 0, 0, 0]);

    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let events = demuxer.push(&buf);
    let _ = demuxer.flush();
    // Demuxer should not have produced PesParseError, and the inner NAL bytes must
    // remain intact in any video frame emitted later (we only check no diagnostics).
    for ev in events {
        if let PsDemuxEvent::Diagnostic(diag) = ev {
            panic!("unexpected diagnostic during unbounded video PES parse: {diag:?}");
        }
    }
}

fn limit_config() -> PsDemuxerConfig {
    PsDemuxerConfig {
        max_reassembly_bytes: 4 * 1024 * 1024,
        max_tracks: 32,
        max_pes_packet_size: 8 * 1024 * 1024,
        max_access_unit_size: 16 * 1024 * 1024,
        max_probe_packets: 1024,
        max_codec_probe_packets: 8,
    }
}

#[test]
fn ps_demuxer_respects_max_probe_packets() {
    let mut config = limit_config();
    config.max_probe_packets = 0;
    let mut demuxer = PsDemuxer::new(config);

    let mut buf = Vec::new();
    // Pack header with zero stuffing.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    let events = demuxer.push(&buf);
    assert!(events.iter().any(|e| matches!(
        e,
        PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
        if resource == "probe_packets"
    )));
}

#[test]
fn ps_demuxer_respects_max_tracks() {
    let mut config = limit_config();
    config.max_tracks = 0;
    let mut demuxer = PsDemuxer::new(config);

    let mut muxer = PsMuxer::new();
    muxer.add_track(TrackInfo::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    ));

    let mut video_payload = vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x0A];
    video_payload.extend_from_slice(b"v");
    let mut video_frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(video_payload),
    );
    video_frame.flags.insert(FrameFlags::KEY);

    let muxed = muxer.mux(&video_frame).expect("mux");
    let events = demuxer.push(&muxed);
    assert!(events.iter().any(|e| matches!(
        e,
        PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
        if resource == "tracks"
    )));
}

#[test]
fn ps_demuxer_respects_max_pes_packet_size() {
    let mut config = limit_config();
    config.max_pes_packet_size = 30;
    let mut demuxer = PsDemuxer::new(config);

    let payload = Bytes::from(vec![0u8; 50]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };
    let ps = PsPacket { pes: vec![pes] };
    let events = demuxer.push(&ps.encode());
    assert!(events.iter().any(|e| matches!(
        e,
        PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
        if resource == "pes_packet_size"
    )));
}

#[test]
fn ps_demuxer_respects_max_access_unit_size() {
    let mut config = limit_config();
    config.max_access_unit_size = 10;
    let mut demuxer = PsDemuxer::new(config);

    let mut buf = Vec::new();
    // Pack header.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    let payload = Bytes::from(vec![0u8; 20]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };
    buf.extend_from_slice(&pes.encode());

    let events = demuxer.push(&buf);
    assert!(events.iter().any(|e| matches!(
        e,
        PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
        if resource == "access_unit"
    )));
}

#[test]
fn ps_demuxer_periodic_psm_at_track_limit_does_not_wipe_tracks() {
    let mut config = limit_config();
    config.max_tracks = 1;
    let mut demuxer = PsDemuxer::new(config);

    let mut muxer = PsMuxer::new();
    muxer.add_track(TrackInfo::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    ));

    let mut video_payload = vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x0A];
    video_payload.extend_from_slice(b"v");
    let mut video_frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(video_payload),
    );
    video_frame.flags.insert(FrameFlags::KEY);

    let muxed1 = muxer.mux(&video_frame).expect("mux");
    let muxed2 = muxer.mux(&video_frame).expect("mux again");

    let events1 = demuxer.push(&muxed1);
    let events2 = demuxer.push(&muxed2);

    let limit1 = events1.iter().any(|e| {
        matches!(
            e,
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
            if resource == "tracks"
        )
    });
    let limit2 = events2.iter().any(|e| {
        matches!(
            e,
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
            if resource == "tracks"
        )
    });
    assert!(!limit1, "first PSM should fit within max_tracks");
    assert!(
        !limit2,
        "periodic retransmitted PSM must not wipe tracks at limit"
    );
}

#[test]
fn ps_demuxer_periodic_psm_preserves_audio_sample_rate() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let mut muxer = PsMuxer::new();

    muxer.add_track(TrackInfo::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    ));
    muxer.add_track(TrackInfo::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::AAC,
        8_000,
    ));

    let mut video_payload = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A];
    video_payload.extend_from_slice(b"v");
    let mut video_frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(video_payload),
    );
    video_frame.flags.insert(FrameFlags::KEY);

    // Valid ADTS frame: 44100 Hz, 2 channels, frame length 16.
    let mut audio_payload = vec![0xFF, 0xF1, 0x50, 0x80, 0x02, 0x00, 0x00];
    audio_payload.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x00]);
    let audio_frame = AVFrame::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::AAC,
        FrameFormat::AacRaw,
        90_000,
        90_000,
        Timebase::new(1, 44_100),
        Bytes::from(audio_payload),
    );

    let muxed_key1 = muxer.mux(&video_frame).expect("mux key1");
    let _ = demuxer.push(&muxed_key1);
    let _ = demuxer.push(&muxer.mux(&audio_frame).expect("mux audio"));

    let muxed_key2 = muxer.mux(&video_frame).expect("mux key2");
    let events2 = demuxer.push(&muxed_key2);
    assert!(
        !events2
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "periodic PSM must not re-announce a runtime-refined audio track"
    );

    let events3 = demuxer.push(&muxer.mux(&audio_frame).expect("mux audio again"));
    let audio_frame_after_psm = events3.iter().find_map(|e| match e {
        PsDemuxEvent::Frame(f) if f.media_kind == MediaKind::Audio => Some(f),
        _ => None,
    });
    assert_eq!(
        audio_frame_after_psm.map(|f| f.timebase.den),
        Some(44_100),
        "audio clock rate must stay at the ADTS-derived sample rate after periodic PSM"
    );
}

#[test]
fn ps_demuxer_over_limit_psm_keeps_existing_tracks_and_continues() {
    let mut config = limit_config();
    config.max_tracks = 1;
    let mut demuxer = PsDemuxer::new(config);

    // First muxer with a single video track.
    let mut muxer_one = PsMuxer::new();
    muxer_one.add_track(TrackInfo::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    ));

    let mut video_payload = vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x0A];
    video_payload.extend_from_slice(b"v1");
    let mut video_frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(video_payload.clone()),
    );
    video_frame.flags.insert(FrameFlags::KEY);
    let muxed_one = muxer_one.mux(&video_frame).expect("mux one");

    // Second muxer with two tracks; its PSM exceeds max_tracks=1.
    let mut muxer_two = PsMuxer::new();
    muxer_two.add_track(TrackInfo::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        90_000,
    ));
    muxer_two.add_track(TrackInfo::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::G711A,
        8_000,
    ));
    let muxed_two = muxer_two.mux(&video_frame).expect("mux two");

    let _ = demuxer.push(&muxed_one);
    let events = demuxer.push(&muxed_two);
    let flushed = demuxer.flush();

    let limit = events.iter().any(|e| {
        matches!(
            e,
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
            if resource == "tracks"
        )
    });
    assert!(
        limit,
        "PSM introducing more than max_tracks should emit LimitExceeded"
    );

    // The video PES that follows the over-limit PSM must still be decoded.
    let found_frame = events
        .iter()
        .chain(flushed.iter())
        .any(|e| matches!(e, PsDemuxEvent::Frame(f) if f.track_id == TrackId(0xE0)));
    assert!(
        found_frame,
        "video frame after over-limit PSM must still be emitted"
    );
}

#[test]
fn ps_demuxer_probe_limit_recovers_when_media_arrives() {
    let mut config = limit_config();
    config.max_probe_packets = 0;
    let mut demuxer = PsDemuxer::new(config);

    let mut buf = Vec::new();
    // Pack header with zero stuffing.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    // Video PES with an Annex-B start code and H.264 SPS NAL header so the
    // codec probe can identify the stream once the pack-header budget is reset.
    let payload = Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };
    buf.extend_from_slice(&pes.encode());

    let events = demuxer.push(&buf);
    let flushed = demuxer.flush();

    let probe_limit = events.iter().any(|e| {
        matches!(
            e,
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
            if resource == "probe_packets"
        )
    });
    assert!(
        probe_limit,
        "probe budget exceeded before media should be reported"
    );

    let found_track = events
        .iter()
        .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..)));
    let found_frame = flushed.iter().any(|e| matches!(e, PsDemuxEvent::Frame(..)));
    assert!(
        found_track,
        "media arriving after probe limit must still be parsed"
    );
    assert!(found_frame, "video frame must be emitted after recovery");
}

#[test]
fn ps_demuxer_probes_h264_without_psm() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let payload = Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };

    let events = demuxer.push(&pes.encode());
    let flushed = demuxer.flush();

    let track = events.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.first(),
        _ => None,
    });
    assert!(track.is_some(), "track must be discovered by probe");
    assert_eq!(track.unwrap().codec, CodecId::H264);

    assert!(flushed.iter().any(|e| matches!(e, PsDemuxEvent::Frame(..))));
}

#[test]
fn ps_demuxer_probes_h265_without_psm() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // H.265 VPS NAL unit: 4-byte start code, nal_unit_type 32, temporal id 1.
    let payload = Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x00, 0x00]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };

    let events = demuxer.push(&pes.encode());
    let track = events.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.first(),
        _ => None,
    });
    assert_eq!(track.map(|t| t.codec), Some(CodecId::H265));
}

#[test]
fn ps_demuxer_probes_aac_without_psm() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // Valid ADTS header for a 16-byte AAC frame: profile 1 (LC),
    // sampling_frequency_index 4 (44100 Hz), 2 channels, frame length 16.
    let mut payload = vec![0xFF, 0xF1, 0x50, 0x80, 0x02, 0x00, 0x00];
    payload.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x00]);
    let payload = Bytes::from(payload);

    let pes = PesPacket {
        stream_id: 0xC0,
        kind: PsStreamKind::Audio,
        pts: Some(90_000),
        dts: None,
        payload,
    };

    let events = demuxer.push(&pes.encode());
    let track = events.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.first(),
        _ => None,
    });
    assert_eq!(track.map(|t| t.codec), Some(CodecId::AAC));
    assert_eq!(track.map(|t| t.clock_rate), Some(44_100));

    let frame = events.iter().find_map(|e| match e {
        PsDemuxEvent::Frame(f) => Some(f),
        _ => None,
    });
    assert!(frame.is_some());
    assert_eq!(
        frame.unwrap().payload.as_ref(),
        &[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x00]
    );
}

#[test]
fn ps_demuxer_falls_back_to_g711_for_audio_without_psm() {
    let payload = Bytes::from_static(b"g711 audio samples");

    let pes_a = PesPacket {
        stream_id: 0xC0,
        kind: PsStreamKind::Audio,
        pts: Some(90_000),
        dts: None,
        payload,
    };
    let pes_u = PesPacket {
        stream_id: 0xD0,
        kind: PsStreamKind::Audio,
        pts: Some(90_000),
        dts: None,
        payload: Bytes::from_static(b"g711 audio samples"),
    };

    let mut demuxer_a = PsDemuxer::new(PsDemuxerConfig::default());
    let events_a = demuxer_a.push(&pes_a.encode());
    let track_a = events_a.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.iter().find(|x| x.track_id == TrackId(0xC0)),
        _ => None,
    });
    assert_eq!(track_a.map(|t| t.codec), Some(CodecId::G711A));

    let mut demuxer_u = PsDemuxer::new(PsDemuxerConfig::default());
    let events_u = demuxer_u.push(&pes_u.encode());
    let track_u = events_u.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.iter().find(|x| x.track_id == TrackId(0xD0)),
        _ => None,
    });
    assert_eq!(track_u.map(|t| t.codec), Some(CodecId::G711U));
}

#[test]
fn ps_demuxer_probe_h264_p_slice_not_h265() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // A common H.264 reference P-slice NAL header (0x41) collides with the H.265
    // VPS type 32 when only the first byte is inspected. The layer-id / temporal-id
    // consistency check should keep it as H.264.
    let payload = Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x41, 0xE0, 0x00, 0x00]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };

    let events = demuxer.push(&pes.encode());
    let track = events.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => t.first(),
        _ => None,
    });
    assert_eq!(track.map(|t| t.codec), Some(CodecId::H264));
}

#[test]
fn ps_demuxer_emits_unsupported_payload_after_codec_probe_budget() {
    let config = PsDemuxerConfig {
        max_codec_probe_packets: 1,
        ..Default::default()
    };
    let mut demuxer = PsDemuxer::new(config);

    let payload = Bytes::from(vec![0u8; 8]);
    let pes = PesPacket {
        stream_id: 0xE0,
        kind: PsStreamKind::Video,
        pts: Some(90_000),
        dts: None,
        payload,
    };

    // First unknown PES consumes the single probe packet budget.
    let _ = demuxer.push(&pes.encode());
    // Second unknown PES exceeds the budget and emits UnsupportedPayload.
    let events = demuxer.push(&pes.encode());
    // Third and later unknown PESes must stay silent; the diagnostic is emitted once.
    let mut all_events = events;
    all_events.extend(demuxer.push(&pes.encode()));
    all_events.extend(demuxer.push(&pes.encode()));

    let unsupported_count = all_events
        .iter()
        .filter(|e| {
            matches!(
                e,
                PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::UnsupportedPayload { stream_id: 0xE0 })
            )
        })
        .count();
    assert_eq!(unsupported_count, 1);
}

/// Build a minimal Program Stream Map payload for testing PSM version/duplicate logic.
fn encode_psm_payload(version: u8, entries: &[(u8, u8)]) -> Vec<u8> {
    let current_next = 1u8; // always applicable in tests
    let version_byte = (current_next << 7) | (0b11 << 5) | (version & 0x1F);

    let mut es_map = Vec::new();
    for (es_type, es_id) in entries {
        es_map.extend_from_slice(&[*es_type, *es_id, 0x00, 0x00]);
    }

    let es_map_length = es_map.len();
    let data_len = 10 + es_map_length + 4; // header + es_map + crc
    let mut out = Vec::with_capacity(data_len);
    out.push(version_byte);
    out.push(0xFF); // reserved/marker
    out.extend_from_slice(&0u16.to_be_bytes()); // program_stream_info_length
    out.extend_from_slice(&(es_map_length as u16).to_be_bytes());
    out.extend_from_slice(&es_map);

    let crc = crc32_mpeg2(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

#[test]
fn ps_demuxer_duplicate_psm_is_ignored() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let psm = encode_psm_payload(0, &[(0x1B, 0xE0)]);
    let mut first = vec![0x00, 0x00, 0x01, 0xBC];
    first.extend_from_slice(&(psm.len() as u16).to_be_bytes());
    first.extend_from_slice(&psm);

    let events1 = demuxer.push(&first);
    assert!(events1
        .iter()
        .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))));

    let events2 = demuxer.push(&first);
    assert!(
        !events2
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "duplicate PSM must not re-announce tracks"
    );
    assert!(
        !events2
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackRemoved(..))),
        "duplicate PSM must not remove tracks"
    );
}

#[test]
fn ps_demuxer_psm_version_change_adds_and_removes_tracks() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let psm0 = encode_psm_payload(0, &[(0x1B, 0xE0), (0x0F, 0xC0)]);
    let mut packet0 = vec![0x00, 0x00, 0x01, 0xBC];
    packet0.extend_from_slice(&(psm0.len() as u16).to_be_bytes());
    packet0.extend_from_slice(&psm0);

    let events0 = demuxer.push(&packet0);
    assert_eq!(
        events0
            .iter()
            .filter(|e| matches!(e, PsDemuxEvent::TrackInfo(..)))
            .count(),
        1,
        "initial PSM announces two tracks once"
    );
    let announced0 = events0.iter().find_map(|e| match e {
        PsDemuxEvent::TrackInfo(t) => Some(t.len()),
        _ => None,
    });
    assert_eq!(announced0, Some(2));

    // New PSM version removes the audio track.
    let psm1 = encode_psm_payload(1, &[(0x1B, 0xE0)]);
    let mut packet1 = vec![0x00, 0x00, 0x01, 0xBC];
    packet1.extend_from_slice(&(psm1.len() as u16).to_be_bytes());
    packet1.extend_from_slice(&psm1);

    let events1 = demuxer.push(&packet1);
    assert!(
        events1.iter().any(|e| matches!(
            e,
            PsDemuxEvent::TrackRemoved(ids) if ids.contains(&TrackId(0xC0))
        )),
        "audio track must be removed when PSM no longer lists it"
    );
    assert!(
        !events1
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "removing a track should not re-announce unchanged video track"
    );
}

#[test]
fn ps_demuxer_pes_probed_audio_not_removed_on_psm_version_bump() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // PSM declares only video; audio will be discovered from PES.
    let psm0 = encode_psm_payload(0, &[(0x1B, 0xE0)]);
    let mut packet0 = vec![0x00, 0x00, 0x01, 0xBC];
    packet0.extend_from_slice(&(psm0.len() as u16).to_be_bytes());
    packet0.extend_from_slice(&psm0);
    let _ = demuxer.push(&packet0);

    let audio_pes = PesPacket {
        stream_id: 0xC0,
        kind: PsStreamKind::Audio,
        pts: Some(90_000),
        dts: None,
        payload: Bytes::from_static(b"g711 audio samples"),
    };
    let events_audio = demuxer.push(&audio_pes.encode());
    assert!(
        events_audio
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "audio track should be discovered from PES"
    );

    // PSM re-issued with a new version but still declares only video.
    let psm1 = encode_psm_payload(1, &[(0x1B, 0xE0)]);
    let mut packet1 = vec![0x00, 0x00, 0x01, 0xBC];
    packet1.extend_from_slice(&(psm1.len() as u16).to_be_bytes());
    packet1.extend_from_slice(&psm1);
    let events = demuxer.push(&packet1);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackRemoved(..))),
        "PES-probed audio must not be removed by a PSM that never declared it"
    );
}

#[test]
fn ps_demuxer_psm_not_current_next_is_ignored() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // current_next_indicator = 0 means the PSM is not yet applicable.
    let version_byte = 0u8; // current_next=0, version=0
    let es_map = [(0x1B, 0xE0)];
    let mut es_map_bytes = Vec::new();
    for (es_type, es_id) in &es_map {
        es_map_bytes.extend_from_slice(&[*es_type, *es_id, 0x00, 0x00]);
    }
    let es_map_length = es_map_bytes.len();
    let data_len = 10 + es_map_length + 4;
    let mut payload = Vec::with_capacity(data_len);
    payload.push(version_byte);
    payload.push(0xFF);
    payload.extend_from_slice(&0u16.to_be_bytes());
    payload.extend_from_slice(&(es_map_length as u16).to_be_bytes());
    payload.extend_from_slice(&es_map_bytes);
    let crc = crc32_mpeg2(&payload);
    payload.extend_from_slice(&crc.to_be_bytes());

    let mut packet = vec![0x00, 0x00, 0x01, 0xBC];
    packet.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    packet.extend_from_slice(&payload);

    let events = demuxer.push(&packet);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "PSM with current_next=0 must not announce tracks"
    );
}

#[test]
fn ps_demuxer_empty_supported_psm_does_not_wipe_tracks() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let psm0 = encode_psm_payload(0, &[(0x1B, 0xE0)]);
    let mut packet0 = vec![0x00, 0x00, 0x01, 0xBC];
    packet0.extend_from_slice(&(psm0.len() as u16).to_be_bytes());
    packet0.extend_from_slice(&psm0);
    let _ = demuxer.push(&packet0);

    // A PSM that lists only an unsupported stream type yields no supported tracks;
    // it must not remove the existing video track.
    let psm1 = encode_psm_payload(1, &[(0xFF, 0xC0)]);
    let mut packet1 = vec![0x00, 0x00, 0x01, 0xBC];
    packet1.extend_from_slice(&(psm1.len() as u16).to_be_bytes());
    packet1.extend_from_slice(&psm1);
    let events = demuxer.push(&packet1);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackRemoved(..))),
        "unsupported-only PSM must not wipe existing tracks"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PsDemuxEvent::TrackInfo(..))),
        "unsupported-only PSM must not re-announce tracks"
    );
}

#[test]
fn ps_demuxer_over_limit_psm_retransmission_still_emits_limit() {
    let mut config = limit_config();
    config.max_tracks = 1;
    let mut demuxer = PsDemuxer::new(config);

    // First PSM with one track fits.
    let psm0 = encode_psm_payload(0, &[(0x1B, 0xE0)]);
    let mut packet0 = vec![0x00, 0x00, 0x01, 0xBC];
    packet0.extend_from_slice(&(psm0.len() as u16).to_be_bytes());
    packet0.extend_from_slice(&psm0);
    let _ = demuxer.push(&packet0);

    // Second PSM with two tracks exceeds the limit; each retransmission must still
    // report the limit so the cache does not swallow the diagnostic.
    let psm1 = encode_psm_payload(1, &[(0x1B, 0xE0), (0x0F, 0xC0)]);
    let mut packet1 = vec![0x00, 0x00, 0x01, 0xBC];
    packet1.extend_from_slice(&(psm1.len() as u16).to_be_bytes());
    packet1.extend_from_slice(&psm1);

    let events1 = demuxer.push(&packet1);
    let events2 = demuxer.push(&packet1);

    for events in [events1, events2] {
        assert!(
            events.iter().any(|e| matches!(
                e,
                PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded { resource })
                if resource == "tracks"
            )),
            "over-limit PSM retransmission must still emit LimitExceeded"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PsDemuxEvent::TrackRemoved(..))),
            "over-limit PSM must not remove existing tracks"
        );
    }
}

// Build a raw PES packet for tests. `pes_len` is the explicit PES_packet_length;
// `None` means unbounded (0). `header_stuffing` is the number of 0xFF stuffing bytes
// placed after the PTS/DTS fields inside the PES_header_data area.
fn encode_pes_raw(
    stream_id: u8,
    pts: Option<i64>,
    dts: Option<i64>,
    data_alignment: bool,
    header_stuffing: usize,
    pes_len: Option<usize>,
    payload: &[u8],
) -> Vec<u8> {
    let mut header_data = Vec::new();
    let mut flags2 = 0u8;
    if let Some(pts) = pts {
        flags2 |= 0x80;
        header_data.extend_from_slice(&encode_pts_dts(pts, 0x2));
    }
    if let Some(dts) = dts {
        flags2 |= 0x40;
        header_data.extend_from_slice(&encode_pts_dts(dts, 0x1));
    }
    header_data.extend((0..header_stuffing).map(|_| 0xFFu8));

    let length = pes_len.unwrap_or(0);
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x00, 0x01, stream_id]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    let mut flags1 = 0x80u8;
    if data_alignment {
        flags1 |= 0x04;
    }
    out.push(flags1);
    out.push(flags2);
    out.push(header_data.len() as u8);
    out.extend_from_slice(&header_data);
    out.extend_from_slice(payload);
    out
}

#[test]
fn pes_packet_parse_unbounded_stops_at_next_ps_start_code() {
    let first_payload = b"first";
    let second_payload = b"second";
    let first = encode_pes_raw(
        0xE0,
        Some(90_000),
        None,
        false,
        0,
        None,
        first_payload.as_slice(),
    );
    let second = encode_pes_raw(
        0xC0,
        Some(90_000),
        None,
        false,
        0,
        Some(3 + 5 + second_payload.len()),
        second_payload.as_slice(),
    );

    let mut buf = Vec::new();
    buf.extend_from_slice(&first);
    buf.extend_from_slice(&second);

    let (decoded, consumed) = PesPacket::parse(&buf).expect("parse first PES");
    assert_eq!(decoded.stream_id, 0xE0);
    assert_eq!(decoded.payload.as_ref(), first_payload.as_slice());
    assert_eq!(consumed, first.len());

    let (decoded2, _) = PesPacket::parse(&buf[consumed..]).expect("parse second PES");
    assert_eq!(decoded2.stream_id, 0xC0);
    assert_eq!(decoded2.payload.as_ref(), second_payload.as_slice());
}

#[test]
fn ps_demuxer_cross_pes_access_unit_reassembles_unbounded_pes() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // First part of the frame: Annex-B start code + SPS NAL header.
    let first_part = &[0x00, 0x00, 0x00, 0x01, 0x67, b'v'][..];
    // Second part: arbitrary continuation bytes without any start code.
    let second_part = b"ideo-continuation";

    let mut buf = Vec::new();
    // Pack header with zero stuffing.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    // Two unbounded PES packets (PES_packet_length == 0) carrying one split video AU.
    // data_alignment is false on both: the second PES is a continuation of the same AU.
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(100_000),
        None,
        false,
        0,
        None,
        first_part,
    ));
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        None,
        None,
        false,
        0,
        None,
        second_part,
    ));

    let _events = demuxer.push(&buf);
    let flush_events = demuxer.flush();

    let mut found_frame = None;
    for ev in &flush_events {
        if let PsDemuxEvent::Frame(frame) = ev {
            if frame.track_id == TrackId(0xE0) {
                found_frame = Some(frame.clone());
            }
        }
    }

    let frame = found_frame.expect("cross-PES video AU should be emitted on flush");
    let expected: Vec<u8> = first_part.iter().chain(second_part).copied().collect();
    assert_eq!(frame.payload.as_ref(), expected.as_slice());
    assert_eq!(frame.pts, 100_000);
}

#[test]
fn ps_demuxer_data_alignment_splits_access_units() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let mut buf = Vec::new();
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    // Two back-to-back unbounded video PES packets, each with data_alignment set,
    // so each starts a new access unit. No pack header between them. A trailing
    // zero-length system header delimits the second PES so it can be parsed during
    // the same push.
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(100_000),
        None,
        true,
        0,
        None,
        &[0x00, 0x00, 0x00, 0x01, 0x67, b'1'][..],
    ));
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(200_000),
        None,
        true,
        0,
        None,
        &[0x00, 0x00, 0x00, 0x01, 0x68, b'2'][..],
    ));
    // system_header_start_code with zero-length header body.
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBB, 0x00, 0x00]);

    let events = demuxer.push(&buf);
    let frames: Vec<_> = events
        .into_iter()
        .filter_map(|e| match e {
            PsDemuxEvent::Frame(f) if f.track_id == TrackId(0xE0) => Some(*f),
            _ => None,
        })
        .collect();

    assert_eq!(
        frames.len(),
        1,
        "first frame is emitted at data_alignment boundary"
    );
    assert_eq!(frames[0].pts, 100_000);

    let mut flush_events = demuxer.flush();
    let flush_frames: Vec<_> = flush_events
        .drain(..)
        .filter_map(|e| match e {
            PsDemuxEvent::Frame(f) if f.track_id == TrackId(0xE0) => Some(*f),
            _ => None,
        })
        .collect();

    assert_eq!(
        flush_frames.len(),
        1,
        "second frame is emitted on flush and not merged into first"
    );
    assert_eq!(flush_frames[0].pts, 200_000);
}

#[test]
fn ps_demuxer_pes_stuffing_is_skipped() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let payload: Vec<u8> = [0x00, 0x00, 0x00, 0x01, 0x67]
        .into_iter()
        .chain(b"actual-payload".iter().copied())
        .collect();
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

    // Bounded video PES with 4 stuffing bytes (0xFF) after the PTS.
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(90_000),
        None,
        false,
        4,
        Some(3 + (5 + 4) + payload.len()),
        &payload,
    ));

    let events = demuxer.push(&buf);
    let mut flush = demuxer.flush();

    let mut found = false;
    for ev in events.into_iter().chain(flush.drain(..)) {
        if let PsDemuxEvent::Frame(frame) = ev {
            if frame.track_id == TrackId(0xE0) {
                assert_eq!(frame.payload.as_ref(), payload.as_slice());
                assert_eq!(frame.pts, 90_000);
                found = true;
            }
        }
    }
    assert!(found, "payload should be extracted after stuffing bytes");
}

#[test]
fn ps_demuxer_private_stream_is_ignored() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let mut buf = Vec::new();
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);
    buf.extend_from_slice(&encode_pes_raw(
        0xBD,
        Some(90_000),
        None,
        false,
        0,
        Some(3 + 5 + 8),
        b"private1",
    ));

    let events = demuxer.push(&buf);
    let flush = demuxer.flush();

    for ev in events.into_iter().chain(flush.into_iter()) {
        match ev {
            PsDemuxEvent::TrackInfo(_) | PsDemuxEvent::Frame(_) => {
                panic!("private stream must not produce track or frame")
            }
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::PesParseError) => {
                panic!("private stream must not be treated as malformed")
            }
            _ => {}
        }
    }
}

#[test]
fn ps_demuxer_caches_h264_parameter_sets_and_prepends_to_keyframe() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let sps: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1E];
    let pps: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE, 0x3C, 0x80];
    let idr: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84];

    let config_payload = [sps, pps].concat();

    // Pack 1: SPS/PPS buffered (no data_alignment, no keyframe).
    let mut pack1 = Vec::new();
    pack1.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    pack1.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);
    pack1.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(90_000),
        None,
        false,
        0,
        Some(8 + config_payload.len()),
        &config_payload,
    ));

    // Pack 2: IDR PES with data_alignment; the SPS/PPS AU is flushed before it,
    // and the keyframe AU is emitted on flush with cached SPS/PPS prepended.
    let mut pack2 = Vec::new();
    pack2.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    pack2.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);
    pack2.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(180_000),
        None,
        true,
        0,
        Some(8 + idr.len()),
        idr,
    ));

    let mut all_events = demuxer.push(&pack1);
    all_events.extend(demuxer.push(&pack2));
    all_events.extend(demuxer.flush());

    let mut keyframe: Option<AVFrame> = None;
    let mut track_ready = false;

    for ev in all_events {
        match ev {
            PsDemuxEvent::TrackInfo(tracks) => {
                if let Some(t) = tracks.iter().find(|t| t.track_id == TrackId(0xE0)) {
                    if t.readiness == TrackReadiness::Ready {
                        track_ready = true;
                        assert!(
                            matches!(
                                &t.extradata,
                                CodecExtradata::H264 { sps, pps, .. }
                                    if sps.len() == 1 && pps.len() == 1
                            ),
                            "track should carry cached H.264 parameter sets"
                        );
                    }
                }
            }
            PsDemuxEvent::Frame(frame) => {
                if frame.track_id == TrackId(0xE0) && frame.flags.contains(FrameFlags::KEY) {
                    keyframe = Some(*frame);
                }
            }
            _ => {}
        }
    }

    assert!(
        track_ready,
        "track should be reported Ready once parameter sets are cached"
    );

    let keyframe = keyframe.expect("keyframe should be emitted on flush");
    let payload = keyframe.payload.as_ref();

    // Expected prefix: start-code + SPS (without start code) + start-code + PPS + start-code + IDR.
    let mut expected_prefix = Vec::new();
    expected_prefix.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    expected_prefix.extend_from_slice(&[0x67, 0x42, 0xC0, 0x1E]);
    expected_prefix.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    expected_prefix.extend_from_slice(&[0x68, 0xCE, 0x3C, 0x80]);
    expected_prefix.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    expected_prefix.extend_from_slice(&[0x65, 0x88, 0x84]);

    assert_eq!(
        payload,
        expected_prefix.as_slice(),
        "keyframe should be prefixed with cached SPS/PPS"
    );
}

#[test]
fn ps_demuxer_reports_missing_parameter_sets_for_keyframe() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    // A single PES containing only an IDR, with no preceding SPS/PPS.
    let idr: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84];

    let mut buf = Vec::new();
    buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
    buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);
    buf.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(90_000),
        None,
        false,
        0,
        Some(8 + idr.len()),
        idr,
    ));

    let events = demuxer.push(&buf);
    let mut flush = demuxer.flush();

    let mut keyframe: Option<AVFrame> = None;
    let mut missing_reported = false;

    for ev in events.into_iter().chain(flush.drain(..)) {
        match ev {
            PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::MissingParameterSets {
                stream_id: 0xE0,
                codec: CodecId::H264,
            }) => missing_reported = true,
            PsDemuxEvent::Frame(frame) => {
                if frame.track_id == TrackId(0xE0) && frame.flags.contains(FrameFlags::KEY) {
                    keyframe = Some(*frame);
                }
            }
            _ => {}
        }
    }

    assert!(keyframe.is_some(), "IDR frame should still be emitted");
    assert!(
        missing_reported,
        "keyframe without cached SPS/PPS should report MissingParameterSets"
    );
}

#[test]
fn ps_demuxer_clears_parameter_set_cache_on_psm_stream_removal() {
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());

    let sps: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1E];
    let pps: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE, 0x3C, 0x80];
    let idr: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84];

    let pack_header = [
        0x00, 0x00, 0x01, 0xBA, 0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8,
    ];

    // PSM 0: declare H.264 video at 0xE0.
    let psm0 = encode_psm_payload(0, &[(0x1B, 0xE0)]);
    let mut pack0 = Vec::new();
    pack0.extend_from_slice(&pack_header);
    pack0.extend_from_slice(&[0x00, 0x00, 0x01, 0xBC]);
    pack0.extend_from_slice(&(psm0.len() as u16).to_be_bytes());
    pack0.extend_from_slice(&psm0);

    // PES 0: SPS/PPS, no keyframe.
    let config_payload = [sps, pps].concat();
    let mut pack1 = Vec::new();
    pack1.extend_from_slice(&pack_header);
    pack1.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(90_000),
        None,
        false,
        0,
        Some(8 + config_payload.len()),
        &config_payload,
    ));

    // PSM 1: replace 0xE0 with an unrelated audio stream so the video track is removed.
    let psm1 = encode_psm_payload(1, &[(0x0F, 0xC0)]);
    let mut pack2 = Vec::new();
    pack2.extend_from_slice(&pack_header);
    pack2.extend_from_slice(&[0x00, 0x00, 0x01, 0xBC]);
    pack2.extend_from_slice(&(psm1.len() as u16).to_be_bytes());
    pack2.extend_from_slice(&psm1);

    // PES 1: pure IDR after the video track was removed. The previous SPS/PPS cache
    // must have been cleared, so the keyframe cannot be prefixed and must report MissingParameterSets.
    let mut pack3 = Vec::new();
    pack3.extend_from_slice(&pack_header);
    pack3.extend_from_slice(&encode_pes_raw(
        0xE0,
        Some(180_000),
        None,
        false,
        0,
        Some(8 + idr.len()),
        idr,
    ));

    let mut all_events = demuxer.push(&pack0);
    all_events.extend(demuxer.push(&pack1));
    all_events.extend(demuxer.push(&pack2));
    all_events.extend(demuxer.push(&pack3));
    all_events.extend(demuxer.flush());

    let mut missing_reported = false;
    for ev in all_events {
        if let PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::MissingParameterSets {
            stream_id: 0xE0,
            codec: CodecId::H264,
        }) = ev
        {
            missing_reported = true;
        }
    }

    assert!(
        missing_reported,
        "keyframe after PSM removal must not reuse stale parameter set cache"
    );
}
