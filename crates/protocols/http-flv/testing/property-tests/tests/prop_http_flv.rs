//! Property-based tests for the HTTP-FLV protocol.
//!
//! Two surfaces are exercised:
//! * `cheetah-codec` FLV egress mapping (the shared FLV container the protocol
//!   plays out) — tag-header framing invariants and bootstrap ordering.
//! * `cheetah-http-flv-core` play-request / WebSocket-accept parsing.
//!
//! HTTP-FLV 协议属性测试。
//!
//! 两个表面被测试：
//! * `cheetah-codec` 的 FLV 出口映射（协议实际输出的共享 FLV 容器）——标签头
//!   成帧不变量与引导顺序。
//! * `cheetah-http-flv-core` 的播放请求 / WebSocket 接受键解析。

use bytes::Bytes;
use cheetah_codec::{
    build_track_bootstrap_payloads, map_frame_to_rtmp_flv_payload, AVFrame, CodecId, FrameFlags,
    FrameFormat, MediaKind, RtmpFlvPayloadKind, RtmpFlvPlayMode, Timebase, TrackId, TrackInfo,
};
use cheetah_http_flv_core::error::HttpFlvCoreError;
use cheetah_http_flv_core::{parse_play_request_target, websocket_accept_key, HttpFlvQueryMode};
use proptest::prelude::*;

/// Build an H.264 `AVFrame` with a leading Annex-B start code for FLV mapping tests.
///
/// 构造一个带 Annex-B 起始码的 H.264 `AVFrame`，用于 FLV 映射测试。
fn h264_frame(dts_us: i64, keyframe: bool) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        dts_us,
        dts_us,
        Timebase::new(1, 1_000_000),
        Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB, 0xCC]),
    );
    if keyframe {
        frame.flags = FrameFlags::KEY;
    }
    frame
}

/// Build a single H.264 video track fixture.
///
/// 构造单 H.264 视频轨道 fixture。
fn h264_track() -> TrackInfo {
    TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000)
}

/// Build a single AAC audio track fixture.
///
/// 构造单 AAC 音频轨道 fixture。
fn aac_track() -> TrackInfo {
    let mut t = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44_100);
    t.sample_rate = Some(44_100);
    t.channels = Some(2);
    t
}

proptest! {
    /// An H.264 coded frame maps to a Video FLV payload whose first byte encodes
    /// the key/inter frame type (0x17 vs 0x27 in legacy/Normal mode) followed by
    /// the AVC NALU packet-type byte 0x01.
    ///
    /// H.264 编码帧映射为 Video FLV 负载，首字节编码关键/非关键帧类型
    /// （传统/Normal 模式下 0x17 与 0x27），随后是 AVC NALU 包类型字节 0x01。
    #[test]
    fn prop_h264_frame_tag_header(
        dts_ms in 0i64..100_000,
        keyframe in any::<bool>(),
    ) {
        let frame = h264_frame(dts_ms * 1_000, keyframe);
        let payload = map_frame_to_rtmp_flv_payload(&frame, RtmpFlvPlayMode::Normal, &[])
            .expect("h264 frame must map");
        prop_assert_eq!(payload.kind, RtmpFlvPayloadKind::Video);
        let expected_first = if keyframe { 0x17 } else { 0x27 };
        prop_assert_eq!(payload.payload[0], expected_first);
        prop_assert_eq!(payload.payload[1], 0x01);
    }

    /// The mapped FLV timestamp equals the frame DTS truncated to milliseconds.
    ///
    /// 映射后的 FLV 时间戳等于帧 DTS 截断到毫秒。
    #[test]
    fn prop_h264_timestamp_matches_dts_ms(
        dts_ms in 0i64..1_000_000,
    ) {
        let frame = h264_frame(dts_ms * 1_000, true);
        let payload = map_frame_to_rtmp_flv_payload(&frame, RtmpFlvPlayMode::Normal, &[])
            .expect("h264 frame must map");
        prop_assert_eq!(payload.timestamp_ms as i64, dts_ms);
    }

    /// When play metadata is requested, the bootstrap always leads with a single
    /// `onMetaData` script-data (Data) payload at timestamp 0.
    ///
    /// 当请求播放元数据时，引导序列始终以时间戳 0 的单个 `onMetaData` 脚本数据
    /// （Data）负载开头。
    #[test]
    fn prop_bootstrap_metadata_leads(
        with_audio in any::<bool>(),
        mode_enhanced in any::<bool>(),
    ) {
        let mut tracks = vec![h264_track()];
        if with_audio {
            tracks.push(aac_track());
        }
        let mode = if mode_enhanced {
            RtmpFlvPlayMode::Enhanced
        } else {
            RtmpFlvPlayMode::Normal
        };

        let payloads = build_track_bootstrap_payloads(&tracks, mode, false, true);
        prop_assert!(!payloads.is_empty());
        prop_assert_eq!(payloads[0].kind, RtmpFlvPayloadKind::Data);
        prop_assert_eq!(payloads[0].timestamp_ms, 0);

        // Exactly one metadata (Data) payload is emitted at the head.
        let data_count = payloads
            .iter()
            .filter(|p| p.kind == RtmpFlvPayloadKind::Data)
            .count();
        prop_assert_eq!(data_count, 1);
    }

    /// Suppressing play metadata never emits a Data payload.
    ///
    /// 抑制播放元数据时不会发出 Data 负载。
    #[test]
    fn prop_bootstrap_without_metadata_has_no_data(
        with_audio in any::<bool>(),
    ) {
        let mut tracks = vec![h264_track()];
        if with_audio {
            tracks.push(aac_track());
        }
        let payloads = build_track_bootstrap_payloads(&tracks, RtmpFlvPlayMode::Normal, false, false);
        prop_assert!(payloads.iter().all(|p| p.kind != RtmpFlvPayloadKind::Data));
    }

    /// A well-formed `/{ns}/{stream}.flv` target parses back to the original
    /// namespace and stream path, and the `type=` query selects the play mode.
    ///
    /// 格式正确的 `/{ns}/{stream}.flv` 目标解析回原始命名空间与流路径，
    /// `type=` 查询参数选择播放模式。
    #[test]
    fn prop_play_request_roundtrip(
        namespace in "[a-zA-Z0-9_-]{1,20}",
        stream in "[a-zA-Z0-9_-]{1,20}",
        mode in prop_oneof![
            Just(("", HttpFlvQueryMode::Normal)),
            Just(("?type=enhanced", HttpFlvQueryMode::Enhanced)),
            Just(("?type=fastPts", HttpFlvQueryMode::FastPts)),
        ],
    ) {
        let (query, expected_mode) = mode;
        let target = format!("/{namespace}/{stream}.flv{query}");
        let parsed = parse_play_request_target(&target).expect("valid target must parse");
        prop_assert_eq!(parsed.stream_key.namespace, namespace);
        prop_assert_eq!(parsed.stream_key.stream_path, stream);
        prop_assert_eq!(parsed.mode, expected_mode);
    }

    /// Targets without the `.flv` suffix are always rejected.
    ///
    /// 没有 `.flv` 后缀的目标始终被拒绝。
    #[test]
    fn prop_play_request_rejects_non_flv(
        namespace in "[a-zA-Z0-9_-]{1,20}",
        stream in "[a-zA-Z0-9_-]{1,20}",
        ext in prop_oneof![Just(""), Just(".ts"), Just(".mp4"), Just(".m3u8")],
    ) {
        let target = format!("/{namespace}/{stream}{ext}");
        let is_invalid_flv = matches!(
            parse_play_request_target(&target),
            Err(HttpFlvCoreError::InvalidFlvPath { .. })
        );
        prop_assert!(is_invalid_flv);
    }

    /// The WebSocket accept key is deterministic and a fixed-length base64 SHA-1
    /// digest (28 chars ending with `=`) for any non-empty client key.
    ///
    /// WebSocket 接受键是确定性的，对任何非空客户端密钥都是固定长度 28 字符、
    /// 以 `=` 结尾的 base64 SHA-1 摘要。
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
