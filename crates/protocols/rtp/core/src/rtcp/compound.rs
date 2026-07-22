use super::{
    padding_to_4, RtcpAppPacket, RtcpBye, RtcpEncodeError, RtcpPacketType, RtcpParseError,
    RtcpReceiverReport, RtcpSenderReport, RtcpSourceDescription,
};
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// A single parsed RTCP packet.
///
/// 单个解析后的 RTCP 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcpPacket {
    SenderReport(RtcpSenderReport),
    ReceiverReport(RtcpReceiverReport),
    SourceDescription(RtcpSourceDescription),
    Bye(RtcpBye),
    App(RtcpAppPacket),
    Unknown { pt: u8, count: u8, payload: Bytes },
}

impl RtcpPacket {
    /// Encode this packet into its wire representation.
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        match self {
            Self::SenderReport(p) => p.encode(),
            Self::ReceiverReport(p) => p.encode(),
            Self::SourceDescription(p) => p.encode(),
            Self::Bye(p) => p.encode(),
            Self::App(p) => p.encode(),
            Self::Unknown { pt, count, payload } => {
                let payload_len = payload.len();
                let total_len = 4 + payload_len + padding_to_4(payload_len);
                let length = ((total_len / 4) - 1) as u16;
                let mut out = BytesMut::with_capacity(total_len);
                out.put_u8(0x80 | (*count & 0x1f));
                out.put_u8(*pt);
                out.put_u16(length);
                out.extend_from_slice(payload);
                let pad = padding_to_4(payload_len);
                for _ in 0..pad {
                    out.put_u8(0);
                }
                Ok(out.freeze())
            }
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
            Some(RtcpPacketType::App) => Ok(Self::App(RtcpAppPacket::parse(buf, count)?)),
            None => Ok(Self::Unknown {
                pt,
                count,
                payload: buf.clone(),
            }),
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
            let padding = (byte0 >> 5) & 0x01;
            let count = byte0 & 0x1f;
            let pt = buf.get_u8();
            let length = buf.get_u16() as usize * 4;
            if buf.len() < length {
                return Err(RtcpParseError::Truncated { pt });
            }
            let mut body = buf.split_to(length);
            if padding != 0 {
                if body.is_empty() {
                    return Err(RtcpParseError::InvalidPadding { pt, count: 0 });
                }
                let pad_count = body[body.len() - 1];
                if pad_count == 0 || pad_count as usize > body.len() {
                    return Err(RtcpParseError::InvalidPadding {
                        pt,
                        count: pad_count,
                    });
                }
                let trimmed_len = body.len() - pad_count as usize;
                body = body.split_to(trimmed_len);
            }
            packets.push(RtcpPacket::parse_body(pt, count, &mut body)?);
        }
        Ok(Self { packets })
    }

    /// Encode the compound packet as bytes.
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let mut out = BytesMut::new();
        for packet in &self.packets {
            out.extend_from_slice(&packet.encode()?);
        }
        Ok(out.freeze())
    }
}
