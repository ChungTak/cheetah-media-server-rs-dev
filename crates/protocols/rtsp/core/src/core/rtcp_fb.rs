//! RTCP Feedback packets: NACK (RFC 4585 §6.2.1), PLI (§6.3.1), FIR (RFC 5104 §4.3.1).

/// RTCP Transport Layer Feedback (PT=205).
pub const RTCP_PT_RTPFB: u8 = 205;
/// RTCP Payload-Specific Feedback (PT=206).
pub const RTCP_PT_PSFB: u8 = 206;

/// FMT values for RTPFB (PT=205).
pub const RTPFB_FMT_NACK: u8 = 1;

/// FMT values for PSFB (PT=206).
pub const PSFB_FMT_PLI: u8 = 1;
pub const PSFB_FMT_FIR: u8 = 4;

/// RTCP Generic NACK feedback (RFC 4585 §6.2.1).
///
/// RTCP Generic NACK 反馈（RFC 4585 §6.2.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpNack {
    pub sender_ssrc: u32,
    pub media_ssrc: u32,
    pub nack_items: Vec<NackItem>,
}

/// A single NACK FCI entry: one PID + 16-bit bitmask of following lost packets.
///
/// NACK FCI 单个条目：一个 PID + 后续丢包的 16 位掩码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NackItem {
    pub pid: u16,
    pub blp: u16,
}

impl NackItem {
    /// Iterate all lost sequence numbers represented by this item.
    ///
    /// 遍历该条目表示的所有丢失序列号。
    pub fn lost_seqs(&self) -> impl Iterator<Item = u16> + '_ {
        core::iter::once(self.pid).chain((0..16u16).filter_map(|bit| {
            if self.blp & (1 << bit) != 0 {
                Some(self.pid.wrapping_add(bit + 1))
            } else {
                None
            }
        }))
    }
}

/// RTCP Picture Loss Indication (RFC 4585 §6.3.1).
///
/// RTCP 图片丢失指示（RFC 4585 §6.3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcpPli {
    pub sender_ssrc: u32,
    pub media_ssrc: u32,
}

/// RTCP Full Intra Request (RFC 5104 §4.3.1).
///
/// RTCP 全帧内请求（RFC 5104 §4.3.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpFir {
    pub sender_ssrc: u32,
    pub media_ssrc: u32,
    pub fci: Vec<FirEntry>,
}

/// A single FIR FCI entry.
///
/// FIR FCI 单个条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirEntry {
    pub ssrc: u32,
    pub seq_nr: u8,
}

/// Parsed RTCP Feedback packet (NACK, PLI, or FIR).
///
/// 解析后的 RTCP Feedback 包（NACK、PLI 或 FIR）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcpFeedback {
    Nack(RtcpNack),
    Pli(RtcpPli),
    Fir(RtcpFir),
}

/// Parse an RTCP-FB packet from raw payload (after the common header).
///
/// `pt` is the payload type (205 or 206) and `fmt` is the FMT/count field that
/// selects between NACK, PLI, and FIR.
///
/// 从原始负载解析 RTCP-FB 包（位于公共头之后）。
///
/// `pt` 为 payload type（205 或 206），`fmt` 为选择 NACK、PLI、FIR 的 FMT/count 字段。
pub fn parse_rtcp_fb(pt: u8, fmt: u8, payload: &[u8]) -> Option<RtcpFeedback> {
    if payload.len() < 8 {
        return None;
    }
    let sender_ssrc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let media_ssrc = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let fci = &payload[8..];

    match (pt, fmt) {
        (RTCP_PT_RTPFB, RTPFB_FMT_NACK) => {
            let mut nack_items = Vec::new();
            let mut offset = 0;
            while offset + 4 <= fci.len() {
                let pid = u16::from_be_bytes([fci[offset], fci[offset + 1]]);
                let blp = u16::from_be_bytes([fci[offset + 2], fci[offset + 3]]);
                nack_items.push(NackItem { pid, blp });
                offset += 4;
            }
            Some(RtcpFeedback::Nack(RtcpNack {
                sender_ssrc,
                media_ssrc,
                nack_items,
            }))
        }
        (RTCP_PT_PSFB, PSFB_FMT_PLI) => Some(RtcpFeedback::Pli(RtcpPli {
            sender_ssrc,
            media_ssrc,
        })),
        (RTCP_PT_PSFB, PSFB_FMT_FIR) => {
            let mut entries = Vec::new();
            let mut offset = 0;
            while offset + 8 <= fci.len() {
                let ssrc = u32::from_be_bytes([
                    fci[offset],
                    fci[offset + 1],
                    fci[offset + 2],
                    fci[offset + 3],
                ]);
                let seq_nr = fci[offset + 4];
                entries.push(FirEntry { ssrc, seq_nr });
                offset += 8;
            }
            Some(RtcpFeedback::Fir(RtcpFir {
                sender_ssrc,
                media_ssrc,
                fci: entries,
            }))
        }
        _ => None,
    }
}

/// Encode an RTCP NACK packet (PT=205, FMT=1).
///
/// 编码 RTCP NACK 包（PT=205，FMT=1）。
pub fn build_rtcp_nack(nack: &RtcpNack) -> Vec<u8> {
    let fci_len = nack.nack_items.len() * 4;
    let payload_len = 8 + fci_len;
    let length_words = payload_len / 4;
    let mut out = Vec::with_capacity(4 + payload_len);
    // Common header: V=2, P=0, FMT=1, PT=205
    out.push(0x80 | RTPFB_FMT_NACK);
    out.push(RTCP_PT_RTPFB);
    out.extend_from_slice(&(length_words as u16).to_be_bytes());
    out.extend_from_slice(&nack.sender_ssrc.to_be_bytes());
    out.extend_from_slice(&nack.media_ssrc.to_be_bytes());
    for item in &nack.nack_items {
        out.extend_from_slice(&item.pid.to_be_bytes());
        out.extend_from_slice(&item.blp.to_be_bytes());
    }
    out
}

/// Encode an RTCP PLI packet (PT=206, FMT=1).
///
/// 编码 RTCP PLI 包（PT=206，FMT=1）。
pub fn build_rtcp_pli(pli: &RtcpPli) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    // Common header: V=2, P=0, FMT=1, PT=206, length=2 (8 bytes payload)
    out.push(0x80 | PSFB_FMT_PLI);
    out.push(RTCP_PT_PSFB);
    out.extend_from_slice(&2_u16.to_be_bytes());
    out.extend_from_slice(&pli.sender_ssrc.to_be_bytes());
    out.extend_from_slice(&pli.media_ssrc.to_be_bytes());
    out
}

/// Encode an RTCP FIR packet (PT=206, FMT=4).
///
/// 编码 RTCP FIR 包（PT=206，FMT=4）。
pub fn build_rtcp_fir(fir: &RtcpFir) -> Vec<u8> {
    let fci_len = fir.fci.len() * 8;
    let payload_len = 8 + fci_len;
    let length_words = payload_len / 4;
    let mut out = Vec::with_capacity(4 + payload_len);
    // Common header: V=2, P=0, FMT=4, PT=206
    out.push(0x80 | PSFB_FMT_FIR);
    out.push(RTCP_PT_PSFB);
    out.extend_from_slice(&(length_words as u16).to_be_bytes());
    out.extend_from_slice(&fir.sender_ssrc.to_be_bytes());
    out.extend_from_slice(&fir.media_ssrc.to_be_bytes());
    for entry in &fir.fci {
        out.extend_from_slice(&entry.ssrc.to_be_bytes());
        out.push(entry.seq_nr);
        out.extend_from_slice(&[0, 0, 0]); // reserved
    }
    out
}

/// Build NACK items from a list of lost sequence numbers.
///
/// Sorts, deduplicates, and groups consecutive losses into `NackItem` entries
/// where the 16-bit `blp` bitmask follows the PID.
///
/// 从丢失序列号列表构造 NACK 条目。
///
/// 排序、去重，并将连续丢包分组为 PID 后带 16 位 `blp` 掩码的 `NackItem`。
pub fn nack_items_from_lost_seqs(lost: &[u16]) -> Vec<NackItem> {
    if lost.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<u16> = lost.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut items = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let pid = sorted[i];
        let mut blp: u16 = 0;
        let mut j = i + 1;
        while j < sorted.len() {
            let diff = sorted[j].wrapping_sub(pid);
            if (1..=16).contains(&diff) {
                blp |= 1 << (diff - 1);
                j += 1;
            } else {
                break;
            }
        }
        items.push(NackItem { pid, blp });
        i = j;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nack_roundtrip() {
        let nack = RtcpNack {
            sender_ssrc: 0x1111_2222,
            media_ssrc: 0x3333_4444,
            nack_items: vec![NackItem {
                pid: 100,
                blp: 0b0000_0000_0000_0101,
            }],
        };
        let encoded = build_rtcp_nack(&nack);
        let parsed = parse_rtcp_fb(RTCP_PT_RTPFB, RTPFB_FMT_NACK, &encoded[4..]);
        assert_eq!(parsed, Some(RtcpFeedback::Nack(nack)));
    }

    #[test]
    fn pli_roundtrip() {
        let pli = RtcpPli {
            sender_ssrc: 0xAAAA_BBBB,
            media_ssrc: 0xCCCC_DDDD,
        };
        let encoded = build_rtcp_pli(&pli);
        assert_eq!(encoded.len(), 12);
        let parsed = parse_rtcp_fb(RTCP_PT_PSFB, PSFB_FMT_PLI, &encoded[4..]);
        assert_eq!(parsed, Some(RtcpFeedback::Pli(pli)));
    }

    #[test]
    fn fir_roundtrip() {
        let fir = RtcpFir {
            sender_ssrc: 0x1000_2000,
            media_ssrc: 0x0000_0000,
            fci: vec![FirEntry {
                ssrc: 0x5555_6666,
                seq_nr: 7,
            }],
        };
        let encoded = build_rtcp_fir(&fir);
        let parsed = parse_rtcp_fb(RTCP_PT_PSFB, PSFB_FMT_FIR, &encoded[4..]);
        assert_eq!(parsed, Some(RtcpFeedback::Fir(fir)));
    }

    #[test]
    fn nack_item_lost_seqs() {
        let item = NackItem {
            pid: 10,
            blp: 0b0000_0000_0000_0101,
        };
        let seqs: Vec<u16> = item.lost_seqs().collect();
        assert_eq!(seqs, vec![10, 11, 13]); // pid=10, pid+1=11, pid+3=13
    }

    #[test]
    fn nack_items_from_lost_seqs_groups_correctly() {
        let lost = vec![100, 101, 103, 200, 201];
        let items = nack_items_from_lost_seqs(&lost);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].pid, 100);
        assert_eq!(items[0].blp, 0b0000_0000_0000_0101); // +1, +3
        assert_eq!(items[1].pid, 200);
        assert_eq!(items[1].blp, 0b0000_0000_0000_0001); // +1
    }

    #[test]
    fn parse_returns_none_for_unknown_fmt() {
        let payload = [0u8; 8];
        assert_eq!(parse_rtcp_fb(RTCP_PT_RTPFB, 99, &payload), None);
    }

    #[test]
    fn parse_returns_none_for_short_payload() {
        assert_eq!(parse_rtcp_fb(RTCP_PT_PSFB, PSFB_FMT_PLI, &[0; 4]), None);
    }
}
