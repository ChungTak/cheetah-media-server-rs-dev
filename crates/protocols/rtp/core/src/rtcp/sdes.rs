use super::{padding_to_4, RtcpEncodeError, RtcpPacketType, RtcpParseError};
use bytes::{Buf, BufMut, Bytes, BytesMut};

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
    pub fn encode(&self) -> Result<Bytes, RtcpEncodeError> {
        let chunk_count = self.chunks.len();
        if chunk_count > 31 {
            return Err(RtcpEncodeError::TooManySdesChunks { count: chunk_count });
        }
        let sc = chunk_count as u8;
        let total_len = 4 + self.encoded_chunks_len();
        let length = ((total_len / 4) - 1) as u16;
        let mut out = BytesMut::with_capacity(total_len);
        out.put_u8(0x80 | sc);
        out.put_u8(RtcpPacketType::SourceDescription as u8);
        out.put_u16(length);
        for chunk in &self.chunks {
            out.put_u32(chunk.ssrc);
            for item in &chunk.items {
                let text_len = item.text.len();
                if text_len > u8::MAX as usize {
                    return Err(RtcpEncodeError::SdesItemTooLong { length: text_len });
                }
                out.put_u8(item.item_type.as_u8());
                out.put_u8(text_len as u8);
                out.extend_from_slice(item.text.as_bytes());
            }
            // End-of-list marker and pad to 4-byte boundary within the chunk.
            let used = 1 + chunk.items.iter().map(|i| 2 + i.text.len()).sum::<usize>();
            let pad = padding_to_4(used);
            out.put_u8(0);
            for _ in 0..pad {
                out.put_u8(0);
            }
        }
        Ok(out.freeze())
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
