// 来源: vendor-ref/rtsp-rs/pbt/tests/pbt_rtp.rs

use cheetah_rtsp_core::{RtpError, RtpExtension, RtpHeader, RtpPacket};
use proptest::prelude::*;

/// 生成有效 RTP payload type（0-127）。
fn valid_payload_type() -> impl Strategy<Value = u8> {
    0..128_u8
}

/// 生成 RTP payload。
fn valid_payload() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..1024)
}

/// 生成有效 CSRC 列表（0-15 个元素）。
fn valid_csrc_list() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(any::<u32>(), 0..15)
}

/// 生成 RTP 扩展数据（按 4 字节边界补齐）。
fn valid_extension_data() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..64).prop_map(|mut data| {
        while data.len() % 4 != 0 {
            data.push(0);
        }
        data
    })
}

/// 生成可选 RTP 扩展头。
fn valid_extension() -> impl Strategy<Value = Option<RtpExtension>> {
    prop_oneof![
        Just(None),
        (any::<u16>(), valid_extension_data())
            .prop_map(|(profile, data)| Some(RtpExtension { profile, data })),
    ]
}

/// 生成 RTP padding 大小（0 表示无 padding）。
fn valid_padding_size() -> impl Strategy<Value = u8> {
    prop_oneof![Just(0_u8), 1..32_u8]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// RTP 包 build/parse roundtrip（基础场景）。
    #[test]
    fn test_rtp_packet_roundtrip_basic(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        payload in valid_payload(),
    ) {
        let header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        let packet = RtpPacket::new(header, payload.clone());

        let encoded = packet.build().expect("build rtp packet");
        let decoded = RtpPacket::parse(&encoded).expect("parse rtp packet");

        prop_assert_eq!(decoded.header.version, 2);
        prop_assert_eq!(decoded.header.payload_type, payload_type);
        prop_assert_eq!(decoded.header.sequence_number, sequence_number);
        prop_assert_eq!(decoded.header.timestamp, timestamp);
        prop_assert_eq!(decoded.header.ssrc, ssrc);
        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包 build/parse roundtrip（带 marker 标志）。
    #[test]
    fn test_rtp_packet_roundtrip_with_marker(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        marker in any::<bool>(),
        payload in valid_payload(),
    ) {
        let mut header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        header.marker = marker;
        let packet = RtpPacket::new(header, payload.clone());

        let encoded = packet.build().expect("build rtp packet with marker");
        let decoded = RtpPacket::parse(&encoded).expect("parse rtp packet with marker");

        prop_assert_eq!(decoded.header.version, 2);
        prop_assert_eq!(decoded.header.payload_type, payload_type);
        prop_assert_eq!(decoded.header.sequence_number, sequence_number);
        prop_assert_eq!(decoded.header.timestamp, timestamp);
        prop_assert_eq!(decoded.header.ssrc, ssrc);
        prop_assert_eq!(decoded.header.marker, marker);
        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包 build/parse roundtrip（带 CSRC 列表）。
    #[test]
    fn test_rtp_packet_roundtrip_with_csrc(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        csrc in valid_csrc_list(),
        payload in valid_payload(),
    ) {
        let mut header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        header.csrc = csrc.clone();
        let packet = RtpPacket::new(header, payload.clone());

        let encoded = packet.build().expect("build rtp packet with csrc");
        let decoded = RtpPacket::parse(&encoded).expect("parse rtp packet with csrc");

        prop_assert_eq!(decoded.header.version, 2);
        prop_assert_eq!(decoded.header.payload_type, payload_type);
        prop_assert_eq!(decoded.header.sequence_number, sequence_number);
        prop_assert_eq!(decoded.header.timestamp, timestamp);
        prop_assert_eq!(decoded.header.ssrc, ssrc);
        prop_assert_eq!(decoded.header.csrc, csrc);
        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包 build/parse roundtrip（带 extension 扩展头）。
    #[test]
    fn test_rtp_packet_roundtrip_with_extension(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        extension in valid_extension(),
        payload in valid_payload(),
    ) {
        let header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        let mut packet = RtpPacket::new(header, payload.clone());
        packet.extension = extension.clone();

        let encoded = packet.build().expect("build rtp packet with extension");
        let decoded = RtpPacket::parse(&encoded).expect("parse rtp packet with extension");

        match (&decoded.extension, &extension) {
            (Some(decoded_ext), Some(original_ext)) => {
                prop_assert_eq!(decoded_ext.profile, original_ext.profile);
                prop_assert_eq!(decoded_ext.data.len(), original_ext.data.len());
                prop_assert_eq!(&decoded_ext.data[..original_ext.data.len()], &original_ext.data);
            }
            (None, None) => {}
            _ => prop_assert!(false, "extension roundtrip mismatch"),
        }

        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包 build/parse roundtrip（带 padding）。
    #[test]
    fn test_rtp_packet_roundtrip_with_padding(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        padding_size in valid_padding_size(),
        payload in valid_payload(),
    ) {
        let header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        let mut packet = RtpPacket::new(header, payload.clone());
        packet.padding_size = padding_size;

        let encoded = packet.build().expect("build rtp packet with padding");
        let decoded = RtpPacket::parse(&encoded).expect("parse rtp packet with padding");

        prop_assert_eq!(decoded.padding_size, padding_size);
        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包 build/parse roundtrip（全选项组合）。
    #[test]
    fn test_rtp_packet_roundtrip_full(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        marker in any::<bool>(),
        csrc in valid_csrc_list(),
        extension in valid_extension(),
        padding_size in valid_padding_size(),
        payload in valid_payload(),
    ) {
        let mut header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        header.marker = marker;
        header.csrc = csrc.clone();

        let mut packet = RtpPacket::new(header, payload.clone());
        packet.extension = extension;
        packet.padding_size = padding_size;

        let encoded = packet.build().expect("build full-options rtp packet");
        let decoded = RtpPacket::parse(&encoded).expect("parse full-options rtp packet");

        prop_assert_eq!(decoded.header.version, 2);
        prop_assert_eq!(decoded.header.payload_type, payload_type);
        prop_assert_eq!(decoded.header.sequence_number, sequence_number);
        prop_assert_eq!(decoded.header.timestamp, timestamp);
        prop_assert_eq!(decoded.header.ssrc, ssrc);
        prop_assert_eq!(decoded.header.marker, marker);
        prop_assert_eq!(decoded.header.csrc, csrc);
        prop_assert_eq!(decoded.padding_size, padding_size);
        prop_assert_eq!(decoded.payload, payload);
    }

    /// RTP 包长度计算应与实际编码结果一致。
    #[test]
    fn test_rtp_packet_size(
        payload_type in valid_payload_type(),
        sequence_number in any::<u16>(),
        timestamp in any::<u32>(),
        ssrc in any::<u32>(),
        csrc in valid_csrc_list(),
        payload in valid_payload(),
    ) {
        let mut header = RtpHeader::new(payload_type, sequence_number, timestamp, ssrc);
        header.csrc = csrc.clone();
        let packet = RtpPacket::new(header, payload.clone());

        let encoded = packet.build().expect("build rtp packet for size check");
        let expected_size = 12 + csrc.len() * 4 + payload.len();

        prop_assert_eq!(encoded.len(), expected_size);
        prop_assert_eq!(packet.size(), expected_size);
    }
}

/// 无效 RTP 输入应返回显式错误，不得误解析成功。
#[test]
fn test_rtp_parse_invalid_data() {
    let short_empty = RtpPacket::parse(&[]).expect_err("empty packet must fail");
    assert!(matches!(
        short_empty,
        RtpError::InsufficientData {
            context: "rtp fixed header",
            needed: 12,
            actual: 0
        }
    ));

    let short_header = RtpPacket::parse(&[0; 11]).expect_err("short fixed header must fail");
    assert!(matches!(
        short_header,
        RtpError::InsufficientData {
            context: "rtp fixed header",
            needed: 12,
            actual: 11
        }
    ));

    for raw_first_byte in [0b0000_0000, 0b0100_0000, 0b1100_0000] {
        let mut invalid_version = vec![0_u8; 12];
        invalid_version[0] = raw_first_byte;
        let err =
            RtpPacket::parse(&invalid_version).expect_err("unsupported rtp version must fail");
        let version = raw_first_byte >> 6;
        assert!(matches!(err, RtpError::UnsupportedVersion { actual } if actual == version));
    }
}
