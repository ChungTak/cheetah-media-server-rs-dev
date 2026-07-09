#![allow(dead_code)]

use bytes::Bytes;
use cheetah_rtsp_core::{
    CoreInput, RtspCommand, RtspCore, RtspMessageLimits, RtspRequestDecoder, RtspResponseDecoder,
};

pub const KIND_RTSP_TCP_C2S: u8 = 1;
pub const KIND_RTSP_TCP_S2C: u8 = 2;
pub const KIND_UDP_PUBLISH_RTP: u8 = 3;
pub const KIND_UDP_PUBLISH_RTCP: u8 = 4;
pub const KIND_UDP_PLAY_RTP: u8 = 5;
pub const KIND_UDP_PLAY_RTCP: u8 = 6;
pub const KIND_TCP_INTERLEAVED_RTP: u8 = 7;
pub const KIND_TCP_INTERLEAVED_RTCP: u8 = 8;

const MAX_FIXTURE_RECORDS: usize = 32_768;
const RTSPCAP_MAGIC: &[u8; 4] = b"RSF1";

const H264_TCP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_tcp_publish_play.rtspcap"
);
const H264_UDP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_udp_publish_play.rtspcap"
);
const H265_TCP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/h265_tcp_publish_play.rtspcap"
);
const AUDIO_ONLY_UDP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/audio_only_udp_publish_play.rtspcap"
);
const AV1_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/av1_probe.rtspcap"
);
const VP8_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp8_probe.rtspcap"
);
const VP9_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp9_probe.rtspcap"
);
const H266_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/h266_probe.rtspcap"
);
const HIGH_BITRATE_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/high_bitrate_probe.rtspcap"
);

#[derive(Debug, Clone)]
pub struct CaptureRecord {
    pub kind: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub enum TcpChunkStrategy {
    OriginalRecords,
    SingleBuffer,
    OneByteChunks,
    Coalesced(usize),
}

#[derive(Debug, Clone, Copy)]
pub enum TcpFaultMode {
    OriginalRecords,
    SingleBuffer,
    OneByteChunks,
    CoalescedN,
    PrefixTruncated,
    DuplicateRecord,
    SwapAdjacent,
    DropEveryNth,
}

#[derive(Debug, Clone, Copy)]
pub enum UdpFaultMode {
    DropEveryNthDatagram,
    DropFirstMediaDatagram,
    DuplicateDatagram,
    SwapAdjacentDatagrams,
    ReverseSmallWindow,
    TruncateDatagramPayload,
    MixRtpRtcpOrder,
}

pub fn fuzz_core_entry(data: &[u8], limits: RtspMessageLimits, max_chunk: usize) {
    let mut core = RtspCore::with_limits(limits);
    let chunk_size = max_chunk.clamp(1, 64);

    for chunk in data.chunks(chunk_size) {
        let _ = core.handle_input(CoreInput::Bytes(Bytes::copy_from_slice(chunk)));
    }

    for command in build_commands(data, 8) {
        let _ = core.handle_input(CoreInput::Command(command));
    }

    let _ = core.handle_input(CoreInput::PeerClosed);
}

pub fn feed_core_bytes_in_chunks(data: &[u8], max_chunk: usize) {
    fuzz_core_entry(data, RtspMessageLimits::default(), max_chunk);
}

pub fn fuzz_message_decoders(data: &[u8], limits: RtspMessageLimits, max_chunk: usize) {
    fuzz_request_decoder(data, &limits, max_chunk);
    fuzz_response_decoder(data, &limits, max_chunk);
}

pub fn decode_rtspcap(bytes: &[u8]) -> Option<Vec<CaptureRecord>> {
    if bytes.len() < 8 || &bytes[..4] != RTSPCAP_MAGIC {
        return None;
    }
    let mut cursor = 4usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    if count > MAX_FIXTURE_RECORDS {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = read_u8(bytes, &mut cursor)?;
        if !(KIND_RTSP_TCP_C2S..=KIND_TCP_INTERLEAVED_RTCP).contains(&kind) {
            return None;
        }
        let _flags = read_u8(bytes, &mut cursor)?;
        let _flow_id = read_u16(bytes, &mut cursor)?;
        let _delta_us = read_u32(bytes, &mut cursor)?;
        let payload_len = read_u32(bytes, &mut cursor)? as usize;
        if payload_len == 0 {
            return None;
        }
        let payload = read_bytes(bytes, &mut cursor, payload_len)?.to_vec();
        out.push(CaptureRecord { kind, payload });
    }
    if cursor != bytes.len() {
        return None;
    }
    Some(out)
}

pub fn seed_capture_fixtures() -> &'static [&'static [u8]] {
    &[
        H264_TCP_CAPTURE,
        H264_UDP_CAPTURE,
        H265_TCP_CAPTURE,
        AUDIO_ONLY_UDP_CAPTURE,
        AV1_PROBE_CAPTURE,
        VP8_PROBE_CAPTURE,
        VP9_PROBE_CAPTURE,
        H266_PROBE_CAPTURE,
        HIGH_BITRATE_PROBE_CAPTURE,
    ]
}

pub fn select_seed_records(data: &[u8]) -> Vec<CaptureRecord> {
    let seeds = seed_capture_fixtures();
    let seed_idx = if seeds.is_empty() {
        0
    } else {
        usize::from(data.first().copied().unwrap_or(0)) % seeds.len()
    };
    decode_rtspcap(seeds[seed_idx]).unwrap_or_default()
}

pub fn decode_or_select_records(data: &[u8], max_records: usize) -> Vec<CaptureRecord> {
    let mut records = decode_rtspcap(data).unwrap_or_else(|| select_seed_records(data));
    if records.len() > max_records {
        records.truncate(max_records);
    }
    records
}

pub fn tcp_control_payloads(records: &[CaptureRecord], max_payloads: usize) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == KIND_RTSP_TCP_C2S)
        .filter(|record| {
            record
                .payload
                .first()
                .copied()
                .is_some_and(|b| b.is_ascii_uppercase())
        })
        .take(max_payloads)
        .map(|record| record.payload.clone())
        .collect()
}

pub fn tcp_interleaved_payloads(
    records: &[CaptureRecord],
    max_payloads: usize,
) -> Vec<(u8, Vec<u8>)> {
    records
        .iter()
        .filter(|record| {
            record.kind == KIND_TCP_INTERLEAVED_RTP || record.kind == KIND_TCP_INTERLEAVED_RTCP
        })
        .take(max_payloads)
        .map(|record| {
            let channel = if record.kind == KIND_TCP_INTERLEAVED_RTP {
                0
            } else {
                1
            };
            (channel, record.payload.clone())
        })
        .collect()
}

pub fn udp_datagram_payloads(records: &[CaptureRecord], max_payloads: usize) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| {
            matches!(
                record.kind,
                KIND_UDP_PUBLISH_RTP
                    | KIND_UDP_PUBLISH_RTCP
                    | KIND_UDP_PLAY_RTP
                    | KIND_UDP_PLAY_RTCP
            )
        })
        .take(max_payloads)
        .map(|record| record.payload.clone())
        .collect()
}

pub fn udp_rtp_payloads(records: &[CaptureRecord], max_payloads: usize) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTP || record.kind == KIND_UDP_PLAY_RTP)
        .take(max_payloads)
        .map(|record| record.payload.clone())
        .collect()
}

pub fn udp_rtcp_payloads(records: &[CaptureRecord], max_payloads: usize) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTCP || record.kind == KIND_UDP_PLAY_RTCP)
        .take(max_payloads)
        .map(|record| record.payload.clone())
        .collect()
}

pub fn bounded_bytes_payload(data: &[u8], max_bytes: usize) -> Vec<u8> {
    let limit = max_bytes.max(1);
    let mut out = data.to_vec();
    if out.len() > limit {
        out.truncate(limit);
    }
    out
}

pub fn build_tcp_stream(
    payloads: &[Vec<u8>],
    strategy: TcpChunkStrategy,
    max_chunks: usize,
) -> Vec<Vec<u8>> {
    match strategy {
        TcpChunkStrategy::OriginalRecords => payloads.iter().take(max_chunks).cloned().collect(),
        TcpChunkStrategy::SingleBuffer => vec![payloads
            .iter()
            .take(max_chunks)
            .flat_map(|payload| payload.iter().copied())
            .collect()],
        TcpChunkStrategy::OneByteChunks => payloads
            .iter()
            .take(max_chunks)
            .flat_map(|payload| payload.iter().map(|byte| vec![*byte]))
            .collect(),
        TcpChunkStrategy::Coalesced(size) => {
            let mut out = Vec::new();
            let mut staging = Vec::new();
            let cap = size.max(1);
            for payload in payloads.iter().take(max_chunks) {
                staging.extend_from_slice(payload);
                while staging.len() >= cap {
                    out.push(staging.drain(..cap).collect::<Vec<u8>>());
                }
            }
            if !staging.is_empty() {
                out.push(staging);
            }
            out
        }
    }
}

pub fn build_tcp_fault_chunks(
    data: &[u8],
    payloads: &[Vec<u8>],
    max_chunks: usize,
) -> Vec<Vec<u8>> {
    let mode = match data.get(3).copied().unwrap_or(0) % 8 {
        0 => TcpFaultMode::OriginalRecords,
        1 => TcpFaultMode::SingleBuffer,
        2 => TcpFaultMode::OneByteChunks,
        3 => TcpFaultMode::CoalescedN,
        4 => TcpFaultMode::PrefixTruncated,
        5 => TcpFaultMode::DuplicateRecord,
        6 => TcpFaultMode::SwapAdjacent,
        _ => TcpFaultMode::DropEveryNth,
    };

    let nth = usize::from(data.get(4).copied().unwrap_or(0) % 6) + 2;
    let coalesced_n = usize::from(data.get(5).copied().unwrap_or(0)).clamp(2, 48);

    let mut transformed = payloads.to_vec();
    match mode {
        TcpFaultMode::OriginalRecords => {}
        TcpFaultMode::SingleBuffer => {
            return build_tcp_stream(&transformed, TcpChunkStrategy::SingleBuffer, max_chunks);
        }
        TcpFaultMode::OneByteChunks => {
            return build_tcp_stream(&transformed, TcpChunkStrategy::OneByteChunks, max_chunks);
        }
        TcpFaultMode::CoalescedN => {
            return build_tcp_stream(
                &transformed,
                TcpChunkStrategy::Coalesced(coalesced_n),
                max_chunks,
            );
        }
        TcpFaultMode::PrefixTruncated => {
            if let Some(first) = transformed.first_mut() {
                let keep = (first.len() / 2).max(1);
                first.truncate(keep);
            }
        }
        TcpFaultMode::DuplicateRecord => {
            if let Some(first) = transformed.first().cloned() {
                transformed.push(first);
            }
        }
        TcpFaultMode::SwapAdjacent => {
            swap_adjacent_payloads(&mut transformed);
        }
        TcpFaultMode::DropEveryNth => {
            transformed = transformed
                .into_iter()
                .enumerate()
                .filter(|(idx, _)| (idx + 1) % nth != 0)
                .map(|(_, payload)| payload)
                .collect();
        }
    }

    build_tcp_stream(&transformed, TcpChunkStrategy::OriginalRecords, max_chunks)
}

pub fn build_udp_fault_datagrams(
    data: &[u8],
    rtp: &[Vec<u8>],
    rtcp: &[Vec<u8>],
    max_datagrams: usize,
) -> Vec<Vec<u8>> {
    let mode = match data.get(6).copied().unwrap_or(0) % 7 {
        0 => UdpFaultMode::DropEveryNthDatagram,
        1 => UdpFaultMode::DropFirstMediaDatagram,
        2 => UdpFaultMode::DuplicateDatagram,
        3 => UdpFaultMode::SwapAdjacentDatagrams,
        4 => UdpFaultMode::ReverseSmallWindow,
        5 => UdpFaultMode::TruncateDatagramPayload,
        _ => UdpFaultMode::MixRtpRtcpOrder,
    };

    let nth = usize::from(data.get(7).copied().unwrap_or(0) % 6) + 2;
    let reverse_window = usize::from(data.get(8).copied().unwrap_or(0) % 6) + 3;
    let dup_idx = usize::from(data.get(9).copied().unwrap_or(0));

    let mut mixed = interleave_rtp_rtcp(rtp, rtcp);
    if mixed.is_empty() {
        return Vec::new();
    }

    match mode {
        UdpFaultMode::DropEveryNthDatagram => {
            mixed = mixed
                .into_iter()
                .enumerate()
                .filter(|(idx, _)| (idx + 1) % nth != 0)
                .map(|(_, payload)| payload)
                .collect();
        }
        UdpFaultMode::DropFirstMediaDatagram => {
            if !mixed.is_empty() {
                mixed.remove(0);
            }
        }
        UdpFaultMode::DuplicateDatagram => {
            let idx = dup_idx % mixed.len();
            let duplicated = mixed[idx].clone();
            mixed.insert(idx, duplicated);
        }
        UdpFaultMode::SwapAdjacentDatagrams => {
            swap_adjacent_payloads(&mut mixed);
        }
        UdpFaultMode::ReverseSmallWindow => {
            reverse_small_window_payloads(&mut mixed, reverse_window);
        }
        UdpFaultMode::TruncateDatagramPayload => {
            for payload in &mut mixed {
                let keep = (payload.len() / 2).max(1);
                payload.truncate(keep);
            }
        }
        UdpFaultMode::MixRtpRtcpOrder => {
            mixed = splice_rtcp_into_rtp(rtp, rtcp);
        }
    }

    if mixed.len() > max_datagrams {
        mixed.truncate(max_datagrams);
    }
    mixed
}

pub fn build_mixed_rtsp_interleaved_input(data: &[u8], max_frame_size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(data.get(..data.len().min(64)).unwrap_or_default());

    let request_body_len = usize::from(data.first().copied().unwrap_or(0) % 32);
    out.extend_from_slice(
        format!(
            "OPTIONS rtsp://example.com/live/test RTSP/1.0\r\nCSeq: 1\r\nContent-Length: {}\r\n\r\n",
            request_body_len
        )
        .as_bytes(),
    );
    out.extend_from_slice(&select_payload(data, 1, request_body_len));

    let mut frame_len = usize::from(data.get(2).copied().unwrap_or(0));
    if data.get(3).copied().unwrap_or(0) & 1 == 1 {
        frame_len = max_frame_size.saturating_add(1);
    }
    frame_len = frame_len.min(u16::MAX as usize);
    out.push(b'$');
    out.push(data.get(4).copied().unwrap_or(0));
    out.push((frame_len >> 8) as u8);
    out.push((frame_len & 0xFF) as u8);
    out.extend_from_slice(&select_payload(data, 5, frame_len.min(128)));

    out.extend_from_slice(data.get(64..).unwrap_or_default());
    out
}

fn fuzz_request_decoder(data: &[u8], limits: &RtspMessageLimits, max_chunk: usize) {
    fuzz_request_input(data, limits, max_chunk);
    let structured = build_structured_request(data);
    fuzz_request_input(&structured, limits, max_chunk.saturating_add(2));
}

fn fuzz_response_decoder(data: &[u8], limits: &RtspMessageLimits, max_chunk: usize) {
    fuzz_response_input(data, limits, max_chunk);
    let structured = build_structured_response(data);
    fuzz_response_input(&structured, limits, max_chunk.saturating_add(2));
}

fn fuzz_request_input(data: &[u8], limits: &RtspMessageLimits, max_chunk: usize) {
    let mut decoder = RtspRequestDecoder::with_limits(limits.clone());
    let chunk_size = max_chunk.clamp(1, 64);
    for chunk in data.chunks(chunk_size) {
        if decoder.feed(chunk).is_err() {
            return;
        }
        if drain_request_decoder(&mut decoder).is_err() {
            return;
        }
    }
    let _ = drain_request_decoder(&mut decoder);
}

fn fuzz_response_input(data: &[u8], limits: &RtspMessageLimits, max_chunk: usize) {
    let mut decoder = RtspResponseDecoder::with_limits(limits.clone());
    let chunk_size = max_chunk.clamp(1, 64);
    for chunk in data.chunks(chunk_size) {
        if decoder.feed(chunk).is_err() {
            return;
        }
        if drain_response_decoder(&mut decoder).is_err() {
            return;
        }
    }
    let _ = drain_response_decoder(&mut decoder);
}

fn drain_request_decoder(decoder: &mut RtspRequestDecoder) -> Result<(), ()> {
    loop {
        match decoder.decode() {
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(()),
            Err(_) => return Err(()),
        }
    }
}

fn drain_response_decoder(decoder: &mut RtspResponseDecoder) -> Result<(), ()> {
    loop {
        match decoder.decode() {
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(()),
            Err(_) => return Err(()),
        }
    }
}

fn build_structured_request(data: &[u8]) -> Vec<u8> {
    let body_len = data.len().min(8 * 1024);
    let mut message = format!(
        "OPTIONS rtsp://example.com/live/test RTSP/1.0\r\nCSeq: 1\r\nContent-Length: {}\r\n\r\n",
        body_len
    )
    .into_bytes();
    message.extend_from_slice(&data[..body_len]);
    message
}

fn build_structured_response(data: &[u8]) -> Vec<u8> {
    let body_len = data.len().min(8 * 1024);
    let mut message = format!(
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: {}\r\n\r\n",
        body_len
    )
    .into_bytes();
    message.extend_from_slice(&data[..body_len]);
    message
}

fn build_commands(data: &[u8], max_commands: usize) -> Vec<RtspCommand> {
    let mut commands = Vec::new();
    let mut cursor = 0usize;

    while cursor < data.len() && commands.len() < max_commands {
        let selector = data[cursor] % 3;
        cursor += 1;

        let command = match selector {
            0 => {
                let status_code = 100 + u16::from(data.get(cursor).copied().unwrap_or(0) % 100);
                let cseq_flag = data.get(cursor + 1).copied().unwrap_or(0);
                let cseq = if cseq_flag & 1 == 1 {
                    Some(u32::from(data.get(cursor + 2).copied().unwrap_or(0)))
                } else {
                    None
                };
                let reason =
                    ascii_visible(data.get(cursor + 3..cursor + 12).unwrap_or_default(), "OK");
                let header_name = ascii_token(
                    data.get(cursor + 12..cursor + 20).unwrap_or_default(),
                    "Server",
                );
                let header_value = ascii_visible(
                    data.get(cursor + 20..cursor + 36).unwrap_or_default(),
                    "cheetah",
                );
                let body_len = usize::from(data.get(cursor + 36).copied().unwrap_or(0) % 64);
                let body = select_payload(data, cursor + 37, body_len);
                cursor = cursor.saturating_add(37 + body_len.min(8));

                RtspCommand::SendResponse {
                    cseq,
                    status_code,
                    reason,
                    headers: vec![(header_name, header_value)],
                    body: Bytes::from(body),
                }
            }
            1 => {
                let channel = data.get(cursor).copied().unwrap_or(0);
                let size_hint = usize::from(data.get(cursor + 1).copied().unwrap_or(0));
                let payload_len = if data.get(cursor + 2).copied().unwrap_or(0) & 1 == 1 {
                    u16::MAX as usize + 1
                } else {
                    size_hint.min(512)
                };
                let payload = if payload_len > u16::MAX as usize {
                    vec![0; u16::MAX as usize + 1]
                } else {
                    select_payload(data, cursor + 3, payload_len)
                };
                cursor = cursor.saturating_add(3 + payload_len.min(8));

                RtspCommand::SendInterleaved {
                    channel,
                    payload: Bytes::from(payload),
                }
            }
            _ => RtspCommand::Close,
        };

        commands.push(command);
    }

    commands
}

fn select_payload(data: &[u8], start: usize, len: usize) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    if data.is_empty() {
        return vec![0; len];
    }

    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        out.push(data[(start + i) % data.len()]);
    }
    out
}

fn swap_adjacent_payloads(payloads: &mut [Vec<u8>]) {
    let mut idx = 0usize;
    while idx + 1 < payloads.len() {
        payloads.swap(idx, idx + 1);
        idx += 2;
    }
}

fn reverse_small_window_payloads(payloads: &mut [Vec<u8>], window: usize) {
    let mut start = 0usize;
    let span = window.max(2);
    while start < payloads.len() {
        let end = (start + span).min(payloads.len());
        payloads[start..end].reverse();
        start += span;
    }
}

fn interleave_rtp_rtcp(rtp: &[Vec<u8>], rtcp: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < rtp.len() || j < rtcp.len() {
        if i < rtp.len() {
            out.push(rtp[i].clone());
            i += 1;
        }
        if j < rtcp.len() {
            out.push(rtcp[j].clone());
            j += 1;
        }
    }
    out
}

fn splice_rtcp_into_rtp(rtp: &[Vec<u8>], rtcp: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if rtcp.is_empty() {
        return rtp.to_vec();
    }
    let mut out = Vec::new();
    let mut rtcp_idx = 0usize;
    for (idx, packet) in rtp.iter().enumerate() {
        out.push(packet.clone());
        if idx % 2 == 1 && rtcp_idx < rtcp.len() {
            out.push(rtcp[rtcp_idx].clone());
            rtcp_idx += 1;
        }
    }
    while rtcp_idx < rtcp.len() {
        out.push(rtcp[rtcp_idx].clone());
        rtcp_idx += 1;
    }
    out
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Option<u8> {
    if *cursor + 1 > bytes.len() {
        return None;
    }
    let value = bytes[*cursor];
    *cursor += 1;
    Some(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let raw = read_bytes(bytes, cursor, 2)?;
    Some(u16::from_be_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let raw = read_bytes(bytes, cursor, 4)?;
    Some(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    if *cursor + len > bytes.len() {
        return None;
    }
    let out = &bytes[*cursor..*cursor + len];
    *cursor += len;
    Some(out)
}

fn ascii_token(raw: &[u8], fallback: &str) -> String {
    let mut text = String::new();
    for &byte in raw {
        let is_allowed = byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
        if is_allowed {
            text.push(byte as char);
        }
    }
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn ascii_visible(raw: &[u8], fallback: &str) -> String {
    let mut text = String::new();
    for &byte in raw {
        if byte.is_ascii_graphic() && byte != b':' {
            text.push(byte as char);
        }
    }
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}
