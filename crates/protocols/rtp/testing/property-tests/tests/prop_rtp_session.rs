use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    encode_tcp_rtp_frame, parse_tcp_rtp_frame, AVFrame, CodecId, EhomeDecoder, EhomeOutput,
    FrameFormat, MediaKind, PsDemuxEvent, PsDemuxer, PsDemuxerConfig, PsMuxer, RtpHeader,
    RtpPacket, Timebase, TrackId, TrackInfo, TrackReadiness,
};
use proptest::prelude::*;

/// 生成有效 RTP payload type（0-127）。
fn valid_payload_type() -> impl Strategy<Value = u8> {
    0..128_u8
}

/// 生成 RTP payload。
fn valid_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..1024)
}

/// 生成有效切分位置。
fn split_positions(max_len: usize) -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(1..max_len, 0..5).prop_map(move |mut v| {
        v.sort();
        v.dedup();
        v
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 测试 RTP over TCP frame 在任意字节切分下的接收与还原
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

        // 根据随机切分点切成 chunks
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < frame_bytes.len() {
                chunks.push(&frame_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&frame_bytes[last_idx..]);

        // 模拟 TCP Stream 拼包与解析
        let mut buffer = Vec::new();
        let mut parsed_packet = None;

        for chunk in chunks {
            buffer.extend_from_slice(chunk);
            if let Some((parsed, consumed)) = parse_tcp_rtp_frame(&buffer) {
                parsed_packet = Some(parsed);
                buffer.drain(0..consumed);
                break; // 只要还原出这一个完整的包即可
            }
        }

        let decoded = parsed_packet.expect("Must decode the packet successfully after receiving all chunks");
        prop_assert_eq!(decoded.header.payload_type, packet.header.payload_type);
        prop_assert_eq!(decoded.header.sequence_number, packet.header.sequence_number);
        prop_assert_eq!(decoded.header.timestamp, packet.header.timestamp);
        prop_assert_eq!(decoded.header.ssrc, packet.header.ssrc);
        prop_assert_eq!(decoded.payload, packet.payload);
        prop_assert!(buffer.is_empty());
    }

    /// 测试 EhomeDecoder 在任意字节切分下握手与媒体解析的正确性
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

        // 1. 首包握手，如有必要加入 Ehome2 的 256 字节前缀
        if is_ehome2 {
            let mut prefix = vec![0; 256];
            prefix[0] = 0x01;
            prefix[1] = 0x00;
            prefix[2] = 0x02; // Ehome2 mode
            full_bytes.extend_from_slice(&prefix);
        }

        // 2. Handshake SSRC 包
        let mut ssrc_payload = vec![0; 32];
        let ssrc_bytes = ssrc_str.as_bytes();
        ssrc_payload[..ssrc_bytes.len()].copy_from_slice(ssrc_bytes);
        // Ehome 封包：[0, 0, len_hi, len_lo] + payload
        let len = ssrc_payload.len() as u16;
        full_bytes.extend_from_slice(&[0, 0, (len >> 8) as u8, (len & 0xFF) as u8]);
        full_bytes.extend_from_slice(&ssrc_payload);

        // 3. Handshake Codec 包
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

        // 4. Media Payload 包
        if !media_payload.is_empty() {
            // Ehome 媒体包结构包含 4 字节前缀头，然后 inner_payload.len() > 4，
            // 且 media_payload = inner_payload[4..]
            let mut inner_media = vec![0; 4];
            inner_media.extend_from_slice(&media_payload);
            let len = inner_media.len() as u16;
            full_bytes.extend_from_slice(&[0, 0, (len >> 8) as u8, (len & 0xFF) as u8]);
            full_bytes.extend_from_slice(&inner_media);
        }

        // 按照 splits 随机切片
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < full_bytes.len() {
                chunks.push(&full_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&full_bytes[last_idx..]);

        // 逐个喂入 EhomeDecoder
        let mut decoder = EhomeDecoder::new();
        let mut all_outputs = Vec::new();
        let mut incoming_buffer = BytesMut::new();

        for chunk in chunks {
            incoming_buffer.extend_from_slice(chunk);
            let outs = decoder.decode(&mut incoming_buffer);
            all_outputs.extend(outs);
        }

        // 检验解析到的 Ssrc
        let ssrc_out = all_outputs.iter().find_map(|out| {
            if let EhomeOutput::HandshakeSsrc(s) = out {
                Some(s)
            } else {
                None
            }
        });
        prop_assert_eq!(ssrc_out, Some(&ssrc_str));

        // 检验解析到的 Codec
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

        // 检验 Media Payload
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

    /// 测试 PS Mux/Demux 在任意字节切片下的拼包与属性一致性
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

        let muxed = muxer.mux(&frame).expect("Mux PS frame");
        let muxed_bytes = muxed.to_vec();

        // 随机切片
        let mut chunks = Vec::new();
        let mut last_idx = 0;
        for &idx in &splits {
            if idx > last_idx && idx < muxed_bytes.len() {
                chunks.push(&muxed_bytes[last_idx..idx]);
                last_idx = idx;
            }
        }
        chunks.push(&muxed_bytes[last_idx..]);

        // 依次喂给 PsDemuxer
        let mut demuxer = PsDemuxer::new(PsDemuxerConfig {
            max_reassembly_bytes: 10 * 1024 * 1024,
            max_tracks: 8,
        });

        let mut decoded_frames = Vec::new();
        for chunk in chunks {
            let evs = demuxer.push(chunk);
            for ev in evs {
                if let PsDemuxEvent::Frame(decoded_f) = ev {
                    decoded_frames.push(decoded_f);
                }
            }
        }

        // 只要有一帧解码出来，我们就验证其基本属性与 payload
        if !decoded_frames.is_empty() {
            let last_decoded = decoded_frames.last().unwrap();
            prop_assert_eq!(last_decoded.track_id, frame.track_id);
            prop_assert_eq!(last_decoded.media_kind, frame.media_kind);
            prop_assert_eq!(last_decoded.codec, frame.codec);
            prop_assert_eq!(last_decoded.payload.to_vec(), payload);
        }
    }
}
