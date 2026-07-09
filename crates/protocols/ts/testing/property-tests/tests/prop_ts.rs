//! Property-based tests for the TS protocol.
//!
//! Two surfaces are exercised:
//! * `cheetah-codec` MPEG-TS mux/demux (the shared container the TS protocol
//!   relies on) — packet framing invariants and mux→demux roundtrips.
//! * `cheetah-ts-core` request-target / WebSocket-accept parsing.

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, MpegTsDemuxEvent, MpegTsDemuxer,
    MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig, Timebase, TrackId, TrackInfo,
};
use cheetah_ts_core::request::TsCoreError;
use cheetah_ts_core::{parse_ts_request_target, websocket_accept_key};
use proptest::prelude::*;

const TS_PACKET_SIZE: usize = 188;
const TS_SYNC_BYTE: u8 = 0x47;

fn h264_track() -> TrackInfo {
    TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000)
}

/// Annex-B H264 access unit with a leading start code so the muxer/demuxer
/// treat it as a complete NAL. `nal_type` 5 = IDR (keyframe), 1 = non-IDR.
fn annexb_au(nal_type: u8, extra_len: usize) -> Bytes {
    let mut buf = vec![0x00, 0x00, 0x00, 0x01, nal_type];
    buf.extend(std::iter::repeat_n(0xAA, extra_len));
    Bytes::from(buf)
}

fn video_frame(dts_us: i64, pts_us: i64, keyframe: bool, payload: Bytes) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        pts_us,
        dts_us,
        Timebase::new(1, 1_000_000),
        payload,
    );
    if keyframe {
        frame.flags = FrameFlags::KEY;
    }
    frame
}

fn mux_stream(frames: &[AVFrame]) -> Vec<u8> {
    let tracks = vec![h264_track()];
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    let mut out = Vec::new();
    let mut push = |events: Vec<MpegTsMuxEvent>| {
        for ev in events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                out.extend_from_slice(&data);
            }
        }
    };
    push(muxer.write_tables());
    for f in frames {
        push(muxer.push_frame(f));
    }
    push(muxer.flush());
    out
}

proptest! {
    /// Every byte produced by the muxer is 188-byte aligned and each packet
    /// begins with the TS sync byte 0x47.
    #[test]
    fn prop_ts_packets_are_aligned_and_synced(
        frame_count in 1usize..8,
        payload_len in 0usize..320,
    ) {
        let frames: Vec<AVFrame> = (0..frame_count)
            .map(|i| {
                let keyframe = i == 0;
                video_frame(
                    i as i64 * 40_000,
                    i as i64 * 40_000,
                    keyframe,
                    annexb_au(if keyframe { 5 } else { 1 }, payload_len),
                )
            })
            .collect();

        let stream = mux_stream(&frames);
        prop_assert!(!stream.is_empty());
        prop_assert_eq!(stream.len() % TS_PACKET_SIZE, 0);
        for pkt in stream.chunks(TS_PACKET_SIZE) {
            prop_assert_eq!(pkt[0], TS_SYNC_BYTE);
        }
    }

    /// Mux then demux yields at least one track and one frame regardless of the
    /// payload size / frame count, and the demuxer never panics.
    #[test]
    fn prop_mux_demux_roundtrip_recovers_track_and_frames(
        frame_count in 1usize..8,
        payload_len in 1usize..320,
    ) {
        let frames: Vec<AVFrame> = (0..frame_count)
            .map(|i| {
                let keyframe = i == 0;
                video_frame(
                    i as i64 * 40_000,
                    i as i64 * 40_000,
                    keyframe,
                    annexb_au(if keyframe { 5 } else { 1 }, payload_len),
                )
            })
            .collect();

        let stream = mux_stream(&frames);

        let mut demuxer = MpegTsDemuxer::default();
        let mut events = demuxer.push(&stream);
        events.extend(demuxer.flush());

        let tracks_found = events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
            .count();
        let frames_out = events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();

        prop_assert!(tracks_found >= 1);
        prop_assert!(frames_out >= 1);
    }

    /// Arbitrary chunk splitting of the demuxer input recovers the same frame
    /// count as a single push (the driver may deliver bytes in any framing).
    #[test]
    fn prop_demux_chunk_split_invariant(
        split_points in proptest::collection::vec(1usize..200, 1..12),
    ) {
        let frames = vec![
            video_frame(0, 0, true, annexb_au(5, 64)),
            video_frame(40_000, 40_000, false, annexb_au(1, 48)),
        ];
        let stream = mux_stream(&frames);

        let mut single = MpegTsDemuxer::default();
        let mut single_events = single.push(&stream);
        single_events.extend(single.flush());
        let single_frames = single_events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();

        let mut chunked = MpegTsDemuxer::default();
        let mut chunked_frames = 0usize;
        let mut offset = 0usize;
        for &sp in &split_points {
            if offset >= stream.len() {
                break;
            }
            let end = (offset + sp).min(stream.len());
            chunked_frames += chunked
                .push(&stream[offset..end])
                .iter()
                .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
                .count();
            offset = end;
        }
        if offset < stream.len() {
            chunked_frames += chunked
                .push(&stream[offset..])
                .iter()
                .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
                .count();
        }
        chunked_frames += chunked
            .flush()
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();

        prop_assert_eq!(single_frames, chunked_frames);
    }

    /// A well-formed `/{ns}/{stream}.ts` (and `.live.ts`) target parses back to
    /// the original namespace and stream path.
    #[test]
    fn prop_request_target_roundtrip(
        namespace in "[a-zA-Z0-9_-]{1,20}",
        stream in "[a-zA-Z0-9_-]{1,20}",
        live in any::<bool>(),
    ) {
        let suffix = if live { ".live.ts" } else { ".ts" };
        let target = format!("/{namespace}/{stream}{suffix}");
        let parsed = parse_ts_request_target(&target).expect("valid target must parse");
        prop_assert_eq!(parsed.stream_key.namespace, namespace);
        prop_assert_eq!(parsed.stream_key.stream_path, stream);
    }

    /// A trailing query string never changes the parsed stream key.
    #[test]
    fn prop_request_target_ignores_query(
        namespace in "[a-zA-Z0-9_-]{1,20}",
        stream in "[a-zA-Z0-9_-]{1,20}",
        query in "[a-zA-Z0-9_=&-]{0,20}",
    ) {
        let plain = parse_ts_request_target(&format!("/{namespace}/{stream}.ts"))
            .expect("plain target parses");
        let with_query = parse_ts_request_target(&format!("/{namespace}/{stream}.ts?{query}"))
            .expect("query target parses");
        prop_assert_eq!(plain.stream_key, with_query.stream_key);
    }

    /// Path traversal / percent-escapes are always rejected as InvalidPath.
    #[test]
    fn prop_request_target_rejects_traversal(
        prefix in "[a-zA-Z0-9/_-]{0,10}",
        suffix in "[a-zA-Z0-9/_-]{0,10}",
        marker in prop_oneof![Just(".."), Just("%2e"), Just("%2F")],
    ) {
        let target = format!("/{prefix}{marker}{suffix}.ts");
        prop_assert!(matches!(
            parse_ts_request_target(&target),
            Err(TsCoreError::InvalidPath)
        ));
    }

    /// The WebSocket accept key is deterministic and a fixed-length base64 SHA-1
    /// digest (28 chars ending with `=`) for any non-empty client key.
    #[test]
    fn prop_websocket_accept_key_deterministic(
        key in "[A-Za-z0-9+/]{1,40}",
    ) {
        let a = websocket_accept_key(&key).expect("non-empty key accepts");
        let b = websocket_accept_key(&key).expect("non-empty key accepts");
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(a.len(), 28);
        prop_assert!(a.ends_with('='));
    }
}
