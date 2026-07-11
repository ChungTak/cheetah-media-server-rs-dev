//! MPEG-TS demuxer: parses TS packets into PES frames.
//!
//! MPEG-TS 解复用器：将 TS 包解析为 PES 帧。
//! 提取 PAT/PMT 发现轨道，重组 PES 包，并输出带 PTS/DTS 时间戳的帧。

use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind};

const TS_PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;

/// Events emitted by the TS demuxer.
///
/// TS 解复用器发出的事件。
#[derive(Debug, Clone)]
pub enum TsDemuxEvent {
    /// A track was discovered from PMT.
    ///
    /// 从 PMT 发现的轨道。
    TrackFound {
        pid: u16,
        codec: CodecId,
        media_kind: MediaKind,
    },
    /// A complete frame was reassembled from PES.
    ///
    /// 从 PES 重组完成的帧。
    Frame {
        media_kind: MediaKind,
        codec: CodecId,
        pts: u64,
        dts: u64,
        keyframe: bool,
        data: Bytes,
    },
}

/// MPEG-TS demuxer state machine.
///
/// MPEG-TS 解复用器状态机。
///
/// Tracks the PMT PID, per-track PES buffers, and the sync state. The feed APIs handle
/// both aligned (188-byte) segment data and unaligned raw streams.
///
/// 跟踪 PMT PID、每轨道 PES 缓冲与同步状态。
/// `feed` API 同时处理已对齐（188 字节）的分段数据与未对齐的原始流。
pub struct TsDemuxer {
    pmt_pid: Option<u16>,
    tracks: Vec<TsTrackState>,
    /// Remainder bytes from unaligned feed (less than 188 bytes).
    ///
    /// 未对齐输入的剩余字节（少于 188 字节）。
    remainder: Vec<u8>,
    /// Count of sync losses (for diagnostics).
    ///
    /// 同步丢失次数（用于诊断）。
    pub sync_losses: u64,
}

/// Internal per-track state for PES reassembly.
///
/// PES 重组的每轨道内部状态。
struct TsTrackState {
    pid: u16,
    codec: CodecId,
    media_kind: MediaKind,
    pes_buf: Vec<u8>,
    pes_started: bool,
}

impl TsDemuxer {
    /// Create a new TS demuxer.
    ///
    /// 创建新的 TS 解复用器。
    pub fn new() -> Self {
        Self {
            pmt_pid: None,
            tracks: Vec::new(),
            remainder: Vec::new(),
            sync_losses: 0,
        }
    }

    /// Feed a complete TS segment (multiple of 188 bytes) and collect events.
    ///
    /// Iterates 188-byte chunks, validates sync byte, and dispatches each packet to
    /// `feed_packet`. After all packets, remaining PES buffers are flushed.
    ///
    /// 输入完整的 TS 分段（188 字节的整数倍）并收集事件。
    /// 遍历每个 188 字节块，校验同步字节，并分派给 `feed_packet`。
    /// 所有包处理完成后，刷新剩余 PES 缓冲。
    pub fn feed_segment(&mut self, data: &[u8]) -> Vec<TsDemuxEvent> {
        let mut events = Vec::new();
        for chunk in data.chunks(TS_PACKET_SIZE) {
            if chunk.len() == TS_PACKET_SIZE && chunk[0] == SYNC_BYTE {
                self.feed_packet(chunk, &mut events);
            }
        }
        // Flush remaining PES buffers
        for track in &mut self.tracks {
            if !track.pes_buf.is_empty() {
                if let Some(ev) = parse_pes_to_frame(track) {
                    events.push(ev);
                }
            }
        }
        events
    }

    /// Feed raw data that may not be aligned to 188-byte boundaries.
    ///
    /// Handles sync byte search and buffering of partial packets. Keeps at most 187
    /// bytes as remainder when no full packet can be parsed.
    ///
    /// 输入可能未对齐到 188 字节边界的原始数据。
    /// 处理同步字节搜索与部分包缓冲；当无法解析完整包时最多保留 187 字节。
    pub fn feed_unaligned(&mut self, data: &[u8]) -> Vec<TsDemuxEvent> {
        let mut events = Vec::new();
        let mut buf = std::mem::take(&mut self.remainder);
        buf.extend_from_slice(data);

        // Find first sync byte with double-check
        let mut offset = match find_sync(&buf, 0) {
            Some(sync_pos) => {
                if sync_pos > 0 {
                    self.sync_losses += 1;
                }
                sync_pos
            }
            None => {
                // No sync found — keep last 187 bytes as remainder
                let keep = buf.len().min(TS_PACKET_SIZE - 1);
                self.remainder = buf[buf.len() - keep..].to_vec();
                return events;
            }
        };

        while offset + TS_PACKET_SIZE <= buf.len() {
            if buf[offset] != SYNC_BYTE {
                // Lost sync — search forward
                if let Some(next) = find_sync(&buf, offset) {
                    self.sync_losses += 1;
                    offset = next;
                    continue;
                } else {
                    break;
                }
            }
            self.feed_packet(&buf[offset..offset + TS_PACKET_SIZE], &mut events);
            offset += TS_PACKET_SIZE;
        }

        // Save remainder
        self.remainder = buf[offset..].to_vec();
        events
    }

    /// Parse a single TS packet and update state.
    ///
    /// Reads PID, payload unit start indicator (PUSI), and adaptation field control to
    /// determine payload start. PAT (PID 0) and PMT packets are parsed to discover tracks;
    /// PES packets for known tracks are accumulated.
    ///
    /// 解析单个 TS 包并更新状态。
    /// 读取 PID、PUSI 与适配域控制以确定负载起始。
    /// 解析 PAT（PID 0）与 PMT 包以发现轨道；已知轨道的 PES 包被累积。
    fn feed_packet(&mut self, pkt: &[u8], events: &mut Vec<TsDemuxEvent>) {
        let pid = ((pkt[1] as u16 & 0x1F) << 8) | pkt[2] as u16;
        let pusi = pkt[1] & 0x40 != 0;
        let af_control = (pkt[3] >> 4) & 0x03;

        // Determine payload start
        let mut offset = 4;
        if af_control == 0x02 || af_control == 0x03 {
            // Adaptation field present
            let af_len = pkt[4] as usize;
            offset = 5 + af_len;
        }
        if af_control == 0x00 || af_control == 0x02 {
            return; // No payload
        }
        if offset >= TS_PACKET_SIZE {
            return;
        }

        let payload = &pkt[offset..];

        // PAT (PID 0)
        if pid == 0x0000 {
            self.parse_pat(payload, pusi);
            return;
        }

        // PMT
        if Some(pid) == self.pmt_pid {
            self.parse_pmt(payload, pusi, events);
            return;
        }

        // PES data for known tracks
        if let Some(track_idx) = self.tracks.iter().position(|t| t.pid == pid) {
            if pusi {
                // New PES starts — flush previous
                if !self.tracks[track_idx].pes_buf.is_empty() {
                    if let Some(ev) = parse_pes_to_frame(&mut self.tracks[track_idx]) {
                        events.push(ev);
                    }
                }
                self.tracks[track_idx].pes_started = true;
                self.tracks[track_idx].pes_buf.clear();
            }
            if self.tracks[track_idx].pes_started {
                self.tracks[track_idx].pes_buf.extend_from_slice(payload);
            }
        }
    }

    /// Parse the PAT to find the PMT PID.
    ///
    /// 解析 PAT 以找到 PMT PID。
    fn parse_pat(&mut self, payload: &[u8], pusi: bool) {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            &payload[1 + pointer..]
        } else {
            payload
        };
        // PAT: table_id(1) + section_length(2) + tsid(2) + version(1) + section(1) + last(1) + [program_num(2) + pmt_pid(2)]*
        if data.len() < 12 {
            return;
        }
        // First program entry at offset 8
        let pmt_pid = ((data[10] as u16 & 0x1F) << 8) | data[11] as u16;
        self.pmt_pid = Some(pmt_pid);
    }

    /// Parse the PMT to discover tracks and their PID/codec mapping.
    ///
    /// Iterates the program descriptor loop. For each stream, it maps the MPEG stream type
    /// to a codec and media kind. Private streams (`stream_type == 0x06`) are further
    /// disambiguated by `identify_private_stream` from the ES_info descriptors.
    ///
    /// 解析 PMT 以发现轨道及其 PID/编解码器映射。
    /// 遍历节目描述循环；将 MPEG 流类型映射为编解码器与媒体类型。
    /// 私有流（`stream_type == 0x06`）通过 ES_info 描述符进一步由 `identify_private_stream` 识别。
    fn parse_pmt(&mut self, payload: &[u8], pusi: bool, events: &mut Vec<TsDemuxEvent>) {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            &payload[1 + pointer..]
        } else {
            payload
        };
        // PMT: table_id(1) + section_length(2) + program(2) + version(1) + section(1) + last(1) + pcr_pid(2) + prog_info_len(2) + streams...
        if data.len() < 12 {
            return;
        }
        let prog_info_len = ((data[10] as usize & 0x0F) << 8) | data[11] as usize;
        let mut pos = 12 + prog_info_len;

        // Section length tells us where CRC starts
        let section_len = ((data[1] as usize & 0x0F) << 8) | data[2] as usize;
        let section_end = (3 + section_len).min(data.len()).saturating_sub(4); // exclude CRC

        while pos + 5 <= section_end {
            let stream_type = data[pos];
            let es_pid = ((data[pos + 1] as u16 & 0x1F) << 8) | data[pos + 2] as u16;
            let es_info_len = ((data[pos + 3] as usize & 0x0F) << 8) | data[pos + 4] as usize;
            let es_info_start = pos + 5;
            pos += 5 + es_info_len;

            let (codec, media_kind) = match stream_type {
                0x1B => (CodecId::H264, MediaKind::Video),
                0x24 => (CodecId::H265, MediaKind::Video),
                0x9D => (CodecId::VP8, MediaKind::Video),
                0x9E => (CodecId::VP9, MediaKind::Video),
                0x9F => (CodecId::AV1, MediaKind::Video),
                0x0F => (CodecId::AAC, MediaKind::Audio),
                0x03 => (CodecId::MP2, MediaKind::Audio),
                0x04 => (CodecId::MP3, MediaKind::Audio),
                0x90 => (CodecId::G711A, MediaKind::Audio),
                0x91 => (CodecId::G711U, MediaKind::Audio),
                0x9C => (CodecId::Opus, MediaKind::Audio),
                0x06 => {
                    // Private data — identify by registration/AV1 descriptor
                    let es_info = &data[es_info_start
                        ..es_info_start + es_info_len.min(data.len() - es_info_start)];
                    match identify_private_stream(es_info) {
                        Some(id) => id,
                        None => continue,
                    }
                }
                _ => continue,
            };

            if !self.tracks.iter().any(|t| t.pid == es_pid) {
                self.tracks.push(TsTrackState {
                    pid: es_pid,
                    codec,
                    media_kind,
                    pes_buf: Vec::new(),
                    pes_started: false,
                });
                events.push(TsDemuxEvent::TrackFound {
                    pid: es_pid,
                    codec,
                    media_kind,
                });
            }
        }
    }
}

impl Default for TsDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a PES buffer into a frame event.
///
/// Validates the PES start code, extracts PTS/DTS from the optional PES header, and
/// detects keyframes for H.264/H.265 by scanning the payload for IDR/IRAP NAL units.
///
/// 将 PES 缓冲解析为帧事件。
/// 校验 PES 起始码，从可选 PES 头提取 PTS/DTS，并扫描 H.264/H.265 负载中的 IDR/IRAP NAL 以检测关键帧。
fn parse_pes_to_frame(track: &mut TsTrackState) -> Option<TsDemuxEvent> {
    let buf = &track.pes_buf;
    // PES start code: 00 00 01 stream_id
    if buf.len() < 9 || buf[0] != 0x00 || buf[1] != 0x00 || buf[2] != 0x01 {
        track.pes_buf.clear();
        return None;
    }

    let _stream_id = buf[3];
    // PES header data length
    let header_data_len = buf[8] as usize;
    let pts_dts_flags = (buf[7] >> 6) & 0x03;

    let mut pts: u64 = 0;
    let mut dts: u64 = 0;

    if pts_dts_flags >= 2 && buf.len() >= 14 {
        pts = decode_timestamp(&buf[9..14]);
        dts = pts;
    }
    if pts_dts_flags == 3 && buf.len() >= 19 {
        dts = decode_timestamp(&buf[14..19]);
    }

    let payload_start = 9 + header_data_len;
    if payload_start >= buf.len() {
        track.pes_buf.clear();
        return None;
    }

    let data = Bytes::copy_from_slice(&buf[payload_start..]);
    let keyframe = track.media_kind == MediaKind::Video && is_keyframe_payload(&data, track.codec);

    track.pes_buf.clear();

    Some(TsDemuxEvent::Frame {
        media_kind: track.media_kind,
        codec: track.codec,
        pts,
        dts,
        keyframe,
        data,
    })
}

/// Decode a 33-bit MPEG-2 PES timestamp from 5 bytes.
///
/// 从 5 字节解码 33 位 MPEG-2 PES 时间戳。
pub(crate) fn decode_timestamp(buf: &[u8]) -> u64 {
    let b0 = buf[0] as u64;
    let b1 = buf[1] as u64;
    let b2 = buf[2] as u64;
    let b3 = buf[3] as u64;
    let b4 = buf[4] as u64;
    ((b0 >> 1) & 0x07) << 30 | (b1 << 22) | ((b2 >> 1) << 15) | (b3 << 7) | (b4 >> 1)
}

/// Determine whether the payload starts with a keyframe for the given codec.
///
/// For H.264, looks for NAL type 5 (IDR) in Annex-B start code format.
/// For H.265, NAL types 16-21 are IRAP (keyframe) frames.
///
/// 判断给定编解码器负载是否以关键帧开始。
/// H.264 在 Annex-B 起始码格式中查找 NAL type 5（IDR）。
/// H.265 中 NAL type 16-21 为 IRAP（关键帧）。
fn is_keyframe_payload(data: &[u8], codec: CodecId) -> bool {
    match codec {
        CodecId::H264 => {
            // Look for NAL type 5 (IDR) in Annex-B
            data.windows(4)
                .any(|w| w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x01 && (w[3] & 0x1F) == 5)
                || data.windows(5).any(|w| {
                    w[0] == 0x00
                        && w[1] == 0x00
                        && w[2] == 0x00
                        && w[3] == 0x01
                        && (w[4] & 0x1F) == 5
                })
        }
        CodecId::H265 => {
            // NAL types 16-21 are IRAP (keyframe)
            data.windows(5).any(|w| {
                w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x00 && w[3] == 0x01 && {
                    let nal_type = (w[4] >> 1) & 0x3F;
                    (16..=21).contains(&nal_type)
                }
            })
        }
        _ => false,
    }
}

/// Identify a private stream (`stream_type=0x06`) by parsing ES_info descriptors.
///
/// Checks for registration descriptor format identifiers (AV01, Opus, VP09) and the
/// AV1 video descriptor tag (`0x80`).
///
/// 通过解析 ES_info 描述符识别私有流（`stream_type=0x06`）。
/// 检查 registration descriptor 的 format_identifier（AV01、Opus、VP09）以及
/// AV1 视频描述符标签（`0x80`）。
fn identify_private_stream(es_info: &[u8]) -> Option<(CodecId, MediaKind)> {
    let mut offset = 0;
    while offset + 2 <= es_info.len() {
        let tag = es_info[offset];
        let len = es_info[offset + 1] as usize;
        if offset + 2 + len > es_info.len() {
            break;
        }
        let desc_data = &es_info[offset + 2..offset + 2 + len];
        match tag {
            0x05 if len >= 4 => {
                // Registration descriptor — check format_identifier
                match desc_data[..4] {
                    [b'A', b'V', b'0', b'1'] => return Some((CodecId::AV1, MediaKind::Video)),
                    [b'O', b'p', b'u', b's'] => return Some((CodecId::Opus, MediaKind::Audio)),
                    [b'V', b'P', b'0', b'9'] => return Some((CodecId::VP9, MediaKind::Video)),
                    _ => {}
                }
            }
            0x80 => {
                // AV1 video descriptor
                return Some((CodecId::AV1, MediaKind::Video));
            }
            _ => {}
        }
        offset += 2 + len;
    }
    None
}

/// Find a sync byte (`0x47`) that is confirmed by a second `0x47` at +188.
///
/// 找到同步字节（`0x47`），并通过 +188 位置再次为 `0x47` 进行确认。
fn find_sync(data: &[u8], start: usize) -> Option<usize> {
    let end = data.len().saturating_sub(TS_PACKET_SIZE);
    for i in start..end {
        if data[i] == SYNC_BYTE
            && (i + TS_PACKET_SIZE >= data.len() || data[i + TS_PACKET_SIZE] == SYNC_BYTE)
        {
            return Some(i);
        }
    }
    // Fallback: near end, accept single sync byte
    (end.max(start)..data.len()).find(|&i| data[i] == SYNC_BYTE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TsMuxer;

    #[test]
    fn roundtrip_mux_demux() {
        // Mux a video frame
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, true);
        muxer.write_pat_pmt();
        let video_data = b"\x00\x00\x00\x01\x65\xAA\xBB\xCC"; // IDR NAL
        muxer.write_video(video_data, 90000, 90000, true);
        muxer.write_audio(&[0xFF, 0xF1, 0x50, 0x80, 0x02, 0x00, 0xAA], 90000);
        let segment = muxer.take_segment();

        // Demux it
        let mut demuxer = TsDemuxer::new();
        let events = demuxer.feed_segment(&segment);

        // Should find tracks
        let track_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, TsDemuxEvent::TrackFound { .. }))
            .collect();
        assert!(track_events.len() >= 1, "should find at least 1 track");

        // Should produce at least one frame
        let frame_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, TsDemuxEvent::Frame { .. }))
            .collect();
        assert!(!frame_events.is_empty(), "should produce frames");
    }

    #[test]
    fn decode_timestamp_correct() {
        // PTS = 90000 (1 second at 90kHz)
        // Encoded as: marker(4bits) | ts[32..30](3) | 1 | ts[29..15](15) | 1 | ts[14..0](15) | 1
        let ts: u64 = 90000;
        let mut encoded = Vec::new();
        let marker = 0x02_u8;
        encoded.push((marker << 4) | ((ts >> 29) as u8 & 0x0E) | 0x01);
        encoded.push((ts >> 22) as u8);
        encoded.push(((ts >> 14) as u8 & 0xFE) | 0x01);
        encoded.push((ts >> 7) as u8);
        encoded.push(((ts << 1) as u8 & 0xFE) | 0x01);

        let decoded = decode_timestamp(&encoded);
        assert_eq!(decoded, ts);
    }

    #[test]
    fn av1_roundtrip_mux_demux() {
        // AV1 uses stream_type=0x06 with registration + AV1 video descriptor
        let mut muxer = TsMuxer::new(CodecId::AV1, CodecId::AAC, false);
        muxer.write_pat_pmt();
        // Fake AV1 OBU data (temporal delimiter + frame)
        let av1_data = b"\x12\x00\x32\x10\xAA\xBB\xCC\xDD";
        muxer.write_video(av1_data, 90000, 90000, true);
        let segment = muxer.take_segment();

        let mut demuxer = TsDemuxer::new();
        let events = demuxer.feed_segment(&segment);

        // Should find AV1 video track via descriptor parsing
        let av1_tracks: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    TsDemuxEvent::TrackFound {
                        codec: CodecId::AV1,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            av1_tracks.len(),
            1,
            "should detect AV1 track from descriptor"
        );
    }
}
