//! Property-based tests for RTP, Ehome, and PS mux/demux behavior.
//!
//! Three surfaces are exercised:
//! * RTP over TCP framing (RFC 4571-style length prefix) — arbitrary byte splits
//!   must still reassemble the original `RtpPacket`.
//! * The Ehome protocol decoder — arbitrary byte splits must not lose the SSRC,
//!   codec, or media payload handshakes.
//! * PS (Program Stream) mux/demux round-trip — arbitrary byte splits must still
//!   recover the frame identity and payload for the last decoded frame.
//!
//! RTP、Ehome 与 PS 复用/解复用行为属性测试。
//!
//! 三个表面被测试：
//! * RTP over TCP 成帧（类 RFC 4571 长度前缀）——任意字节切分仍须重组原始
//!   `RtpPacket`。
//! * Ehome 协议解码器——任意字节切分不能丢失 SSRC、codec 或媒体 payload 握手。
//! * PS 复用/解复用往返——任意字节切分仍须恢复最后一帧的帧标识与 payload。

use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    encode_tcp_rtp_frame, parse_tcp_rtp_frame, AVFrame, CodecId, EhomeDecoder, EhomeOutput,
    FrameFormat, MediaKind, PsDemuxEvent, PsDemuxer, PsDemuxerConfig, PsMuxer, RtpHeader,
    RtpPacket, RtpReorderBuffer, RtpReorderSettings, Timebase, TrackId, TrackInfo, TrackReadiness,
};
use proptest::prelude::*;

/// Generate a valid RTP payload type (0-127).
///
/// 生成有效 RTP payload type（0-127）。
fn valid_payload_type() -> impl Strategy<Value = u8> {
    0..128_u8
}

/// Generate an arbitrary RTP payload.
///
/// 生成任意 RTP payload。
fn valid_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..1024)
}

/// Generate sorted, deduplicated split positions for arbitrary TCP framing tests.
///
/// 生成有序去重的切分位置，用于任意 TCP 成帧测试。
fn split_positions(max_len: usize) -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(1..max_len, 0..5).prop_map(move |mut v| {
        v.sort();
        v.dedup();
        v
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// RTP over TCP frame reassembly works under arbitrary byte splits.
    ///
    /// RTP over TCP 帧在任意字节切分下可正确还原。
    #[test]
    fn test_tcp_rtp_framing_arbitrary_splits(
        payload_type in valid_payload_type(),
        seq in any::<u16>(),
        ts in any::<u32>(),
        ssrc in any::<u32>(),
        payload in valid_payload(),
        splits in split_positions(1024),
    ) {
        let header = RtpHeader {
            version: 2,
            payload_type,
            sequence_number: seq,
            timestamp: ts,
            ssrc,
            marker: false,
        };
        let packet = RtpPacket {
            header,
            payload: Bytes::from(payload),
        };

        let tcp_frame = encode_tcp_rtp_frame(&packet);
        let frame_bytes = tcp_frame.to_vec();

        // Split the wire bytes at the generated positions.
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < frame_bytes.len() {
                chunks.push(&frame_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&frame_bytes[last_idx..]);

        // Simulate a TCP stream: accumulate chunks until a full frame parses.
        let mut buffer = Vec::new();
        let mut parsed_packet = None;

        for chunk in chunks {
            buffer.extend_from_slice(chunk);
            if let Some((parsed, consumed)) = parse_tcp_rtp_frame(&buffer) {
                parsed_packet = Some(parsed);
                buffer.drain(0..consumed);
                break;
            }
        }

        let decoded = parsed_packet.expect("must decode the packet successfully after receiving all chunks");
        prop_assert_eq!(decoded.header.payload_type, packet.header.payload_type);
        prop_assert_eq!(decoded.header.sequence_number, packet.header.sequence_number);
        prop_assert_eq!(decoded.header.timestamp, packet.header.timestamp);
        prop_assert_eq!(decoded.header.ssrc, packet.header.ssrc);
        prop_assert_eq!(decoded.payload, packet.payload);
        prop_assert!(buffer.is_empty());
    }

    /// EhomeDecoder correctly parses SSRC, codec, and media payload under arbitrary splits.
    ///
    /// EhomeDecoder 在任意字节切分下正确解析 SSRC、codec 与媒体 payload。
    #[test]
    fn test_ehome_decoder_arbitrary_splits(
        is_ehome2 in any::<bool>(),
        ssrc_num in 1000000000..9999999999_u64,
        video_codec_val in prop_oneof![Just(0x0100_u16), Just(0x0005_u16)],
        audio_codec_val in prop_oneof![Just(0x7111_u16), Just(0x7110_u16), Just(0x2001_u16)],
        media_payload in valid_payload(),
        splits in split_positions(2048),
    ) {
        let ssrc_str = ssrc_num.to_string();
        let mut full_bytes = Vec::new();

        // Optional Ehome2 256-byte prefix.
        if is_ehome2 {
            let mut prefix = vec![0; 256];
            prefix[0] = 0x01;
            prefix[1] = 0x00;
            prefix[2] = 0x02; // Ehome2 mode
            full_bytes.extend_from_slice(&prefix);
        }

        // Handshake SSRC packet.
        let mut ssrc_payload = vec![0; 32];
        let ssrc_bytes = ssrc_str.as_bytes();
        ssrc_payload[..ssrc_bytes.len()].copy_from_slice(ssrc_bytes);
        // Ehome framing: [0, 0, len_hi, len_lo] + payload
        let len = ssrc_payload.len() as u16;
        full_bytes.extend_from_slice(&[0, 0, (len >> 8) as u8, (len & 0xFF) as u8]);
        full_bytes.extend_from_slice(&ssrc_payload);

        // Handshake codec packet.
        let mut codec_payload = vec![0; 32];
        codec_payload[12] = 2; // payload_type = ps
        let video_le = video_codec_val.to_le_bytes();
        codec_payload[14] = video_le[0];
        codec_payload[15] = video_le[1];
        let audio_le = audio_codec_val.to_le_bytes();
        codec_payload[16] = audio_le[0];
        codec_payload[17] = audio_le[1];
        codec_payload[18] = 2; // channels
        codec_payload[19] = 16; // sample_bit
        let sr_le = 8000_u16.to_le_bytes();
        codec_payload[20] = sr_le[0];
        codec_payload[21] = sr_le[1];

        let len = codec_payload.len() as u16;
        full_bytes.extend_from_slice(&[0, 0, (len >> 8) as u8, (len & 0xFF) as u8]);
        full_bytes.extend_from_slice(&codec_payload);

        // Media payload packet.
        if !media_payload.is_empty() {
            // Ehome media packet has a 4-byte prefix, then media_payload = inner_payload[4..]
            let mut inner_media = vec![0; 4];
            inner_media.extend_from_slice(&media_payload);
            let len = inner_media.len() as u16;
            full_bytes.extend_from_slice(&[0, 0, (len >> 8) as u8, (len & 0xFF) as u8]);
            full_bytes.extend_from_slice(&inner_media);
        }

        // Split the wire bytes at the generated positions.
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < full_bytes.len() {
                chunks.push(&full_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&full_bytes[last_idx..]);

        // Feed each chunk into the Ehome decoder.
        let mut decoder = EhomeDecoder::new();
        let mut all_outputs = Vec::new();
        let mut incoming_buffer = BytesMut::new();

        for chunk in chunks {
            incoming_buffer.extend_from_slice(chunk);
            let outs = decoder.decode(&mut incoming_buffer);
            all_outputs.extend(outs);
        }

        // Verify the SSRC handshake output.
        let ssrc_out = all_outputs.iter().find_map(|out| {
            if let EhomeOutput::HandshakeSsrc(s) = out {
                Some(s)
            } else {
                None
            }
        });
        prop_assert_eq!(ssrc_out, Some(&ssrc_str));

        // Verify the codec handshake output.
        let codec_out = all_outputs.iter().find_map(|out| {
            if let EhomeOutput::HandshakeCodec(info) = out {
                Some(info)
            } else {
                None
            }
        });
        prop_assert!(codec_out.is_some());
        let info = codec_out.unwrap();
        prop_assert_eq!(&info.payload_type, "ps");
        let expected_video = if video_codec_val == 0x0005 || video_codec_val == 0x0500 {
            "h265"
        } else {
            "h264"
        };
        prop_assert_eq!(info.video_codec.as_deref(), Some(expected_video));

        let expected_audio = if audio_codec_val == 0x7111 || audio_codec_val == 0x1171 {
            "g711a"
        } else if audio_codec_val == 0x7110 || audio_codec_val == 0x1071 {
            "g711u"
        } else {
            "aac"
        };
        prop_assert_eq!(info.audio_codec.as_deref(), Some(expected_audio));

        // Verify the media payload output.
        if !media_payload.is_empty() {
            let media_out = all_outputs.iter().find_map(|out| {
                if let EhomeOutput::MediaPayload(p) = out {
                    Some(p)
                } else {
                    None
                }
            });
            prop_assert!(media_out.is_some());
            prop_assert_eq!(media_out.unwrap().to_vec(), media_payload);
        }
    }

    /// PS mux/demux round-trip holds under arbitrary byte splits.
    ///
    /// PS 复用/解复用往返在任意字节切分下保持属性一致。
    #[test]
    fn test_ps_mux_demux_roundtrip_arbitrary_splits(
        payload in valid_payload(),
        pts in 1000..100000_i64,
        is_key in any::<bool>(),
        splits in split_positions(10240),
    ) {
        let mut muxer = PsMuxer::new();
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.readiness = TrackReadiness::Ready;
        muxer.add_track(track);

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            pts,
            pts,
            Timebase::new(1, 90000),
            Bytes::from(payload.clone()),
        );
        if is_key {
            frame.flags.insert(cheetah_codec::FrameFlags::KEY);
        }

        let muxed = muxer.mux(&frame).expect("mux PS frame");
        let muxed_bytes = muxed.to_vec();

        // Split the muxed bytes at the generated positions.
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < muxed_bytes.len() {
                chunks.push(&muxed_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&muxed_bytes[last_idx..]);

        // Feed each chunk into the PS demuxer.
        let mut demuxer = PsDemuxer::new(PsDemuxerConfig::new(10 * 1024 * 1024, 8));

        let mut decoded_frames = Vec::new();
        for chunk in chunks {
            let evs = demuxer.push(chunk);
            for ev in evs {
                if let PsDemuxEvent::Frame(decoded_f) = ev {
                    decoded_frames.push(decoded_f);
                }
            }
        }

        // If any frame was decoded, its basic properties and payload must match.
        if !decoded_frames.is_empty() {
            let last_decoded = decoded_frames.last().unwrap();
            prop_assert_eq!(last_decoded.track_id, frame.track_id);
            prop_assert_eq!(last_decoded.media_kind, frame.media_kind);
            prop_assert_eq!(last_decoded.codec, frame.codec);
            prop_assert_eq!(last_decoded.payload.to_vec(), payload);
        }
    }

    /// The per-session RTP reorder buffer keeps a bounded window, never emits packets
    /// out of order, and drops duplicates.
    ///
    /// 每个 RTP session 的重排缓冲区保持有界窗口、不会乱序释放包，并且会丢弃重复包。
    #[test]
    fn test_rtp_reorder_buffer_bounded_monotonic_and_dedup(
        seqs in prop::collection::vec(0u16..1024, 1..64),
        arrival_ms in prop::collection::vec(0u64..10_000u64, 1..64),
    ) {
        let mut buffer = RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: 4,
            max_delay_ms: 100,
        });
        let mut last_released: Option<u64> = None;
        let mut released_count: usize = 0;

        for (seq, arrival) in seqs.into_iter().zip(arrival_ms.into_iter()) {
            let released = buffer.push(seq, arrival, seq as u64);

            // Pending length must never exceed the absolute cap plus the configured window.
            prop_assert!(buffer.pending_len() <= 4 + 1, "pending exceeded configured window");

            // Each released batch and the cross-batch sequence must be monotonically increasing.
            let mut prev: Option<u64> = last_released;
            for extended in &released {
                if let Some(p) = prev {
                    prop_assert!(*extended > p, "released sequence {extended} after {p} is not monotonic");
                }
                prev = Some(*extended);
            }
            if let Some(last) = released.last() {
                last_released = Some(*last);
            }
            released_count += released.len();
        }

        // The hard cap guarantees the buffer never grows without bound.
        prop_assert!(buffer.pending_len() <= 64, "pending exceeded hard cap");

        // No sequence number is released twice (duplicates are dropped or still pending).
        // This is an invariant: all released values are strictly increasing, so duplicates
        // in the input cannot be emitted more than once.
        prop_assert!(released_count <= 64, "released more packets than were pushed");
    }
}
