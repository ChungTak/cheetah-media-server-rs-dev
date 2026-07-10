//! Property-based tests for transport fault-view generators and related helpers.
//!
//! These tests verify `RtspTransport` parse/display round-trip over multiple
//! candidates, HTTP-tunnel base64 split reassembly, and RTP sequence reordering
//! over the UDP fault-view generator.
//!
//! 传输层故障视图生成器与相关 helper 的属性测试。
//!
//! 这些测试验证多候选 `RtspTransport` 解析/显示往返、HTTP 隧道 base64 分片
//! 重组以及基于 UDP 故障视图生成器的 RTP 序列重排。

#[allow(dead_code)]
#[path = "support/rtsp_capture_fixture.rs"]
mod rtsp_capture_fixture;

use cheetah_rtsp_core::RtspTransport;
use proptest::prelude::*;
use rtsp_capture_fixture::{
    build_transport_fault_views, build_udp_rtp_fault_views, CaptureRecord, CaptureRecordKind,
};

/// Generate a valid transport protocol string.
///
/// 生成有效 transport 协议字符串。
fn valid_transport_protocol() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("RTP/AVP".to_string()),
        Just("RTP/AVP/TCP".to_string()),
        Just("RTP/SAVP".to_string()),
    ]
}

/// Generate a valid `RtspTransport` structure.
///
/// 生成有效 `RtspTransport` 结构。
fn valid_transport() -> impl Strategy<Value = RtspTransport> {
    (
        valid_transport_protocol(),
        any::<bool>(),
        prop::option::of((0..128_u8, 0..128_u8)),
        prop::option::of((1024..65000_u16, 1024..65000_u16)),
        prop::option::of((1024..65000_u16, 1024..65000_u16)),
        prop::option::of(any::<u32>()),
        prop::option::of(prop_oneof![
            Just("PLAY".to_string()),
            Just("RECORD".to_string()),
        ]),
        prop::option::of(
            prop::string::string_regex("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}")
                .expect("valid destination regex"),
        ),
        prop::option::of(
            prop::string::string_regex("[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}")
                .expect("valid source regex"),
        ),
        prop::option::of(1..255_u8),
    )
        .prop_map(
            |(
                protocol,
                unicast,
                interleaved,
                client_port,
                server_port,
                ssrc,
                mode,
                destination,
                source,
                ttl,
            )| RtspTransport {
                protocol,
                unicast,
                interleaved,
                client_port,
                server_port,
                ssrc,
                mode,
                destination,
                source,
                ttl,
                ..RtspTransport::default()
            },
        )
}

/// Find a fault view by name and panic if it is missing.
///
/// 按名称查找故障视图，缺失时 panic。
fn find_view<'a>(views: &'a [rtsp_capture_fixture::NamedPayloadView], name: &str) -> &'a [Vec<u8>] {
    views
        .iter()
        .find(|view| view.name == name)
        .map(|view| view.payloads.as_slice())
        .unwrap_or_else(|| panic!("missing view {name}"))
}

/// Decode a standard base64 byte stream (no line breaks).
///
/// 解码标准 base64 字节流（无换行）。
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::new();
    let mut chunk = [0u8; 4];
    let mut used = 0usize;

    for &byte in input {
        if byte == b'\r' || byte == b'\n' {
            continue;
        }
        chunk[used] = byte;
        used += 1;
        if used < 4 {
            continue;
        }

        let mut pad = 0usize;
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = if chunk[2] == b'=' {
            pad += 1;
            0
        } else {
            val(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            pad += 1;
            0
        } else {
            val(chunk[3])?
        };

        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | d as u32;
        out.push(((n >> 16) & 0xff) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if pad == 0 {
            out.push((n & 0xff) as u8);
        }

        used = 0;
    }

    if used != 0 {
        return None;
    }
    Some(out)
}

/// Build a minimal 12-byte RTP packet with 3 bytes of payload.
///
/// 构造一个带 3 字节 payload 的最小 12 字节 RTP 包。
fn build_rtp_packet(seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; 12 + 3];
    packet[0] = 0x80;
    packet[1] = 96;
    packet[2..4].copy_from_slice(&seq.to_be_bytes());
    packet[4..8].copy_from_slice(&1u32.to_be_bytes());
    packet[8..12].copy_from_slice(&0x1122_3344u32.to_be_bytes());
    packet[12..].copy_from_slice(&[1, 2, 3]);
    packet
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Parse/display round-trip across a comma-separated list of three transports.
    ///
    /// 三个逗号分隔 transport 的解析/显示往返。
    #[test]
    fn prop_transport_parse_roundtrip_with_candidates(
        t1 in valid_transport(),
        t2 in valid_transport(),
        t3 in valid_transport(),
    ) {
        let header = format!("{}, {}, {}", t1.to_header(), t2.to_header(), t3.to_header());
        let parsed = RtspTransport::parse_multiple(&header).expect("parse multiple transport");
        prop_assert_eq!(parsed.len(), 3);

        let reparsed = parsed
            .iter()
            .map(RtspTransport::to_header)
            .collect::<Vec<_>>()
            .join(",");
        let parsed_again = RtspTransport::parse_multiple(&reparsed).expect("reparse transport");
        prop_assert_eq!(parsed_again.len(), parsed.len());

        for (lhs, rhs) in parsed.iter().zip(parsed_again.iter()) {
            prop_assert_eq!(&lhs.protocol, &rhs.protocol);
            prop_assert_eq!(lhs.unicast, rhs.unicast);
            prop_assert_eq!(lhs.interleaved, rhs.interleaved);
            prop_assert_eq!(lhs.client_port, rhs.client_port);
            prop_assert_eq!(lhs.server_port, rhs.server_port);
            prop_assert_eq!(lhs.ssrc, rhs.ssrc);
            prop_assert_eq!(&lhs.mode, &rhs.mode);
            prop_assert_eq!(&lhs.destination, &rhs.destination);
            prop_assert_eq!(&lhs.source, &rhs.source);
            prop_assert_eq!(lhs.ttl, rhs.ttl);
        }
    }

    /// HTTP tunnel base64 payload split reassembles the original TCP bytes.
    ///
    /// HTTP 隧道 base64 payload 分片后能重组为原始 TCP 字节。
    #[test]
    fn prop_http_tunnel_base64_split_reassembles(
        suffix in prop::string::string_regex("[A-Za-z0-9_/]{4,24}").expect("suffix regex"),
    ) {
        let mut records = Vec::new();
        records.push(CaptureRecord {
            kind: CaptureRecordKind::RtspTcpC2s,
            flags: 0x01,
            flow_id: 1,
            delta_us: 0,
            payload: format!("OPTIONS rtsp://127.0.0.1/live/{suffix} RTSP/1.0\r\nCSeq: 1\r\n\r\n").into_bytes(),
        });
        records.push(CaptureRecord {
            kind: CaptureRecordKind::RtspTcpS2c,
            flags: 0x01,
            flow_id: 1,
            delta_us: 10,
            payload: b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n".to_vec(),
        });

        let views = build_transport_fault_views(&records, 2, 2, 2)
            .expect("transport fault views should build");
        let chunks = find_view(&views, "transport_http_base64_split_1_3");
        let encoded = chunks.concat();
        let decoded = base64_decode(&encoded).expect("base64 decode should succeed");

        let original = records
            .iter()
            .filter(|record| {
                matches!(record.kind, CaptureRecordKind::RtspTcpC2s | CaptureRecordKind::RtspTcpS2c)
            })
            .flat_map(|record| record.payload.clone())
            .collect::<Vec<_>>();

        prop_assert_eq!(decoded, original);

        let invalid = find_view(&views, "transport_http_invalid_base64");
        prop_assert!(!invalid.is_empty());
        prop_assert!(base64_decode(&invalid[0]).is_none());
    }

    /// RTP sequence reordering preserves the original packet multiset.
    ///
    /// RTP 序列重排保持原始包的多重集合。
    #[test]
    fn prop_rtp_reorder_sequence_wrap_preserves_packet_multiset(start in 65534u16..=65535u16) {
        let s0 = start;
        let s1 = start.wrapping_add(1);
        let s2 = start.wrapping_add(2);
        let records = vec![
            CaptureRecord {
                kind: CaptureRecordKind::UdpPublishRtp,
                flags: 0x01,
                flow_id: 1,
                delta_us: 0,
                payload: build_rtp_packet(s0),
            },
            CaptureRecord {
                kind: CaptureRecordKind::UdpPublishRtp,
                flags: 0x01,
                flow_id: 1,
                delta_us: 10,
                payload: build_rtp_packet(s1),
            },
            CaptureRecord {
                kind: CaptureRecordKind::UdpPublishRtp,
                flags: 0x01,
                flow_id: 1,
                delta_us: 20,
                payload: build_rtp_packet(s2),
            },
        ];

        let views = build_udp_rtp_fault_views(&records, 2, 2)
            .expect("udp fault views should build");
        let reordered = find_view(&views, "rtp_sequence_reorder");

        let mut got = reordered
            .iter()
            .map(|payload| u16::from_be_bytes([payload[2], payload[3]]))
            .collect::<Vec<_>>();
        let mut expected = vec![s0, s1, s2];
        got.sort_unstable();
        expected.sort_unstable();
        prop_assert_eq!(got, expected);

        if s0 == 65535 {
            prop_assert!(reordered.iter().any(|payload| payload[2] == 0 && payload[3] == 0));
        }
    }
}
