use crate::prelude::*;
use bytes::{Buf, Bytes, BytesMut};

/// RTP fixed header fields (RFC 3550).
///
/// RTP 固定头部字段（RFC 3550）。
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
    /// Parse an RTP header from a byte slice, returning the header and payload offset.
    ///
    /// 从字节切片解析 RTP 头部，返回头部和负载起始偏移。
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

    /// Encode the RTP header into the canonical 12-byte form.
    ///
    /// 将 RTP 头部编码为标准 12 字节形式。
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

/// RTP packet with header and payload.
///
/// 包含头部和负载的 RTP 包。
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Bytes,
}

impl RtpPacket {
    /// Parse a complete RTP packet (header + payload) from bytes.
    ///
    /// 从字节解析完整 RTP 包（头部 + 负载）。
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

    /// Encode the RTP packet as bytes.
    ///
    /// 将 RTP 包编码为字节。
    pub fn encode(&self) -> Bytes {
        let mut out = Vec::with_capacity(12 + self.payload.len());
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&self.payload);
        Bytes::from(out)
    }
}

/// RTP clock rate helper for timestamp conversion.
///
/// 用于时间戳转换的 RTP 时钟速率辅助结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpClock {
    pub rate: u32,
}

impl RtpClock {
    /// Convert RTP clock ticks to microseconds.
    ///
    /// 将 RTP 时钟刻度转换为微秒。
    pub fn ticks_to_micros(self, ticks: u32) -> i64 {
        if self.rate == 0 {
            return 0;
        }
        (i64::from(ticks) * 1_000_000_i64) / i64::from(self.rate)
    }

    /// Convert microseconds to RTP clock ticks.
    ///
    /// 将微秒转换为 RTP 时钟刻度。
    pub fn micros_to_ticks(self, micros: i64) -> u32 {
        if self.rate == 0 || micros <= 0 {
            return 0;
        }
        ((micros as i128 * self.rate as i128) / 1_000_000_i128) as u32
    }
}

/// Split a payload into MTU-sized RTP packets with incrementing sequence numbers.
///
/// 将负载拆分为 MTU 大小的 RTP 包，序列号递增，最后一个包标记 marker。
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
///
/// 将 G711 音频帧按目标包时长（毫秒）打包为 RTP 包。ZLMediaKit 默认按 100ms 对齐 G711
/// 以兼容 GB28181；较小值（如 20ms）在 WebRTC 桥接下更优。
///
/// `sample_rate` 为音频采样率（G711 通常为 8000 Hz）。
/// `header.timestamp` 为起始 RTP 时间戳，按每包采样数递增。
/// `header.sequence_number` 为起始序列号，每包递增 1。
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

/// Reassemble payloads from RTP packets, sorting by sequence number and handling wraparound.
///
/// 从 RTP 包重组负载，按序列号排序并处理序列号回绕。
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

/// Detected higher-level payload encapsulation inside an RTP packet.
///
/// 检测到的 RTP 包内更高层负载封装类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpPayloadMode {
    /// Program Stream (PS) over RTP.
    ///
    /// RTP 上承载的节目流（PS）。
    Ps,
    /// Transport Stream (TS) over RTP.
    ///
    /// RTP 上承载的传输流（TS）。
    Ts,
    /// Elementary Stream (ES) over RTP.
    ///
    /// RTP 上承载的基本流（ES）。
    Es,
    /// Hikvision Ehome2 private protocol.
    ///
    /// 海康 Ehome2 私有协议。
    Ehome,
    /// Hikvision XHB private container (also seen as `xhb`/`hk` in vendor stacks).
    ///
    /// 海康 XHB 私有容器（厂商栈中亦写作 `xhb`/`hk`）。
    Xhb,
    /// JT/T 1078 vehicle terminal video transport.
    ///
    /// JT/T 1078 车载终端视频传输。
    Jtt1078,
    /// Raw elementary audio stream.
    ///
    /// 原始音频基本流。
    RawAudio,
    /// Raw elementary video stream.
    ///
    /// 原始视频基本流。
    RawVideo,
    /// Payload mode could not be determined.
    ///
    /// 无法确定负载模式。
    Unknown,
}

/// Probe an RTP payload to determine the higher-level encapsulation mode.
///
/// 探测 RTP 负载以确定更高层封装模式。
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
///
/// 解析 RFC 4571 TCP RTP 帧（2 字节大端长度前缀 + RTP 包）。
/// 返回解析后的 RTP 包与总共消费的字节数。
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
///
/// 将 RTP 包编码为 RFC 4571 TCP RTP 帧（2 字节大端长度前缀 + RTP 包）。
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
/// 在厂商栈中观察到的 TCP RTP 分帧变体。
///
/// `TwoByte` is the RFC 4571 framing (2-byte big-endian length, then RTP packet).
///
/// `TwoByte` 为 RFC 4571 分帧（2 字节大端长度后跟 RTP 包）。
///
/// `Interleaved4Byte` is RTSP-style RTP-over-TCP framing (`$ + channel + 2-byte length + RTP packet`).
/// ABLMediaServer / Hikvision sub-platforms occasionally negotiate this variant when carrying
/// GB28181 RTP over a TCP control channel.
///
/// `Interleaved4Byte` 为 RTSP 风格 RTP over TCP 分帧（`$ + channel + 2 字节长度 + RTP 包`）。
/// ABLMediaServer/海康子平台在通过 TCP 控制通道承载 GB28181 RTP 时偶尔会协商此变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTcpFraming {
    /// RFC 4571 — 2-byte big-endian length prefix + RTP packet.
    ///
    /// RFC 4571 — 2 字节大端长度前缀 + RTP 包。
    TwoByte,
    /// RTSP-style interleaved framing — `$ + channel(u8) + length(u16 BE) + RTP`.
    ///
    /// RTSP 风格交错分帧 — `$ + channel(u8) + 长度(u16 大端) + RTP`。
    Interleaved4Byte,
    /// Auto-detect: try `Interleaved4Byte` first when the leading byte is `$`, otherwise
    /// fall back to `TwoByte`.
    ///
    /// 自动检测：当首字节为 `$` 时优先尝试 `Interleaved4Byte`，否则回退到 `TwoByte`。
    AutoDetect,
}

/// Parse a 4-byte interleaved TCP RTP frame (`$ + channel + length(u16) + payload`).
///
/// Returns the parsed RTP packet, the channel number, and the total number of bytes consumed.
///
/// 解析 4 字节交错 TCP RTP 帧（`$ + channel + length(u16) + payload`）。
/// 返回解析后的 RTP 包、通道号和总共消费的字节数。
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
///
/// 使用 4 字节交错分帧编码 RTP 包。
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
///
/// `AutoDetect` 模式下解析单条 TCP RTP 帧的结果。
#[derive(Debug, Clone)]
pub struct ParsedTcpRtpFrame {
    /// Parsed RTP packet.
    ///
    /// 解析出的 RTP 包。
    pub packet: RtpPacket,
    /// Number of bytes consumed from the input buffer.
    ///
    /// 从输入缓冲区消费的字节数。
    pub consumed: usize,
    /// `Some(channel)` for `Interleaved4Byte` framing, `None` for `TwoByte`.
    ///
    /// `Interleaved4Byte` 分帧时为 `Some(channel)`，`TwoByte` 时为 `None`。
    pub channel: Option<u8>,
    /// Detected framing mode.
    ///
    /// 检测到的分帧模式。
    pub framing: RtpTcpFraming,
}

/// Attempt to parse a single TCP RTP frame from `raw` using the configured framing mode.
///
/// `AutoDetect` prefers the interleaved variant when the buffer starts with `$`. If that fails
/// it falls back to the 2-byte length form. Returns `None` when more bytes are needed.
///
/// 使用配置的分帧模式从 `raw` 解析单条 TCP RTP 帧。
///
/// `AutoDetect` 在缓冲区以 `$` 开头时优先尝试交错模式；失败则回退到 2 字节长度模式。
/// 需要更多字节时返回 `None`。
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

/// Codec information extracted from the Hikvision Ehome handshake packet.
///
/// 从海康 Ehome 握手包中提取的编解码器信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhomeCodecInfo {
    /// Payload encapsulation type: "ps" or "nalu".
    ///
    /// 负载封装类型："ps" 或 "nalu"。
    pub payload_type: String,
    /// Video codec identifier: "h264" or "h265".
    ///
    /// 视频编解码器标识："h264" 或 "h265"。
    pub video_codec: Option<String>,
    /// Audio codec identifier: "g711a", "g711u" or "aac".
    ///
    /// 音频编解码器标识："g711a"、"g711u" 或 "aac"。
    pub audio_codec: Option<String>,
    /// Audio channel count.
    ///
    /// 音频通道数。
    pub channels: u8,
    /// Audio sample bit depth.
    ///
    /// 音频采样位深。
    pub sample_bit: u8,
    /// Audio sample rate in Hz.
    ///
    /// 音频采样率（Hz）。
    pub sample_rate: u32,
}

/// Output of the Ehome decoder.
///
/// Ehome 解码器的输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EhomeOutput {
    /// SSRC handshake payload.
    ///
    /// SSRC 握手负载。
    HandshakeSsrc(String),
    /// Codec handshake payload.
    ///
    /// 编解码器握手负载。
    HandshakeCodec(EhomeCodecInfo),
    /// Media payload.
    ///
    /// 媒体负载。
    MediaPayload(Bytes),
}

/// Decoder for the Hikvision Ehome2 TCP stream format.
///
/// 海康 Ehome2 TCP 流格式解码器。
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
    /// Create a new Ehome decoder.
    ///
    /// 创建新的 Ehome 解码器。
    pub fn new() -> Self {
        Self {
            packet_seq: 0,
            ssrc: None,
            codec_info: None,
            is_ehome2: false,
            first_packet: true,
        }
    }

    /// Return the discovered codec info, if the handshake has completed.
    ///
    /// 返回已发现的编解码器信息（若握手已完成）。
    pub fn codec_info(&self) -> Option<EhomeCodecInfo> {
        self.codec_info.clone()
    }

    /// Decode available Ehome frames from the buffer.
    ///
    /// Each call consumes complete 4-byte length-prefixed packets from `buf`,
    /// producing handshake or media outputs.
    ///
    /// 从缓冲区解码可用的 Ehome 帧。
    ///
    /// 每次调用从 `buf` 消费完整的 4 字节长度前缀包，产生握手或媒体输出。
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
