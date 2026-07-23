//! Property-test scaffold for `cheetah-rtp-core` and `cheetah-codec` RTP paths.
//!
//! Tests live under `#[cfg(test)]` so `proptest` stays a dev-dependency.
//!
//! `cheetah-rtp-core` 的属性测试承载 crate，测试位于 `#[cfg(test)]` 下以保持
//! `proptest` 为 dev-dependency。

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_codec::{
        RtpHeader, RtpPacket, RtpReorderBuffer, RtpReorderSettings, RtpSequenceUnwrapper,
    };
    use cheetah_rtp_core::rtcp::{
        RtcpAppPacket, RtcpBye, RtcpCompoundPacket, RtcpPacket, RtcpReceiverReport,
        RtcpReportBlock, RtcpSdesChunk, RtcpSdesItem, RtcpSdesItemType, RtcpSenderReport,
        RtcpSourceDescription,
    };
    use proptest::prelude::*;

    // RTP header roundtrip through 12-byte wire encoding.
    fn arb_rtp_header() -> impl Strategy<Value = RtpHeader> {
        (
            0..4u8,
            0..128u8,
            any::<bool>(),
            any::<u16>(),
            any::<u32>(),
            any::<u32>(),
        )
            .prop_map(
                |(version, payload_type, marker, sequence_number, timestamp, ssrc)| RtpHeader {
                    version,
                    payload_type,
                    marker,
                    sequence_number,
                    timestamp,
                    ssrc,
                },
            )
    }

    proptest! {
        #[test]
        fn rtp_header_encode_parse_roundtrip(header in arb_rtp_header()) {
            let encoded = header.encode();
            let (parsed, offset) = RtpHeader::parse(&encoded).expect("parse should succeed");
            prop_assert_eq!(offset, 12);
            prop_assert_eq!(parsed, header);
        }
    }

    proptest! {
        #[test]
        fn rtp_packet_encode_parse_roundtrip(
            header in arb_rtp_header(),
            payload in prop::collection::vec(any::<u8>(), 0..1024),
        ) {
            let packet = RtpPacket {
                header,
                payload: Bytes::from(payload),
            };
            let encoded = packet.encode();
            let parsed = RtpPacket::parse(&encoded).expect("parse should succeed");
            prop_assert_eq!(parsed.header, header);
            prop_assert_eq!(parsed.payload, packet.payload);
        }
    }

    // Sequence unwrapper never moves backwards and correctly drops late/duplicate packets.
    proptest! {
        #[test]
        fn sequence_unwrapper_is_monotonic_and_drops_late(raw_seqs in prop::collection::vec(any::<u16>(), 1..256)) {
            let mut unwrapper = RtpSequenceUnwrapper::new();
            let mut last_extended: Option<u64> = None;
            let mut last_max: Option<u64> = None;

            for seq in raw_seqs {
                let extended = unwrapper.extend(seq);
                let expected = unwrapper.expected_seq().unwrap();
                let max = unwrapper.max_seq().unwrap();

                // max is non-decreasing.
                if let Some(lm) = last_max {
                    prop_assert!(max >= lm, "max decreased from {lm} to {max}");
                }
                last_max = Some(max);

                // expected is non-decreasing.
                prop_assert!(expected > 0);
                if let Some(le) = last_extended {
                    prop_assert!(expected > le, "expected regressed");
                }

                // Late/duplicate packets are returned as expected - 1 and do not advance state.
                let is_late = extended == expected.saturating_sub(1) && extended != max;
                if is_late {
                    prop_assert_eq!(expected, last_extended.map(|e| e + 1).unwrap_or(1));
                } else {
                    prop_assert!(extended >= last_extended.unwrap_or(0), "extended seq regressed");
                    last_extended = Some(extended);
                }
            }
        }
    }

    // Reorder buffer invariants: bounded pending, ready sequences are monotonic.
    proptest! {
        #[test]
        fn reorder_buffer_bounded_and_monotonic(
            inputs in prop::collection::vec((any::<u16>(), 0..1000u64), 0..256),
        ) {
            let settings = RtpReorderSettings {
                max_packets: 2,
                max_delay_ms: 0,
            };
            let mut buffer = RtpReorderBuffer::<u16>::new(settings);
            let mut last_ready: Option<u64> = None;
            let mut check_unwrapper = RtpSequenceUnwrapper::new();

            for (seq, arrival_ms) in inputs {
                let ready = buffer.push(seq, arrival_ms, seq);

                prop_assert!(buffer.pending_len() <= 2, "pending exceeded max_packets");

                // Each ready batch must be in strictly increasing extended sequence order.
                for &returned_seq in &ready {
                    let returned_extended = check_unwrapper.extend(returned_seq);
                    if let Some(le) = last_ready {
                        prop_assert!(returned_extended > le, "ready sequence regressed: {returned_extended} vs {le}");
                    }
                    last_ready = Some(returned_extended);
                }
            }
        }
    }

    /// RTCP report block within the 24-bit signed cumulative-lost range.
    fn arb_report_block() -> impl Strategy<Value = RtcpReportBlock> {
        (
            any::<u32>(),
            any::<u8>(),
            -8_388_608i32..8_388_607i32, // 24-bit signed range
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
        )
            .prop_map(
                |(ssrc, fraction_lost, cumulative_lost, highest_seq, jitter, last_sr, delay)| {
                    RtcpReportBlock {
                        ssrc,
                        fraction_lost,
                        cumulative_lost,
                        highest_seq,
                        jitter,
                        last_sr,
                        delay_since_last_sr: delay,
                    }
                },
            )
    }

    fn arb_sdes_item_type() -> impl Strategy<Value = RtcpSdesItemType> {
        prop::sample::select(vec![
            RtcpSdesItemType::CName,
            RtcpSdesItemType::Name,
            RtcpSdesItemType::Email,
            RtcpSdesItemType::Phone,
            RtcpSdesItemType::Location,
            RtcpSdesItemType::Tool,
            RtcpSdesItemType::Note,
            RtcpSdesItemType::Priv,
        ])
    }

    fn arb_rtcp_packet() -> impl Strategy<Value = RtcpPacket> {
        let sender_report = (
            any::<u32>(),
            any::<u64>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            prop::collection::vec(arb_report_block(), 0..=31),
        )
            .prop_map(|(ssrc, ntp, rtp, sent, octets, blocks)| {
                RtcpPacket::SenderReport(RtcpSenderReport {
                    ssrc,
                    ntp_timestamp: ntp,
                    rtp_timestamp: rtp,
                    packets_sent: sent,
                    octets_sent: octets,
                    report_blocks: blocks,
                })
            });

        let receiver_report = (
            any::<u32>(),
            prop::collection::vec(arb_report_block(), 0..=31),
        )
            .prop_map(|(ssrc, blocks)| {
                RtcpPacket::ReceiverReport(RtcpReceiverReport {
                    ssrc,
                    report_blocks: blocks,
                })
            });

        let bye = (
            prop::collection::vec(any::<u32>(), 0..=31),
            prop::option::of(".{1,63}"),
        )
            .prop_map(|(ssrcs, reason)| RtcpPacket::Bye(RtcpBye { ssrcs, reason }));

        let app = (
            0..32u8,
            any::<u32>(),
            [any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()],
            prop::collection::vec(any::<u8>(), 0..=80usize)
                .prop_filter("payload must be 4-aligned", |v| v.len() % 4 == 0),
        )
            .prop_map(|(subtype, ssrc, name, payload)| {
                RtcpPacket::App(RtcpAppPacket {
                    subtype,
                    ssrc,
                    name,
                    payload: Bytes::from(payload),
                })
            });

        let sdes = prop::collection::vec(
            (
                any::<u32>(),
                prop::collection::vec((arb_sdes_item_type(), ".{0,63}"), 0..8),
            )
                .prop_map(|(ssrc, items)| RtcpSdesChunk {
                    ssrc,
                    items: items
                        .into_iter()
                        .map(|(ty, text)| RtcpSdesItem {
                            item_type: ty,
                            text,
                        })
                        .collect(),
                }),
            0..=31,
        )
        .prop_map(|chunks| RtcpPacket::SourceDescription(RtcpSourceDescription { chunks }));

        let unknown = (
            prop::sample::select(vec![0u8, 1, 2, 3, 50, 100, 150, 199, 205, 250]),
            0..32u8,
            prop::collection::vec(any::<u8>(), 0..=80usize)
                .prop_filter("payload must be 4-aligned", |v| v.len() % 4 == 0),
        )
            .prop_map(|(pt, count, payload)| RtcpPacket::Unknown {
                pt,
                count,
                payload: Bytes::from(payload),
            });

        prop_oneof![sender_report, receiver_report, bye, app, sdes, unknown]
    }

    proptest! {
        #[test]
        fn rtcp_compound_packet_roundtrip(packets in prop::collection::vec(arb_rtcp_packet(), 1..6)) {
            let compound = RtcpCompoundPacket { packets };
            let encoded = compound.encode().expect("encode should succeed");
            let parsed = RtcpCompoundPacket::parse(encoded).expect("parse should succeed");
            prop_assert_eq!(parsed, compound);
        }
    }
}
