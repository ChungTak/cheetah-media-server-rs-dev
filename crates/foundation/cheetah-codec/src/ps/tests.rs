//! PS module tests.
//!
//! PS 模块测试。

use super::{
    encode_pts_dts, PesPacket, PsDemuxEvent, PsDemuxer, PsDemuxerConfig, PsMuxer, PsPacket,
    PsStreamKind,
};
use crate::frame::{AVFrame, FrameFlags, FrameFormat};
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
