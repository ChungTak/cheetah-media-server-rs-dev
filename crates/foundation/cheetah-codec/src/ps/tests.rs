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
use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};
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
