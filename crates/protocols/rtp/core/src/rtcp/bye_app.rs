use super::{padding_to_4, RtcpEncodeError, RtcpPacketType, RtcpParseError};
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// RTCP BYE packet (PT=203).
///
/// RTCP BYE 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpBye {
    pub ssrcs: Vec<u32>,
    pub reason: Option<String>,
}

impl RtcpBye {
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let reason_len = self.reason.as_ref().map(|r| r.len()).unwrap_or(0);
        if reason_len > u8::MAX as usize {
            return Err(RtcpEncodeError::ByeReasonTooLong { length: reason_len });
        }
        let ssrc_count = self.ssrcs.len();
        if ssrc_count > 31 {
            return Err(RtcpEncodeError::TooManyByeSsrcs { count: ssrc_count });
        }
        let rc = ssrc_count as u8;
        let reason_subfield = if reason_len > 0 {
            1 + reason_len + padding_to_4(1 + reason_len)
        } else {
            0
        };
        let total_len = 4 + ssrc_count * 4 + reason_subfield;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | rc);
        out.put_u8(RtcpPacketType::Bye as u8);
        out.put_u16(length);
        for ssrc in &self.ssrcs {
            out.put_u32(*ssrc);
        }
        if let Some(reason) = self.reason.as_ref() {
            out.put_u8(reason_len as u8);
            out.extend_from_slice(reason.as_bytes());
            let pad = padding_to_4(1 + reason_len);
            for _ in 0..pad {
                out.put_u8(0);
            }
        }
        Ok(out.freeze())
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
            Some(String::from_utf8_lossy(&text).into_owned())
        } else {
            None
        };
        Ok(Self { ssrcs, reason })
    }
}

/// RTCP Application-Defined packet (PT=204).
///
/// RTCP 应用自定义包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpAppPacket {
    pub subtype: u8,
    pub ssrc: u32,
    pub name: [u8; 4],
    pub payload: Bytes,
}

impl RtcpAppPacket {
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let payload_len = self.payload.len();
        if !payload_len.is_multiple_of(4) {
            return Err(RtcpEncodeError::UnalignedPayload {
                length: payload_len,
            });
        }
        let total_len = 4 + 4 + 4 + payload_len;
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | (self.subtype & 0x1f));
        out.put_u8(RtcpPacketType::App as u8);
        out.put_u16(length);
        out.put_u32(self.ssrc);
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&self.payload);
        Ok(out.freeze())
    }

    pub fn parse(buf: &mut Bytes, subtype: u8) -> Result<Self, RtcpParseError> {
        if buf.len() < 8 {
            return Err(RtcpParseError::Truncated {
                pt: RtcpPacketType::App as u8,
            });
        }
        let ssrc = buf.get_u32();
        let mut name = [0u8; 4];
        name.copy_from_slice(&buf.split_to(4));
        let payload = buf.clone();
        Ok(Self {
            subtype,
            ssrc,
            name,
            payload,
        })
    }
}
