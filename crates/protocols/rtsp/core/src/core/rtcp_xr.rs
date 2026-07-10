//! RTCP Extended Reports (RFC 3611).

/// RTCP XR payload type.
pub const RTCP_PT_XR: u8 = 207;

/// RTCP XR block types.
#[allow(dead_code)]
pub const XR_BLOCK_LOSS_RLE: u8 = 1;
/// `XR_BLOCK_DUPLICATE_RLE` constant.
/// `XR_BLOCK_DUPLICATE_RLE` 常量.
#[allow(dead_code)]
pub const XR_BLOCK_DUPLICATE_RLE: u8 = 2;
/// `XR_BLOCK_PACKET_RECEIPT_TIMES` constant.
/// `XR_BLOCK_PACKET_RECEIPT_TIMES` 常量.
#[allow(dead_code)]
pub const XR_BLOCK_PACKET_RECEIPT_TIMES: u8 = 3;
/// `XR_BLOCK_RECEIVER_REFERENCE_TIME` constant.
/// `XR_BLOCK_RECEIVER_REFERENCE_TIME` 常量.
pub const XR_BLOCK_RECEIVER_REFERENCE_TIME: u8 = 4;
/// `XR_BLOCK_DLRR` constant.
/// `XR_BLOCK_DLRR` 常量.
pub const XR_BLOCK_DLRR: u8 = 5;
/// `XR_BLOCK_STATISTICS_SUMMARY` constant.
/// `XR_BLOCK_STATISTICS_SUMMARY` 常量.
#[allow(dead_code)]
pub const XR_BLOCK_STATISTICS_SUMMARY: u8 = 6;
/// `XR_BLOCK_VOIP_METRICS` constant.
/// `XR_BLOCK_VOIP_METRICS` 常量.
pub const XR_BLOCK_VOIP_METRICS: u8 = 7;

/// Parsed RTCP XR packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpXr {
    /// `sender_ssrc` field of type `u32`.
    /// `sender_ssrc` 字段，类型为 `u32`.
    pub sender_ssrc: u32,
    /// `blocks` field.
    /// `blocks` 字段.
    pub blocks: Vec<XrBlock>,
}

/// An individual XR report block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XrBlock {
    /// §4.4 Receiver Reference Time Report Block.
    ReceiverReferenceTime { ntp_timestamp: u64 },
    /// §4.5 DLRR Report Block.
    Dlrr { sub_blocks: Vec<DlrrSubBlock> },
    /// §4.7 VoIP Metrics Report Block.
    VoipMetrics(VoipMetricsBlock),
    /// Unknown block type (forward-compatible).
    Unknown { block_type: u8, data: Vec<u8> },
}

/// DLRR sub-block entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlrrSubBlock {
    /// `ssrc` field of type `u32`.
    /// `ssrc` 字段，类型为 `u32`.
    pub ssrc: u32,
    /// `last_rr` field of type `u32`.
    /// `last_rr` 字段，类型为 `u32`.
    pub last_rr: u32,
    /// `delay_since_last_rr` field of type `u32`.
    /// `delay_since_last_rr` 字段，类型为 `u32`.
    pub delay_since_last_rr: u32,
}

/// VoIP Metrics Report Block (§4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoipMetricsBlock {
    /// `ssrc` field of type `u32`.
    /// `ssrc` 字段，类型为 `u32`.
    pub ssrc: u32,
    /// `loss_rate` field of type `u8`.
    /// `loss_rate` 字段，类型为 `u8`.
    pub loss_rate: u8,
    /// `discard_rate` field of type `u8`.
    /// `discard_rate` 字段，类型为 `u8`.
    pub discard_rate: u8,
    /// `burst_density` field of type `u8`.
    /// `burst_density` 字段，类型为 `u8`.
    pub burst_density: u8,
    /// `gap_density` field of type `u8`.
    /// `gap_density` 字段，类型为 `u8`.
    pub gap_density: u8,
    /// `burst_duration` field of type `u16`.
    /// `burst_duration` 字段，类型为 `u16`.
    pub burst_duration: u16,
    /// `gap_duration` field of type `u16`.
    /// `gap_duration` 字段，类型为 `u16`.
    pub gap_duration: u16,
    /// `round_trip_delay` field of type `u16`.
    /// `round_trip_delay` 字段，类型为 `u16`.
    pub round_trip_delay: u16,
    /// `end_system_delay` field of type `u16`.
    /// `end_system_delay` 字段，类型为 `u16`.
    pub end_system_delay: u16,
    /// `signal_level` field of type `u8`.
    /// `signal_level` 字段，类型为 `u8`.
    pub signal_level: u8,
    /// `noise_level` field of type `u8`.
    /// `noise_level` 字段，类型为 `u8`.
    pub noise_level: u8,
    /// `rerl` field of type `u8`.
    /// `rerl` 字段，类型为 `u8`.
    pub rerl: u8,
    /// `gmin` field of type `u8`.
    /// `gmin` 字段，类型为 `u8`.
    pub gmin: u8,
    /// `r_factor` field of type `u8`.
    /// `r_factor` 字段，类型为 `u8`.
    pub r_factor: u8,
    /// `ext_r_factor` field of type `u8`.
    /// `ext_r_factor` 字段，类型为 `u8`.
    pub ext_r_factor: u8,
    /// `mos_lq` field of type `u8`.
    /// `mos_lq` 字段，类型为 `u8`.
    pub mos_lq: u8,
    /// `mos_cq` field of type `u8`.
    /// `mos_cq` 字段，类型为 `u8`.
    pub mos_cq: u8,
    /// `jb_nominal` field of type `u16`.
    /// `jb_nominal` 字段，类型为 `u16`.
    pub jb_nominal: u16,
    /// `jb_maximum` field of type `u16`.
    /// `jb_maximum` 字段，类型为 `u16`.
    pub jb_maximum: u16,
    /// `jb_abs_max` field of type `u16`.
    /// `jb_abs_max` 字段，类型为 `u16`.
    pub jb_abs_max: u16,
}

/// Parse an RTCP XR packet from payload (after common header).
pub fn parse_rtcp_xr(payload: &[u8]) -> Option<RtcpXr> {
    if payload.len() < 4 {
        return None;
    }
    let sender_ssrc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let mut blocks = Vec::new();
    let mut offset = 4;

    while offset + 4 <= payload.len() {
        let block_type = payload[offset];
        let _type_specific = payload[offset + 1];
        let block_length_words = u16::from_be_bytes([payload[offset + 2], payload[offset + 3]]);
        let block_data_len = block_length_words as usize * 4;
        offset += 4;
        if offset + block_data_len > payload.len() {
            break;
        }
        let block_data = &payload[offset..offset + block_data_len];
        let block = match block_type {
            XR_BLOCK_RECEIVER_REFERENCE_TIME if block_data.len() >= 8 => {
                let ntp = u64::from_be_bytes([
                    block_data[0],
                    block_data[1],
                    block_data[2],
                    block_data[3],
                    block_data[4],
                    block_data[5],
                    block_data[6],
                    block_data[7],
                ]);
                XrBlock::ReceiverReferenceTime { ntp_timestamp: ntp }
            }
            XR_BLOCK_DLRR => {
                let mut sub_blocks = Vec::new();
                let mut i = 0;
                while i + 12 <= block_data.len() {
                    sub_blocks.push(DlrrSubBlock {
                        ssrc: u32::from_be_bytes([
                            block_data[i],
                            block_data[i + 1],
                            block_data[i + 2],
                            block_data[i + 3],
                        ]),
                        last_rr: u32::from_be_bytes([
                            block_data[i + 4],
                            block_data[i + 5],
                            block_data[i + 6],
                            block_data[i + 7],
                        ]),
                        delay_since_last_rr: u32::from_be_bytes([
                            block_data[i + 8],
                            block_data[i + 9],
                            block_data[i + 10],
                            block_data[i + 11],
                        ]),
                    });
                    i += 12;
                }
                XrBlock::Dlrr { sub_blocks }
            }
            XR_BLOCK_VOIP_METRICS if block_data.len() >= 28 => {
                XrBlock::VoipMetrics(VoipMetricsBlock {
                    ssrc: u32::from_be_bytes([
                        block_data[0],
                        block_data[1],
                        block_data[2],
                        block_data[3],
                    ]),
                    loss_rate: block_data[4],
                    discard_rate: block_data[5],
                    burst_density: block_data[6],
                    gap_density: block_data[7],
                    burst_duration: u16::from_be_bytes([block_data[8], block_data[9]]),
                    gap_duration: u16::from_be_bytes([block_data[10], block_data[11]]),
                    round_trip_delay: u16::from_be_bytes([block_data[12], block_data[13]]),
                    end_system_delay: u16::from_be_bytes([block_data[14], block_data[15]]),
                    signal_level: block_data[16],
                    noise_level: block_data[17],
                    rerl: block_data[18],
                    gmin: block_data[19],
                    r_factor: block_data[20],
                    ext_r_factor: block_data[21],
                    mos_lq: block_data[22],
                    mos_cq: block_data[23],
                    jb_nominal: u16::from_be_bytes([block_data[24], block_data[25]]),
                    jb_maximum: u16::from_be_bytes([block_data[26], block_data[27]]),
                    jb_abs_max: if block_data.len() >= 30 {
                        u16::from_be_bytes([block_data[28], block_data[29]])
                    } else {
                        0
                    },
                })
            }
            _ => XrBlock::Unknown {
                block_type,
                data: block_data.to_vec(),
            },
        };
        blocks.push(block);
        offset += block_data_len;
    }

    Some(RtcpXr {
        sender_ssrc,
        blocks,
    })
}

/// Build an RTCP XR Receiver Reference Time block.
pub fn build_rtcp_xr_receiver_reference_time(sender_ssrc: u32, ntp_timestamp: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    // Common header: V=2, P=0, reserved=0, PT=207, length=3 (12 bytes payload)
    out.push(0x80);
    out.push(RTCP_PT_XR);
    out.extend_from_slice(&3_u16.to_be_bytes());
    out.extend_from_slice(&sender_ssrc.to_be_bytes());
    // Block header: type=4, type_specific=0, length=2 (8 bytes)
    out.push(XR_BLOCK_RECEIVER_REFERENCE_TIME);
    out.push(0);
    out.extend_from_slice(&2_u16.to_be_bytes());
    out.extend_from_slice(&ntp_timestamp.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_receiver_reference_time() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x1234_5678u32.to_be_bytes()); // sender SSRC
                                                                  // Block: type=4, type_specific=0, length=2
        payload.push(XR_BLOCK_RECEIVER_REFERENCE_TIME);
        payload.push(0);
        payload.extend_from_slice(&2_u16.to_be_bytes());
        payload.extend_from_slice(&0xAABB_CCDD_EEFF_0011u64.to_be_bytes());

        let xr = parse_rtcp_xr(&payload).unwrap();
        assert_eq!(xr.sender_ssrc, 0x1234_5678);
        assert_eq!(xr.blocks.len(), 1);
        match &xr.blocks[0] {
            XrBlock::ReceiverReferenceTime { ntp_timestamp } => {
                assert_eq!(*ntp_timestamp, 0xAABB_CCDD_EEFF_0011);
            }
            _ => panic!("expected ReceiverReferenceTime"),
        }
    }

    #[test]
    fn build_and_parse_receiver_reference_time() {
        let encoded = build_rtcp_xr_receiver_reference_time(0x1111, 0x2222_3333_4444_5555);
        // Skip common header (4 bytes) to get payload
        let xr = parse_rtcp_xr(&encoded[4..]).unwrap();
        assert_eq!(xr.sender_ssrc, 0x1111);
        match &xr.blocks[0] {
            XrBlock::ReceiverReferenceTime { ntp_timestamp } => {
                assert_eq!(*ntp_timestamp, 0x2222_3333_4444_5555);
            }
            _ => panic!("expected ReceiverReferenceTime"),
        }
    }

    #[test]
    fn parse_unknown_block_type() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // sender SSRC
                                                        // Unknown block: type=99, length=1 (4 bytes data)
        payload.push(99);
        payload.push(0);
        payload.extend_from_slice(&1_u16.to_be_bytes());
        payload.extend_from_slice(&[1, 2, 3, 4]);

        let xr = parse_rtcp_xr(&payload).unwrap();
        assert_eq!(xr.blocks.len(), 1);
        match &xr.blocks[0] {
            XrBlock::Unknown { block_type, data } => {
                assert_eq!(*block_type, 99);
                assert_eq!(data, &[1, 2, 3, 4]);
            }
            _ => panic!("expected Unknown"),
        }
    }
}
