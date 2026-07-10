use crate::prelude::*;
use bytes::{Buf, Bytes, BytesMut};

/// RTP fixed header parsed from / serialized to the wire format in RFC 3550.
///
/// All numeric fields are stored in host byte order after parsing so callers can
/// compare and manipulate them without repeated `to_be`/`from_be` conversions.
///
/// 从 RFC 3550 线格式解析/序列化后的 RTP 固定头。
///
/// 所有数字字段在解析后均以主机字节序存储，调用方无需反复进行
/// `to_be`/`from_be` 转换即可比较和操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub version: u8,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
}

impl RtpHeader {
    /// Parses a 12-byte RTP base header plus optional CSRC list and extension.
    ///
    /// Returns the parsed header and the byte offset where the payload begins.
    /// The function validates version, padding, extension length and CSRC count
    /// in one pass, returning `None` for any malformed or truncated input.
    ///
    /// 解析 12 字节 RTP 基础头以及可选的 CSRC 列表和扩展头。
    ///
    /// 返回解析后的头和有效负载起始的字节偏移量。
    /// 该函数一次性校验版本、填充、扩展长度和 CSRC 数量，
    /// 任何格式错误或截断的输入都会返回 `None`。
    pub fn parse(raw: &[u8]) -> Option<(Self, usize)> {
        if raw.len() < 12 {
            return None;
        }

        let version = raw[0] >> 6;
        let has_padding = (raw[0] & 0x20) != 0;
        let has_extension = (raw[0] & 0x10) != 0;
        let csrc_count = (raw[0] & 0x0f) as usize;
        let marker = (raw[1] & 0x80) != 0;
        let payload_type = raw[1] & 0x7f;
        let sequence_number = u16::from_be_bytes([raw[2], raw[3]]);
        let timestamp = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let ssrc = u32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]);

        let mut offset = 12 + csrc_count * 4;
        if raw.len() < offset {
            return None;
        }

        if has_extension {
            if raw.len() < offset + 4 {
                return None;
            }
            let ext_len_words = u16::from_be_bytes([raw[offset + 2], raw[offset + 3]]) as usize;
            offset += 4 + ext_len_words * 4;
            if raw.len() < offset {
                return None;
            }
        }

        if has_padding && raw.len() <= offset {
            return None;
        }

        Some((
            Self {
                version,
                payload_type,
                sequence_number,
                timestamp,
                ssrc,
                marker,
            },
            offset,
        ))
    }

    /// Serializes the header into a 12-byte big-endian RTP header.
    ///
    /// Marker bit and payload type are packed into the second byte; sequence
    /// number, timestamp and SSRC are written as big-endian integers.
    ///
    /// 将头序列化为 12 字节大端序 RTP 头。
    ///
    /// 标记位和负载类型被打包进第二个字节；序列号、时间戳和 SSRC
    /// 以大端整数写入。
    pub fn encode(self) -> [u8; 12] {
        let mut out = [0u8; 12];
        out[0] = (self.version & 0x03) << 6;
        out[1] = (u8::from(self.marker) << 7) | (self.payload_type & 0x7f);
        out[2..4].copy_from_slice(&self.sequence_number.to_be_bytes());
        out[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        out[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        out
    }
}

/// A complete RTP packet composed of a parsed header and a payload byte buffer.
///
/// This is the canonical wire representation used by the codec layer; protocol
/// cores are expected to produce or consume this type after framing is removed.
///
/// 由解析后的头和负载字节缓冲区组成的完整 RTP 包。
///
/// 这是 codec 层的标准线表示；协议核心在移除分帧后应生成或消费该类型。
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Bytes,
}

impl RtpPacket {
    /// Parses a full RTP packet, separating header from payload.
    ///
    /// Uses `RtpHeader::parse` for the header, then strips any padding bytes
    /// indicated by the padding flag before copying the payload. Returns `None`
    /// if the header is malformed or the payload length is inconsistent.
    ///
    /// 解析完整的 RTP 包，将头与负载分离。
    ///
    /// 使用 `RtpHeader::parse` 解析头，然后根据填充标志剥除填充字节，
    /// 再拷贝负载。如果头格式错误或负载长度不一致则返回 `None`。
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let (header, header_len) = RtpHeader::parse(raw)?;
        let payload_end = if (raw[0] & 0x20) != 0 {
            let pad_len = *raw.last()? as usize;
            raw.len().checked_sub(pad_len)?
        } else {
            raw.len()
        };
        if payload_end < header_len {
            return None;
        }
        Some(Self {
            header,
            payload: Bytes::copy_from_slice(&raw[header_len..payload_end]),
        })
    }

    /// Serializes the packet by concatenating the encoded header and payload.
    ///
    /// Allocates a fresh buffer sized to `12 + payload.len()` and writes the
    /// big-endian header followed by the raw payload bytes.
    ///
    /// 将编码后的头与负载拼接，序列化整个包。
    ///
    /// 分配大小为 `12 + payload.len()` 的新缓冲区，写入大端头后跟原始负载字节。
    pub fn encode(&self) -> Bytes {
        let mut out = Vec::with_capacity(12 + self.payload.len());
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&self.payload);
        Bytes::from(out)
    }
}

/// RTP clock-rate helper for timestamp unit conversion.
///
/// RTP timestamps are expressed in clock ticks whose frequency depends on the
/// codec (e.g. 90000 Hz for video, 8000 Hz for G.711). This type converts
/// between those ticks and normalized microseconds without losing precision.
///
/// 用于时间戳单位转换的 RTP 时钟速率辅助结构。
///
/// RTP 时间戳以时钟滴答表示，频率取决于编解码器（如视频 90000 Hz、
/// G.711 8000 Hz）。该类型在这些滴答与归一化微秒之间进行高精度转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpClock {
    pub rate: u32,
}

impl RtpClock {
    /// Converts RTP clock ticks to microseconds using the configured rate.
    ///
    /// Returns 0 when the rate is uninitialized to avoid divide-by-zero and to
    /// signal that no timing information is available yet.
    ///
    /// 使用配置的速率将 RTP 时钟滴答转换为微秒。
    ///
    /// 当速率未初始化时返回 0，避免除零并表明尚无可用时间信息。
    pub fn ticks_to_micros(self, ticks: u32) -> i64 {
        if self.rate == 0 {
            return 0;
        }
        (i64::from(ticks) * 1_000_000_i64) / i64::from(self.rate)
    }

    /// Converts microseconds back to RTP clock ticks.
    ///
    /// Uses 128-bit intermediate arithmetic to prevent overflow when the clock
    /// rate is high, and returns 0 for zero/negative input or uninitialized rate.
    ///
    /// 将微秒转换回 RTP 时钟滴答。
    ///
    /// 使用 128 位中间运算防止时钟速率高时溢出，对零/负输入或未初始化速率返回 0。
    pub fn micros_to_ticks(self, micros: i64) -> u32 {
        if self.rate == 0 || micros <= 0 {
            return 0;
        }
        ((micros as i128 * self.rate as i128) / 1_000_000_i128) as u32
    }
}

/// Splits a raw payload into a sequence of `RtpPacket`s respecting an MTU.
///
/// The 12-byte RTP header size is subtracted from `mtu`, then the payload is
/// consumed in chunks. Each chunk gets a copy of `header` with an incremented
/// sequence number; the marker bit is set only on the final packet.
///
/// 将原始负载按 MTU 拆分为一系列 `RtpPacket`。
///
/// 从 `mtu` 中减去 12 字节 RTP 头大小，然后分块消费负载。
/// 每块获得 `header` 的副本并递增序列号；仅在最后一个包上设置标记位。
pub fn packetize_payload(payload: &[u8], mtu: usize, mut header: RtpHeader) -> Vec<RtpPacket> {
    let payload_mtu = mtu.saturating_sub(12).max(1);
    let mut out = Vec::new();
    let mut seq = header.sequence_number;
    let mut cursor = payload;
    while !cursor.is_empty() {
        let take = cursor.len().min(payload_mtu);
        let chunk = &cursor[..take];
        cursor = &cursor[take..];
        header.sequence_number = seq;
        header.marker = cursor.is_empty();
        out.push(RtpPacket {
            header,
            payload: Bytes::copy_from_slice(chunk),
        });
        seq = seq.wrapping_add(1);
    }
    out
}

/// Packetize a G711 audio frame into RTP packets respecting a target packet
/// duration in milliseconds. ZLMediaKit aligns G711 to 100ms by default for
/// GB28181 interop; smaller values (e.g. 20ms) work better over WebRTC bridges.
///
/// `sample_rate` is the audio sample rate (typically 8000 Hz for G711).
/// `header.timestamp` is the starting RTP timestamp; it advances by samples-per-packet.
/// `header.sequence_number` is the starting sequence number; it advances by 1 per packet.
pub fn packetize_g711(
    payload: &[u8],
    sample_rate: u32,
    packet_duration_ms: u32,
    mut header: RtpHeader,
) -> Vec<RtpPacket> {
    if payload.is_empty() {
        return Vec::new();
    }
    // 1 byte == 1 sample at the given sample_rate. Cap to 1400-byte MTU as a sanity
    // bound so a misconfigured 10s packet does not exceed UDP datagram limits.
    let dur_ms = packet_duration_ms.max(1);
    let samples_per_packet = ((sample_rate as u64) * dur_ms as u64 / 1000) as usize;
    let chunk_bytes = samples_per_packet.clamp(1, 1400);

    let mut out = Vec::with_capacity(payload.len().div_ceil(chunk_bytes));
    let mut cursor = payload;
    let mut seq = header.sequence_number;
    let mut ts = header.timestamp;
    while !cursor.is_empty() {
        let take = cursor.len().min(chunk_bytes);
        let chunk = &cursor[..take];
        cursor = &cursor[take..];
        header.sequence_number = seq;
        header.timestamp = ts;
        header.marker = false;
        out.push(RtpPacket {
            header,
            payload: Bytes::copy_from_slice(chunk),
        });
        seq = seq.wrapping_add(1);
        ts = ts.wrapping_add(take as u32);
    }
    out
}

/// Reassembles a fragmented RTP payload into a single contiguous byte buffer.
///
/// Sorts packets by sequence number, detects the largest sequence gap to drop
/// stale leading packets, handles 16-bit sequence wrap-around, then concatenates
/// the payloads. Returns an empty `Bytes` if the input vector is empty.
///
/// 将分片的 RTP 负载重组为单一连续字节缓冲区。
///
/// 按序列号排序包，检测最大序列间隙以丢弃陈旧的前导包，
/// 处理 16 位序列号回绕，然后拼接负载。输入为空时返回空 `Bytes`。
pub fn depacketize_payload(mut packets: Vec<RtpPacket>) -> Bytes {
    if packets.is_empty() {
        return Bytes::new();
    }
    packets.sort_by_key(|pkt| pkt.header.sequence_number);
    if packets.len() > 1 {
        let mut largest_gap = 0u32;
        let mut split_after = packets.len() - 1;

        for idx in 0..(packets.len() - 1) {
            let curr = packets[idx].header.sequence_number;
            let next = packets[idx + 1].header.sequence_number;
            let gap = u32::from(next.wrapping_sub(curr));
            if gap > largest_gap {
                largest_gap = gap;
                split_after = idx;
            }
        }

        let first = u32::from(packets[0].header.sequence_number);
        let last = u32::from(packets[packets.len() - 1].header.sequence_number);
        let wrap_gap = (u32::from(u16::MAX) + 1 + first).saturating_sub(last);
        if wrap_gap > largest_gap {
            split_after = packets.len() - 1;
        }

        let start = (split_after + 1) % packets.len();
        if start != 0 {
            packets.rotate_left(start);
        }
    }

    let total = packets.iter().map(|pkt| pkt.payload.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    for pkt in packets {
        out.extend_from_slice(&pkt.payload);
    }
    Bytes::from(out)
}

/// Identifies the encapsulation mode carried inside an RTP payload.
///
/// RTP does not mandate a payload format; the same PT can carry PS/TS/ES, Ehome,
/// JT/T 1078 or raw audio/video elementary streams. This enum lets the demuxer
/// choose the right parser after probing the first few bytes.
///
/// 标识 RTP 负载内部采用的封装模式。
///
/// RTP 不强制负载格式；同一 PT 可能承载 PS/TS/ES、Ehome、JT/T 1078
/// 或原始音视频基本流。该枚举让解复用器在探测前几个字节后选择正确解析器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpPayloadMode {
    Ps,
    Ts,
    Es,
    Ehome,
    /// Hikvision XHB private container (also seen as `xhb`/`hk` in vendor stacks).
    Xhb,
    /// JT/T 1078 vehicle terminal video transport.
    Jtt1078,
    RawAudio,
    RawVideo,
    Unknown,
}

/// Probes the first bytes of an RTP payload to select the encapsulation mode.
///
/// Looks for JT/T 1078 magic, MPEG-PS start code, MPEG-TS sync byte, Ehome,
/// raw H26x NAL start code, or raw audio/video elementary streams. Falls back
/// to `Unknown` when the payload is empty or does not match any known signature.
///
/// 探测 RTP 负载的前几个字节以选择封装模式。
///
/// 查找 JT/T 1078 魔数、MPEG-PS 起始码、MPEG-TS 同步字节、Ehome、
/// 原始 H26x NAL 起始码或原始音视频基本流。当负载为空或不匹配任何已知签名时
/// 回退到 `Unknown`。
pub fn probe_rtp_payload(payload: &[u8]) -> RtpPayloadMode {
    if payload.is_empty() {
        return RtpPayloadMode::Unknown;
    }

    // JT/T 1078 magic header: "0x30 0x31 0x63 0x64" ("01cd"). The header is 26 bytes (2013/2016)
    // or 30 bytes (2019); we only need the magic to differentiate it from raw RTP.
    if payload.len() >= 4
        && payload[0] == 0x30
        && payload[1] == 0x31
        && payload[2] == 0x63
        && payload[3] == 0x64
    {
        return RtpPayloadMode::Jtt1078;
    }

    // Ehome2 signature: 256-byte header starting with [0x01, 0x00, 0x01] or [0x01, 0x00, 0x02]
    if payload.len() >= 256
        && payload[0] == 0x01
        && payload[1] == 0x00
        && (payload[2] == 0x01 || payload[2] == 0x02)
    {
        return RtpPayloadMode::Ehome;
    }

    // PS: starts with pack start code 0x000001BA
    if payload.len() >= 4
        && payload[0] == 0x00
        && payload[1] == 0x00
        && payload[2] == 0x01
        && payload[3] == 0xBA
    {
        return RtpPayloadMode::Ps;
    }

    // TS: starts with 0x47 and ideally has another 0x47 at +188
    if payload[0] == 0x47 {
        if payload.len() >= 376 && payload[188] == 0x47 {
            return RtpPayloadMode::Ts;
        }
        // Single TS packet or less - still likely TS if starts with sync
        if payload.len() <= 188 {
            return RtpPayloadMode::Ts;
        }
        // Multiple packets but second doesn't start with 0x47 - check if aligned
        if payload.len().is_multiple_of(188) {
            return RtpPayloadMode::Ts;
        }
    }

    // Try to find 0x47 with vendor prefix
    if let Some(pos) = payload.iter().position(|&b| b == 0x47) {
        if pos < 12 && pos + 188 <= payload.len() {
            return RtpPayloadMode::Ts;
        }
    }

    RtpPayloadMode::Unknown
}

/// Parses an RFC 4571 TCP RTP frame (2-byte big-endian length prefix + RTP packet).
/// Returns the parsed RTP packet and the total number of bytes consumed.
pub fn parse_tcp_rtp_frame(raw: &[u8]) -> Option<(RtpPacket, usize)> {
    if raw.len() < 2 {
        return None;
    }
    let len = u16::from_be_bytes([raw[0], raw[1]]) as usize;
    if raw.len() < 2 + len {
        return None;
    }
    let packet = RtpPacket::parse(&raw[2..2 + len])?;
    Some((packet, 2 + len))
}

/// Encodes an RTP packet as an RFC 4571 TCP RTP frame (2-byte big-endian length prefix + RTP packet).
pub fn encode_tcp_rtp_frame(packet: &RtpPacket) -> Bytes {
    let encoded = packet.encode();
    let len = encoded.len();
    let mut out = Vec::with_capacity(2 + len);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&encoded);
    Bytes::from(out)
}

/// TCP RTP framing variants observed across vendor stacks.
///
/// `TwoByte` is the RFC 4571 framing (2-byte big-endian length, then RTP packet).
///
/// `Interleaved4Byte` is RTSP-style RTP-over-TCP framing (`$ + channel + 2-byte length + RTP packet`).
/// ABLMediaServer / Hikvision sub-platforms occasionally negotiate this variant when carrying
/// GB28181 RTP over a TCP control channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTcpFraming {
    /// RFC 4571 — 2-byte big-endian length prefix + RTP packet.
    TwoByte,
    /// RTSP-style interleaved framing — `$ + channel(u8) + length(u16 BE) + RTP`.
    Interleaved4Byte,
    /// Auto-detect: try `Interleaved4Byte` first when the leading byte is `$`, otherwise
    /// fall back to `TwoByte`.
    AutoDetect,
}

/// Parse a 4-byte interleaved TCP RTP frame (`$ + channel + length(u16) + payload`).
///
/// Returns the parsed RTP packet, the channel number, and the total number of bytes consumed.
pub fn parse_interleaved_rtp_frame(raw: &[u8]) -> Option<(RtpPacket, u8, usize)> {
    if raw.len() < 4 {
        return None;
    }
    if raw[0] != b'$' {
        return None;
    }
    let channel = raw[1];
    let len = u16::from_be_bytes([raw[2], raw[3]]) as usize;
    if raw.len() < 4 + len {
        return None;
    }
    let packet = RtpPacket::parse(&raw[4..4 + len])?;
    Some((packet, channel, 4 + len))
}

/// Encode an RTP packet using 4-byte interleaved framing.
pub fn encode_interleaved_rtp_frame(packet: &RtpPacket, channel: u8) -> Bytes {
    let encoded = packet.encode();
    let len = encoded.len();
    let mut out = Vec::with_capacity(4 + len);
    out.push(b'$');
    out.push(channel);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&encoded);
    Bytes::from(out)
}

/// Result of parsing a single TCP RTP frame in `AutoDetect` mode.
#[derive(Debug, Clone)]
pub struct ParsedTcpRtpFrame {
    pub packet: RtpPacket,
    /// Number of bytes consumed from the input buffer.
    pub consumed: usize,
    /// `Some(channel)` for `Interleaved4Byte` framing, `None` for `TwoByte`.
    pub channel: Option<u8>,
    pub framing: RtpTcpFraming,
}

/// Attempt to parse a single TCP RTP frame from `raw` using the configured framing mode.
///
/// `AutoDetect` prefers the interleaved variant when the buffer starts with `$`. If that fails
/// it falls back to the 2-byte length form. Returns `None` when more bytes are needed.
pub fn parse_tcp_rtp_frame_with(raw: &[u8], mode: RtpTcpFraming) -> Option<ParsedTcpRtpFrame> {
    match mode {
        RtpTcpFraming::TwoByte => {
            let (packet, consumed) = parse_tcp_rtp_frame(raw)?;
            Some(ParsedTcpRtpFrame {
                packet,
                consumed,
                channel: None,
                framing: RtpTcpFraming::TwoByte,
            })
        }
        RtpTcpFraming::Interleaved4Byte => {
            let (packet, channel, consumed) = parse_interleaved_rtp_frame(raw)?;
            Some(ParsedTcpRtpFrame {
                packet,
                consumed,
                channel: Some(channel),
                framing: RtpTcpFraming::Interleaved4Byte,
            })
        }
        RtpTcpFraming::AutoDetect => {
            if raw.first().copied() == Some(b'$') {
                if let Some((packet, channel, consumed)) = parse_interleaved_rtp_frame(raw) {
                    return Some(ParsedTcpRtpFrame {
                        packet,
                        consumed,
                        channel: Some(channel),
                        framing: RtpTcpFraming::Interleaved4Byte,
                    });
                }
            }
            let (packet, consumed) = parse_tcp_rtp_frame(raw)?;
            Some(ParsedTcpRtpFrame {
                packet,
                consumed,
                channel: None,
                framing: RtpTcpFraming::TwoByte,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4571_tcp_framing_roundtrip() {
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 12,
                timestamp: 42,
                ssrc: 1234,
                marker: true,
            },
            payload: Bytes::from_static(b"testpayload"),
        };
        let encoded = encode_tcp_rtp_frame(&packet);
        assert!(encoded.len() >= 14);
        let len = u16::from_be_bytes([encoded[0], encoded[1]]) as usize;
        assert_eq!(len, encoded.len() - 2);

        let (decoded, consumed) = parse_tcp_rtp_frame(&encoded).expect("parse tcp rtp frame");
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.header, packet.header);
        assert_eq!(decoded.payload, packet.payload);
    }

    #[test]
    fn rtsp_interleaved_4byte_framing_roundtrip() {
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 7,
                timestamp: 42,
                ssrc: 0xCAFEBABE,
                marker: false,
            },
            payload: Bytes::from_static(b"interleaved-payload"),
        };
        let encoded = encode_interleaved_rtp_frame(&packet, 0);
        assert_eq!(encoded[0], b'$');
        assert_eq!(encoded[1], 0);
        let (decoded, channel, consumed) =
            parse_interleaved_rtp_frame(&encoded).expect("interleaved parse");
        assert_eq!(channel, 0);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.header, packet.header);
        assert_eq!(decoded.payload, packet.payload);
    }

    #[test]
    fn auto_detect_prefers_interleaved_when_dollar_prefix_present() {
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 7,
                timestamp: 42,
                ssrc: 1,
                marker: false,
            },
            payload: Bytes::from_static(b"x"),
        };
        let interleaved = encode_interleaved_rtp_frame(&packet, 2);
        let parsed = parse_tcp_rtp_frame_with(&interleaved, RtpTcpFraming::AutoDetect).unwrap();
        assert_eq!(parsed.framing, RtpTcpFraming::Interleaved4Byte);
        assert_eq!(parsed.channel, Some(2));

        let two_byte = encode_tcp_rtp_frame(&packet);
        let parsed = parse_tcp_rtp_frame_with(&two_byte, RtpTcpFraming::AutoDetect).unwrap();
        assert_eq!(parsed.framing, RtpTcpFraming::TwoByte);
        assert!(parsed.channel.is_none());
    }

    #[test]
    fn test_probe_rtp_payload() {
        let ts_payload = [0x47; 376];
        assert_eq!(probe_rtp_payload(&ts_payload), RtpPayloadMode::Ts);

        let ps_payload = [0x00, 0x00, 0x01, 0xBA, 0x00, 0x00];
        assert_eq!(probe_rtp_payload(&ps_payload), RtpPayloadMode::Ps);

        let unknown_payload = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(probe_rtp_payload(&unknown_payload), RtpPayloadMode::Unknown);

        // JT/T 1078 magic prefix
        let jtt1078_payload = [0x30, 0x31, 0x63, 0x64, 0x00, 0x00];
        assert_eq!(probe_rtp_payload(&jtt1078_payload), RtpPayloadMode::Jtt1078);
    }

    #[test]
    fn header_roundtrip() {
        let header = RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 12,
            timestamp: 42,
            ssrc: 1234,
            marker: true,
        };
        let encoded = header.encode();
        let (decoded, offset) = RtpHeader::parse(&encoded).expect("header parse");
        assert_eq!(decoded, header);
        assert_eq!(offset, 12);
    }

    #[test]
    fn packet_roundtrip() {
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 111,
                sequence_number: 7,
                timestamp: 9000,
                ssrc: 100,
                marker: false,
            },
            payload: Bytes::from_static(b"opus"),
        };
        let encoded = packet.encode();
        let decoded = RtpPacket::parse(&encoded).expect("rtp parse");
        assert_eq!(decoded.header, packet.header);
        assert_eq!(decoded.payload, packet.payload);
    }

    #[test]
    fn packetize_and_depacketize() {
        let payload = vec![0x11; 100];
        let packets = packetize_payload(
            &payload,
            40,
            RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 200,
                timestamp: 123,
                ssrc: 1,
                marker: false,
            },
        );
        assert!(packets.len() > 1);
        let rebuilt = depacketize_payload(packets);
        assert_eq!(rebuilt, Bytes::from(payload));
    }

    #[test]
    fn packetize_g711_default_100ms_at_8khz_emits_800_byte_chunks() {
        // 800 sample-bytes per packet at 100ms / 8kHz; 1700 byte input -> 3 packets
        let payload = vec![0xA5; 1700];
        let header = RtpHeader {
            version: 2,
            payload_type: 8,
            sequence_number: 100,
            timestamp: 0,
            ssrc: 7,
            marker: false,
        };
        let packets = packetize_g711(&payload, 8000, 100, header);
        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].payload.len(), 800);
        assert_eq!(packets[1].payload.len(), 800);
        assert_eq!(packets[2].payload.len(), 100);
        assert_eq!(packets[0].header.sequence_number, 100);
        assert_eq!(packets[1].header.sequence_number, 101);
        assert_eq!(packets[2].header.sequence_number, 102);
        // Each packet's RTP timestamp advances by samples consumed.
        assert_eq!(packets[0].header.timestamp, 0);
        assert_eq!(packets[1].header.timestamp, 800);
        assert_eq!(packets[2].header.timestamp, 1600);
    }

    #[test]
    fn packetize_g711_clamps_oversized_duration() {
        // Asking for a 10s packet at 8kHz would need 80000 bytes; we cap to 1400 to avoid
        // exceeding UDP datagram size.
        let payload = vec![0u8; 1500];
        let header = RtpHeader {
            version: 2,
            payload_type: 8,
            sequence_number: 0,
            timestamp: 0,
            ssrc: 1,
            marker: false,
        };
        let packets = packetize_g711(&payload, 8000, 10_000, header);
        assert!(packets.iter().all(|p| p.payload.len() <= 1400));
        // 1500 / 1400 -> 2 packets
        assert_eq!(packets.len(), 2);
    }

    #[test]
    fn depacketize_handles_sequence_wraparound() {
        let packets = vec![
            RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type: 96,
                    sequence_number: 0,
                    timestamp: 100,
                    ssrc: 1,
                    marker: true,
                },
                payload: Bytes::from_static(b"world"),
            },
            RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type: 96,
                    sequence_number: u16::MAX,
                    timestamp: 100,
                    ssrc: 1,
                    marker: false,
                },
                payload: Bytes::from_static(b"hello "),
            },
        ];

        let rebuilt = depacketize_payload(packets);
        assert_eq!(rebuilt, Bytes::from_static(b"hello world"));
    }

    #[test]
    fn test_ehome_decoder_handshake_and_media() {
        let mut decoder = EhomeDecoder::new();

        // 1. Pack seq 0: SSRC handshake
        let mut ssrc_payload = Vec::new();
        ssrc_payload.extend_from_slice(b"ssrc-123456");
        ssrc_payload.push(0x00); // null term
                                 // Pad with zeros to 32 bytes
        ssrc_payload.resize(32, 0);

        let mut pkt0 = Vec::new();
        pkt0.extend_from_slice(&[0, 0]);
        pkt0.extend_from_slice(&(ssrc_payload.len() as u16).to_be_bytes()); // length
        pkt0.extend_from_slice(&ssrc_payload);

        let mut buf = BytesMut::from(&pkt0[..]);
        let outputs = decoder.decode(&mut buf);
        assert_eq!(outputs.len(), 1);
        if let EhomeOutput::HandshakeSsrc(ssrc) = &outputs[0] {
            assert_eq!(ssrc, "ssrc-123456");
        } else {
            panic!("Expected HandshakeSsrc");
        }
        assert_eq!(buf.len(), 0);

        // 2. Pack seq 1: Codec parameters handshake
        let mut codec_payload = vec![0u8; 32];
        codec_payload[12] = 4; // payload type: ES/NALU
                               // video codec: 0x0100 -> H264
        codec_payload[14] = 0x00;
        codec_payload[15] = 0x01;
        // audio codec: 0x7111 -> G711A
        codec_payload[16] = 0x11;
        codec_payload[17] = 0x71;
        codec_payload[18] = 2; // channel
        codec_payload[19] = 16; // sample bit
                                // sample rate: 8000
        codec_payload[20] = 0x40;
        codec_payload[21] = 0x1f;

        let mut pkt1 = Vec::new();
        pkt1.extend_from_slice(&[0, 0]);
        pkt1.extend_from_slice(&(codec_payload.len() as u16).to_be_bytes());
        pkt1.extend_from_slice(&codec_payload);

        let mut buf = BytesMut::from(&pkt1[..]);
        let outputs = decoder.decode(&mut buf);
        assert_eq!(outputs.len(), 1);
        if let EhomeOutput::HandshakeCodec(codec) = &outputs[0] {
            assert_eq!(codec.payload_type, "nalu");
            assert_eq!(codec.video_codec, Some("h264".to_string()));
            assert_eq!(codec.audio_codec, Some("g711a".to_string()));
            assert_eq!(codec.channels, 2);
            assert_eq!(codec.sample_bit, 16);
            assert_eq!(codec.sample_rate, 8000);
        } else {
            panic!("Expected HandshakeCodec");
        }

        // 3. Pack seq 2: Media packet (should skip first 4 bytes of inner payload)
        let media_payload = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let mut pkt2 = Vec::new();
        pkt2.extend_from_slice(&[0, 0]);
        pkt2.extend_from_slice(&(media_payload.len() as u16).to_be_bytes());
        pkt2.extend_from_slice(&media_payload);

        let mut buf = BytesMut::from(&pkt2[..]);
        let outputs = decoder.decode(&mut buf);
        assert_eq!(outputs.len(), 1);
        if let EhomeOutput::MediaPayload(payload) = &outputs[0] {
            assert_eq!(payload.as_ref(), &[0x55, 0x66, 0x77, 0x88]);
        } else {
            panic!("Expected MediaPayload");
        }
    }
}

/// Information about `Ehome Codec`.
/// `Ehome Codec` 的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhomeCodecInfo {
    pub payload_type: String,        // "ps" or "nalu"
    pub video_codec: Option<String>, // "h264", "h265"
    pub audio_codec: Option<String>, // "g711a", "g711u", "aac"
    pub channels: u8,
    pub sample_bit: u8,
    pub sample_rate: u32,
}

/// `EhomeOutput` enumeration.
/// `EhomeOutput` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EhomeOutput {
    HandshakeSsrc(String),
    HandshakeCodec(EhomeCodecInfo),
    MediaPayload(Bytes),
}

/// `EhomeDecoder` data structure.
/// `EhomeDecoder` 数据结构。
pub struct EhomeDecoder {
    packet_seq: u32,
    ssrc: Option<String>,
    codec_info: Option<EhomeCodecInfo>,
    is_ehome2: bool,
    first_packet: bool,
}

impl Default for EhomeDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl EhomeDecoder {
    /// Creates a new `EhomeDecoder` instance.
    /// 创建新的 `EhomeDecoder` 实例。
    pub fn new() -> Self {
        Self {
            packet_seq: 0,
            ssrc: None,
            codec_info: None,
            is_ehome2: false,
            first_packet: true,
        }
    }

    /// `codec_info` function of `EhomeDecoder`.
    /// `EhomeDecoder` 的 `codec_info` 函数。
    pub fn codec_info(&self) -> Option<EhomeCodecInfo> {
        self.codec_info.clone()
    }

    /// Decodes the value from the input buffer.
    /// 从输入缓冲区解码值。
    pub fn decode(&mut self, buf: &mut BytesMut) -> Vec<EhomeOutput> {
        let mut outputs = Vec::new();

        if self.first_packet {
            if buf.len() < 3 {
                if (!buf.is_empty() && buf[0] != 0x01) || (buf.len() >= 2 && buf[1] != 0x00) {
                    self.first_packet = false;
                } else {
                    return outputs;
                }
            } else {
                if buf[0] == 0x01 && buf[1] == 0x00 && (buf[2] == 0x01 || buf[2] == 0x02) {
                    if buf.len() < 256 {
                        return outputs;
                    }
                    self.is_ehome2 = true;
                    buf.advance(256);
                }
                self.first_packet = false;
            }
        }

        while buf.len() >= 4 {
            let payload_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            let total_len = 4 + payload_len;
            if buf.len() >= total_len {
                let packet_data = buf.split_to(total_len).freeze();
                let inner_payload = packet_data.slice(4..total_len);

                if self.packet_seq == 0 {
                    self.packet_seq += 1;
                    let mut ssrc_str = String::new();
                    for &b in inner_payload.iter() {
                        if b == 0x00 {
                            break;
                        }
                        ssrc_str.push(b as char);
                    }
                    self.ssrc = Some(ssrc_str.clone());
                    outputs.push(EhomeOutput::HandshakeSsrc(ssrc_str));
                } else if self.packet_seq == 1 {
                    self.packet_seq += 1;
                    if inner_payload.len() >= 20 {
                        let payload_type_byte = inner_payload[12];
                        let payload_type = if payload_type_byte == 2 {
                            "ps".to_string()
                        } else {
                            "nalu".to_string()
                        };

                        let video_val_le =
                            u16::from_le_bytes([inner_payload[14], inner_payload[15]]);
                        let video_val_be =
                            u16::from_be_bytes([inner_payload[14], inner_payload[15]]);
                        let video_codec = if video_val_le == 0x0100
                            || video_val_be == 0x0100
                            || video_val_le == 0x0001
                            || video_val_be == 0x0001
                        {
                            Some("h264".to_string())
                        } else if video_val_le == 0x0005
                            || video_val_be == 0x0005
                            || video_val_le == 0x0500
                            || video_val_be == 0x0500
                        {
                            Some("h265".to_string())
                        } else {
                            None
                        };

                        let audio_val_le =
                            u16::from_le_bytes([inner_payload[16], inner_payload[17]]);
                        let audio_val_be =
                            u16::from_be_bytes([inner_payload[16], inner_payload[17]]);
                        let audio_codec = if audio_val_le == 0x7111
                            || audio_val_be == 0x7111
                            || audio_val_le == 0x1171
                            || audio_val_be == 0x1171
                        {
                            Some("g711a".to_string())
                        } else if audio_val_le == 0x7110
                            || audio_val_be == 0x7110
                            || audio_val_le == 0x1071
                            || audio_val_be == 0x1071
                        {
                            Some("g711u".to_string())
                        } else if audio_val_le == 0x2001
                            || audio_val_be == 0x2001
                            || audio_val_le == 0x0120
                            || audio_val_be == 0x0120
                        {
                            Some("aac".to_string())
                        } else {
                            None
                        };

                        let channels = inner_payload[18];
                        let sample_bit = inner_payload[19];
                        let sample_rate_val_le =
                            u16::from_le_bytes([inner_payload[20], inner_payload[21]]) as u32;
                        let sample_rate_val_be =
                            u16::from_be_bytes([inner_payload[20], inner_payload[21]]) as u32;
                        let sample_rate = if sample_rate_val_le > 0 && sample_rate_val_le < 96000 {
                            sample_rate_val_le
                        } else {
                            sample_rate_val_be
                        };

                        let codec = EhomeCodecInfo {
                            payload_type,
                            video_codec,
                            audio_codec,
                            channels,
                            sample_bit,
                            sample_rate,
                        };
                        self.codec_info = Some(codec.clone());
                        outputs.push(EhomeOutput::HandshakeCodec(codec));
                    }
                } else {
                    if inner_payload.len() > 4 {
                        let media_data = inner_payload.slice(4..inner_payload.len());
                        outputs.push(EhomeOutput::MediaPayload(media_data));
                    }
                }
            } else {
                break;
            }
        }

        outputs
    }
}
