//! PES packet and PS packet parsing.
//!
//! PES 包与 PS 包解析。

use crate::prelude::*;
use crate::ps::{encode_pts_dts, find_start_code, parse_pts_dts, stream_kind};
use bytes::Bytes;

/// Stream kind classification for PES payloads inside a PS packet.
///
/// PS 包中 PES 负载的流类型分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsStreamKind {
    /// Video elementary stream.
    ///
    /// 视频基本流。
    Video,
    /// Audio elementary stream.
    ///
    /// 音频基本流。
    Audio,
    /// Private or other stream.
    ///
    /// 私有或其他流。
    Private,
}

/// A single Program Stream (PS) Packetized Elementary Stream (PES) packet.
///
/// 单个节目流（PS）的分组基本流（PES）包。
#[derive(Debug, Clone)]
pub struct PesPacket {
    /// PES stream ID (e.g. 0xE0 for video, 0xC0 for audio).
    ///
    /// PES 流 ID（例如 0xE0 表示视频，0xC0 表示音频）。
    pub stream_id: u8,
    /// Classified stream kind.
    ///
    /// 已分类的流类型。
    pub kind: PsStreamKind,
    /// Presentation timestamp in 90 kHz ticks, if present.
    ///
    /// 显示时间戳（90kHz  ticks），若存在。
    pub pts: Option<i64>,
    /// Decode timestamp in 90 kHz ticks, if present.
    ///
    /// 解码时间戳（90kHz ticks），若存在。
    pub dts: Option<i64>,
    /// PES payload bytes.
    ///
    /// PES 负载字节。
    pub payload: Bytes,
}

impl PesPacket {
    /// Parse a single PES packet from a raw byte slice.
    ///
    /// Returns the parsed packet and the number of bytes consumed.
    ///
    /// 从原始字节切片解析单个 PES 包。
    /// 返回解析后的包及消耗的字节数。
    pub fn parse(raw: &[u8]) -> Option<(Self, usize)> {
        if raw.len() < 9 {
            return None;
        }
        if raw[0..3] != [0x00, 0x00, 0x01] {
            return None;
        }

        let stream_id = raw[3];
        let pes_len = u16::from_be_bytes([raw[4], raw[5]]) as usize;
        let flags2 = raw[7];
        let header_len = raw[8] as usize;
        let data_start = 9 + header_len;
        if raw.len() < data_start {
            return None;
        }

        let total_len = if pes_len == 0 {
            raw.len()
        } else {
            (6 + pes_len).min(raw.len())
        };
        if total_len < data_start {
            return None;
        }

        let mut cursor = 9usize;
        let mut pts = None;
        let mut dts = None;
        let has_pts = (flags2 & 0x80) != 0;
        let has_dts = (flags2 & 0x40) != 0;
        if has_pts && header_len >= 5 && raw.len() >= cursor + 5 {
            pts = parse_pts_dts(&raw[cursor..cursor + 5]);
            cursor += 5;
        }
        if has_dts && header_len >= 10 && raw.len() >= cursor + 5 {
            dts = parse_pts_dts(&raw[cursor..cursor + 5]);
        }

        let payload = Bytes::copy_from_slice(&raw[data_start..total_len]);
        Some((
            Self {
                stream_id,
                kind: stream_kind(stream_id),
                pts,
                dts,
                payload,
            },
            total_len,
        ))
    }

    /// Encode this PES/PS packet back into raw bytes.
    ///
    /// 将此 PES/PS 包重新编码为原始字节。
    pub fn encode(&self) -> Bytes {
        let mut header_data = Vec::new();
        let mut flags2 = 0u8;
        if let Some(pts) = self.pts {
            flags2 |= 0x80;
            header_data.extend_from_slice(&encode_pts_dts(pts, 0x2));
        }
        if let Some(dts) = self.dts {
            flags2 |= 0x40;
            header_data.extend_from_slice(&encode_pts_dts(dts, 0x1));
        }

        let pes_len = (3 + header_data.len() + self.payload.len()).min(u16::MAX as usize) as u16;
        let mut out = Vec::with_capacity(6 + pes_len as usize);
        out.extend_from_slice(&[0x00, 0x00, 0x01, self.stream_id]);
        out.extend_from_slice(&pes_len.to_be_bytes());
        out.push(0x80);
        out.push(flags2);
        out.push(header_data.len() as u8);
        out.extend_from_slice(&header_data);
        out.extend_from_slice(&self.payload);
        Bytes::from(out)
    }
}

/// A Program Stream (PS) packet containing one or more PES packets.
///
/// 包含一个或多个 PES 包的节目流（PS）包。
#[derive(Debug, Clone)]
pub struct PsPacket {
    /// Contained PES packets.
    ///
    /// 包含的 PES 包列表。
    pub pes: Vec<PesPacket>,
}

impl PsPacket {
    /// Parse all PES packets from a raw PS packet.
    ///
    /// 从原始 PS 包中解析出所有 PES 包。
    pub fn parse(raw: &[u8]) -> Self {
        Self::parse_bounded(raw, raw.len(), usize::MAX)
    }

    /// Parse PES packets with bounds on total bytes and packet count.
    ///
    /// 在总字节数和包数限制下解析 PES 包。
    pub fn parse_bounded(raw: &[u8], max_bytes: usize, max_pes: usize) -> Self {
        if max_bytes == 0 || max_pes == 0 {
            return Self { pes: Vec::new() };
        }

        let mut raw = &raw[..raw.len().min(max_bytes)];
        let mut pes = Vec::new();
        while raw.len() >= 9 && pes.len() < max_pes {
            let Some(start) = find_start_code(raw) else {
                break;
            };
            raw = &raw[start..];
            if let Some((packet, consumed)) = PesPacket::parse(raw) {
                pes.push(packet);
                raw = &raw[consumed..];
            } else {
                raw = &raw[3..];
            }
        }
        Self { pes }
    }

    /// Encode this PES/PS packet back into raw bytes.
    ///
    /// 将此 PES/PS 包重新编码为原始字节。
    pub fn encode(&self) -> Bytes {
        let total = self.pes.iter().map(|p| p.payload.len() + 32).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        for pes in &self.pes {
            out.extend_from_slice(&pes.encode());
        }
        Bytes::from(out)
    }
}
