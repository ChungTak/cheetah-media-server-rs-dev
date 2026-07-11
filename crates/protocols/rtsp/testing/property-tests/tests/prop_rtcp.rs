//! Property-based tests for RTCP packet build/parse round-trips.
//!
//! These tests cover Sender/Receiver Reports, SDES, BYE, APP, compound packets,
//! and report block value preservation. A final unit test verifies safe handling
//! of invalid RTCP data.
//!
//! RTCP 包构造/解析往返属性测试。
//!
//! 测试覆盖 Sender/Receiver Report、SDES、BYE、APP、复合包与报告块字段保持。
//! 最后的单元测试验证对非法 RTCP 数据的安全处理。

use cheetah_rtsp_core::{
    RtcpApp, RtcpBye, RtcpPacket, RtcpReceiverReport, RtcpReportBlock, RtcpSdes, RtcpSdesChunk,
    RtcpSdesItem, RtcpSenderReport,
};
use proptest::prelude::*;

/// Generate a valid CNAME string.
///
/// 生成有效 CNAME 字符串。
fn valid_cname() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9@._-]{1,50}")
        .expect("valid cname regex")
        .prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a valid BYE reason string.
///
/// 生成有效 BYE reason 字符串。
fn valid_reason() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 ._-]{0,50}").expect("valid reason regex")
}

/// Generate an RTCP report block.
///
/// 生成 RTCP 报告块。
fn valid_report_block() -> impl Strategy<Value = RtcpReportBlock> {
    (
        any::<u32>(),      // ssrc
        any::<u8>(),       // fraction_lost
        0..0x00FF_FFFFu32, // cumulative_lost (24 bit)
        any::<u32>(),      // highest_seq
        any::<u32>(),      // jitter
        any::<u32>(),      // last_sr
        any::<u32>(),      // delay_since_sr
    )
        .prop_map(
            |(
                ssrc,
                fraction_lost,
                cumulative_lost,
                highest_seq,
                jitter,
                last_sr,
                delay_since_sr,
            )| RtcpReportBlock {
                ssrc,
                fraction_lost,
                cumulative_lost,
                highest_seq,
                jitter,
                last_sr,
                delay_since_sr,
            },
        )
}

/// Generate a Sender Report.
///
/// 生成 Sender Report。
fn valid_sender_report() -> impl Strategy<Value = RtcpSenderReport> {
    (
        any::<u32>(),                                      // ssrc
        any::<u64>(),                                      // ntp_timestamp
        any::<u32>(),                                      // rtp_timestamp
        any::<u32>(),                                      // packet_count
        any::<u32>(),                                      // octet_count
        prop::collection::vec(valid_report_block(), 0..5), // reports
    )
        .prop_map(
            |(ssrc, ntp_timestamp, rtp_timestamp, packet_count, octet_count, reports)| {
                RtcpSenderReport {
                    ssrc,
                    ntp_timestamp,
                    rtp_timestamp,
                    packet_count,
                    octet_count,
                    reports,
                }
            },
        )
}

/// Generate a Receiver Report.
///
/// 生成 Receiver Report。
fn valid_receiver_report() -> impl Strategy<Value = RtcpReceiverReport> {
    (
        any::<u32>(),                                      // ssrc
        prop::collection::vec(valid_report_block(), 0..5), // reports
    )
        .prop_map(|(ssrc, reports)| RtcpReceiverReport { ssrc, reports })
}

/// Generate an SDES item.
///
/// 生成 SDES Item。
fn valid_sdes_item() -> impl Strategy<Value = RtcpSdesItem> {
    prop_oneof![
        valid_cname().prop_map(RtcpSdesItem::Cname),
        valid_cname().prop_map(RtcpSdesItem::Name),
        valid_cname().prop_map(RtcpSdesItem::Email),
        valid_cname().prop_map(RtcpSdesItem::Tool),
    ]
}

/// Generate an SDES chunk.
///
/// 生成 SDES Chunk。
fn valid_sdes_chunk() -> impl Strategy<Value = RtcpSdesChunk> {
    (any::<u32>(), prop::collection::vec(valid_sdes_item(), 1..4))
        .prop_map(|(ssrc, items)| RtcpSdesChunk { ssrc, items })
}

/// Generate an SDES packet.
///
/// 生成 SDES 包。
fn valid_sdes() -> impl Strategy<Value = RtcpSdes> {
    prop::collection::vec(valid_sdes_chunk(), 1..4).prop_map(|chunks| RtcpSdes { chunks })
}

/// Generate a BYE packet.
///
/// 生成 BYE 包。
fn valid_bye() -> impl Strategy<Value = RtcpBye> {
    (
        prop::collection::vec(any::<u32>(), 1..5),
        prop::option::of(valid_reason()),
    )
        .prop_map(|(ssrcs, reason)| RtcpBye { ssrcs, reason })
}

/// Generate an APP packet.
///
/// 生成 APP 包。
fn valid_app() -> impl Strategy<Value = RtcpApp> {
    (
        0..32u8, // subtype (5 bit)
        any::<u32>(),
        prop::collection::vec(any::<u8>(), 4).prop_map(|v| {
            let mut name = [0_u8; 4];
            name.copy_from_slice(&v);
            name
        }),
        prop::collection::vec(any::<u8>(), 0..64).prop_map(|mut data| {
            // Pad to a 4-byte boundary so the build-side padding does not affect the round-trip.
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
            data
        }),
    )
        .prop_map(|(subtype, ssrc, name, data)| RtcpApp {
            subtype,
            ssrc,
            name,
            data,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Sender Report build/parse round-trip.
    ///
    /// Sender Report 构造/解析往返。
    #[test]
    fn test_rtcp_sender_report_roundtrip(sr in valid_sender_report()) {
        let packets = vec![RtcpPacket::SenderReport(sr.clone())];
        let encoded = RtcpPacket::build(&packets).expect("build sender report packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse sender report packet");

        prop_assert_eq!(decoded.len(), 1);
        if let RtcpPacket::SenderReport(decoded_sr) = &decoded[0] {
            prop_assert_eq!(decoded_sr.ssrc, sr.ssrc);
            prop_assert_eq!(decoded_sr.ntp_timestamp, sr.ntp_timestamp);
            prop_assert_eq!(decoded_sr.rtp_timestamp, sr.rtp_timestamp);
            prop_assert_eq!(decoded_sr.packet_count, sr.packet_count);
            prop_assert_eq!(decoded_sr.octet_count, sr.octet_count);
            prop_assert_eq!(decoded_sr.reports.len(), sr.reports.len());
        } else {
            prop_assert!(false, "expected SenderReport");
        }
    }

    /// Receiver Report build/parse round-trip.
    ///
    /// Receiver Report 构造/解析往返。
    #[test]
    fn test_rtcp_receiver_report_roundtrip(rr in valid_receiver_report()) {
        let packets = vec![RtcpPacket::ReceiverReport(rr.clone())];
        let encoded = RtcpPacket::build(&packets).expect("build receiver report packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse receiver report packet");

        prop_assert_eq!(decoded.len(), 1);
        if let RtcpPacket::ReceiverReport(decoded_rr) = &decoded[0] {
            prop_assert_eq!(decoded_rr.ssrc, rr.ssrc);
            prop_assert_eq!(decoded_rr.reports.len(), rr.reports.len());
        } else {
            prop_assert!(false, "expected ReceiverReport");
        }
    }

    /// Source Description build/parse round-trip.
    ///
    /// Source Description 构造/解析往返。
    #[test]
    fn test_rtcp_sdes_roundtrip(sdes in valid_sdes()) {
        let packets = vec![RtcpPacket::SourceDescription(sdes.clone())];
        let encoded = RtcpPacket::build(&packets).expect("build sdes packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse sdes packet");

        prop_assert_eq!(decoded.len(), 1);
        if let RtcpPacket::SourceDescription(decoded_sdes) = &decoded[0] {
            prop_assert_eq!(decoded_sdes.chunks.len(), sdes.chunks.len());
            for (orig_chunk, dec_chunk) in sdes.chunks.iter().zip(decoded_sdes.chunks.iter()) {
                prop_assert_eq!(dec_chunk.ssrc, orig_chunk.ssrc);
                prop_assert_eq!(dec_chunk.items.len(), orig_chunk.items.len());
            }
        } else {
            prop_assert!(false, "expected SourceDescription");
        }
    }

    /// BYE build/parse round-trip.
    ///
    /// BYE 构造/解析往返。
    #[test]
    fn test_rtcp_bye_roundtrip(bye in valid_bye()) {
        let packets = vec![RtcpPacket::Bye(bye.clone())];
        let encoded = RtcpPacket::build(&packets).expect("build bye packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse bye packet");

        prop_assert_eq!(decoded.len(), 1);
        if let RtcpPacket::Bye(decoded_bye) = &decoded[0] {
            prop_assert_eq!(&decoded_bye.ssrcs, &bye.ssrcs);
            prop_assert_eq!(&decoded_bye.reason, &bye.reason);
        } else {
            prop_assert!(false, "expected BYE");
        }
    }

    /// APP build/parse round-trip.
    ///
    /// APP 构造/解析往返。
    #[test]
    fn test_rtcp_app_roundtrip(app in valid_app()) {
        let packets = vec![RtcpPacket::App(app.clone())];
        let encoded = RtcpPacket::build(&packets).expect("build app packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse app packet");

        prop_assert_eq!(decoded.len(), 1);
        if let RtcpPacket::App(decoded_app) = &decoded[0] {
            prop_assert_eq!(decoded_app.subtype, app.subtype);
            prop_assert_eq!(decoded_app.ssrc, app.ssrc);
            prop_assert_eq!(decoded_app.name, app.name);
            prop_assert_eq!(&decoded_app.data, &app.data);
        } else {
            prop_assert!(false, "expected APP");
        }
    }

    /// Compound RTCP packet build/parse round-trip.
    ///
    /// 复合 RTCP 包构造/解析往返。
    #[test]
    fn test_rtcp_compound_packet_roundtrip(
        sr in valid_sender_report(),
        sdes in valid_sdes(),
    ) {
        let packets = vec![
            RtcpPacket::SenderReport(sr.clone()),
            RtcpPacket::SourceDescription(sdes.clone()),
        ];
        let encoded = RtcpPacket::build(&packets).expect("build compound rtcp packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse compound rtcp packet");

        prop_assert_eq!(decoded.len(), 2);

        if let RtcpPacket::SenderReport(decoded_sr) = &decoded[0] {
            prop_assert_eq!(decoded_sr.ssrc, sr.ssrc);
        } else {
            prop_assert!(false, "expected SenderReport");
        }

        if let RtcpPacket::SourceDescription(decoded_sdes) = &decoded[1] {
            prop_assert_eq!(decoded_sdes.chunks.len(), sdes.chunks.len());
        } else {
            prop_assert!(false, "expected SourceDescription");
        }
    }

    /// Report block values are preserved through build/parse.
    ///
    /// 报告块字段值在构造/解析后保持一致。
    #[test]
    fn test_rtcp_report_block_values(report in valid_report_block()) {
        let sr = RtcpSenderReport {
            ssrc: 0x1234_5678,
            ntp_timestamp: 0,
            rtp_timestamp: 0,
            packet_count: 0,
            octet_count: 0,
            reports: vec![report.clone()],
        };

        let packets = vec![RtcpPacket::SenderReport(sr)];
        let encoded = RtcpPacket::build(&packets).expect("build sender report packet");
        let decoded = RtcpPacket::parse(&encoded).expect("parse sender report packet");

        if let RtcpPacket::SenderReport(decoded_sr) = &decoded[0] {
            let decoded_report = &decoded_sr.reports[0];
            prop_assert_eq!(decoded_report.ssrc, report.ssrc);
            prop_assert_eq!(decoded_report.fraction_lost, report.fraction_lost);
            prop_assert_eq!(decoded_report.cumulative_lost, report.cumulative_lost);
            prop_assert_eq!(decoded_report.highest_seq, report.highest_seq);
            prop_assert_eq!(decoded_report.jitter, report.jitter);
            prop_assert_eq!(decoded_report.last_sr, report.last_sr);
            prop_assert_eq!(decoded_report.delay_since_sr, report.delay_since_sr);
        } else {
            prop_assert!(false, "expected SenderReport");
        }
    }
}

/// Invalid RTCP inputs must return explicit errors or empty results, not panic.
///
/// 非法 RTCP 输入必须返回显式错误或空结果，不得 panic。
#[test]
fn test_rtcp_parse_invalid_data() {
    // Empty input parses as an empty result.
    assert!(RtcpPacket::parse(&[])
        .expect("empty input should parse")
        .is_empty());
    assert!(RtcpPacket::parse(&[0, 0, 0])
        .expect("truncated header should parse as empty")
        .is_empty());

    // Non-version-2 packets must return an explicit error.
    let mut invalid_version = vec![0_u8; 8];
    invalid_version[0] = 0b0000_0000; // version = 0
    invalid_version[1] = 200; // Sender Report
    invalid_version[2] = 0;
    invalid_version[3] = 1; // payload bytes = 4
    assert!(RtcpPacket::parse(&invalid_version).is_err());
}
