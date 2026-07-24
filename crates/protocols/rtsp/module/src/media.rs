//! Media processing for the RTSP module: RTP depacketization/packetization,
//! transport parsing, URI extraction, and RTCP helpers.
//!
//! This module re-exports codec-specific helpers from `depacketize`, `packetize`,
//! and `rtcp`, and exposes the top-level transport parsing and stream key
//! helpers used by the RTSP request handlers.
//!
//! RTSP 模块的媒体处理：RTP 解包/打包、传输解析、URI 提取与 RTCP 辅助。
//!
//! 本模块从 `depacketize`、`packetize`、`rtcp` 重导出编解码辅助函数，并暴露
//! RTSP 请求处理器使用的顶层传输解析与流 key 辅助函数。

use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    h26x_nalu_is_random_access, vp8_frame_is_keyframe, AVFrame, AacRtpPacketization,
    CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, RtpHeader, RtpPacket,
    RtpRtcpMapping, RtpTimestamp, SourceTimestamp, Timebase, TrackInfo,
};
#[cfg(test)]
use cheetah_rtsp_core::RtcpPacket;
use cheetah_rtsp_core::RtspTransport;
use cheetah_sdk::StreamKey;
use std::net::{IpAddr, Ipv4Addr};

use crate::session::{
    PublishAv1Depacketizer, PublishH264Depacketizer, PublishH265Depacketizer, PublishTrackClock,
    PublishVp8Depacketizer, PublishVp9Depacketizer,
};

mod depacketize;
mod packetize;
pub(crate) mod ps_compat;
mod rtcp;

use depacketize::{av1_obu_is_keyframe, av1_read_leb128};
#[cfg(test)]
use depacketize::{
    av1_rtp_payload_is_keyframe, build_frame_from_rtp, vp8_rtp_payload_is_keyframe,
    vp9_rtp_payload_is_keyframe,
};
/// Re-exports from `depacketize.rs`: convert RTP payloads into `AVFrame`s.
///
/// 从 `depacketize.rs` 重导出：将 RTP 负载转换为 `AVFrame`。
pub use depacketize::{build_frames_from_rtp, build_vp8_frame_from_rtp, build_vp9_frame_from_rtp};
/// Re-exports from `packetize.rs`: convert an `AVFrame` into a sequence of RTP packets.
///
/// 从 `packetize.rs` 重导出：将 `AVFrame` 转换为一系列 RTP 包。
pub use packetize::packetize_frame_to_rtp_with_timestamp;
/// Re-exports from `rtcp.rs`: RTCP sender/receiver reports, SDES, and BYE helpers.
///
/// 从 `rtcp.rs` 重导出：RTCP 发送/接收报告、SDES 与 BYE 辅助函数。
pub use rtcp::{
    build_rtcp_bye, build_rtcp_empty_rr, build_rtcp_receiver_report, build_rtcp_sdes_cname,
    build_rtcp_sender_report, parse_rtcp_sender_report, RtcpReceiverReportBlock,
};

/// Result of depacketizing an RTP packet into a frame and optional metadata.
///
/// RTP 包解包为帧以及可选元数据的结果。
pub struct BuiltFrameFromRtp {
    pub frame: AVFrame,
    pub discovered_audio_asc: Option<Bytes>,
    pub discovered_av1_sequence_header: Option<Bytes>,
    pub discovered_av1_codec_config: Option<Bytes>,
    pub discovered_video_dimensions: Option<(u32, u32)>,
}

struct DepacketizedAac {
    payload: Bytes,
    timestamp_offset: u32,
    discovered_asc: Option<Bytes>,
}

/// RTP/RTCP over RTSP interleaved TCP channel pair.
///
/// RTP/RTCP over RTSP 交错 TCP 通道对。
#[derive(Debug, Clone, Copy)]
pub struct TransportInterleaved {
    pub rtp_channel: u8,
    pub rtcp_channel: u8,
}

/// UDP unicast transport port and address pair from a SETUP Transport header.
///
/// 从 SETUP Transport 头解析的 UDP 单播端口与地址对。
#[derive(Debug, Clone, Copy)]
pub struct TransportUdpPorts {
    pub client_rtp_port: u16,
    pub client_rtcp_port: u16,
    pub server_rtp_port: Option<u16>,
    pub server_rtcp_port: Option<u16>,
    pub destination: Option<IpAddr>,
    pub source: Option<IpAddr>,
}

/// UDP multicast transport port, address, and TTL from a SETUP Transport header.
///
/// 从 SETUP Transport 头解析的 UDP 组播端口、地址与 TTL。
#[derive(Debug, Clone, Copy)]
pub struct TransportUdpMulticast {
    pub rtp_port: Option<u16>,
    pub rtcp_port: Option<u16>,
    pub destination: Option<Ipv4Addr>,
    pub ttl: Option<u8>,
}

/// Parsed RTSP transport preference from a SETUP request.
///
/// 从 SETUP 请求解析的 RTSP 传输偏好。
#[derive(Debug, Clone, Copy)]
pub enum RtspSetupTransport {
    TcpInterleaved(TransportInterleaved),
    TcpInterleavedAuto,
    UdpUnicast(TransportUdpPorts),
    UdpMulticast(TransportUdpMulticast),
}

/// Parses a comma-separated Transport header and selects the first supported
/// RTSP transport (TCP interleaved, UDP unicast, or UDP multicast).
///
/// 解析逗号分隔的 Transport 头，并选择第一个支持的 RTSP 传输方式
/// （TCP 交错、UDP 单播或 UDP 组播）。
pub fn parse_setup_transport(value: &str) -> Option<RtspSetupTransport> {
    for candidate in value.split(',').map(str::trim) {
        if candidate.is_empty() {
            continue;
        }
        if let Some(parsed) = parse_setup_transport_candidate(candidate) {
            return Some(parsed);
        }
    }
    None
}

fn parse_setup_transport_candidate(candidate: &str) -> Option<RtspSetupTransport> {
    let transport = RtspTransport::parse(candidate).ok()?;
    let protocol = transport.protocol.as_str();

    if let Some((rtp_channel, rtcp_channel)) = transport.interleaved {
        if rtp_channel == rtcp_channel {
            return None;
        }
        if protocol.eq_ignore_ascii_case("RTP/AVP/TCP") || protocol.eq_ignore_ascii_case("RTP/AVP")
        {
            return Some(RtspSetupTransport::TcpInterleaved(TransportInterleaved {
                rtp_channel,
                rtcp_channel,
            }));
        }
    }

    if transport.interleaved.is_none()
        && transport.client_port.is_none()
        && transport.unicast
        && protocol.eq_ignore_ascii_case("RTP/AVP/TCP")
    {
        return Some(RtspSetupTransport::TcpInterleavedAuto);
    }

    if let Some((client_rtp_port, client_rtcp_port)) = transport.client_port {
        if client_rtp_port == client_rtcp_port {
            return None;
        }
        if protocol.eq_ignore_ascii_case("RTP/AVP") || protocol.eq_ignore_ascii_case("RTP/AVP/UDP")
        {
            let destination = match transport.destination {
                Some(destination) => Some(destination.parse::<IpAddr>().ok()?),
                None => None,
            };
            let source = match transport.source {
                Some(source) => Some(source.parse::<IpAddr>().ok()?),
                None => None,
            };
            let (server_rtp_port, server_rtcp_port) = transport
                .server_port
                .map(|(rtp, rtcp)| (Some(rtp), Some(rtcp)))
                .unwrap_or((None, None));
            return Some(RtspSetupTransport::UdpUnicast(TransportUdpPorts {
                client_rtp_port,
                client_rtcp_port,
                server_rtp_port,
                server_rtcp_port,
                destination,
                source,
            }));
        }
    }

    if transport.port.is_some() || !transport.unicast {
        let (rtp_port, rtcp_port) = transport
            .port
            .map(|(rtp, rtcp)| (Some(rtp), Some(rtcp)))
            .unwrap_or((None, None));
        if matches!((rtp_port, rtcp_port), (Some(rtp), Some(rtcp)) if rtp == rtcp) {
            return None;
        }
        if transport.unicast {
            return None;
        }
        if protocol.eq_ignore_ascii_case("RTP/AVP") || protocol.eq_ignore_ascii_case("RTP/AVP/UDP")
        {
            let destination = match transport.destination {
                Some(destination) => Some(destination.parse::<Ipv4Addr>().ok()?),
                None => None,
            };
            return Some(RtspSetupTransport::UdpMulticast(TransportUdpMulticast {
                rtp_port,
                rtcp_port,
                destination,
                ttl: transport.ttl,
            }));
        }
    }

    None
}

#[cfg(test)]
pub fn parse_transport_interleaved(value: &str) -> Option<TransportInterleaved> {
    let mut rtp_channel = None;
    let mut rtcp_channel = None;

    for part in value.split(';') {
        let part = part.trim();
        if let Some(raw) = transport_param_value(part, "interleaved") {
            let mut channels = raw.split('-');
            let first = channels.next()?.trim().parse::<u8>().ok()?;
            let second = channels
                .next()
                .and_then(|v| v.trim().parse::<u8>().ok())
                .unwrap_or(first.checked_add(1)?);
            if second == first {
                return None;
            }
            rtp_channel = Some(first);
            rtcp_channel = Some(second);
        }
    }

    Some(TransportInterleaved {
        rtp_channel: rtp_channel?,
        rtcp_channel: rtcp_channel?,
    })
}

#[cfg(test)]
pub fn parse_transport_udp_ports(value: &str) -> Option<TransportUdpPorts> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(raw) = transport_param_value(part, "client_port") {
            let mut ports = raw.split('-');
            let rtp = ports.next()?.trim().parse::<u16>().ok()?;
            let rtcp = ports
                .next()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .unwrap_or(rtp.checked_add(1)?);
            if rtcp == rtp {
                return None;
            }
            return Some(TransportUdpPorts {
                client_rtp_port: rtp,
                client_rtcp_port: rtcp,
                server_rtp_port: None,
                server_rtcp_port: None,
                destination: None,
                source: None,
            });
        }
    }
    None
}

#[cfg(test)]
fn transport_param_value<'a>(part: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = part.split_once('=')?;
    if key.trim().eq_ignore_ascii_case(name) {
        Some(value.trim())
    } else {
        None
    }
}

/// Extracts the stream key from an RTSP request URI path.
///
/// Paths with one segment use `live` as the namespace; two or more segments use
/// the first segment as namespace and the rest as the path.
///
/// 从 RTSP 请求 URI 路径提取流 key。
///
/// 单段路径使用 `live` 作为命名空间；两段及以上使用第一段作为命名空间，
/// 其余作为路径。
pub fn parse_stream_key_from_uri(uri: &str) -> Option<StreamKey> {
    let path = extract_uri_path(uri)?;
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut segments = trimmed.split('/');
    let namespace = segments.next()?.trim();
    let rest = segments.collect::<Vec<_>>().join("/");
    if namespace.is_empty() {
        return None;
    }
    if rest.is_empty() {
        Some(StreamKey::new("live", namespace))
    } else {
        Some(StreamKey::new(namespace, rest))
    }
}

/// Extracts the track control suffix from an RTSP URI path.
///
/// 从 RTSP URI 路径提取 Track 控制后缀。
pub fn parse_track_control_from_uri(uri: &str) -> Option<String> {
    let path = extract_uri_path(uri)?;
    let path = path.trim_matches('/');
    let suffix = path.rsplit('/').next()?;
    if suffix.is_empty() {
        None
    } else {
        Some(suffix.to_string())
    }
}

const H264_DEPACKETIZER_MAX_FU_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const H264_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES: usize = 8 * 1024 * 1024;
const H265_DEPACKETIZER_MAX_FU_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const H265_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES: usize = 8 * 1024 * 1024;
const AV1_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES: usize = 8 * 1024 * 1024;

fn reset_publish_h264_depacketizer_state(state: &mut PublishH264Depacketizer) {
    state.fu_buffer.clear();
    state.access_unit.clear();
    state.access_unit_keyframe = false;
    state.access_unit_marker_seen = false;
    state.access_unit_timestamp = None;
    state.access_unit_last_sequence = None;
}

fn depacketize_h264_access_unit(
    packet: &RtpPacket,
    state: &mut PublishH264Depacketizer,
) -> Option<(Bytes, bool, u32, u16)> {
    let timestamp = packet.header.timestamp;
    let mut completed_previous = None;
    if let Some(current_ts) = state.access_unit_timestamp {
        if current_ts != timestamp {
            if state.access_unit_marker_seen && !state.access_unit.is_empty() {
                completed_previous = Some((
                    Bytes::from(std::mem::take(&mut state.access_unit)),
                    state.access_unit_keyframe,
                    current_ts,
                    state
                        .access_unit_last_sequence
                        .unwrap_or(packet.header.sequence_number),
                ));
            }
            state.fu_buffer.clear();
            state.access_unit.clear();
            state.access_unit_keyframe = false;
            state.access_unit_marker_seen = false;
            state.access_unit_timestamp = Some(timestamp);
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
        }
    } else {
        state.access_unit_timestamp = Some(timestamp);
        state.access_unit_last_sequence = Some(packet.header.sequence_number);
    }

    match depacketize_h264_with_status(&packet.payload, &mut state.fu_buffer) {
        H264DepacketizeResult::Nalu(nalu_payload, keyframe) => {
            let Some(next_len) = state.access_unit.len().checked_add(nalu_payload.len()) else {
                reset_publish_h264_depacketizer_state(state);
                return None;
            };
            if next_len > H264_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES {
                reset_publish_h264_depacketizer_state(state);
                return None;
            }
            state.access_unit.extend_from_slice(nalu_payload.as_ref());
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
            state.access_unit_keyframe |= keyframe;
        }
        H264DepacketizeResult::NeedMore => {}
        H264DepacketizeResult::DropCurrent => {
            reset_publish_h264_depacketizer_state(state);
            return completed_previous;
        }
    }

    if packet.header.marker {
        let first_marker_for_timestamp = !state.access_unit_marker_seen;
        state.access_unit_marker_seen = true;
        if first_marker_for_timestamp
            && completed_previous.is_none()
            && state.access_unit_keyframe
            && !state.access_unit.is_empty()
        {
            let completed = (
                Bytes::from(std::mem::take(&mut state.access_unit)),
                state.access_unit_keyframe,
                timestamp,
                packet.header.sequence_number,
            );
            state.access_unit_keyframe = false;
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
            return Some(completed);
        }
    }

    completed_previous
}

fn reset_publish_h265_depacketizer_state(state: &mut PublishH265Depacketizer) {
    state.fu_buffer.clear();
    state.access_unit.clear();
    state.access_unit_keyframe = false;
    state.access_unit_marker_seen = false;
    state.access_unit_timestamp = None;
    state.access_unit_last_sequence = None;
}

fn depacketize_h265_access_unit(
    codec: CodecId,
    packet: &RtpPacket,
    state: &mut PublishH265Depacketizer,
) -> Option<(Bytes, bool, u32, u16)> {
    let timestamp = packet.header.timestamp;
    let mut completed_previous = None;
    if let Some(current_ts) = state.access_unit_timestamp {
        if current_ts != timestamp {
            if state.access_unit_marker_seen && !state.access_unit.is_empty() {
                completed_previous = Some((
                    Bytes::from(std::mem::take(&mut state.access_unit)),
                    state.access_unit_keyframe,
                    current_ts,
                    state
                        .access_unit_last_sequence
                        .unwrap_or(packet.header.sequence_number),
                ));
            }
            state.fu_buffer.clear();
            state.access_unit.clear();
            state.access_unit_keyframe = false;
            state.access_unit_marker_seen = false;
            state.access_unit_timestamp = Some(timestamp);
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
        }
    } else {
        state.access_unit_timestamp = Some(timestamp);
        state.access_unit_last_sequence = Some(packet.header.sequence_number);
    }

    match depacketize_h265_with_status(codec, &packet.payload, &mut state.fu_buffer) {
        H265DepacketizeResult::Nalu(nalu_payload, keyframe) => {
            let Some(next_len) = state.access_unit.len().checked_add(nalu_payload.len()) else {
                reset_publish_h265_depacketizer_state(state);
                return None;
            };
            if next_len > H265_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES {
                reset_publish_h265_depacketizer_state(state);
                return None;
            }
            state.access_unit.extend_from_slice(nalu_payload.as_ref());
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
            state.access_unit_keyframe |= keyframe;
        }
        H265DepacketizeResult::NeedMore => {}
        H265DepacketizeResult::DropCurrent => {
            reset_publish_h265_depacketizer_state(state);
            return completed_previous;
        }
    }

    if packet.header.marker {
        let first_marker_for_timestamp = !state.access_unit_marker_seen;
        state.access_unit_marker_seen = true;
        if first_marker_for_timestamp
            && completed_previous.is_none()
            && state.access_unit_keyframe
            && !state.access_unit.is_empty()
        {
            let completed = (
                Bytes::from(std::mem::take(&mut state.access_unit)),
                state.access_unit_keyframe,
                timestamp,
                packet.header.sequence_number,
            );
            state.access_unit_keyframe = false;
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
            return Some(completed);
        }
    }

    completed_previous
}

fn reset_publish_av1_depacketizer_state(state: &mut PublishAv1Depacketizer) {
    state.access_unit.clear();
    state.current_obu.clear();
    state.access_unit_keyframe = false;
    state.access_unit_marker_seen = false;
    state.access_unit_timestamp = None;
    state.access_unit_last_sequence = None;
}

fn reset_publish_vp9_depacketizer_state(state: &mut PublishVp9Depacketizer) {
    state.access_unit.clear();
    state.access_unit_timestamp = None;
    state.access_unit_last_sequence = None;
    state.access_unit_keyframe = false;
}

fn reset_publish_vp8_depacketizer_state(state: &mut PublishVp8Depacketizer) {
    state.access_unit.clear();
    state.access_unit_timestamp = None;
    state.access_unit_last_sequence = None;
    state.access_unit_keyframe = false;
}

fn depacketize_av1_access_unit(
    packet: &RtpPacket,
    state: &mut PublishAv1Depacketizer,
) -> Option<(Bytes, bool, u32, u16)> {
    let timestamp = packet.header.timestamp;
    let mut completed_previous = None;
    if let Some(current_ts) = state.access_unit_timestamp {
        if current_ts != timestamp {
            if state.access_unit_marker_seen && !state.access_unit.is_empty() {
                completed_previous = Some((
                    Bytes::from(std::mem::take(&mut state.access_unit)),
                    state.access_unit_keyframe,
                    current_ts,
                    state
                        .access_unit_last_sequence
                        .unwrap_or(packet.header.sequence_number),
                ));
            }
            state.access_unit.clear();
            state.current_obu.clear();
            state.access_unit_keyframe = false;
            state.access_unit_marker_seen = false;
            state.access_unit_timestamp = Some(timestamp);
            state.access_unit_last_sequence = Some(packet.header.sequence_number);
        }
    } else {
        state.access_unit_timestamp = Some(timestamp);
        state.access_unit_last_sequence = Some(packet.header.sequence_number);
    }

    if !append_av1_rtp_payload_to_access_unit(&packet.payload, state) {
        reset_publish_av1_depacketizer_state(state);
        return completed_previous;
    };
    state.access_unit_last_sequence = Some(packet.header.sequence_number);

    if packet.header.marker {
        if !state.current_obu.is_empty() && !finish_av1_obu(state) {
            reset_publish_av1_depacketizer_state(state);
            return completed_previous;
        }
        let completed = if !state.access_unit.is_empty() {
            Some((
                Bytes::from(std::mem::take(&mut state.access_unit)),
                state.access_unit_keyframe,
                timestamp,
                packet.header.sequence_number,
            ))
        } else {
            None
        };
        state.access_unit_keyframe = false;
        state.current_obu.clear();
        state.access_unit_marker_seen = true;
        state.access_unit_last_sequence = Some(packet.header.sequence_number);
        return completed.or(completed_previous);
    }

    completed_previous
}

fn append_av1_rtp_payload_to_access_unit(
    payload: &[u8],
    state: &mut PublishAv1Depacketizer,
) -> bool {
    if payload.len() < 2 {
        return false;
    }
    let aggregation = payload[0];
    let z = (aggregation & 0x80) != 0;
    let y = (aggregation & 0x40) != 0;
    let w = ((aggregation >> 4) & 0x03) as usize;
    let mut cursor = &payload[1..];
    let mut elements = Vec::new();

    if w == 0 {
        while !cursor.is_empty() {
            let Some((obu_len, leb_len)) = av1_read_leb128(cursor) else {
                return false;
            };
            cursor = &cursor[leb_len..];
            if obu_len > cursor.len() {
                return false;
            }
            elements.push(&cursor[..obu_len]);
            cursor = &cursor[obu_len..];
        }
    } else {
        for index in 0..w {
            let obu = if index + 1 == w {
                let last = cursor;
                cursor = &[];
                last
            } else {
                let Some((obu_len, leb_len)) = av1_read_leb128(cursor) else {
                    return false;
                };
                cursor = &cursor[leb_len..];
                if obu_len > cursor.len() {
                    return false;
                }
                let obu = &cursor[..obu_len];
                cursor = &cursor[obu_len..];
                obu
            };
            if !obu.is_empty() {
                elements.push(obu);
            }
        }
    }

    if elements.is_empty() {
        return false;
    }
    if z && state.current_obu.is_empty() {
        return false;
    }
    if !z && !state.current_obu.is_empty() {
        state.current_obu.clear();
    }

    let last_index = elements.len().saturating_sub(1);
    for (index, element) in elements.into_iter().enumerate() {
        if state
            .current_obu
            .len()
            .checked_add(element.len())
            .is_none_or(|len| len > AV1_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES)
        {
            return false;
        }
        state.current_obu.extend_from_slice(element);
        if (index != last_index || !y) && !finish_av1_obu(state) {
            return false;
        }
    }
    true
}

fn finish_av1_obu(state: &mut PublishAv1Depacketizer) -> bool {
    let obu = std::mem::take(&mut state.current_obu);
    if obu.is_empty() {
        return true;
    }
    state.access_unit_keyframe |= av1_obu_is_keyframe(&obu).unwrap_or(false);
    let Some(sized_obu) = av1_obu_with_size_field(&obu) else {
        return false;
    };
    if state
        .access_unit
        .len()
        .checked_add(sized_obu.len())
        .is_none_or(|len| len > AV1_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES)
    {
        return false;
    }
    state.access_unit.extend_from_slice(&sized_obu);
    true
}

fn av1_obu_with_size_field(obu: &[u8]) -> Option<Vec<u8>> {
    let header = *obu.first()?;
    let has_extension = (header & 0x04) != 0;
    let header_len = 1 + usize::from(has_extension);
    if obu.len() < header_len {
        return None;
    }
    let payload = &obu[header_len..];
    let mut out = Vec::with_capacity(obu.len() + 8);
    out.push(header | 0x02);
    if has_extension {
        out.push(obu[1]);
    }
    av1_write_leb128(payload.len(), &mut out);
    out.extend_from_slice(payload);
    Some(out)
}

fn av1_write_leb128(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn av1_sequence_header_obu_from_low_overhead(payload: &[u8]) -> Option<Bytes> {
    let mut cursor = payload;
    while !cursor.is_empty() {
        let obu_header = *cursor.first()?;
        let obu_type = (obu_header >> 3) & 0x0f;
        let has_extension = (obu_header & 0x04) != 0;
        let has_size_field = (obu_header & 0x02) != 0;
        let header_len = 1 + usize::from(has_extension);
        if cursor.len() < header_len {
            return None;
        }

        let mut obu_len = cursor.len();
        if has_size_field {
            let (declared_len, leb_len) = av1_read_leb128(&cursor[header_len..])?;
            obu_len = header_len.checked_add(leb_len)?.checked_add(declared_len)?;
            if obu_len > cursor.len() {
                return None;
            }
        }

        if obu_type == 1 {
            return Some(Bytes::copy_from_slice(&cursor[..obu_len]));
        }
        if !has_size_field {
            return None;
        }
        cursor = &cursor[obu_len..];
    }
    None
}

fn av1_codec_config_from_sequence_header(sequence_header: &[u8]) -> Option<Bytes> {
    let fields = av1_sequence_header_config_fields_from_obu(sequence_header)?;

    let mut config = Vec::with_capacity(4 + sequence_header.len());
    config.push(0x81);
    config.push(((fields.seq_profile & 0x07) << 5) | (fields.seq_level_idx_0 & 0x1f));
    config.push(
        ((fields.seq_tier_0 & 0x01) << 7)
            | (u8::from(fields.high_bitdepth) << 6)
            | (u8::from(fields.twelve_bit) << 5)
            | (u8::from(fields.monochrome) << 4)
            | (u8::from(fields.chroma_subsampling_x) << 3)
            | (u8::from(fields.chroma_subsampling_y) << 2)
            | (fields.chroma_sample_position & 0x03),
    );
    config.push(0);
    config.extend_from_slice(sequence_header);
    Some(Bytes::from(config))
}

fn av1_dimensions_from_sequence_header(sequence_header: &[u8]) -> Option<(u32, u32)> {
    let fields = av1_sequence_header_config_fields_from_obu(sequence_header)?;
    Some((
        fields.max_frame_width_minus_1.checked_add(1)?,
        fields.max_frame_height_minus_1.checked_add(1)?,
    ))
}

fn av1_sequence_header_config_fields_from_obu(
    sequence_header: &[u8],
) -> Option<Av1SequenceHeaderConfigFields> {
    let header = *sequence_header.first()?;
    let has_extension = (header & 0x04) != 0;
    let has_size_field = (header & 0x02) != 0;
    let header_len = 1 + usize::from(has_extension);
    if !has_size_field || sequence_header.len() < header_len {
        return None;
    }
    let (payload_len, leb_len) = av1_read_leb128(&sequence_header[header_len..])?;
    let payload_start = header_len.checked_add(leb_len)?;
    let payload_end = payload_start.checked_add(payload_len)?;
    let sequence_payload = sequence_header.get(payload_start..payload_end)?;
    parse_av1_sequence_header_config_fields(sequence_payload)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av1SequenceHeaderConfigFields {
    seq_profile: u8,
    seq_level_idx_0: u8,
    seq_tier_0: u8,
    high_bitdepth: bool,
    twelve_bit: bool,
    monochrome: bool,
    chroma_subsampling_x: bool,
    chroma_subsampling_y: bool,
    chroma_sample_position: u8,
    max_frame_width_minus_1: u32,
    max_frame_height_minus_1: u32,
}

fn parse_av1_sequence_header_config_fields(
    payload: &[u8],
) -> Option<Av1SequenceHeaderConfigFields> {
    let mut bits = BitReader::new(payload);
    let seq_profile = bits.read_bits(3)? as u8;
    let still_picture = bits.read_bit()?;
    let reduced_still_picture_header = bits.read_bit()?;
    let mut seq_level_idx_0 = 0u8;
    let mut seq_tier_0 = 0u8;

    if reduced_still_picture_header == 1 {
        seq_level_idx_0 = bits.read_bits(5)? as u8;
    } else {
        let timing_info_present_flag = bits.read_bit()?;
        let mut decoder_model_info_present_flag = 0;
        let mut buffer_delay_length_minus_1 = 0;
        if timing_info_present_flag == 1 {
            bits.skip_bits(32 + 32)?;
            let equal_picture_interval = bits.read_bit()?;
            if equal_picture_interval == 1 {
                let _num_ticks_per_picture_minus_1 = av1_read_uvlc(&mut bits)?;
            }
            decoder_model_info_present_flag = bits.read_bit()?;
            if decoder_model_info_present_flag == 1 {
                buffer_delay_length_minus_1 = bits.read_bits(5)?;
                bits.skip_bits(32 + 4)?;
            }
        }
        let initial_display_delay_present_flag = bits.read_bit()?;
        let operating_points_cnt_minus_1 = bits.read_bits(5)?;
        for idx in 0..=operating_points_cnt_minus_1 {
            bits.skip_bits(12)?;
            let seq_level_idx = bits.read_bits(5)? as u8;
            let seq_tier = if seq_level_idx > 7 {
                bits.read_bit()?
            } else {
                0
            };
            if idx == 0 {
                seq_level_idx_0 = seq_level_idx;
                seq_tier_0 = seq_tier;
            }
            if decoder_model_info_present_flag == 1 {
                let decoder_model_present_for_this_op = bits.read_bit()?;
                if decoder_model_present_for_this_op == 1 {
                    let n = usize::try_from(buffer_delay_length_minus_1).ok()? + 1;
                    bits.skip_bits(n * 2 + 1)?;
                }
            }
            if initial_display_delay_present_flag == 1 {
                let initial_display_delay_present_for_this_op = bits.read_bit()?;
                if initial_display_delay_present_for_this_op == 1 {
                    bits.skip_bits(4)?;
                }
            }
        }
    }

    let frame_width_bits_minus_1 = bits.read_bits(4)? as usize;
    let frame_height_bits_minus_1 = bits.read_bits(4)? as usize;
    let max_frame_width_minus_1 = bits.read_bits(frame_width_bits_minus_1 + 1)?;
    let max_frame_height_minus_1 = bits.read_bits(frame_height_bits_minus_1 + 1)?;
    if reduced_still_picture_header == 0 {
        let frame_id_numbers_present_flag = bits.read_bit()?;
        if frame_id_numbers_present_flag == 1 {
            let delta_frame_id_length_minus_2 = bits.read_bits(4)? as usize;
            let additional_frame_id_length_minus_1 = bits.read_bits(3)? as usize;
            let id_len = delta_frame_id_length_minus_2 + 2 + additional_frame_id_length_minus_1 + 1;
            if id_len > 16 {
                return None;
            }
        }
    }

    bits.skip_bits(3)?;
    if reduced_still_picture_header == 0 {
        bits.skip_bits(4)?;
    }
    let enable_order_hint = if reduced_still_picture_header == 0 {
        let value = bits.read_bit()?;
        if value == 1 {
            bits.skip_bits(2)?;
        }
        let seq_choose_screen_content_tools = bits.read_bit()?;
        let seq_force_screen_content_tools = if seq_choose_screen_content_tools == 1 {
            2
        } else {
            bits.read_bit()?
        };
        if seq_force_screen_content_tools > 0 {
            let seq_choose_integer_mv = bits.read_bit()?;
            if seq_choose_integer_mv == 0 {
                let _seq_force_integer_mv = bits.read_bit()?;
            }
        }
        if value == 1 {
            bits.skip_bits(3)?;
        }
        bits.skip_bits(3)?;
        value
    } else {
        0
    };
    let _ = enable_order_hint;
    let high_bitdepth = bits.read_bit()? == 1;
    let twelve_bit = seq_profile == 2 && high_bitdepth && bits.read_bit()? == 1;
    let monochrome = seq_profile != 1 && bits.read_bit()? == 1;
    let color_description_present_flag = bits.read_bit()?;
    if color_description_present_flag == 1 {
        bits.skip_bits(24)?;
    }
    let _color_range = bits.read_bit()?;

    let (chroma_subsampling_x, chroma_subsampling_y, chroma_sample_position) = if monochrome {
        (true, true, 0)
    } else if seq_profile == 0 {
        (true, true, bits.read_bits(2)? as u8)
    } else if seq_profile == 1 {
        (false, false, 0)
    } else if twelve_bit {
        let x = bits.read_bit()? == 1;
        let y = if x { bits.read_bit()? == 1 } else { false };
        let sample = if x && y { bits.read_bits(2)? as u8 } else { 0 };
        (x, y, sample)
    } else {
        (true, false, 0)
    };

    let _separate_uv_delta_q = if chroma_subsampling_x && chroma_subsampling_y {
        0
    } else {
        bits.read_bit()?
    };
    let _film_grain_params_present = if seq_profile == 0 && still_picture == 1 {
        0
    } else {
        bits.read_bit()?
    };

    Some(Av1SequenceHeaderConfigFields {
        seq_profile,
        seq_level_idx_0,
        seq_tier_0,
        high_bitdepth,
        twelve_bit,
        monochrome,
        chroma_subsampling_x,
        chroma_subsampling_y,
        chroma_sample_position,
        max_frame_width_minus_1,
        max_frame_height_minus_1,
    })
}

fn av1_read_uvlc(bits: &mut BitReader<'_>) -> Option<u32> {
    let mut leading_zeroes = 0usize;
    while leading_zeroes < 32 {
        if bits.read_bit()? == 1 {
            break;
        }
        leading_zeroes += 1;
    }
    if leading_zeroes == 32 {
        return Some(u32::MAX);
    }
    let suffix = if leading_zeroes == 0 {
        0
    } else {
        bits.read_bits(leading_zeroes)?
    };
    Some((1u32 << leading_zeroes) - 1 + suffix)
}

enum H264DepacketizeResult {
    Nalu(Bytes, bool),
    NeedMore,
    DropCurrent,
}

enum H265DepacketizeResult {
    Nalu(Bytes, bool),
    NeedMore,
    DropCurrent,
}

fn depacketize_h264_with_status(payload: &[u8], fu_buffer: &mut Vec<u8>) -> H264DepacketizeResult {
    if payload.is_empty() {
        return H264DepacketizeResult::NeedMore;
    }

    let nal_type = payload[0] & 0x1f;
    match nal_type {
        1..=23 => {
            // RTSP publishers may emit standalone AUD (NAL type 9). Forwarding AUD as an
            // independent RTMP video packet produces decoder warnings like
            // "missing picture in access unit", so skip it here.
            if nal_type == 9 {
                return H264DepacketizeResult::NeedMore;
            }
            let keyframe = h264_nalu_is_keyframe(payload);
            let mut out = Vec::with_capacity(payload.len() + 4);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(payload);
            H264DepacketizeResult::Nalu(Bytes::from(out), keyframe)
        }
        24 => {
            let mut cursor = &payload[1..];
            let mut out = Vec::with_capacity(payload.len() + 8);
            let mut keyframe = false;
            while cursor.len() >= 2 {
                let size = u16::from_be_bytes([cursor[0], cursor[1]]) as usize;
                cursor = &cursor[2..];
                if size == 0 || cursor.len() < size {
                    break;
                }
                let unit = &cursor[..size];
                if (unit[0] & 0x1f) == 9 {
                    cursor = &cursor[size..];
                    continue;
                }
                keyframe |= h264_nalu_is_keyframe(unit);
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(unit);
                cursor = &cursor[size..];
            }
            if out.is_empty() {
                H264DepacketizeResult::NeedMore
            } else {
                H264DepacketizeResult::Nalu(Bytes::from(out), keyframe)
            }
        }
        28 => {
            if payload.len() < 2 {
                return H264DepacketizeResult::NeedMore;
            }
            let fu_header = payload[1];
            let start = (fu_header & 0x80) != 0;
            let end = (fu_header & 0x40) != 0;
            let nal_header = (payload[0] & 0xe0) | (fu_header & 0x1f);
            if start {
                fu_buffer.clear();
                fu_buffer.push(nal_header);
            }
            if fu_buffer.is_empty() {
                return H264DepacketizeResult::NeedMore;
            }
            let Some(next_len) = fu_buffer.len().checked_add(payload[2..].len()) else {
                fu_buffer.clear();
                return H264DepacketizeResult::DropCurrent;
            };
            if next_len > H264_DEPACKETIZER_MAX_FU_BUFFER_BYTES {
                fu_buffer.clear();
                return H264DepacketizeResult::DropCurrent;
            }
            fu_buffer.extend_from_slice(&payload[2..]);
            if !end {
                return H264DepacketizeResult::NeedMore;
            }

            let keyframe = h264_nalu_is_keyframe(fu_buffer);
            let mut out = Vec::with_capacity(fu_buffer.len() + 4);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(fu_buffer);
            fu_buffer.clear();
            H264DepacketizeResult::Nalu(Bytes::from(out), keyframe)
        }
        _ => H264DepacketizeResult::NeedMore,
    }
}

fn depacketize_h265_with_status(
    codec: CodecId,
    payload: &[u8],
    fu_buffer: &mut Vec<u8>,
) -> H265DepacketizeResult {
    if payload.len() < 2 {
        return H265DepacketizeResult::NeedMore;
    }
    if codec == CodecId::H266 {
        let keyframe = h26x_nalu_is_random_access(codec, payload);
        let mut out = Vec::with_capacity(payload.len() + 4);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(payload);
        return H265DepacketizeResult::Nalu(Bytes::from(out), keyframe);
    }
    let nal_type = (payload[0] >> 1) & 0x3f;
    match nal_type {
        0..=47 => {
            if nal_type == 35 {
                return H265DepacketizeResult::NeedMore;
            }
            let keyframe = h26x_nalu_is_random_access(codec, payload);
            let mut out = Vec::with_capacity(payload.len() + 4);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(payload);
            H265DepacketizeResult::Nalu(Bytes::from(out), keyframe)
        }
        48 => {
            let mut cursor = &payload[2..];
            let mut out = Vec::with_capacity(payload.len() + 8);
            let mut keyframe = false;
            while cursor.len() >= 2 {
                let size = u16::from_be_bytes([cursor[0], cursor[1]]) as usize;
                cursor = &cursor[2..];
                if size == 0 || cursor.len() < size {
                    break;
                }
                let unit = &cursor[..size];
                if unit.len() < 2 {
                    break;
                }
                let unit_nal_type = (unit[0] >> 1) & 0x3f;
                if unit_nal_type == 35 {
                    cursor = &cursor[size..];
                    continue;
                }
                keyframe |= h26x_nalu_is_random_access(codec, unit);
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(unit);
                cursor = &cursor[size..];
            }
            if out.is_empty() {
                H265DepacketizeResult::NeedMore
            } else {
                H265DepacketizeResult::Nalu(Bytes::from(out), keyframe)
            }
        }
        49 => {
            if payload.len() < 3 {
                return H265DepacketizeResult::NeedMore;
            }
            let fu_header = payload[2];
            let start = (fu_header & 0x80) != 0;
            let end = (fu_header & 0x40) != 0;
            let fu_type = fu_header & 0x3f;
            let nal_header_0 = (payload[0] & 0x81) | (fu_type << 1);
            let nal_header_1 = payload[1];
            if start {
                fu_buffer.clear();
                fu_buffer.extend_from_slice(&[nal_header_0, nal_header_1]);
            }
            if fu_buffer.is_empty() {
                return H265DepacketizeResult::NeedMore;
            }
            let Some(next_len) = fu_buffer.len().checked_add(payload[3..].len()) else {
                fu_buffer.clear();
                return H265DepacketizeResult::DropCurrent;
            };
            if next_len > H265_DEPACKETIZER_MAX_FU_BUFFER_BYTES {
                fu_buffer.clear();
                return H265DepacketizeResult::DropCurrent;
            }
            fu_buffer.extend_from_slice(&payload[3..]);
            if !end {
                return H265DepacketizeResult::NeedMore;
            }
            let keyframe = h26x_nalu_is_random_access(codec, fu_buffer);
            let mut out = Vec::with_capacity(fu_buffer.len() + 4);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(fu_buffer);
            fu_buffer.clear();
            H265DepacketizeResult::Nalu(Bytes::from(out), keyframe)
        }
        _ => H265DepacketizeResult::NeedMore,
    }
}

#[cfg(test)]
fn depacketize_h264(payload: &[u8], fu_buffer: &mut Vec<u8>) -> Option<(Bytes, bool)> {
    match depacketize_h264_with_status(payload, fu_buffer) {
        H264DepacketizeResult::Nalu(payload, keyframe) => Some((payload, keyframe)),
        H264DepacketizeResult::NeedMore | H264DepacketizeResult::DropCurrent => None,
    }
}

fn h264_nalu_is_keyframe(nalu: &[u8]) -> bool {
    h26x_nalu_is_random_access(CodecId::H264, nalu)
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn bit_offset(&self) -> usize {
        self.bit_offset
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte_index = self.bit_offset / 8;
        let bit_in_byte = 7usize.saturating_sub(self.bit_offset % 8);
        let byte = *self.data.get(byte_index)?;
        self.bit_offset += 1;
        Some((byte >> bit_in_byte) & 1)
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut out = 0u32;
        for _ in 0..n {
            out = (out << 1) | u32::from(self.read_bit()?);
        }
        Some(out)
    }

    fn read_bytes(&mut self, len: usize) -> Option<Vec<u8>> {
        let bits_needed = len.checked_mul(8)?;
        if self.remaining_bits() < bits_needed {
            return None;
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_bits(8)? as u8);
        }
        Some(out)
    }

    fn skip_bits(&mut self, n: usize) -> Option<()> {
        if self.bit_offset.checked_add(n)? > self.data.len().saturating_mul(8) {
            return None;
        }
        self.bit_offset += n;
        Some(())
    }

    fn peek_bits(&self, n: usize) -> Option<u32> {
        let mut clone = Self {
            data: self.data,
            bit_offset: self.bit_offset,
        };
        clone.read_bits(n)
    }

    fn remaining_bits(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_offset)
    }

    fn remaining_bits_are_zero(&self) -> bool {
        let mut clone = Self {
            data: self.data,
            bit_offset: self.bit_offset,
        };
        while clone.remaining_bits() > 0 {
            if clone.read_bit() != Some(0) {
                return false;
            }
        }
        true
    }

    fn byte_align(&mut self) -> Option<()> {
        let rem = self.bit_offset % 8;
        if rem == 0 {
            return Some(());
        }
        self.skip_bits(8 - rem)
    }
}

fn bits_to_bytes(data: &[u8], start_bit: usize, end_bit: usize) -> Option<Bytes> {
    if end_bit <= start_bit || end_bit > data.len().saturating_mul(8) {
        return None;
    }
    let bit_len = end_bit - start_bit;
    let mut out = vec![0u8; bit_len.div_ceil(8)];
    for i in 0..bit_len {
        let src_bit = start_bit + i;
        let src_byte = *data.get(src_bit / 8)?;
        let src_shift = 7usize.saturating_sub(src_bit % 8);
        let bit = (src_byte >> src_shift) & 1;
        out[i / 8] |= bit << (7usize.saturating_sub(i % 8));
    }
    Some(Bytes::from(out))
}

fn packetize_h264(
    payload: &[u8],
    payload_type: u8,
    seq: &mut u16,
    timestamp: u32,
    ssrc: u32,
    mtu: usize,
) -> Vec<RtpPacket> {
    let max_payload = mtu.saturating_sub(12).max(32);
    let nal_units = split_annexb_units(payload);
    let units: Vec<&[u8]> = if nal_units.is_empty() {
        vec![payload]
    } else {
        nal_units
    };

    let mut packets = Vec::new();
    if units.len() > 1 {
        let stap_len = 1usize.saturating_add(
            units
                .iter()
                .map(|unit| unit.len().saturating_add(2))
                .sum::<usize>(),
        );
        if stap_len <= max_payload && units.iter().all(|unit| unit.len() <= u16::MAX as usize) {
            let nri = units.iter().fold(0u8, |acc, unit| acc | (unit[0] & 0x60));
            let mut stap = BytesMut::with_capacity(stap_len);
            stap.extend_from_slice(&[nri | 24]);
            for unit in units {
                stap.extend_from_slice(&(unit.len() as u16).to_be_bytes());
                stap.extend_from_slice(unit);
            }
            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker: true,
                },
                payload: stap.freeze(),
            });
            *seq = seq.wrapping_add(1);
            return packets;
        }
    }

    for (unit_index, unit) in units.iter().enumerate() {
        if unit.is_empty() {
            continue;
        }
        let marker = unit_index + 1 == units.len();
        if unit.len() <= max_payload {
            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker,
                },
                payload: Bytes::copy_from_slice(unit),
            });
            *seq = seq.wrapping_add(1);
            continue;
        }

        let fu_payload = &unit[1..];
        let fu_indicator = (unit[0] & 0xe0) | 28;
        let max_chunk = max_payload.saturating_sub(2).max(1);
        let mut cursor = 0usize;
        while cursor < fu_payload.len() {
            let remain = fu_payload.len() - cursor;
            let take = remain.min(max_chunk);
            let start = cursor == 0;
            let end = cursor + take >= fu_payload.len();

            let mut frag = BytesMut::with_capacity(2 + take);
            frag.extend_from_slice(&[fu_indicator]);
            let mut fu_header = unit[0] & 0x1f;
            if start {
                fu_header |= 0x80;
            }
            if end {
                fu_header |= 0x40;
            }
            frag.extend_from_slice(&[fu_header]);
            frag.extend_from_slice(&fu_payload[cursor..cursor + take]);

            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker: marker && end,
                },
                payload: frag.freeze(),
            });
            *seq = seq.wrapping_add(1);
            cursor += take;
        }
    }

    packets
}

fn packetize_h265(
    payload: &[u8],
    payload_type: u8,
    seq: &mut u16,
    timestamp: u32,
    ssrc: u32,
    mtu: usize,
) -> Vec<RtpPacket> {
    let max_payload = mtu.saturating_sub(12).max(3);
    let nal_units = split_annexb_units(payload);
    let units: Vec<&[u8]> = if nal_units.is_empty() {
        vec![payload]
    } else {
        nal_units
    }
    .into_iter()
    .filter(|unit| unit.len() >= 2 && h265_nal_type(unit) != Some(35))
    .collect();

    let mut packets = Vec::new();
    for (unit_index, unit) in units.iter().enumerate() {
        let marker = unit_index + 1 == units.len();
        if unit.len() <= max_payload {
            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker,
                },
                payload: Bytes::copy_from_slice(unit),
            });
            *seq = seq.wrapping_add(1);
            continue;
        }

        let Some(nal_type) = h265_nal_type(unit) else {
            continue;
        };
        let fu_payload = &unit[2..];
        if fu_payload.is_empty() {
            continue;
        }
        let fu_indicator = [(unit[0] & 0x81) | (49 << 1), unit[1]];
        let max_chunk = max_payload.saturating_sub(3).max(1);
        let mut cursor = 0usize;
        while cursor < fu_payload.len() {
            let remain = fu_payload.len() - cursor;
            let take = remain.min(max_chunk);
            let start = cursor == 0;
            let end = cursor + take >= fu_payload.len();

            let mut frag = BytesMut::with_capacity(3 + take);
            frag.extend_from_slice(&fu_indicator);
            let mut fu_header = nal_type;
            if start {
                fu_header |= 0x80;
            }
            if end {
                fu_header |= 0x40;
            }
            frag.extend_from_slice(&[fu_header]);
            frag.extend_from_slice(&fu_payload[cursor..cursor + take]);

            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker: marker && end,
                },
                payload: frag.freeze(),
            });
            *seq = seq.wrapping_add(1);
            cursor += take;
        }
    }

    packets
}

fn h265_nal_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|header| (header >> 1) & 0x3f)
}

fn packetize_av1(
    frame: &AVFrame,
    track: &TrackInfo,
    payload_type: u8,
    seq: &mut u16,
    timestamp: u32,
    ssrc: u32,
    mtu: usize,
) -> Vec<RtpPacket> {
    let max_payload = mtu.saturating_sub(12).max(2);
    let frame_obus = split_av1_low_overhead_obus(frame.payload.as_ref());
    let keyframe = frame.flags.contains(FrameFlags::KEY);
    let mut obus = Vec::new();
    if keyframe
        && !frame_obus
            .iter()
            .any(|obu| av1_low_overhead_obu_type(obu) == Some(1))
    {
        obus.extend(av1_sequence_header_obus_from_track(track));
    }
    obus.extend(frame_obus);
    let mut packets = Vec::new();
    let mut first_packet = true;

    for (obu_index, obu) in obus.iter().enumerate() {
        if obu.is_empty() {
            continue;
        }
        let max_chunk = max_payload.saturating_sub(1).max(1);
        let mut cursor = 0usize;
        while cursor < obu.len() {
            let remain = obu.len() - cursor;
            let take = remain.min(max_chunk);
            let first_fragment = cursor == 0;
            let last_fragment = cursor + take >= obu.len();
            let last_obu = obu_index + 1 == obus.len();

            let mut aggregation = 0x10; // W=1: one OBU element, no element length field.
            if !first_fragment {
                aggregation |= 0x80; // Z: continuation from previous packet.
            }
            if !last_fragment {
                aggregation |= 0x40; // Y: continues in the next packet.
            }
            if first_packet && keyframe {
                aggregation |= 0x08; // N: first packet of a coded video sequence.
            }

            let mut out = BytesMut::with_capacity(1 + take);
            out.extend_from_slice(&[aggregation]);
            out.extend_from_slice(&obu[cursor..cursor + take]);
            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    payload_type,
                    sequence_number: *seq,
                    timestamp,
                    ssrc,
                    marker: last_obu && last_fragment,
                },
                payload: out.freeze(),
            });
            *seq = seq.wrapping_add(1);
            first_packet = false;
            cursor += take;
        }
    }

    packets
}

fn split_av1_low_overhead_obus(payload: &[u8]) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut cursor = payload;
    while !cursor.is_empty() {
        let Some(header) = cursor.first().copied() else {
            break;
        };
        let has_extension = (header & 0x04) != 0;
        let has_size_field = (header & 0x02) != 0;
        let mut header_len = 1usize;
        if has_extension {
            header_len = header_len.saturating_add(1);
        }
        if cursor.len() < header_len {
            break;
        }
        if !has_size_field {
            out.push(Bytes::copy_from_slice(cursor));
            break;
        }

        let Some((payload_len, leb_len)) = av1_read_leb128(&cursor[header_len..]) else {
            break;
        };
        let Some(payload_offset) = header_len.checked_add(leb_len) else {
            break;
        };
        let Some(obu_len) = payload_offset.checked_add(payload_len) else {
            break;
        };
        if cursor.len() < obu_len {
            break;
        }

        let mut obu = Vec::with_capacity(header_len + payload_len);
        obu.push(header & !0x02);
        if has_extension {
            obu.push(cursor[1]);
        }
        obu.extend_from_slice(&cursor[payload_offset..obu_len]);
        out.push(Bytes::from(obu));
        cursor = &cursor[obu_len..];
    }
    if out.is_empty() && !payload.is_empty() {
        out.push(Bytes::copy_from_slice(payload));
    }
    out
}

fn av1_sequence_header_obus_from_track(track: &TrackInfo) -> Vec<Bytes> {
    let CodecExtradata::AV1 {
        sequence_header,
        codec_config,
    } = &track.extradata
    else {
        return Vec::new();
    };
    if let Some(sequence_header) = sequence_header {
        let obus = split_av1_low_overhead_obus(sequence_header);
        if obus
            .iter()
            .any(|obu| av1_low_overhead_obu_type(obu) == Some(1))
        {
            return obus;
        }
    }
    let Some(codec_config) = codec_config else {
        return Vec::new();
    };
    if codec_config.len() <= 4 || (codec_config[0] & 0x7f) != 1 {
        return Vec::new();
    }
    split_av1_low_overhead_obus(&codec_config[4..])
        .into_iter()
        .filter(|obu| av1_low_overhead_obu_type(obu) == Some(1))
        .collect()
}

fn av1_low_overhead_obu_type(obu: &[u8]) -> Option<u8> {
    obu.first().map(|header| (header >> 3) & 0x0f)
}

fn depacketize_aac(
    payload: &[u8],
    packetization: AacRtpPacketization,
    _latm_config_in_band: bool,
) -> Option<Vec<DepacketizedAac>> {
    match packetization {
        AacRtpPacketization::Mpeg4Generic => depacketize_aac_mpeg4_generic(payload),
        AacRtpPacketization::Latm => depacketize_aac_latm(payload).map(|payload| vec![payload]),
    }
}

fn depacketize_aac_mpeg4_generic(payload: &[u8]) -> Option<Vec<DepacketizedAac>> {
    if payload.len() < 4 {
        return None;
    }

    let headers_bits = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    let headers_bytes = headers_bits.div_ceil(8);
    let mut offset = 2usize;
    if payload.len() < offset + headers_bytes || headers_bytes < 2 {
        return None;
    }
    let au_count = headers_bits / 16;
    if au_count == 0 || au_count * 16 != headers_bits || headers_bytes < au_count * 2 {
        return None;
    }
    let mut au_size_fields = Vec::with_capacity(au_count);
    for index in 0..au_count {
        let header_offset = 2 + index * 2;
        let au_header = u16::from_be_bytes([payload[header_offset], payload[header_offset + 1]]);
        au_size_fields.push((au_header >> 3) as usize);
    }
    offset += headers_bytes;

    let total_size_fields = au_size_fields
        .iter()
        .try_fold(0usize, |acc, size| acc.checked_add(*size))?;
    let au_sizes = if payload.len() >= offset + total_size_fields {
        // Common encoder behavior (including FFmpeg): AU-size field stores bytes.
        au_size_fields
    } else {
        // RFC-style behavior: AU-size field stores bits.
        let sizes = au_size_fields
            .into_iter()
            .map(|size| size.div_ceil(8))
            .collect::<Vec<_>>();
        let total_bits_to_bytes = sizes
            .iter()
            .try_fold(0usize, |acc, size| acc.checked_add(*size))?;
        if payload.len() < offset + total_bits_to_bytes {
            return None;
        }
        sizes
    };

    let mut payload_offset = offset;
    let mut timestamp_offset = 0u32;
    let mut access_units = Vec::with_capacity(au_sizes.len());
    for au_size in au_sizes {
        let payload_end = payload_offset.checked_add(au_size)?;
        access_units.push(DepacketizedAac {
            payload: Bytes::copy_from_slice(payload.get(payload_offset..payload_end)?),
            timestamp_offset,
            discovered_asc: None,
        });
        payload_offset = payload_end;
        timestamp_offset = timestamp_offset.wrapping_add(1024);
    }
    Some(access_units)
}

fn depacketize_aac_latm(payload: &[u8]) -> Option<DepacketizedAac> {
    depacketize_aac_latm_bitpacked(payload)
        .or_else(|| depacketize_aac_latm_length_prefixed(payload))
}

fn depacketize_aac_latm_length_prefixed(payload: &[u8]) -> Option<DepacketizedAac> {
    if payload.is_empty() {
        return None;
    }
    let mut offset = 0usize;
    let mut payload_len = 0usize;
    loop {
        let chunk = usize::from(*payload.get(offset)?);
        offset += 1;
        payload_len = payload_len.checked_add(chunk)?;
        if chunk != 255 {
            break;
        }
    }
    if payload_len == 0 || payload.len() != offset + payload_len {
        return None;
    }
    Some(DepacketizedAac {
        payload: Bytes::copy_from_slice(&payload[offset..offset + payload_len]),
        timestamp_offset: 0,
        discovered_asc: None,
    })
}

fn depacketize_aac_latm_bitpacked(payload: &[u8]) -> Option<DepacketizedAac> {
    let mut bits = BitReader::new(payload);
    let use_same_stream_mux = bits.read_bit()?;
    let discovered_asc = if use_same_stream_mux == 0 {
        Some(parse_latm_stream_mux_config(&mut bits)?)
    } else {
        None
    };

    let mut payload_len = 0usize;
    loop {
        let chunk = bits.read_bits(8)? as usize;
        payload_len = payload_len.checked_add(chunk)?;
        if chunk != 255 {
            break;
        }
    }
    let raw = bits.read_bytes(payload_len)?;
    if bits.remaining_bits() > 7 || !bits.remaining_bits_are_zero() {
        return None;
    }
    Some(DepacketizedAac {
        payload: Bytes::from(raw),
        timestamp_offset: 0,
        discovered_asc,
    })
}

fn parse_latm_stream_mux_config(bits: &mut BitReader<'_>) -> Option<Bytes> {
    let audio_mux_version = bits.read_bit()?;
    let audio_mux_version_a = if audio_mux_version == 1 {
        bits.read_bit()?
    } else {
        0
    };
    if audio_mux_version_a != 0 {
        return None;
    }
    if audio_mux_version == 1 {
        let _tara_buffer_fullness = latm_get_value(bits)?;
    }
    let all_streams_same_time_framing = bits.read_bit()?;
    if all_streams_same_time_framing == 0 {
        return None;
    }
    let num_sub_frames = bits.read_bits(6)?;
    if num_sub_frames != 0 {
        return None;
    }
    let num_programs = bits.read_bits(4)?;
    if num_programs != 0 {
        return None;
    }
    let num_layers = bits.read_bits(3)?;
    if num_layers != 0 {
        return None;
    }

    let asc = if audio_mux_version == 0 {
        let asc_start = bits.bit_offset();
        let asc_tail = bits_to_bytes(bits.data, asc_start, bits.data.len().saturating_mul(8))?;
        let mut asc_bits = BitReader::new(asc_tail.as_ref());
        parse_latm_audio_specific_config(&mut asc_bits)?;
        let asc_end = asc_start.checked_add(asc_bits.bit_offset())?;
        bits.skip_bits(asc_end.saturating_sub(asc_start))?;
        bits_to_bytes(bits.data, asc_start, asc_end)?
    } else {
        let asc_len_bits = usize::try_from(latm_get_value(bits)?).ok()?;
        if asc_len_bits == 0 {
            return None;
        }
        let asc_start = bits.bit_offset();
        bits.skip_bits(asc_len_bits)?;
        bits_to_bytes(bits.data, asc_start, bits.bit_offset())?
    };

    let mut asc_bits = BitReader::new(asc.as_ref());
    parse_latm_audio_specific_config(&mut asc_bits)?;
    if asc
        .len()
        .saturating_mul(8)
        .saturating_sub(asc_bits.bit_offset())
        > 7
        || !asc_bits.remaining_bits_are_zero()
    {
        return None;
    }

    let frame_length_type = bits.read_bits(3)?;
    match frame_length_type {
        0 => bits.skip_bits(8)?,
        1 => bits.skip_bits(9)?,
        3..=5 => bits.skip_bits(6)?,
        6 | 7 => bits.skip_bits(1)?,
        _ => return None,
    }

    let other_data_present = bits.read_bit()?;
    if other_data_present == 1 {
        if audio_mux_version == 1 {
            let other_data_len_bits = usize::try_from(latm_get_value(bits)?).ok()?;
            bits.skip_bits(other_data_len_bits)?;
        } else {
            loop {
                let esc = bits.read_bit()?;
                bits.skip_bits(8)?;
                if esc == 0 {
                    break;
                }
            }
        }
    }

    let crc_check_present = bits.read_bit()?;
    if crc_check_present == 1 {
        bits.skip_bits(8)?;
    }
    Some(asc)
}

fn latm_get_value(bits: &mut BitReader<'_>) -> Option<u32> {
    let bytes_for_value = (bits.read_bits(2)? as usize) + 1;
    let mut value = 0u32;
    for _ in 0..bytes_for_value {
        value = (value << 8) | bits.read_bits(8)?;
    }
    Some(value)
}

fn parse_latm_audio_specific_config(bits: &mut BitReader<'_>) -> Option<()> {
    parse_latm_audio_specific_config_without_sync_extension(bits)?;

    if bits.remaining_bits() >= 11 && bits.peek_bits(11)? == 0x2B7 {
        bits.skip_bits(11)?;
        let ext_object_type = get_latm_audio_object_type(bits)?;
        if ext_object_type == 5 {
            bits.skip_bits(1)?;
            let ext_sampling_frequency_index = bits.read_bits(4)?;
            if ext_sampling_frequency_index == 0x0F {
                bits.skip_bits(24)?;
            }
            if bits.remaining_bits() >= 11 && bits.peek_bits(11)? == 0x548 {
                bits.skip_bits(11)?;
                bits.skip_bits(1)?;
            }
        }
    }
    Some(())
}

fn parse_latm_audio_specific_config_without_sync_extension(bits: &mut BitReader<'_>) -> Option<()> {
    let audio_object_type = get_latm_audio_object_type(bits)?;
    let sampling_frequency_index = bits.read_bits(4)?;
    if sampling_frequency_index == 0x0F {
        bits.skip_bits(24)?;
    }
    let channel_configuration = bits.read_bits(4)? as u8;

    if audio_object_type == 5 || audio_object_type == 29 {
        let extension_sampling_frequency_index = bits.read_bits(4)?;
        if extension_sampling_frequency_index == 0x0F {
            bits.skip_bits(24)?;
        }
        let ext_object_type = get_latm_audio_object_type(bits)?;
        if ext_object_type == 22 {
            bits.skip_bits(4)?;
        }
    }

    match audio_object_type {
        1 | 2 | 3 | 4 | 6 | 7 | 17 | 19 | 20 | 21 | 22 | 23 => {
            parse_latm_ga_specific_config(bits, audio_object_type, channel_configuration)?;
        }
        _ => return None,
    }

    if audio_object_type == 17
        || audio_object_type == 19
        || audio_object_type == 20
        || audio_object_type == 23
    {
        bits.skip_bits(2)?;
    }
    Some(())
}

fn get_latm_audio_object_type(bits: &mut BitReader<'_>) -> Option<u8> {
    let object_type = bits.read_bits(5)? as u8;
    if object_type == 31 {
        let extended = bits.read_bits(6)? as u8;
        Some(32 + extended)
    } else {
        Some(object_type)
    }
}

fn parse_latm_ga_specific_config(
    bits: &mut BitReader<'_>,
    object_type: u8,
    channel_configuration: u8,
) -> Option<()> {
    bits.skip_bits(1)?;
    if bits.read_bit()? == 1 {
        bits.skip_bits(14)?;
    }
    let extension_flag = bits.read_bit()?;
    if channel_configuration == 0 {
        parse_latm_program_config_element(bits)?;
    }
    if matches!(object_type, 6 | 20) {
        bits.skip_bits(3)?;
    }
    if extension_flag == 1 {
        if object_type == 22 {
            bits.skip_bits(16)?;
        }
        if matches!(object_type, 17 | 19 | 20 | 23) {
            bits.skip_bits(3)?;
        }
        bits.skip_bits(1)?;
    }
    Some(())
}

fn parse_latm_program_config_element(bits: &mut BitReader<'_>) -> Option<()> {
    bits.skip_bits(10)?;
    let num_front = bits.read_bits(4)? as usize;
    let num_side = bits.read_bits(4)? as usize;
    let num_back = bits.read_bits(4)? as usize;
    let num_lfe = bits.read_bits(2)? as usize;
    let num_assoc_data = bits.read_bits(3)? as usize;
    let num_valid_cc = bits.read_bits(4)? as usize;

    if bits.read_bit()? == 1 {
        bits.skip_bits(4)?;
    }
    if bits.read_bit()? == 1 {
        bits.skip_bits(4)?;
    }
    if bits.read_bit()? == 1 {
        bits.skip_bits(3)?;
    }

    for _ in 0..num_front {
        bits.skip_bits(5)?;
    }
    for _ in 0..num_side {
        bits.skip_bits(5)?;
    }
    for _ in 0..num_back {
        bits.skip_bits(5)?;
    }
    for _ in 0..num_lfe {
        bits.skip_bits(4)?;
    }
    for _ in 0..num_assoc_data {
        bits.skip_bits(4)?;
    }
    for _ in 0..num_valid_cc {
        bits.skip_bits(5)?;
    }

    bits.byte_align()?;
    let comment_bytes = bits.read_bits(8)? as usize;
    bits.skip_bits(comment_bytes.saturating_mul(8))?;
    Some(())
}

fn packetize_aac(
    payload: &[u8],
    payload_type: u8,
    seq: &mut u16,
    timestamp: u32,
    ssrc: u32,
    mtu: usize,
) -> Vec<RtpPacket> {
    if payload.is_empty() || payload.len() > 0x1fff {
        return Vec::new();
    }

    let max_payload = mtu.saturating_sub(12).max(1);
    if max_payload <= 4 || payload.len() > (max_payload - 4) {
        return Vec::new();
    }

    let au_header = ((payload.len() as u16) << 3).to_be_bytes();
    let mut out = BytesMut::with_capacity(payload.len() + 4);
    out.extend_from_slice(&[0x00, 0x10]);
    out.extend_from_slice(&au_header);
    out.extend_from_slice(payload);

    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type,
            sequence_number: *seq,
            timestamp,
            ssrc,
            marker: true,
        },
        payload: out.freeze(),
    };
    *seq = seq.wrapping_add(1);
    vec![packet]
}

fn packetize_passthrough(
    payload: &[u8],
    payload_type: u8,
    seq: &mut u16,
    timestamp: u32,
    ssrc: u32,
    mtu: usize,
    marker: bool,
) -> Vec<RtpPacket> {
    if payload.is_empty() {
        return Vec::new();
    }
    let max_payload = mtu.saturating_sub(12).max(1);
    if payload.len() > max_payload {
        return Vec::new();
    }
    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type,
            sequence_number: *seq,
            timestamp,
            ssrc,
            marker,
        },
        payload: Bytes::copy_from_slice(payload),
    };
    *seq = seq.wrapping_add(1);
    vec![packet]
}

fn split_annexb_units(mut payload: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((start, code_len)) = find_start_code(payload) {
        payload = &payload[start + code_len..];
        let next_start = find_start_code(payload)
            .map(|(idx, _)| idx)
            .unwrap_or(payload.len());
        if next_start > 0 {
            out.push(&payload[..next_start]);
        }
        payload = &payload[next_start..];
    }
    out
}

fn find_start_code(data: &[u8]) -> Option<(usize, usize)> {
    if data.len() < 3 {
        return None;
    }
    for i in 0..(data.len() - 2) {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
    }
    None
}

fn extract_uri_path(uri: &str) -> Option<&str> {
    let mut source = uri.trim();
    if let Some(rest) = source
        .strip_prefix("rtsp://")
        .or_else(|| source.strip_prefix("rtsps://"))
    {
        source = rest;
        if let Some(index) = source.find('/') {
            source = &source[index..];
        } else {
            return None;
        }
    }

    if let Some(index) = source.find('?') {
        source = &source[..index];
    }

    Some(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::TrackId;

    fn single_aac_au(mut access_units: Vec<DepacketizedAac>) -> DepacketizedAac {
        assert_eq!(access_units.len(), 1);
        access_units.remove(0)
    }

    fn encode_aac_latm_bitpacked(raw: &[u8]) -> Vec<u8> {
        if raw.is_empty() {
            return Vec::new();
        }

        let mut bits = Vec::with_capacity((raw.len() + 2) * 8);
        bits.push(1); // useSameStreamMux

        let mut remaining = raw.len();
        while remaining >= 255 {
            for shift in (0..8).rev() {
                bits.push(((255 >> shift) & 1) as u8);
            }
            remaining -= 255;
        }

        let chunk = remaining as u8;
        for shift in (0..8).rev() {
            bits.push((chunk >> shift) & 1);
        }

        for &byte in raw {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1);
            }
        }

        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (idx, bit) in bits.into_iter().enumerate() {
            if bit == 1 {
                out[idx / 8] |= 1 << (7 - (idx % 8));
            }
        }
        out
    }

    fn push_test_bits(bits: &mut Vec<u8>, value: u32, width: usize) {
        for shift in (0..width).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    }

    fn pack_test_bits(bits: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (i, bit) in bits.iter().enumerate() {
            out[i / 8] |= *bit << (7usize.saturating_sub(i % 8));
        }
        out
    }

    fn push_test_bit_slice(bits: &mut Vec<u8>, bytes: &[u8], bit_len: usize) {
        for index in 0..bit_len {
            let byte = bytes[index / 8];
            bits.push((byte >> (7 - (index % 8))) & 1);
        }
    }

    fn push_test_audio_specific_config(bits: &mut Vec<u8>, asc: &[u8]) {
        let mut reader = BitReader::new(asc);
        parse_latm_audio_specific_config(&mut reader).expect("parse test asc");
        push_test_bit_slice(bits, asc, reader.bit_offset);
    }

    #[test]
    fn parses_transport_interleaved() {
        let value = "RTP/AVP/TCP;unicast;interleaved=2-3";
        let parsed = parse_transport_interleaved(value).expect("parse transport");
        assert_eq!(parsed.rtp_channel, 2);
        assert_eq!(parsed.rtcp_channel, 3);
    }

    #[test]
    fn parses_transport_udp_ports() {
        let value = "RTP/AVP;unicast;client_port=5000-5001";
        let parsed = parse_transport_udp_ports(value).expect("parse udp transport");
        assert_eq!(parsed.client_rtp_port, 5000);
        assert_eq!(parsed.client_rtcp_port, 5001);
        assert_eq!(parsed.destination, None);
    }

    #[test]
    fn parses_setup_transport_udp_server_ports() {
        let value = "RTP/AVP;unicast;client_port=5000-5001;server_port=62000-62001";
        let parsed = parse_setup_transport(value).expect("parse transport");
        let RtspSetupTransport::UdpUnicast(parsed) = parsed else {
            panic!("expected udp transport");
        };
        assert_eq!(parsed.server_rtp_port, Some(62000));
        assert_eq!(parsed.server_rtcp_port, Some(62001));
    }

    #[test]
    fn parses_setup_transport_udp_without_unicast_token() {
        let value = "RTP/AVP;client_port=5000-5001";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::UdpUnicast(TransportUdpPorts {
                client_rtp_port: 5000,
                client_rtcp_port: 5001,
                server_rtp_port: None,
                server_rtcp_port: None,
                destination: None,
                source: None
            })
        ));
    }

    #[test]
    fn parses_setup_transport_multicast_without_requested_ports() {
        let value = "RTP/AVP;multicast";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::UdpMulticast(TransportUdpMulticast {
                rtp_port: None,
                rtcp_port: None,
                destination: None,
                ttl: None
            })
        ));
    }

    #[test]
    fn rejects_invalid_interleaved_channel_pairs() {
        assert!(parse_transport_interleaved("RTP/AVP/TCP;interleaved=2-2").is_none());
        assert!(parse_transport_interleaved("RTP/AVP/TCP;interleaved=255").is_none());
    }

    #[test]
    fn rejects_invalid_udp_port_pairs() {
        assert!(parse_transport_udp_ports("RTP/AVP;client_port=5000-5000").is_none());
        assert!(parse_transport_udp_ports("RTP/AVP;client_port=65535").is_none());
    }

    #[test]
    fn parses_stream_key_from_uri() {
        let key = parse_stream_key_from_uri("rtsp://127.0.0.1/live/cam01").expect("stream key");
        assert_eq!(key.namespace, "live");
        assert_eq!(key.path, "cam01");
    }

    #[test]
    fn parses_setup_transport_with_multiple_candidates() {
        let value = "RTP/AVP/TCP;unicast;interleaved=0-1, RTP/AVP;unicast;client_port=5000-5001";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::TcpInterleaved(TransportInterleaved {
                rtp_channel: 0,
                rtcp_channel: 1
            })
        ));
    }

    #[test]
    fn parses_setup_transport_fallbacks_to_next_candidate() {
        let value = "RTP/AVP/TCP;unicast;interleaved=bad, RTP/AVP;unicast;client_port=5000-5001";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::UdpUnicast(TransportUdpPorts {
                client_rtp_port: 5000,
                client_rtcp_port: 5001,
                server_rtp_port: None,
                server_rtcp_port: None,
                destination: None,
                source: None
            })
        ));
    }

    #[test]
    fn parses_setup_transport_accepts_udp_protocol_variant() {
        let value = "RTP/AVP/UDP;unicast;client_port=6200-6201";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::UdpUnicast(TransportUdpPorts {
                client_rtp_port: 6200,
                client_rtcp_port: 6201,
                server_rtp_port: None,
                server_rtcp_port: None,
                destination: None,
                source: None
            })
        ));
    }

    #[test]
    fn parses_setup_transport_with_destination_ip() {
        let value = "RTP/AVP;unicast;client_port=6000-6001;destination=127.0.0.1";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::UdpUnicast(TransportUdpPorts {
                client_rtp_port: 6000,
                client_rtcp_port: 6001,
                server_rtp_port: None,
                server_rtcp_port: None,
                destination: Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
                source: None,
            })
        ));
    }

    #[test]
    fn parses_setup_transport_with_source_ip() {
        let value = "RTP/AVP;unicast;client_port=6000-6001;source=10.1.2.3";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        let RtspSetupTransport::UdpUnicast(ports) = parsed else {
            panic!("expected udp unicast setup transport");
        };
        assert_eq!(ports.client_rtp_port, 6000);
        assert_eq!(ports.client_rtcp_port, 6001);
        assert_eq!(ports.server_rtp_port, None);
        assert_eq!(ports.server_rtcp_port, None);
        assert_eq!(ports.destination, None);
        assert_eq!(
            ports.source,
            Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 1, 2, 3)))
        );
    }

    #[test]
    fn parses_setup_transport_ignores_private_parameters() {
        let value = "RTP/AVP/TCP;unicast;interleaved=6-7;x-dynamic-rate=1;foo=bar";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(
            parsed,
            RtspSetupTransport::TcpInterleaved(TransportInterleaved {
                rtp_channel: 6,
                rtcp_channel: 7
            })
        ));
    }

    #[test]
    fn parses_setup_transport_multicast() {
        let value = "RTP/AVP;multicast;destination=239.1.2.3;port=5000-5001;ttl=16";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        let RtspSetupTransport::UdpMulticast(transport) = parsed else {
            panic!("expected multicast transport");
        };
        assert_eq!(transport.rtp_port, Some(5000));
        assert_eq!(transport.rtcp_port, Some(5001));
        assert_eq!(
            transport.destination,
            Some(std::net::Ipv4Addr::new(239, 1, 2, 3))
        );
        assert_eq!(transport.ttl, Some(16));
    }

    #[test]
    fn parses_setup_transport_tcp_without_interleaved_as_auto_assignment() {
        let value = "RTP/AVP/TCP;unicast";
        let parsed = parse_setup_transport(value).expect("parse setup transport");
        assert!(matches!(parsed, RtspSetupTransport::TcpInterleavedAuto));
    }

    #[test]
    fn parses_transport_params_case_insensitive_with_spaces() {
        let interleaved =
            parse_transport_interleaved("RTP/AVP/TCP;unicast; Interleaved = 4-5").expect("tcp");
        assert_eq!(interleaved.rtp_channel, 4);
        assert_eq!(interleaved.rtcp_channel, 5);

        let udp =
            parse_transport_udp_ports("RTP/AVP;unicast; Client_Port = 6000-6001").expect("udp");
        assert_eq!(udp.client_rtp_port, 6000);
        assert_eq!(udp.client_rtcp_port, 6001);
    }

    #[test]
    fn builds_and_parses_rtcp_sender_report() {
        let sr = build_rtcp_sender_report(0x1122_3344, 9000, 10, 2048, 1_700_000_000_123_456)
            .expect("build sender report");
        let parsed = parse_rtcp_sender_report(&sr)
            .expect("parse sender report")
            .expect("sender report payload");
        assert_eq!(parsed.sender_ssrc, 0x1122_3344);
        assert!(parsed.lsr > 0);
        assert_eq!(sr.len(), 28);
    }

    #[test]
    fn builds_rtcp_receiver_report() {
        let rr = build_rtcp_receiver_report(
            0x5566_7788,
            RtcpReceiverReportBlock {
                sender_ssrc: 0x0102_0304,
                fraction_lost: 2,
                cumulative_lost: 5,
                extended_highest_seq: 99,
                jitter: 7,
                lsr: 88,
                dlsr: 1234,
            },
        )
        .expect("build receiver report");
        let packets = RtcpPacket::parse(&rr).expect("parse receiver report");
        assert!(matches!(
            packets.first(),
            Some(RtcpPacket::ReceiverReport(_))
        ));
        assert_eq!(rr.len(), 32);
    }

    #[test]
    fn builds_rtcp_sdes_and_bye() {
        let sdes = build_rtcp_sdes_cname(0x1122_3344, "cheetah").expect("build sdes");
        let sdes_packets = RtcpPacket::parse(&sdes).expect("parse sdes");
        assert!(matches!(
            sdes_packets.first(),
            Some(RtcpPacket::SourceDescription(_))
        ));
        assert_eq!(sdes[0] & 0x1f, 1);

        let bye = build_rtcp_bye(0x1122_3344, Some("teardown")).expect("build bye");
        let bye_packets = RtcpPacket::parse(&bye).expect("parse bye");
        assert!(matches!(bye_packets.first(), Some(RtcpPacket::Bye(_))));
        assert_eq!(bye[0] & 0x1f, 1);
        assert!(bye.len() >= 8);
    }

    #[test]
    fn parse_sender_report_rejects_invalid_rtcp_payload() {
        let payload = [0b0100_0000, 200, 0, 0];
        let err = parse_rtcp_sender_report(&payload).expect_err("invalid version must fail");
        assert!(matches!(
            err,
            cheetah_rtsp_core::RtcpError::UnsupportedVersion { actual: 1 }
        ));
    }

    #[test]
    fn parse_sender_report_ignores_non_sender_report_packets() {
        let payload = build_rtcp_bye(0x1122_3344, None).expect("build bye");
        let parsed = parse_rtcp_sender_report(&payload).expect("parse payload");
        assert!(parsed.is_none());
    }

    #[test]
    fn depacketize_h264_non_idr_i_slice_is_not_keyframe() {
        let mut fu_buffer = Vec::new();
        // NAL type=1 + first_mb_in_slice=0 + slice_type=2(I)
        let (_payload, keyframe) = depacketize_h264(&[0x41, 0xB8], &mut fu_buffer).expect("pkt");
        assert!(!keyframe);
    }

    #[test]
    fn depacketize_h264_non_idr_p_slice_is_not_keyframe() {
        let mut fu_buffer = Vec::new();
        // NAL type=1 + first_mb_in_slice=0 + slice_type=0(P)
        let (_payload, keyframe) = depacketize_h264(&[0x41, 0xC0], &mut fu_buffer).expect("pkt");
        assert!(!keyframe);
    }

    #[test]
    fn depacketize_h264_fua_non_idr_i_slice_is_not_keyframe() {
        let mut fu_buffer = Vec::new();
        let start = depacketize_h264(&[0x5C, 0x81, 0xB8], &mut fu_buffer);
        assert!(start.is_none(), "fua start should buffer until end");
        let (_payload, keyframe) = depacketize_h264(&[0x5C, 0x41], &mut fu_buffer).expect("fua");
        assert!(!keyframe);
    }

    #[test]
    fn depacketize_h264_ignores_standalone_aud() {
        let mut fu_buffer = Vec::new();
        assert!(depacketize_h264(&[0x09, 0xF0], &mut fu_buffer).is_none());
    }

    #[test]
    fn vp8_rtp_payload_keyframe_detection_uses_start_partition_zero() {
        assert!(vp8_rtp_payload_is_keyframe(&[0x10, 0x00]));
        assert!(!vp8_rtp_payload_is_keyframe(&[0x10, 0x01]));
        assert!(!vp8_rtp_payload_is_keyframe(&[0x00, 0x00]));
    }

    #[test]
    fn av1_rtp_payload_keyframe_detection_uses_frame_header_obu() {
        // W=1, first packet payload contains one full OBU element.
        // OBU type=3(frame header), show_existing_frame=0, frame_type=0(KEY_FRAME).
        assert!(av1_rtp_payload_is_keyframe(&[0x10, 0x18, 0x00]));
        // OBU type=3(frame header), show_existing_frame=0, frame_type=1(INTER_FRAME).
        assert!(!av1_rtp_payload_is_keyframe(&[0x10, 0x18, 0x40]));
        // N-bit does not imply keyframe.
        assert!(!av1_rtp_payload_is_keyframe(&[0x18, 0x18, 0x40]));
        assert!(!av1_rtp_payload_is_keyframe(&[]));
    }

    #[test]
    fn build_frame_from_rtp_av1_strips_rtp_aggregation_header() {
        let track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 44,
                timestamp: 90_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            // AV1 RTP aggregation header W=1 followed by one OBU element.
            // The published media frame must contain only the canonical OBU bytes.
            payload: Bytes::from_static(&[0x10, 0x18, 0x00]),
        };
        let mut clock = PublishTrackClock::default();
        let mut av1_state = PublishAv1Depacketizer::default();

        let built = build_frame_from_rtp(
            &track,
            &packet,
            &mut clock,
            None,
            None,
            Some(&mut av1_state),
        )
        .expect("frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalAv1Obu);
        assert_eq!(built.frame.payload.as_ref(), &[0x1a, 0x01, 0x00]);
        assert!(built.frame.flags.contains(FrameFlags::START_OF_AU));
        assert!(built.frame.flags.contains(FrameFlags::END_OF_AU));
        assert!(built.frame.flags.contains(FrameFlags::KEY));
    }

    #[test]
    fn av1_sequence_header_builds_codec_config_for_rtmp_bootstrap() {
        let sequence_header = Bytes::from_static(&[
            0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x71, 0xab, 0xe7, 0x40, 0x40,
            0x80, 0x49,
        ]);
        let config = av1_codec_config_from_sequence_header(&sequence_header).expect("av1c");

        assert_eq!(
            config.as_ref(),
            &[
                0x81, 0x09, 0x4d, 0x00, 0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x71,
                0xab, 0xe7, 0x40, 0x40, 0x80, 0x49,
            ]
        );
    }

    #[test]
    fn vp9_rtp_payload_keyframe_detection_parses_beginning_of_frame() {
        assert!(vp9_rtp_payload_is_keyframe(&[0x0C, 0x82]));
        assert!(!vp9_rtp_payload_is_keyframe(&[0x0C, 0x86]));
        assert!(!vp9_rtp_payload_is_keyframe(&[0x04, 0x82]));
    }

    #[test]
    fn build_frame_from_rtp_vp9_strips_payload_descriptor() {
        let track = TrackInfo::new(TrackId(7), MediaKind::Video, CodecId::VP9, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 98,
                sequence_number: 17,
                timestamp: 9_000,
                ssrc: 0x2233_4455,
                marker: true,
            },
            payload: Bytes::from_static(&[0x0c, 0x82, 0x49, 0x83]),
        };
        let mut clock = PublishTrackClock::default();

        let built =
            build_frame_from_rtp(&track, &packet, &mut clock, None, None, None).expect("vp9 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalVp9Frame);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x82, 0x49, 0x83]);
    }

    #[test]
    fn build_frame_from_rtp_vp9_aggregates_fragments_until_marker() {
        let track = TrackInfo::new(TrackId(7), MediaKind::Video, CodecId::VP9, 90_000);
        let first = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 98,
                sequence_number: 17,
                timestamp: 9_000,
                ssrc: 0x2233_4455,
                marker: false,
            },
            payload: Bytes::from_static(&[0x08, 0x82, 0x49]),
        };
        let second = RtpPacket {
            header: RtpHeader {
                sequence_number: 18,
                marker: true,
                ..first.header
            },
            payload: Bytes::from_static(&[0x04, 0x83, 0x42]),
        };
        let clock = PublishTrackClock::default();
        let mut state = PublishVp9Depacketizer::default();

        assert!(build_vp9_frame_from_rtp(&track, &first, &clock, &mut state).is_none());
        let built = build_vp9_frame_from_rtp(&track, &second, &clock, &mut state)
            .expect("complete vp9 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalVp9Frame);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x82, 0x49, 0x83, 0x42]);
    }

    #[test]
    fn build_frame_from_rtp_vp8_strips_payload_descriptor() {
        let track = TrackInfo::new(TrackId(8), MediaKind::Video, CodecId::VP8, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 19,
                timestamp: 9_000,
                ssrc: 0x3344_5566,
                marker: true,
            },
            payload: Bytes::from_static(&[0x10, 0x00, 0x9d, 0x01, 0x2a]),
        };
        let mut clock = PublishTrackClock::default();

        let built =
            build_frame_from_rtp(&track, &packet, &mut clock, None, None, None).expect("vp8 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalVp8Frame);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x00, 0x9d, 0x01, 0x2a]);
    }

    #[test]
    fn build_frame_from_rtp_vp8_aggregates_fragments_until_marker() {
        let track = TrackInfo::new(TrackId(8), MediaKind::Video, CodecId::VP8, 90_000);
        let first = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 19,
                timestamp: 9_000,
                ssrc: 0x3344_5566,
                marker: false,
            },
            payload: Bytes::from_static(&[0x10, 0x00, 0x9d]),
        };
        let second = RtpPacket {
            header: RtpHeader {
                sequence_number: 20,
                marker: true,
                ..first.header
            },
            payload: Bytes::from_static(&[0x00, 0x01, 0x2a]),
        };
        let clock = PublishTrackClock::default();
        let mut state = PublishVp8Depacketizer::default();

        assert!(build_vp8_frame_from_rtp(&track, &first, &clock, &mut state).is_none());
        let built = build_vp8_frame_from_rtp(&track, &second, &clock, &mut state)
            .expect("complete vp8 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalVp8Frame);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x00, 0x9d, 0x01, 0x2a]);
    }

    #[test]
    fn build_frame_from_rtp_vp8_keeps_later_partition_starts_in_same_frame() {
        let track = TrackInfo::new(TrackId(8), MediaKind::Video, CodecId::VP8, 90_000);
        let first = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 19,
                timestamp: 9_000,
                ssrc: 0x3344_5566,
                marker: false,
            },
            payload: Bytes::from_static(&[0x10, 0x00, 0x9d]),
        };
        let second_partition = RtpPacket {
            header: RtpHeader {
                sequence_number: 20,
                marker: true,
                ..first.header
            },
            payload: Bytes::from_static(&[0x11, 0xaa, 0xbb]),
        };
        let clock = PublishTrackClock::default();
        let mut state = PublishVp8Depacketizer::default();

        assert!(build_vp8_frame_from_rtp(&track, &first, &clock, &mut state).is_none());
        let built = build_vp8_frame_from_rtp(&track, &second_partition, &clock, &mut state)
            .expect("complete multi-partition vp8 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalVp8Frame);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x00, 0x9d, 0xaa, 0xbb]);
    }

    #[test]
    fn build_frame_from_rtp_vp8_drops_fragment_when_frame_start_was_lost() {
        let track = TrackInfo::new(TrackId(8), MediaKind::Video, CodecId::VP8, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 21,
                timestamp: 9_000,
                ssrc: 0x3344_5566,
                marker: true,
            },
            payload: Bytes::from_static(&[0x00, 0x01, 0x2a]),
        };
        let clock = PublishTrackClock::default();
        let mut state = PublishVp8Depacketizer::default();

        assert!(build_vp8_frame_from_rtp(&track, &packet, &clock, &mut state).is_none());
    }

    #[test]
    fn depacketize_h264_access_unit_waits_for_timestamp_boundary_after_marker() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from_static(&[0x41, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                sequence_number: 3,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xCC]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
        let (au, keyframe, _, _) =
            depacketize_h264_access_unit(&packet3, &mut state).expect("access unit");
        assert!(!keyframe);
        assert_eq!(
            au.as_ref(),
            &[0, 0, 0, 1, 0x41, 0xAA, 0, 0, 0, 1, 0x41, 0xBB]
        );
    }

    #[test]
    fn depacketize_h264_access_unit_drops_partial_on_timestamp_change() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from_static(&[0x41, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
    }

    #[test]
    fn depacketize_h264_access_unit_marker_boundary_clears_stale_fu_state() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from_static(&[0x5C, 0x81, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                sequence_number: 3,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x5C, 0x41, 0xCC]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
        let (au, _, _, _) =
            depacketize_h264_access_unit(&packet3, &mut state).expect("finished access unit");
        assert_eq!(au.as_ref(), &[0, 0, 0, 1, 0x41, 0xBB]);
    }

    #[test]
    fn depacketize_h264_access_unit_drops_when_au_exceeds_limit_and_can_resync() {
        let mut state = PublishH264Depacketizer::default();
        let mut oversized_payload = vec![0xAA; H264_DEPACKETIZER_MAX_ACCESS_UNIT_BYTES - 4];
        oversized_payload[0] = 0x41;
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from(oversized_payload),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                sequence_number: 3,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xCC]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
        assert!(state.access_unit.is_empty());
        assert!(state.fu_buffer.is_empty());
        assert_eq!(state.access_unit_timestamp, None);
        assert!(!state.access_unit_keyframe);

        assert!(depacketize_h264_access_unit(&packet3, &mut state).is_none());
    }

    #[test]
    fn depacketize_h264_access_unit_drops_when_fu_exceeds_limit_and_can_resync() {
        let mut state = PublishH264Depacketizer::default();
        let fu_chunk = vec![0xAB; 64 * 1024];
        let overflow_packet_count = (H264_DEPACKETIZER_MAX_FU_BUFFER_BYTES / fu_chunk.len()) + 2;
        let overflow_packet_count = u16::try_from(overflow_packet_count).expect("count fits u16");
        let mut fu_start_payload = Vec::with_capacity(fu_chunk.len() + 2);
        fu_start_payload.extend_from_slice(&[0x5C, 0x81]);
        fu_start_payload.extend_from_slice(&fu_chunk);
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from(fu_start_payload),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        let mut overflow_triggered = false;
        for sequence_offset in 0..overflow_packet_count {
            let mut payload = Vec::with_capacity(fu_chunk.len() + 2);
            payload.extend_from_slice(&[0x5C, 0x01]);
            payload.extend_from_slice(&fu_chunk);
            let packet = RtpPacket {
                header: RtpHeader {
                    sequence_number: packet1.header.sequence_number + 1 + sequence_offset,
                    ..packet1.header
                },
                payload: Bytes::from(payload),
            };
            assert!(depacketize_h264_access_unit(&packet, &mut state).is_none());
            if state.fu_buffer.is_empty() {
                overflow_triggered = true;
                break;
            }
        }

        assert!(overflow_triggered);
        assert!(state.fu_buffer.is_empty());
        assert!(state.access_unit.is_empty());
        assert_eq!(state.access_unit_timestamp, None);
        assert!(!state.access_unit_keyframe);

        let packet_resume = RtpPacket {
            header: RtpHeader {
                sequence_number: packet1.header.sequence_number + 1 + overflow_packet_count,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xDD]),
        };
        assert!(depacketize_h264_access_unit(&packet_resume, &mut state).is_none());
    }

    #[test]
    fn depacketize_h264_access_unit_handles_multiple_markers_same_timestamp() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x41, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                sequence_number: 3,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xCC]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
        let (au, keyframe, _, _) =
            depacketize_h264_access_unit(&packet3, &mut state).expect("completed access unit");
        assert!(!keyframe);
        assert_eq!(
            au.as_ref(),
            &[0, 0, 0, 1, 0x41, 0xAA, 0, 0, 0, 1, 0x41, 0xBB]
        );
    }

    #[test]
    fn depacketize_h264_keyframe_marker_emits_immediately() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x65, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };

        let (au, keyframe, _, _) =
            depacketize_h264_access_unit(&packet1, &mut state).expect("access unit");
        assert!(keyframe);
        assert_eq!(au.as_ref(), &[0, 0, 0, 1, 0x65, 0xAA]);
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
    }

    #[test]
    fn depacketize_h264_same_timestamp_followup_after_marker_keeps_idr_in_access_unit() {
        let mut state = PublishH264Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x67, 0x64, 0x00, 0x1f]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 2,
                marker: false,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x68, 0xeb, 0xef, 0x20]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                sequence_number: 3,
                marker: false,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x06, 0x05, 0xff, 0xff]),
        };
        let packet4 = RtpPacket {
            header: RtpHeader {
                sequence_number: 4,
                marker: false,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x65, 0x88, 0x84, 0x21]),
        };
        let packet5 = RtpPacket {
            header: RtpHeader {
                sequence_number: 5,
                timestamp: 12_000,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xAA]),
        };

        assert!(depacketize_h264_access_unit(&packet1, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet2, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet3, &mut state).is_none());
        assert!(depacketize_h264_access_unit(&packet4, &mut state).is_none());
        let (au, keyframe, _, _) =
            depacketize_h264_access_unit(&packet5, &mut state).expect("completed access unit");
        assert!(keyframe);
        assert!(
            au.as_ref().windows(5).any(|w| w == [0, 0, 0, 1, 0x65]),
            "access unit must keep IDR from same timestamp follow-up packets"
        );
    }

    #[test]
    fn packetize_h264_aggregates_small_access_unit_as_stap_a() {
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3,
            ]),
        );
        let mut seq = 7;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x1122_3344,
            1200,
            90_000,
        );

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].header.sequence_number, 7);
        assert!(packets[0].header.marker);
        assert_eq!(packets[0].payload[0] & 0x1f, 24);
        assert_eq!(seq, 8);
    }

    #[test]
    fn packetize_h265_splits_annexb_access_unit_into_hevc_rtp_nalus() {
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0, 0, 0, 1, 0x40, 0x01, 0xaa, // VPS
                0, 0, 0, 1, 0x42, 0x01, 0xbb, // SPS
                0, 0, 0, 1, 0x44, 0x01, 0xcc, // PPS
                0, 0, 0, 1, 0x26, 0x01, 0xdd, // IDR
            ]),
        );
        let mut seq = 7;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x1122_3344,
            1200,
            90_000,
        );

        assert_eq!(packets.len(), 4);
        assert_eq!(packets[0].payload.as_ref(), &[0x40, 0x01, 0xaa]);
        assert_eq!(packets[1].payload.as_ref(), &[0x42, 0x01, 0xbb]);
        assert_eq!(packets[2].payload.as_ref(), &[0x44, 0x01, 0xcc]);
        assert_eq!(packets[3].payload.as_ref(), &[0x26, 0x01, 0xdd]);
        assert!(packets[..3].iter().all(|packet| !packet.header.marker));
        assert!(packets[3].header.marker);
        assert_eq!(seq, 11);
    }

    #[test]
    fn depacketize_aac_mpeg4_generic_extracts_au_payload() {
        let payload = [0x00, 0x10, 0x00, 0x18, 0x11, 0x22, 0x33];
        let au = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Mpeg4Generic, false)
                .expect("au payload"),
        );
        assert_eq!(au.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert_eq!(au.timestamp_offset, 0);
        assert!(au.discovered_asc.is_none());
    }

    #[test]
    fn depacketize_aac_mpeg4_generic_splits_multiple_au_payloads() {
        let payload = [
            0x00, 0x20, // AU-headers-length: two 16-bit AU headers.
            0x00, 0x18, // AU-size: 3 bytes, AU-index: 0.
            0x00, 0x10, // AU-size: 2 bytes, AU-index: 0.
            0x11, 0x22, 0x33, 0x44, 0x55,
        ];
        let aus = depacketize_aac(&payload, AacRtpPacketization::Mpeg4Generic, false)
            .expect("multiple au payloads");

        assert_eq!(aus.len(), 2);
        assert_eq!(aus[0].payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert_eq!(aus[0].timestamp_offset, 0);
        assert!(aus[0].discovered_asc.is_none());
        assert_eq!(aus[1].payload.as_ref(), &[0x44, 0x55]);
        assert_eq!(aus[1].timestamp_offset, 1024);
        assert!(aus[1].discovered_asc.is_none());
    }

    #[test]
    fn depacketize_aac_mpeg4_generic_accepts_bit_sized_au_header() {
        // AU-size=24 bits (3 bytes) encoded using bit semantics.
        let payload = [0x00, 0x10, 0x00, 0xC0, 0x11, 0x22, 0x33];
        let au = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Mpeg4Generic, false)
                .expect("au payload"),
        );
        assert_eq!(au.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert_eq!(au.timestamp_offset, 0);
        assert!(au.discovered_asc.is_none());
    }

    #[test]
    fn packetize_aac_mpeg4_generic_uses_full_au_header() {
        let track = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000);
        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 48_000),
            Bytes::from_static(&[0x11, 0x22, 0x33]),
        );
        let mut seq = 41u16;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            97,
            &mut seq,
            0x1122_3344,
            1200,
            1234,
        );

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].header.sequence_number, 41);
        assert!(packets[0].header.marker);
        assert_eq!(
            packets[0].payload.as_ref(),
            &[0x00, 0x10, 0x00, 0x18, 0x11, 0x22, 0x33]
        );
        assert_eq!(seq, 42);
    }

    #[test]
    fn packetize_aac_mpeg4_generic_rejects_over_mtu_frame() {
        let track = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000);
        let payload: Vec<u8> = (0..20).map(|v| v as u8).collect();
        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 48_000),
            Bytes::from(payload.clone()),
        );
        let mut seq = 9u16;

        // mtu 23 => rtp header 12 + au header 4 + chunk 7
        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            97,
            &mut seq,
            0x5566_7788,
            23,
            777,
        );

        assert!(packets.is_empty());
        assert_eq!(seq, 9);
    }

    #[test]
    fn depacketize_h265_access_unit_waits_for_timestamp_boundary_after_marker() {
        let mut state = PublishH265Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 10_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from_static(&[0x62, 0x01, 0x81, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 2,
                timestamp: 10_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x62, 0x01, 0x41, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 3,
                timestamp: 13_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x02, 0x01, 0xCC]),
        };

        assert!(depacketize_h265_access_unit(CodecId::H265, &packet1, &mut state).is_none());
        assert!(depacketize_h265_access_unit(CodecId::H265, &packet2, &mut state).is_none());
        let (au, keyframe, _, _) =
            depacketize_h265_access_unit(CodecId::H265, &packet3, &mut state).expect("access unit");
        assert!(!keyframe);
        assert_eq!(au.as_ref(), &[0, 0, 0, 1, 0x02, 0x01, 0xAA, 0xBB]);
    }

    #[test]
    fn depacketize_h265_access_unit_bad_marker_drop_recovers_on_following_access_unit() {
        let mut state = PublishH265Depacketizer::default();
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 10_000,
                ssrc: 0x1122_3344,
                marker: false,
            },
            payload: Bytes::from_static(&[0x02, 0x01, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 2,
                timestamp: 13_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x02, 0x01, 0xBB]),
        };
        let packet3 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 3,
                timestamp: 16_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from_static(&[0x02, 0x01, 0xCC]),
        };

        assert!(depacketize_h265_access_unit(CodecId::H265, &packet1, &mut state).is_none());
        assert!(depacketize_h265_access_unit(CodecId::H265, &packet2, &mut state).is_none());
        let (au, keyframe, timestamp, sequence) =
            depacketize_h265_access_unit(CodecId::H265, &packet3, &mut state)
                .expect("next complete access unit should recover after bad marker");

        assert!(!keyframe);
        assert_eq!(timestamp, packet2.header.timestamp);
        assert_eq!(sequence, packet2.header.sequence_number);
        assert_eq!(au.as_ref(), &[0, 0, 0, 1, 0x02, 0x01, 0xBB]);
    }

    #[test]
    fn packetize_passthrough_rejects_payload_over_mtu() {
        let track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        let payload: Vec<u8> = (0..20).map(|v| v as u8).collect();
        let frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::DataPacket,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from(payload),
        );
        let mut seq = 100u16;

        // mtu 23 => rtp header 12 + payload 11
        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x0102_0304,
            23,
            900,
        );

        assert!(packets.is_empty());
        assert_eq!(seq, 100);
    }

    #[test]
    fn packetize_passthrough_accepts_payload_within_mtu() {
        let track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        let frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::DataPacket,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
        );
        let mut seq = 7u16;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x0102_0304,
            23,
            900,
        );

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].header.sequence_number, 7);
        assert!(packets[0].header.marker);
        assert_eq!(packets[0].payload.as_ref(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(seq, 8);
    }

    #[test]
    fn packetize_av1_wraps_canonical_obu_in_rtp_aggregation_header() {
        let track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        let mut frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x1a, 0x01, 0x00]),
        );
        frame.flags.insert(FrameFlags::KEY);
        let mut seq = 7u16;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x0102_0304,
            1200,
            900,
        );

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].payload.as_ref(), &[0x18, 0x18, 0x00]);
        assert!(av1_rtp_payload_is_keyframe(packets[0].payload.as_ref()));
        assert!(packets[0].header.marker);
        assert_eq!(seq, 8);
    }

    #[test]
    fn packetize_av1_keyframe_prepends_sequence_header_from_av1c_config() {
        let mut track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        track.extradata = cheetah_codec::CodecExtradata::AV1 {
            sequence_header: None,
            codec_config: Some(Bytes::from_static(&[
                0x81, 0x09, 0x4d, 0x00, // AV1CodecConfigurationRecord header.
                0x0a, 0x01, 0x00, // sized sequence-header OBU.
            ])),
        };
        let mut frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x1a, 0x01, 0x00]),
        );
        frame.flags.insert(FrameFlags::KEY);
        let mut seq = 7u16;

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x0102_0304,
            1200,
            900,
        );

        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].payload.as_ref(), &[0x18, 0x08, 0x00]);
        assert!(!packets[0].header.marker);
        assert_eq!(packets[1].payload.as_ref(), &[0x10, 0x18, 0x00]);
        assert!(av1_rtp_payload_is_keyframe(packets[1].payload.as_ref()));
        assert!(packets[1].header.marker);
        assert_eq!(seq, 9);
    }

    #[test]
    fn build_frame_from_rtp_vp8_emits_source_timestamp_metadata() {
        let track = TrackInfo::new(TrackId(4), MediaKind::Video, CodecId::VP8, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 11,
                timestamp: 9_000,
                ssrc: 0x1122_3344,
                marker: true,
            },
            payload: Bytes::from(vec![0x10, 0x00]),
        };
        let mut clock = PublishTrackClock::default();

        let built =
            build_frame_from_rtp(&track, &packet, &mut clock, None, None, None).expect("frame");

        assert!(built.discovered_audio_asc.is_none());
        assert_eq!(built.frame.format, FrameFormat::CanonicalVp8Frame);
        assert!(built.frame.flags.contains(FrameFlags::START_OF_AU));
        assert!(built.frame.flags.contains(FrameFlags::END_OF_AU));
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        assert_eq!(built.frame.payload.as_ref(), &[0x00]);
        let Some(SourceTimestamp::Rtp(source_ts)) = built.frame.source_timestamp() else {
            panic!("expected rtp source timestamp side data");
        };
        assert_eq!(source_ts.raw_timestamp, packet.header.timestamp);
        assert_eq!(
            source_ts.unwrapped_timestamp,
            u64::from(packet.header.timestamp)
        );
        assert_eq!(source_ts.epoch_offset, 0);
        assert_eq!(
            source_ts.sequence_number,
            Some(packet.header.sequence_number)
        );
        assert!(source_ts.rtcp_mapping.is_none());
    }

    #[test]
    fn build_frame_from_rtp_h264_emits_source_timestamp_metadata() {
        let track = TrackInfo::new(TrackId(14), MediaKind::Video, CodecId::H264, 90_000);
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 15,
                timestamp: 12_345,
                ssrc: 0xAABB_CCDD,
                marker: true,
            },
            payload: Bytes::from_static(&[0x65, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 16,
                timestamp: 12_678,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x41, 0xBB]),
        };
        let mut clock = PublishTrackClock::default();
        let mut h264_state = PublishH264Depacketizer::default();

        let built = build_frame_from_rtp(
            &track,
            &packet1,
            &mut clock,
            Some(&mut h264_state),
            None,
            None,
        )
        .expect("h264 frame");
        assert!(build_frame_from_rtp(
            &track,
            &packet2,
            &mut clock,
            Some(&mut h264_state),
            None,
            None
        )
        .is_none());

        assert_eq!(built.frame.format, FrameFormat::CanonicalH26x);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        let Some(SourceTimestamp::Rtp(source_ts)) = built.frame.source_timestamp() else {
            panic!("expected rtp source timestamp side data");
        };
        assert_eq!(source_ts.raw_timestamp, packet1.header.timestamp);
        assert_eq!(
            source_ts.unwrapped_timestamp,
            u64::from(packet1.header.timestamp)
        );
        assert_eq!(
            source_ts.sequence_number,
            Some(packet1.header.sequence_number)
        );
    }

    #[test]
    fn build_frame_from_rtp_h265_emits_access_unit_and_source_timestamp_metadata() {
        let track = TrackInfo::new(TrackId(15), MediaKind::Video, CodecId::H265, 90_000);
        let packet1 = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 16,
                timestamp: 22_222,
                ssrc: 0xABCD_1234,
                marker: true,
            },
            payload: Bytes::from_static(&[0x26, 0x01, 0xAA]),
        };
        let packet2 = RtpPacket {
            header: RtpHeader {
                sequence_number: 17,
                timestamp: 22_555,
                marker: true,
                ..packet1.header
            },
            payload: Bytes::from_static(&[0x02, 0x01, 0xBB]),
        };
        let mut clock = PublishTrackClock::default();
        let mut h265_state = PublishH265Depacketizer::default();

        let built = build_frame_from_rtp(
            &track,
            &packet1,
            &mut clock,
            None,
            Some(&mut h265_state),
            None,
        )
        .expect("h265 frame");
        assert!(build_frame_from_rtp(
            &track,
            &packet2,
            &mut clock,
            None,
            Some(&mut h265_state),
            None
        )
        .is_none());

        assert_eq!(built.frame.format, FrameFormat::CanonicalH26x);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
        let Some(SourceTimestamp::Rtp(source_ts)) = built.frame.source_timestamp() else {
            panic!("expected rtp source timestamp side data");
        };
        assert_eq!(source_ts.raw_timestamp, packet1.header.timestamp);
        assert_eq!(
            source_ts.unwrapped_timestamp,
            u64::from(packet1.header.timestamp)
        );
        assert_eq!(
            source_ts.sequence_number,
            Some(packet1.header.sequence_number)
        );
    }

    #[test]
    fn build_frame_from_rtp_h266_uses_vvc_nal_type_for_keyframe() {
        let track = TrackInfo::new(TrackId(16), MediaKind::Video, CodecId::H266, 90_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 18,
                timestamp: 22_222,
                ssrc: 0xABCD_1234,
                marker: true,
            },
            payload: Bytes::from_static(&[0x00, 0x38, 0xAA]),
        };
        let mut clock = PublishTrackClock::default();
        let mut h265_state = PublishH265Depacketizer::default();

        let built = build_frame_from_rtp(
            &track,
            &packet,
            &mut clock,
            None,
            Some(&mut h265_state),
            None,
        )
        .expect("h266 frame");

        assert_eq!(built.frame.format, FrameFormat::CanonicalH26x);
        assert!(built.frame.flags.contains(FrameFlags::KEY));
    }

    #[test]
    fn build_frame_from_rtp_passthrough_audio_reuses_payload_bytes() {
        let track = TrackInfo::new(TrackId(5), MediaKind::Audio, CodecId::Opus, 48_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 12,
                timestamp: 4_800,
                ssrc: 0x5566_7788,
                marker: true,
            },
            payload: Bytes::from(vec![0xF8, 0xFF, 0xFE]),
        };
        let mut clock = PublishTrackClock::default();

        let built =
            build_frame_from_rtp(&track, &packet, &mut clock, None, None, None).expect("frame");

        assert!(built.discovered_audio_asc.is_none());
        assert_eq!(built.frame.format, FrameFormat::OpusPacket);
        assert!(built.frame.flags.contains(FrameFlags::START_OF_AU));
        assert!(built.frame.flags.contains(FrameFlags::END_OF_AU));
        assert_eq!(built.frame.payload.as_ref(), packet.payload.as_ref());
        assert_eq!(
            built.frame.payload.as_ref().as_ptr(),
            packet.payload.as_ref().as_ptr()
        );
        let Some(SourceTimestamp::Rtp(source_ts)) = built.frame.source_timestamp() else {
            panic!("expected rtp source timestamp side data");
        };
        assert_eq!(source_ts.raw_timestamp, packet.header.timestamp);
        assert_eq!(
            source_ts.sequence_number,
            Some(packet.header.sequence_number)
        );
    }

    #[test]
    fn build_frame_from_rtp_records_rtcp_sender_report_mapping_when_available() {
        let track = TrackInfo::new(TrackId(6), MediaKind::Audio, CodecId::Opus, 48_000);
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: 13,
                timestamp: 9_600,
                ssrc: 0x2233_4455,
                marker: true,
            },
            payload: Bytes::from_static(&[0x11, 0x22]),
        };
        let mut clock = PublishTrackClock {
            last_sr_lsr: Some(0x1234_5678),
            last_sr_unix_micros: Some(9_876_543),
            ..PublishTrackClock::default()
        };

        let built =
            build_frame_from_rtp(&track, &packet, &mut clock, None, None, None).expect("frame");
        let Some(SourceTimestamp::Rtp(source_ts)) = built.frame.source_timestamp() else {
            panic!("expected rtp source timestamp side data");
        };
        let mapping = source_ts.rtcp_mapping.expect("expected rtcp mapping");
        assert_eq!(mapping.lsr, 0x1234_5678);
        assert_eq!(mapping.arrival_unix_micros, 9_876_543);
    }

    #[test]
    fn depacketize_aac_latm_extracts_payload_with_unaligned_header_bits() {
        // useSameStreamMux=1, payloadLength=3, payload bytes 0x11 0x22 0x33
        let payload = [0x81, 0x88, 0x91, 0x19, 0x80];
        let raw = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, true).expect("latm payload"),
        );
        assert_eq!(raw.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert!(raw.discovered_asc.is_none());
    }

    #[test]
    fn depacketize_aac_latm_extracts_out_of_band_payload_as_audio_mux_element() {
        let payload = [0x81, 0x88, 0x91, 0x19, 0x80];
        let raw = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, false)
                .expect("out-of-band latm payload"),
        );
        assert_eq!(raw.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert!(raw.discovered_asc.is_none());
    }

    #[test]
    fn depacketize_aac_latm_extracts_payload_with_inline_stream_mux_config() {
        let mut bits = Vec::new();
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 1, 1);
        push_test_bits(&mut bits, 0, 6);
        push_test_bits(&mut bits, 0, 4);
        push_test_bits(&mut bits, 0, 3);
        push_test_bits(&mut bits, 0x11, 8);
        push_test_bits(&mut bits, 0x90, 8);
        push_test_bits(&mut bits, 0, 3);
        push_test_bits(&mut bits, 0xff, 8);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 3, 8);
        push_test_bits(&mut bits, 0x11, 8);
        push_test_bits(&mut bits, 0x22, 8);
        push_test_bits(&mut bits, 0x33, 8);

        let payload = pack_test_bits(&bits);
        let raw = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, true).expect("latm payload"),
        );
        assert_eq!(raw.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert_eq!(raw.discovered_asc.as_deref(), Some(&[0x11, 0x90][..]));
    }

    #[test]
    fn depacketize_aac_latm_accepts_inline_stream_mux_config_with_pce() {
        let mut bits = Vec::new();
        let asc_with_pce = [
            0x11, 0x80, 0x04, 0xC8, 0x44, 0x00, 0x20, 0x00, 0xC4, 0x0C, 0x4C, 0x61, 0x76, 0x63,
            0x36, 0x31, 0x2E, 0x33, 0x2E, 0x31, 0x30, 0x30, 0x56, 0xE5, 0x00,
        ];

        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 1, 1);
        push_test_bits(&mut bits, 0, 6);
        push_test_bits(&mut bits, 0, 4);
        push_test_bits(&mut bits, 0, 3);
        push_test_audio_specific_config(&mut bits, &asc_with_pce);
        push_test_bits(&mut bits, 0, 3);
        push_test_bits(&mut bits, 0xff, 8);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 0, 1);
        push_test_bits(&mut bits, 3, 8);
        push_test_bits(&mut bits, 0x11, 8);
        push_test_bits(&mut bits, 0x22, 8);
        push_test_bits(&mut bits, 0x33, 8);

        let payload = pack_test_bits(&bits);
        let raw = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, true)
                .expect("latm payload with pce"),
        );
        assert_eq!(raw.payload.as_ref(), &[0x11, 0x22, 0x33]);
        assert_eq!(raw.discovered_asc.as_deref(), Some(asc_with_pce.as_slice()));
    }

    #[test]
    fn depacketize_aac_latm_prefers_length_prefixed_when_first_length_byte_has_high_bit() {
        let raw: Vec<u8> = (0..160).map(|n| ((n * 13 + 5) & 0xff) as u8).collect();
        let mut payload = Vec::with_capacity(raw.len() + 1);
        payload.push(raw.len() as u8);
        payload.extend_from_slice(&raw);

        let extracted = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, false)
                .expect("length-prefixed latm payload"),
        );
        assert_eq!(extracted.payload.as_ref(), raw.as_slice());
    }

    #[test]
    fn depacketize_aac_latm_prefers_bitpacked_when_length_prefixed_looks_valid_253() {
        let raw: Vec<u8> = (0..253).map(|n| ((n * 37 + 11) & 0xff) as u8).collect();
        let payload = encode_aac_latm_bitpacked(&raw);
        let extracted = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, true).expect("latm payload"),
        );
        assert_eq!(extracted.payload.as_ref(), raw.as_slice());
    }

    #[test]
    fn depacketize_aac_latm_prefers_bitpacked_when_length_prefixed_looks_valid_508() {
        let raw: Vec<u8> = (0..508).map(|n| ((n * 29 + 7) & 0xff) as u8).collect();
        let payload = encode_aac_latm_bitpacked(&raw);
        let extracted = single_aac_au(
            depacketize_aac(&payload, AacRtpPacketization::Latm, true).expect("latm payload"),
        );
        assert_eq!(extracted.payload.as_ref(), raw.as_slice());
    }

    #[test]
    fn depacketize_aac_does_not_cross_fallback_between_generic_and_latm() {
        let generic = [0x00, 0x10, 0x00, 0x18, 0x11, 0x22, 0x33];
        assert!(depacketize_aac(&generic, AacRtpPacketization::Latm, true).is_none());
        assert!(depacketize_aac(&generic, AacRtpPacketization::Latm, false).is_none());

        let latm_length_prefixed = [0x03, 0x11, 0x22, 0x33];
        assert!(depacketize_aac(&latm_length_prefixed, AacRtpPacketization::Latm, false).is_some());

        let latm = [0x81, 0x88, 0x91, 0x19, 0x80];
        assert!(depacketize_aac(&latm, AacRtpPacketization::Mpeg4Generic, false).is_none());
        assert!(depacketize_aac(&latm, AacRtpPacketization::Latm, false).is_some());
    }

    #[test]
    fn depacketize_aac_latm_bitpacked_rejects_oversized_length_field() {
        // use_same_stream_mux=1 followed by a string of 0xFF length chunks and a
        // terminating zero chunk. The declared payload length is far larger than
        // the remaining bits, so the parser must fail without allocating.
        let mut payload = vec![0xffu8; 16];
        payload.push(0x00);
        assert!(depacketize_aac_latm_bitpacked(&payload).is_none());
    }
}
