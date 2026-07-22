//! RTCP compound packet parser/encoder (RFC 3550).
//!
//! Supports Sender Report (SR), Receiver Report (RR), Source Description (SDES)
//! and BYE packets. This is a Sans-I/O data-plane helper: it does not manage
//! timers or session state.
//!
//! RTCP 复合包解析/编码（RFC 3550）。
//!
//! 支持发送者报告（SR）、接收者报告（RR）、源描述（SDES）和 BYE 包。
//! 这是一个 Sans-I/O 数据面辅助模块，不管理定时器或会话状态。

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

/// Errors encountered while parsing RTCP packets.
///
/// 解析 RTCP 包时遇到的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RtcpParseError {
    #[error("rtcp packet too short")]
    TooShort,
    #[error("truncated rtcp {pt} packet")]
    Truncated { pt: u8 },
    #[error("invalid rtcp version: {version}")]
    InvalidVersion { version: u8 },
    #[error("invalid sdes item length")]
    InvalidSdes,
}

/// RTCP packet type identifiers.
///
/// RTCP 包类型标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RtcpPacketType {
    SenderReport = 200,
    ReceiverReport = 201,
    SourceDescription = 202,
    Bye = 203,
    App = 204,
}

impl RtcpPacketType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            200 => Some(Self::SenderReport),
            201 => Some(Self::ReceiverReport),
            202 => Some(Self::SourceDescription),
            203 => Some(Self::Bye),
            204 => Some(Self::App),
            _ => None,
        }
    }
}

/// One report block from an SR or RR packet.
///
/// SR/RR 包中的一个报告块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcpReportBlock {
    pub ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_lost: i32,
    pub highest_seq: u32,
    pub jitter: u32,
    pub last_sr: u32,
    pub delay_since_last_sr: u32,
}

impl RtcpReportBlock {
    /// Parse one 24-byte report block.
    pub fn parse(buf: &mut Bytes) -> Result<Self, RtcpParseError> {
        if buf.len() < 24 {
            return Err(RtcpParseError::TooShort);
        }
        let mut tmp = buf.split_to(24);
        let ssrc = tmp.get_u32();
        let fl_and_cl = tmp.get_u32();
        let fraction_lost = (fl_and_cl >> 24) as u8;
        let cumulative_lost = sign_extend_24(fl_and_cl & 0x00ff_ffff);
        let highest_seq = tmp.get_u32();
        let jitter = tmp.get_u32();
        let last_sr = tmp.get_u32();
        let delay_since_last_sr = tmp.get_u32();
        Ok(Self {
            ssrc,
            fraction_lost,
            cumulative_lost,
            highest_seq,
            jitter,
            last_sr,
            delay_since_last_sr,
        })
    }

    /// Encode this report block into 24 bytes.
    pub fn encode(&self, out: &mut BytesMut) {
        out.put_u32(self.ssrc);
        let cl = self.cumulative_lost as u32 & 0x00ff_ffff;
        out.put_u32((u32::from(self.fraction_lost) << 24) | cl);
        out.put_u32(self.highest_seq);
        out.put_u32(self.jitter);
        out.put_u32(self.last_sr);
        out.put_u32(self.delay_since_last_sr);
    }
}

/// RTCP Sender Report (PT=200).
///
/// RTCP 发送者报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSenderReport {
    pub ssrc: u32,
    pub ntp_timestamp: u64,
    pub rtp_timestamp: u32,
    pub packets_sent: u32,
    pub octets_sent: u32,
    pub report_blocks: Vec<RtcpReportBlock>,
}

impl RtcpSenderReport {
    pub fn encode(&self) -> Bytes {
        let rc = self.report_blocks.len().min(31) as u8;
        let total_len = 4 + 24 + self.report_blocks.len() * 24;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::SenderReport as u8);
        out.put_u16(length);
        out.put_u32(self.ssrc);
        out.put_u64(self.ntp_timestamp);
        out.put_u32(self.rtp_timestamp);
        out.put_u32(self.packets_sent);
        out.put_u32(self.octets_sent);
        for block in &self.report_blocks {
            block.encode(&mut out);
        }
        out.freeze()
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        if buf.len() < 24 {
            return Err(RtcpParseError::Truncated {
                pt: RtcpPacketType::SenderReport as u8,
            });
        }
        let mut header = buf.split_to(24);
        let ssrc = header.get_u32();
        let ntp_timestamp = header.get_u64();
        let rtp_timestamp = header.get_u32();
        let packets_sent = header.get_u32();
        let octets_sent = header.get_u32();
        let mut report_blocks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            report_blocks.push(RtcpReportBlock::parse(buf)?);
        }
        Ok(Self {
            ssrc,
            ntp_timestamp,
            rtp_timestamp,
            packets_sent,
            octets_sent,
            report_blocks,
        })
    }
}

/// RTCP Receiver Report (PT=201).
///
/// RTCP 接收者报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpReceiverReport {
    pub ssrc: u32,
    pub report_blocks: Vec<RtcpReportBlock>,
}

impl RtcpReceiverReport {
    pub fn encode(&self) -> Bytes {
        let rc = self.report_blocks.len().min(31) as u8;
        let total_len = 4 + 4 + self.report_blocks.len() * 24;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::ReceiverReport as u8);
        out.put_u16(length);
        out.put_u32(self.ssrc);
        for block in &self.report_blocks {
            block.encode(&mut out);
        }
        out.freeze()
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        if buf.len() < 4 {
            return Err(RtcpParseError::Truncated {
                pt: RtcpPacketType::ReceiverReport as u8,
            });
        }
        let ssrc = buf.get_u32();
        let mut report_blocks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            report_blocks.push(RtcpReportBlock::parse(buf)?);
        }
        Ok(Self {
            ssrc,
            report_blocks,
        })
    }
}

/// One item inside an SDES chunk.
///
/// SDES chunk 中的一条 item。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSdesItem {
    pub item_type: RtcpSdesItemType,
    pub text: String,
}

/// One SDES chunk (SSRC + items).
///
/// 一个 SDES chunk（SSRC + 条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSdesChunk {
    pub ssrc: u32,
    pub items: Vec<RtcpSdesItem>,
}

/// SDES item type identifiers (RFC 3550).
///
/// SDES 条目类型标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcpSdesItemType {
    End,
    CName,
    Name,
    Email,
    Phone,
    Location,
    Tool,
    Note,
    Priv,
    Unknown(u8),
}

impl RtcpSdesItemType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::End,
            1 => Self::CName,
            2 => Self::Name,
            3 => Self::Email,
            4 => Self::Phone,
            5 => Self::Location,
            6 => Self::Tool,
            7 => Self::Note,
            8 => Self::Priv,
            v => Self::Unknown(v),
        }
    }

    pub const fn as_u8(self) -> u8 {
        match self {
            Self::End => 0,
            Self::CName => 1,
            Self::Name => 2,
            Self::Email => 3,
            Self::Phone => 4,
            Self::Location => 5,
            Self::Tool => 6,
            Self::Note => 7,
            Self::Priv => 8,
            Self::Unknown(v) => v,
        }
    }
}

/// RTCP Source Description packet (PT=202).
///
/// RTCP 源描述包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSourceDescription {
    pub chunks: Vec<RtcpSdesChunk>,
}

impl RtcpSourceDescription {
    pub fn encode(&self) -> Bytes {
        let sc = self.chunks.len().min(31) as u8;
        let total_len = 4 + self.encoded_chunks_len();
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | sc);
        out.put_u8(RtcpPacketType::SourceDescription as u8);
        out.put_u16(length);
        for chunk in &self.chunks {
            out.put_u32(chunk.ssrc);
            for item in &chunk.items {
                out.put_u8(item.item_type.as_u8());
                out.put_u8(item.text.len() as u8);
                out.extend_from_slice(item.text.as_bytes());
            }
            // End-of-list marker and pad to 4-byte boundary within the chunk.
            let used = 1 + chunk.items.iter().map(|i| 2 + i.text.len()).sum::<usize>();
            let pad = padding_to_4(used);
            out.put_u8(0);
            for _ in 0..pad.saturating_sub(1) {
                out.put_u8(0);
            }
        }
        out.freeze()
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        let mut chunks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            if buf.len() < 4 {
                return Err(RtcpParseError::Truncated {
                    pt: RtcpPacketType::SourceDescription as u8,
                });
            }
            let ssrc = buf.get_u32();
            let mut items = Vec::new();
            let mut chunk_consumed = 4usize;
            loop {
                if buf.is_empty() {
                    return Err(RtcpParseError::InvalidSdes);
                }
                let item_type = buf.get_u8();
                chunk_consumed += 1;
                if item_type == 0 {
                    // End marker; consume padding until next 4-byte boundary.
                    let over = chunk_consumed % 4;
                    let pad = if over == 0 { 0 } else { 4 - over };
                    if buf.len() < pad {
                        return Err(RtcpParseError::InvalidSdes);
                    }
                    buf.advance(pad);
                    break;
                }
                if buf.is_empty() {
                    return Err(RtcpParseError::InvalidSdes);
                }
                let len = buf.get_u8() as usize;
                chunk_consumed += 1;
                if buf.len() < len {
                    return Err(RtcpParseError::InvalidSdes);
                }
                let text_bytes = buf.split_to(len);
                chunk_consumed += len;
                let text = String::from_utf8_lossy(&text_bytes).into_owned();
                items.push(RtcpSdesItem {
                    item_type: RtcpSdesItemType::from_u8(item_type),
                    text,
                });
            }
            chunks.push(RtcpSdesChunk { ssrc, items });
        }
        Ok(Self { chunks })
    }

    fn encoded_chunks_len(&self) -> usize {
        self.chunks
            .iter()
            .map(|c| {
                let items_len: usize = c.items.iter().map(|i| 2 + i.text.len()).sum();
                let used = 4 + items_len + 1;
                used + padding_to_4(used)
            })
            .sum()
    }
}

/// RTCP BYE packet (PT=203).
///
/// RTCP BYE 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpBye {
    pub ssrcs: Vec<u32>,
    pub reason: Option<String>,
}

impl RtcpBye {
    pub fn encode(&self) -> Bytes {
        let reason_len = self.reason.as_ref().map(|r| r.len()).unwrap_or(0);
        let rc = self.ssrcs.len().min(31) as u8;
        let reason_subfield = if reason_len > 0 {
            1 + reason_len + padding_to_4(1 + reason_len)
        } else {
            0
        };
        let total_len = 4 + self.ssrcs.len() * 4 + reason_subfield;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::Bye as u8);
        out.put_u16(length);
        for ssrc in &self.ssrcs {
            out.put_u32(*ssrc);
        }
        if reason_len > 0 {
            out.put_u8(reason_len as u8);
            out.extend_from_slice(self.reason.as_ref().unwrap().as_bytes());
            let pad = padding_to_4(1 + reason_len);
            for _ in 0..pad {
                out.put_u8(0);
            }
        }
        out.freeze()
    }

    pub fn parse(buf: &mut Bytes, count: u8) -> Result<Self, RtcpParseError> {
        let mut ssrcs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            if buf.len() < 4 {
                return Err(RtcpParseError::Truncated {
                    pt: RtcpPacketType::Bye as u8,
                });
            }
            ssrcs.push(buf.get_u32());
        }
        let reason = if !buf.is_empty() {
            let len = buf.get_u8() as usize;
            if buf.len() < len {
                return Err(RtcpParseError::Truncated {
                    pt: RtcpPacketType::Bye as u8,
                });
            }
            let text = buf.split_to(len);
            let pad = padding_to_4(1 + len);
            if buf.len() < pad {
                return Err(RtcpParseError::Truncated {
                    pt: RtcpPacketType::Bye as u8,
                });
            }
            buf.advance(pad);
            Some(String::from_utf8_lossy(&text).into_owned())
        } else {
            None
        };
        Ok(Self { ssrcs, reason })
    }
}

/// A single parsed RTCP packet.
///
/// 单个解析后的 RTCP 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcpPacket {
    SenderReport(RtcpSenderReport),
    ReceiverReport(RtcpReceiverReport),
    SourceDescription(RtcpSourceDescription),
    Bye(RtcpBye),
}

impl RtcpPacket {
    /// Encode this packet into its wire representation.
    pub fn encode(&self) -> Bytes {
        match self {
            Self::SenderReport(p) => p.encode(),
            Self::ReceiverReport(p) => p.encode(),
            Self::SourceDescription(p) => p.encode(),
            Self::Bye(p) => p.encode(),
        }
    }

    /// Parse one RTCP packet from a buffer. The header must already be consumed by the caller.
    fn parse_body(pt: u8, count: u8, buf: &mut Bytes) -> Result<Self, RtcpParseError> {
        match RtcpPacketType::from_u8(pt) {
            Some(RtcpPacketType::SenderReport) => {
                Ok(Self::SenderReport(RtcpSenderReport::parse(buf, count)?))
            }
            Some(RtcpPacketType::ReceiverReport) => {
                Ok(Self::ReceiverReport(RtcpReceiverReport::parse(buf, count)?))
            }
            Some(RtcpPacketType::SourceDescription) => Ok(Self::SourceDescription(
                RtcpSourceDescription::parse(buf, count)?,
            )),
            Some(RtcpPacketType::Bye) => Ok(Self::Bye(RtcpBye::parse(buf, count)?)),
            _ => Err(RtcpParseError::Truncated { pt }),
        }
    }
}

/// A compound RTCP packet consisting of one or more RTCP packets.
///
/// 一个或多个 RTCP 包组成的复合包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpCompoundPacket {
    pub packets: Vec<RtcpPacket>,
}

impl RtcpCompoundPacket {
    /// Parse a compound RTCP packet from bytes.
    pub fn parse(data: impl Into<Bytes>) -> Result<Self, RtcpParseError> {
        let mut buf = data.into();
        let mut packets = Vec::new();
        while !buf.is_empty() {
            if buf.len() < 4 {
                return Err(RtcpParseError::TooShort);
            }
            let byte0 = buf.get_u8();
            let version = byte0 >> 6;
            if version != 2 {
                return Err(RtcpParseError::InvalidVersion { version });
            }
            let _padding = (byte0 >> 5) & 0x01;
            let count = byte0 & 0x1f;
            let pt = buf.get_u8();
            let length = buf.get_u16() as usize * 4;
            if buf.len() < length {
                return Err(RtcpParseError::Truncated { pt });
            }
            let mut body = buf.split_to(length);
            packets.push(RtcpPacket::parse_body(pt, count, &mut body)?);
        }
        Ok(Self { packets })
    }

    /// Encode the compound packet as bytes.
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::new();
        for packet in &self.packets {
            out.extend_from_slice(&packet.encode());
        }
        out.freeze()
    }
}

fn sign_extend_24(value: u32) -> i32 {
    let v = value & 0x00ff_ffff;
    if (v & 0x0080_0000) != 0 {
        (v | 0xff00_0000) as i32
    } else {
        v as i32
    }
}

fn padding_to_4(len: usize) -> usize {
    let rem = len % 4;
    if rem == 0 {
        0
    } else {
        4 - rem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_sender_report_with_report_block() {
        let sr = RtcpSenderReport {
            ssrc: 0x12345678,
            ntp_timestamp: 0x1234567890abcdef,
            rtp_timestamp: 0xdeadbeef,
            packets_sent: 1000,
            octets_sent: 50000,
            report_blocks: vec![RtcpReportBlock {
                ssrc: 0x87654321,
                fraction_lost: 10,
                cumulative_lost: -5,
                highest_seq: 0x0001_ffff,
                jitter: 1234,
                last_sr: 0,
                delay_since_last_sr: 0,
            }],
        };
        let encoded = sr.encode();
        let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
        assert_eq!(decoded.packets.len(), 1);
        let RtcpPacket::SenderReport(parsed) = &decoded.packets[0] else {
            panic!("expected sender report");
        };
        assert_eq!(parsed.ssrc, sr.ssrc);
        assert_eq!(parsed.ntp_timestamp, sr.ntp_timestamp);
        assert_eq!(parsed.rtp_timestamp, sr.rtp_timestamp);
        assert_eq!(parsed.packets_sent, sr.packets_sent);
        assert_eq!(parsed.octets_sent, sr.octets_sent);
        assert_eq!(parsed.report_blocks, sr.report_blocks);
    }

    #[test]
    fn roundtrip_receiver_report() {
        let rr = RtcpReceiverReport {
            ssrc: 0x11111111,
            report_blocks: vec![RtcpReportBlock {
                ssrc: 0x22222222,
                fraction_lost: 0,
                cumulative_lost: 0,
                highest_seq: 0x0000_1234,
                jitter: 0,
                last_sr: 0x5555_5555,
                delay_since_last_sr: 0x6666_6666,
            }],
        };
        let encoded = rr.encode();
        let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
        assert_eq!(decoded.packets.len(), 1);
        let RtcpPacket::ReceiverReport(parsed) = &decoded.packets[0] else {
            panic!("expected receiver report");
        };
        assert_eq!(parsed.ssrc, rr.ssrc);
        assert_eq!(parsed.report_blocks, rr.report_blocks);
    }

    #[test]
    fn roundtrip_source_description() {
        let sdes = RtcpSourceDescription {
            chunks: vec![RtcpSdesChunk {
                ssrc: 0x33333333,
                items: vec![RtcpSdesItem {
                    item_type: RtcpSdesItemType::CName,
                    text: "user@host".to_string(),
                }],
            }],
        };
        let encoded = sdes.encode();
        let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
        assert_eq!(decoded.packets.len(), 1);
        let RtcpPacket::SourceDescription(parsed) = &decoded.packets[0] else {
            panic!("expected sdes");
        };
        assert_eq!(parsed.chunks.len(), 1);
        assert_eq!(parsed.chunks[0].ssrc, 0x33333333);
        assert_eq!(parsed.chunks[0].items.len(), 1);
        assert_eq!(parsed.chunks[0].items[0].item_type, RtcpSdesItemType::CName);
        assert_eq!(parsed.chunks[0].items[0].text, "user@host");
    }

    #[test]
    fn roundtrip_bye_with_reason() {
        let bye = RtcpBye {
            ssrcs: vec![0x44444444],
            reason: Some("gone".to_string()),
        };
        let encoded = bye.encode();
        let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
        assert_eq!(decoded.packets.len(), 1);
        let RtcpPacket::Bye(parsed) = &decoded.packets[0] else {
            panic!("expected bye");
        };
        assert_eq!(parsed.ssrcs, bye.ssrcs);
        assert_eq!(parsed.reason, bye.reason);
    }

    #[test]
    fn parses_compound_rr_plus_sdes_plus_bye() {
        let compound = RtcpCompoundPacket {
            packets: vec![
                RtcpPacket::ReceiverReport(RtcpReceiverReport {
                    ssrc: 0x11111111,
                    report_blocks: Vec::new(),
                }),
                RtcpPacket::SourceDescription(RtcpSourceDescription {
                    chunks: vec![RtcpSdesChunk {
                        ssrc: 0x11111111,
                        items: vec![RtcpSdesItem {
                            item_type: RtcpSdesItemType::CName,
                            text: "c".to_string(),
                        }],
                    }],
                }),
                RtcpPacket::Bye(RtcpBye {
                    ssrcs: vec![0x11111111],
                    reason: None,
                }),
            ],
        };
        let encoded = compound.encode();
        let parsed = RtcpCompoundPacket::parse(encoded).unwrap();
        assert_eq!(parsed, compound);
    }

    #[test]
    fn rejects_short_rtcp_packet() {
        // A minimal RR with no report blocks is 8 bytes: header + sender SSRC.
        assert!(RtcpCompoundPacket::parse(Bytes::from_static(&[
            0x80, 201, 0, 1, 0x11, 0x11, 0x11, 0x11
        ]))
        .is_ok());
        // A 2-byte packet cannot contain the common header.
        assert!(RtcpCompoundPacket::parse(Bytes::from_static(&[0x80, 201])).is_err());
    }
}
