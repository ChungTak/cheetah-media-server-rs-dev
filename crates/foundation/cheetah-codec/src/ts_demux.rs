//! Shared MPEG-TS demuxer for all protocol modules.
//!
//! Accepts arbitrary byte slices (unaligned), discovers tracks from PAT/PMT,
//! reassembles PES, and outputs `TrackInfo` + `AVFrame`.

use crate::prelude::*;
use bytes::Bytes;

use crate::frame::{AVFrame, FrameFlags, FrameFormat};
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};
use crate::ts_common::{
    codec_from_stream_type, decode_timestamp, find_sync, identify_private_stream, SYNC_BYTE,
    TS_PACKET_SIZE,
};
use crate::video::{av1_obu_payload_has_keyframe, vp9_frame_is_keyframe, ParameterSetCache};

/// Configuration for the shared TS demuxer.
///
/// 共享 TS 解复用器的配置。
#[derive(Debug, Clone)]
pub struct MpegTsDemuxerConfig {
    /// Maximum reassembly buffer size per PID (default 4 MiB).
    ///
    /// 每个 PID 重组缓冲区的最大字节数（默认 4 MiB）。
    pub max_reassembly_bytes: usize,
    /// Strict CRC mode: reject PAT/PMT with bad CRC (default false).
    ///
    /// 严格 CRC 模式：遇到 CRC 错误的 PAT/PMT 时拒绝（默认 false）。
    pub strict_crc: bool,
}

impl Default for MpegTsDemuxerConfig {
    fn default() -> Self {
        Self {
            max_reassembly_bytes: 4 * 1024 * 1024,
            strict_crc: false,
        }
    }
}

/// Events emitted by the demuxer.
///
/// 解复用器发出的事件。
#[derive(Debug, Clone)]
pub enum MpegTsDemuxEvent {
    /// A track was discovered from PMT.
    ///
    /// 从 PMT 中发现一条轨道。
    TrackFound(TrackInfo),
    /// A complete frame was reassembled from PES.
    ///
    /// 从 PES 重组得到完整帧。
    Frame(AVFrame),
    /// Diagnostic message (non-fatal).
    ///
    /// 非致命诊断信息。
    Diagnostic(MpegTsDemuxDiagnostic),
}

/// Diagnostic messages from the demuxer.
///
/// 解复用器发出的诊断信息。
#[derive(Debug, Clone)]
pub enum MpegTsDemuxDiagnostic {
    /// MPEG-TS sync was lost.
    ///
    /// MPEG-TS 同步丢失。
    SyncLoss,
    /// Continuity counter discontinuity for a PID.
    ///
    /// PID 的连续性计数器不连续。
    ContinuityGap { pid: u16, expected: u8, got: u8 },
    /// CRC mismatch in a PSI section.
    ///
    /// PSI 段 CRC 校验错误。
    CrcError { pid: u16 },
    /// PES reassembly buffer exceeded the configured limit.
    ///
    /// PES 重组缓冲区超过配置限制。
    PesOverflow { pid: u16 },
    /// PMT listed a stream_type we cannot map to a codec.
    ///
    /// PMT 列出无法映射到编解码器的 stream_type。
    UnknownStreamType { stream_type: u8, pid: u16 },
    /// Adaptation field length exceeds packet bounds.
    ///
    /// 自适应字段长度超出包边界。
    AdaptationFieldOverflow { pid: u16 },
    /// Invalid AAC ADTS frame encountered.
    ///
    /// 遇到无效的 AAC ADTS 帧。
    AdtsError { pid: u16, reason: &'static str },
}

/// Per-PID track state.
struct DemuxTrackState {
    pid: u16,
    track_id: TrackId,
    codec: CodecId,
    media_kind: MediaKind,
    clock_rate: u32,
    pes_buf: Vec<u8>,
    pes_started: bool,
    expected_cc: Option<u8>,
    parameter_sets: ParameterSetCache,
    /// Inferred AAC ASC from first ADTS header (2 bytes).
    aac_asc: Option<[u8; 2]>,
    /// Inferred audio sample rate from codec frame headers.
    sample_rate: Option<u32>,
    /// Inferred audio channel count from codec frame headers.
    channels: Option<u8>,
    /// Inferred AV1 sequence header OBU.
    av1_sequence_header: Option<Bytes>,
    /// Private stream_type=0x06 with no descriptor; infer codec from first PES.
    pending_private_probe: bool,
    /// PTS wrap offset for 33-bit unwrapping.
    pts_wrap_offset: i64,
    /// Last raw PTS seen (before unwrap).
    last_raw_pts: Option<u64>,
}

/// Shared MPEG-TS demuxer state machine.
///
/// 共享的 MPEG-TS 解复用状态机，负责 PAT/PMT 发现、PID 过滤、
/// 连续性计数校验与 PES 重组。
pub struct MpegTsDemuxer {
    config: MpegTsDemuxerConfig,
    pmt_pid: Option<u16>,
    tracks: Vec<DemuxTrackState>,
    remainder: Vec<u8>,
    pmt_section_buf: Vec<u8>,
    pmt_section_target_len: Option<usize>,
    next_track_id: u32,
}

impl MpegTsDemuxer {
    /// Create a new TS demuxer with the given configuration.
    ///
    /// 使用给定配置创建新的 TS 解复用器。
    pub fn new(config: MpegTsDemuxerConfig) -> Self {
        Self {
            config,
            pmt_pid: None,
            tracks: Vec::new(),
            remainder: Vec::new(),
            pmt_section_buf: Vec::new(),
            pmt_section_target_len: None,
            next_track_id: 1,
        }
    }

    /// Feed raw bytes (any alignment). Returns events.
    ///
    /// 接收任意对齐的原始字节，查找同步、解析 PAT/PMT、重组 PES 并返回事件。
    pub fn push(&mut self, data: &[u8]) -> Vec<MpegTsDemuxEvent> {
        let mut events = Vec::new();
        let mut buf = core::mem::take(&mut self.remainder);
        buf.extend_from_slice(data);

        let mut offset = match find_sync(&buf, 0) {
            Some(pos) => {
                if pos > 0 {
                    events.push(MpegTsDemuxEvent::Diagnostic(
                        MpegTsDemuxDiagnostic::SyncLoss,
                    ));
                }
                pos
            }
            None => {
                let keep = buf.len().min(TS_PACKET_SIZE - 1);
                self.remainder = buf[buf.len() - keep..].to_vec();
                return events;
            }
        };

        while offset + TS_PACKET_SIZE <= buf.len() {
            if buf[offset] != SYNC_BYTE {
                if let Some(next) = find_sync(&buf, offset) {
                    events.push(MpegTsDemuxEvent::Diagnostic(
                        MpegTsDemuxDiagnostic::SyncLoss,
                    ));
                    offset = next;
                    continue;
                } else {
                    break;
                }
            }
            self.feed_packet(&buf[offset..offset + TS_PACKET_SIZE], &mut events);
            offset += TS_PACKET_SIZE;
        }

        self.remainder = buf[offset..].to_vec();
        events
    }

    /// Flush remaining PES buffers.
    ///
    /// 刷新剩余 PES 缓冲区，将未完成的帧强制输出。
    pub fn flush(&mut self) -> Vec<MpegTsDemuxEvent> {
        let mut events = Vec::new();
        for track in &mut self.tracks {
            if !track.pes_buf.is_empty() {
                events.extend(parse_pes_to_frames(track));
            }
        }
        events
    }

    fn feed_packet(&mut self, pkt: &[u8], events: &mut Vec<MpegTsDemuxEvent>) {
        let pid = ((pkt[1] as u16 & 0x1F) << 8) | pkt[2] as u16;
        let pusi = pkt[1] & 0x40 != 0;
        let af_control = (pkt[3] >> 4) & 0x03;
        let cc = pkt[3] & 0x0F;

        // Determine payload start
        let mut payload_offset = 4;
        if af_control == 0x02 || af_control == 0x03 {
            if pkt.len() <= 4 {
                return;
            }
            let af_len = pkt[4] as usize;
            if 5 + af_len > TS_PACKET_SIZE {
                events.push(MpegTsDemuxEvent::Diagnostic(
                    MpegTsDemuxDiagnostic::AdaptationFieldOverflow { pid },
                ));
                return;
            }
            payload_offset = 5 + af_len;
        }
        if af_control == 0x00 || af_control == 0x02 {
            return; // No payload
        }
        if payload_offset >= TS_PACKET_SIZE {
            return;
        }

        let payload = &pkt[payload_offset..];

        // PAT (PID 0)
        if pid == 0x0000 {
            self.parse_pat(payload, pusi, events);
            return;
        }

        // PMT
        if Some(pid) == self.pmt_pid {
            self.parse_pmt(payload, pusi, events);
            return;
        }

        // Null packet
        if pid == 0x1FFF {
            return;
        }

        // PES data for known tracks
        if let Some(track_idx) = self.tracks.iter().position(|t| t.pid == pid) {
            // Continuity counter check
            if let Some(expected) = self.tracks[track_idx].expected_cc {
                if cc != expected {
                    events.push(MpegTsDemuxEvent::Diagnostic(
                        MpegTsDemuxDiagnostic::ContinuityGap {
                            pid,
                            expected,
                            got: cc,
                        },
                    ));
                }
            }
            self.tracks[track_idx].expected_cc = Some((cc + 1) & 0x0F);

            if pusi {
                // New PES starts — flush previous
                if !self.tracks[track_idx].pes_buf.is_empty() {
                    events.extend(parse_pes_to_frames(&mut self.tracks[track_idx]));
                }
                self.tracks[track_idx].pes_started = true;
                self.tracks[track_idx].pes_buf.clear();
            }
            if self.tracks[track_idx].pes_started {
                let max = self.config.max_reassembly_bytes;
                if self.tracks[track_idx].pes_buf.len() + payload.len() > max {
                    events.push(MpegTsDemuxEvent::Diagnostic(
                        MpegTsDemuxDiagnostic::PesOverflow { pid },
                    ));
                    self.tracks[track_idx].pes_buf.clear();
                    self.tracks[track_idx].pes_started = false;
                } else {
                    self.tracks[track_idx].pes_buf.extend_from_slice(payload);
                }
            }
        }
    }

    fn parse_pat(&mut self, payload: &[u8], pusi: bool, events: &mut Vec<MpegTsDemuxEvent>) {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            if 1 + pointer >= payload.len() {
                return;
            }
            &payload[1 + pointer..]
        } else {
            payload
        };
        if data.len() < 12 {
            return;
        }
        let section_len = ((data[1] as usize & 0x0F) << 8) | data[2] as usize;
        let section_end = (3 + section_len).min(data.len());
        if section_end < 4 {
            return;
        }
        // CRC validation
        if section_end >= 4 && section_end <= data.len() {
            let section_data = &data[..section_end - 4];
            let stored_crc = u32::from_be_bytes([
                data[section_end - 4],
                data[section_end - 3],
                data[section_end - 2],
                data[section_end - 1],
            ]);
            let computed_crc = crate::ts_common::crc32_mpeg2(section_data);
            if stored_crc != computed_crc {
                events.push(MpegTsDemuxEvent::Diagnostic(
                    MpegTsDemuxDiagnostic::CrcError { pid: 0x0000 },
                ));
                if self.config.strict_crc {
                    return;
                }
            }
        }
        let pmt_pid = ((data[10] as u16 & 0x1F) << 8) | data[11] as u16;
        self.pmt_pid = Some(pmt_pid);
    }

    fn parse_pmt(&mut self, payload: &[u8], pusi: bool, events: &mut Vec<MpegTsDemuxEvent>) {
        let Some(data) = self.reassemble_pmt_section(payload, pusi) else {
            return;
        };
        self.parse_pmt_section(&data, events);
    }

    fn reassemble_pmt_section(&mut self, payload: &[u8], pusi: bool) -> Option<Vec<u8>> {
        let data = if pusi && !payload.is_empty() {
            let pointer = payload[0] as usize;
            if 1 + pointer >= payload.len() {
                return None;
            }
            self.pmt_section_buf.clear();
            self.pmt_section_target_len = None;
            &payload[1 + pointer..]
        } else {
            payload
        };
        if data.is_empty() {
            return None;
        }

        self.pmt_section_buf.extend_from_slice(data);
        if self.pmt_section_target_len.is_none() && self.pmt_section_buf.len() >= 3 {
            let section_len =
                ((self.pmt_section_buf[1] as usize & 0x0F) << 8) | self.pmt_section_buf[2] as usize;
            self.pmt_section_target_len = Some(3 + section_len);
        }

        let target_len = self.pmt_section_target_len?;
        if self.pmt_section_buf.len() < target_len {
            return None;
        }

        let section = self.pmt_section_buf[..target_len].to_vec();
        self.pmt_section_buf.clear();
        self.pmt_section_target_len = None;
        Some(section)
    }

    fn parse_pmt_section(&mut self, data: &[u8], events: &mut Vec<MpegTsDemuxEvent>) {
        if data.len() < 12 {
            return;
        }

        let section_len = ((data[1] as usize & 0x0F) << 8) | data[2] as usize;
        let section_end = 3 + section_len;
        if section_end > data.len() {
            return;
        }

        // CRC validation
        if section_end >= 4 && section_end <= data.len() {
            let section_data = &data[..section_end - 4];
            let stored_crc = u32::from_be_bytes([
                data[section_end - 4],
                data[section_end - 3],
                data[section_end - 2],
                data[section_end - 1],
            ]);
            let computed_crc = crate::ts_common::crc32_mpeg2(section_data);
            if stored_crc != computed_crc {
                events.push(MpegTsDemuxEvent::Diagnostic(
                    MpegTsDemuxDiagnostic::CrcError {
                        pid: self.pmt_pid.unwrap_or(0),
                    },
                ));
                if self.config.strict_crc {
                    return;
                }
            }
        }

        let prog_info_len = ((data[10] as usize & 0x0F) << 8) | data[11] as usize;
        let mut pos = 12 + prog_info_len;

        let section_end_no_crc = section_end.saturating_sub(4);

        while pos + 5 <= section_end_no_crc {
            let stream_type = data[pos];
            let es_pid = ((data[pos + 1] as u16 & 0x1F) << 8) | data[pos + 2] as u16;
            let es_info_len = ((data[pos + 3] as usize & 0x0F) << 8) | data[pos + 4] as usize;
            let es_info_start = pos + 5;
            pos += 5 + es_info_len;

            // Already known?
            if self.tracks.iter().any(|t| t.pid == es_pid) {
                continue;
            }

            let resolved = if stream_type == 0x06 {
                let es_info = &data
                    [es_info_start..es_info_start + es_info_len.min(data.len() - es_info_start)];
                identify_private_stream(es_info)
            } else {
                codec_from_stream_type(stream_type)
            };

            let (codec, media_kind, pending_private_probe) = match resolved {
                Some((codec, media_kind)) => (codec, media_kind, false),
                None if stream_type == 0x06 && es_info_len == 0 => {
                    (CodecId::Unknown, MediaKind::Video, true)
                }
                None => {
                    events.push(MpegTsDemuxEvent::Diagnostic(
                        MpegTsDemuxDiagnostic::UnknownStreamType {
                            stream_type,
                            pid: es_pid,
                        },
                    ));
                    continue;
                }
            };

            let track_id = TrackId(self.next_track_id);
            self.next_track_id += 1;

            let clock_rate = match media_kind {
                MediaKind::Video => 90_000,
                MediaKind::Audio => match codec {
                    CodecId::AAC => 48_000,
                    CodecId::Opus => 48_000,
                    CodecId::G711A | CodecId::G711U => 8_000,
                    _ => 90_000,
                },
                _ => 90_000,
            };

            let mut track_info = TrackInfo::new(track_id, media_kind, codec, clock_rate);
            track_info.refresh_readiness();

            self.tracks.push(DemuxTrackState {
                pid: es_pid,
                track_id,
                codec,
                media_kind,
                clock_rate,
                pes_buf: Vec::new(),
                pes_started: false,
                expected_cc: None,
                parameter_sets: ParameterSetCache::default(),
                aac_asc: None,
                sample_rate: None,
                channels: None,
                av1_sequence_header: None,
                pending_private_probe,
                pts_wrap_offset: 0,
                last_raw_pts: None,
            });

            if !pending_private_probe {
                events.push(MpegTsDemuxEvent::TrackFound(track_info));
            }
        }
    }
}

impl Default for MpegTsDemuxer {
    fn default() -> Self {
        Self::new(MpegTsDemuxerConfig::default())
    }
}

/// Parse a PES buffer into one or more AVFrames (multi-frame for AAC ADTS).
fn parse_pes_to_frames(track: &mut DemuxTrackState) -> Vec<MpegTsDemuxEvent> {
    if track.pes_buf.len() < 9
        || track.pes_buf[0] != 0x00
        || track.pes_buf[1] != 0x00
        || track.pes_buf[2] != 0x01
    {
        track.pes_buf.clear();
        return Vec::new();
    }

    let header_data_len = track.pes_buf[8] as usize;
    let pts_dts_flags = (track.pes_buf[7] >> 6) & 0x03;
    let payload_start = 9 + header_data_len;
    if payload_start >= track.pes_buf.len() {
        track.pes_buf.clear();
        return Vec::new();
    }
    if pts_dts_flags == 0x01
        || (pts_dts_flags >= 2 && header_data_len < 5)
        || (pts_dts_flags == 3 && header_data_len < 10)
    {
        track.pes_buf.clear();
        return Vec::new();
    }

    let mut pts: u64 = 0;
    let mut dts: u64 = 0;

    if pts_dts_flags >= 2 {
        pts = decode_timestamp(&track.pes_buf[9..14]);
        dts = pts;
    }
    if pts_dts_flags == 3 {
        dts = decode_timestamp(&track.pes_buf[14..19]);
    }

    // PTS 33-bit wrap unwrapping
    const WRAP_THRESHOLD: u64 = 1 << 32;
    const WRAP_VALUE: i64 = 1 << 33;
    if let Some(last) = track.last_raw_pts {
        if pts < last && (last - pts) > WRAP_THRESHOLD {
            track.pts_wrap_offset += WRAP_VALUE;
        }
    }
    track.last_raw_pts = Some(pts);

    let pts_unwrapped = (pts as i64).wrapping_add(track.pts_wrap_offset) as u64;
    let dts_unwrapped = (dts as i64).wrapping_add(track.pts_wrap_offset) as u64;

    // Take payload out of pes_buf to release borrow
    let raw_payload = track.pes_buf[payload_start..].to_vec();
    track.pes_buf.clear();

    let mut events = Vec::new();
    if track.pending_private_probe {
        let Some((codec, format)) = infer_private_video_codec(&raw_payload) else {
            return events;
        };
        track.codec = codec;
        track.media_kind = MediaKind::Video;
        track.clock_rate = 90_000;
        if codec == CodecId::AV1 {
            track.av1_sequence_header = extract_av1_sequence_header_obu(&raw_payload);
        }
        track.pending_private_probe = false;
        events.push(MpegTsDemuxEvent::TrackFound(track_info_from_state(track)));

        return build_single_frame_event(
            track,
            Bytes::from(raw_payload),
            format,
            pts_unwrapped,
            dts_unwrapped,
            events,
        );
    }

    // AAC ADTS multi-frame split
    if track.codec == CodecId::AAC {
        return split_aac_adts_frames(track, &raw_payload, pts_unwrapped, dts_unwrapped);
    }

    if matches!(track.codec, CodecId::MP2 | CodecId::MP3) {
        if let Some(info) = parse_mpeg_audio_header(&raw_payload) {
            let changed = track.codec != info.codec
                || track.sample_rate != Some(info.sample_rate)
                || track.channels != Some(info.channels);
            track.codec = info.codec;
            track.clock_rate = info.sample_rate;
            track.sample_rate = Some(info.sample_rate);
            track.channels = Some(info.channels);
            if changed {
                events.push(MpegTsDemuxEvent::TrackFound(track_info_from_state(track)));
            }
        }
    }

    let data = Bytes::from(raw_payload);
    let format = frame_format_for_codec(track.codec);
    build_single_frame_event(track, data, format, pts_unwrapped, dts_unwrapped, events)
}

fn build_single_frame_event(
    track: &mut DemuxTrackState,
    data: Bytes,
    format: FrameFormat,
    pts_unwrapped: u64,
    dts_unwrapped: u64,
    mut events: Vec<MpegTsDemuxEvent>,
) -> Vec<MpegTsDemuxEvent> {
    let keyframe = track.media_kind == MediaKind::Video && is_keyframe_payload(&data, track.codec);

    let pts_us = (pts_unwrapped as i64) * 100 / 9;
    let dts_us = (dts_unwrapped as i64) * 100 / 9;

    let timebase = Timebase::new(1, 90_000);
    if matches!(track.codec, CodecId::H264 | CodecId::H265 | CodecId::H266)
        && track
            .parameter_sets
            .update_from_annexb(track.codec, data.as_ref())
        && track.parameter_sets.has_required_sets(track.codec)
    {
        events.push(MpegTsDemuxEvent::TrackFound(track_info_from_state(track)));
    }
    if track.codec == CodecId::AV1 && track.av1_sequence_header.is_none() {
        if let Some(sequence_header) = extract_av1_sequence_header_obu(data.as_ref()) {
            track.av1_sequence_header = Some(sequence_header);
            events.push(MpegTsDemuxEvent::TrackFound(track_info_from_state(track)));
        }
    }

    let mut frame = AVFrame::new(
        track.track_id,
        track.media_kind,
        track.codec,
        format,
        pts_unwrapped as i64,
        dts_unwrapped as i64,
        timebase,
        data,
    );
    frame.pts_us = pts_us;
    frame.dts_us = dts_us;

    // Derive G711 duration from payload length
    if matches!(track.codec, CodecId::G711A | CodecId::G711U) {
        let dur_90k = crate::ts_common::g711_duration_90k(frame.payload.len(), 8000);
        frame.duration = dur_90k as i64;
        frame.duration_us = crate::ts_common::g711_duration_us(frame.payload.len(), 8000) as i64;
    }

    if keyframe {
        frame.flags.insert(FrameFlags::KEY);
    }

    events.push(MpegTsDemuxEvent::Frame(frame));
    events
}

fn frame_format_for_codec(codec: CodecId) -> FrameFormat {
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
        CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
        CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
        CodecId::MJPEG => FrameFormat::MjpegFrame,
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP2 => FrameFormat::Mp2Frame,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::ADPCM => FrameFormat::AdpcmPacket,
        CodecId::Unknown => FrameFormat::Unknown,
    }
}

fn track_info_from_state(track: &DemuxTrackState) -> TrackInfo {
    let mut info = TrackInfo::new(
        track.track_id,
        track.media_kind,
        track.codec,
        track.clock_rate,
    );
    match track.codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => {
            if let Some(extradata) = track.parameter_sets.extradata_for_codec(track.codec) {
                info.extradata = extradata;
            }
        }
        CodecId::AAC => {
            if let Some(asc) = track.aac_asc {
                info.extradata = CodecExtradata::AAC {
                    asc: Bytes::copy_from_slice(&asc),
                };
                if let Some(config) = crate::audio::AacAudioSpecificConfig::from_bytes(&asc) {
                    let sample_rate = aac_sample_rate(config.sampling_frequency_index);
                    if sample_rate > 0 {
                        info.clock_rate = sample_rate;
                        info.sample_rate = Some(sample_rate);
                    }
                    info.channels =
                        crate::audio::aac_channel_count_from_config(config.channel_configuration);
                }
            }
        }
        CodecId::MP2 | CodecId::MP3 => {
            if let Some(sample_rate) = track.sample_rate {
                info.clock_rate = sample_rate;
                info.sample_rate = Some(sample_rate);
            }
            info.channels = track.channels;
        }
        CodecId::AV1 => {
            info.extradata = CodecExtradata::AV1 {
                sequence_header: track.av1_sequence_header.clone(),
                codec_config: None,
            };
        }
        _ => {}
    }
    info.refresh_readiness();
    info
}

fn infer_private_video_codec(payload: &[u8]) -> Option<(CodecId, FrameFormat)> {
    if looks_like_vp8_keyframe(payload) {
        return Some((CodecId::VP8, FrameFormat::CanonicalVp8Frame));
    }
    if looks_like_vp9_frame(payload) {
        return Some((CodecId::VP9, FrameFormat::CanonicalVp9Frame));
    }
    if looks_like_av1_obu_stream(payload) {
        return Some((CodecId::AV1, FrameFormat::CanonicalAv1Obu));
    }
    None
}

fn looks_like_vp8_keyframe(payload: &[u8]) -> bool {
    payload.len() >= 10 && payload.get(3..6) == Some(&[0x9d, 0x01, 0x2a])
}

fn looks_like_vp9_frame(payload: &[u8]) -> bool {
    payload.first().is_some_and(|byte| (byte >> 6) == 0b10) && vp9_frame_is_keyframe(payload)
}

fn looks_like_av1_obu_stream(payload: &[u8]) -> bool {
    av1_obu_payload_has_keyframe(payload) || extract_av1_sequence_header_obu(payload).is_some()
}

fn extract_av1_sequence_header_obu(payload: &[u8]) -> Option<Bytes> {
    let mut offset = 0usize;
    while offset < payload.len() {
        let start = offset;
        let header = *payload.get(offset)?;
        if (header & 0x80) != 0 {
            return None;
        }
        let obu_type = (header >> 3) & 0x0f;
        let has_extension = (header & 0x04) != 0;
        let has_size_field = (header & 0x02) != 0;
        offset = offset.checked_add(1)?;
        if has_extension {
            offset = offset.checked_add(1)?;
            if offset > payload.len() {
                return None;
            }
        }
        if has_size_field {
            let (obu_len, leb_len) = read_leb128_usize(payload.get(offset..)?)?;
            offset = offset.checked_add(leb_len)?;
            let end = offset.checked_add(obu_len)?;
            if end > payload.len() {
                return None;
            }
            if obu_type == 1 {
                return Some(Bytes::copy_from_slice(&payload[start..end]));
            }
            offset = end;
        } else {
            return None;
        }
    }
    None
}

fn read_leb128_usize(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, byte) in data.iter().take(8).copied().enumerate() {
        let shift = i.checked_mul(7)?;
        value |= ((byte & 0x7f) as usize).checked_shl(shift as u32)?;
        if (byte & 0x80) == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

struct MpegAudioFrameInfo {
    codec: CodecId,
    sample_rate: u32,
    channels: u8,
}

fn parse_mpeg_audio_header(payload: &[u8]) -> Option<MpegAudioFrameInfo> {
    if payload.len() < 4 || payload[0] != 0xFF || (payload[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version_id = (payload[1] >> 3) & 0x03;
    let layer = (payload[1] >> 1) & 0x03;
    if version_id == 0x01 || layer == 0x00 {
        return None;
    }
    let sample_rate_index = (payload[2] >> 2) & 0x03;
    if sample_rate_index == 0x03 {
        return None;
    }
    let base_rate = match sample_rate_index {
        0 => 44_100,
        1 => 48_000,
        2 => 32_000,
        _ => return None,
    };
    let sample_rate = match version_id {
        0x03 => base_rate,
        0x02 => base_rate / 2,
        0x00 => base_rate / 4,
        _ => return None,
    };
    let codec = match layer {
        0x01 => CodecId::MP3,
        0x02 | 0x03 => CodecId::MP2,
        _ => return None,
    };
    let channels = if (payload[3] >> 6) == 0x03 { 1 } else { 2 };
    Some(MpegAudioFrameInfo {
        codec,
        sample_rate,
        channels,
    })
}

/// Split AAC ADTS payload into individual frames with incremented timestamps.
fn split_aac_adts_frames(
    track: &mut DemuxTrackState,
    raw_payload: &[u8],
    pts_90k: u64,
    dts_90k: u64,
) -> Vec<MpegTsDemuxEvent> {
    let mut events = Vec::new();
    let mut offset = 0;
    let mut frame_idx: u32 = 0;
    let timebase = Timebase::new(1, 90_000);

    while offset < raw_payload.len() {
        let remaining = &raw_payload[offset..];

        // Try to parse ADTS header
        let Some(header) = crate::audio::AdtsHeader::parse(remaining) else {
            // Not ADTS - treat entire remaining as raw AAC (single frame)
            if offset == 0 {
                let data = Bytes::copy_from_slice(remaining);
                let pts_us = (pts_90k as i64) * 100 / 9;
                let dts_us = (dts_90k as i64) * 100 / 9;
                let mut frame = AVFrame::new(
                    track.track_id,
                    track.media_kind,
                    track.codec,
                    FrameFormat::AacRaw,
                    pts_90k as i64,
                    dts_90k as i64,
                    timebase,
                    data,
                );
                frame.pts_us = pts_us;
                frame.dts_us = dts_us;
                events.push(MpegTsDemuxEvent::Frame(frame));
            }
            break;
        };

        let frame_length = header.frame_length as usize;

        // Validate ADTS frame length
        if frame_length < 7 {
            events.push(MpegTsDemuxEvent::Diagnostic(
                MpegTsDemuxDiagnostic::AdtsError {
                    pid: track.pid,
                    reason: "ADTS frame_length < 7",
                },
            ));
            break;
        }
        if offset + frame_length > raw_payload.len() {
            events.push(MpegTsDemuxEvent::Diagnostic(
                MpegTsDemuxDiagnostic::AdtsError {
                    pid: track.pid,
                    reason: "ADTS frame_length exceeds payload",
                },
            ));
            break;
        }
        if header.sampling_frequency_index > 12 {
            events.push(MpegTsDemuxEvent::Diagnostic(
                MpegTsDemuxDiagnostic::AdtsError {
                    pid: track.pid,
                    reason: "invalid sampling_frequency_index",
                },
            ));
            break;
        }

        // Infer ASC from first valid ADTS header
        if track.aac_asc.is_none() {
            let asc = crate::audio::AacAudioSpecificConfig {
                audio_object_type: header.profile.wrapping_add(1),
                sampling_frequency_index: header.sampling_frequency_index,
                channel_configuration: header.channel_configuration,
            };
            track.aac_asc = Some(asc.to_bytes());
            events.push(MpegTsDemuxEvent::TrackFound(track_info_from_state(track)));
        }

        // Strip ADTS header, output raw AAC
        let aac_data = Bytes::copy_from_slice(&remaining[7..frame_length]);

        // Compute per-frame PTS offset: 1024 samples per AAC frame at 90kHz
        // Duration in 90kHz ticks = 1024 * 90000 / sample_rate
        let sample_rate = aac_sample_rate(header.sampling_frequency_index);
        let duration_90k = if sample_rate > 0 {
            (1024u64 * 90_000) / sample_rate as u64
        } else {
            0
        };
        let frame_pts = pts_90k + (frame_idx as u64) * duration_90k;
        let frame_dts = dts_90k + (frame_idx as u64) * duration_90k;
        let pts_us = (frame_pts as i64) * 100 / 9;
        let dts_us = (frame_dts as i64) * 100 / 9;

        let mut frame = AVFrame::new(
            track.track_id,
            track.media_kind,
            track.codec,
            FrameFormat::AacRaw,
            frame_pts as i64,
            frame_dts as i64,
            timebase,
            aac_data,
        );
        frame.pts_us = pts_us;
        frame.dts_us = dts_us;
        events.push(MpegTsDemuxEvent::Frame(frame));

        offset += frame_length;
        frame_idx += 1;
    }

    events
}

/// AAC sampling frequency table (index -> Hz).
fn aac_sample_rate(index: u8) -> u32 {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    RATES.get(index as usize).copied().unwrap_or(0)
}

fn is_keyframe_payload(data: &[u8], codec: CodecId) -> bool {
    match codec {
        CodecId::H264 => {
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
        CodecId::H265 => data.windows(5).any(|w| {
            w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x00 && w[3] == 0x01 && {
                let nal_type = (w[4] >> 1) & 0x3F;
                (16..=21).contains(&nal_type)
            }
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts_mux::{MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig};

    #[test]
    fn roundtrip_mux_demux() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);

        // Write tables
        let table_events = muxer.write_tables();
        let mut ts_data = Vec::new();
        for ev in &table_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        // Write a video frame
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB]),
        );
        frame.flags = FrameFlags::KEY;
        let frame_events = muxer.push_frame(&frame);
        for ev in &frame_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        // Demux
        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush_events = demuxer.flush();

        let all_events: Vec<_> = events.into_iter().chain(flush_events).collect();

        let track_found = all_events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
            .count();
        assert!(track_found >= 1, "should find at least 1 track");

        let frames = all_events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();
        assert!(frames >= 1, "should produce at least 1 frame");
    }

    #[test]
    fn unaligned_input_resync() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let table_events = muxer.write_tables();
        let mut ts_data = Vec::new();
        for ev in &table_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        // Prepend garbage
        let mut input = vec![0xAA; 50];
        input.extend_from_slice(&ts_data);

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&input);

        // Should have a sync loss diagnostic
        let has_sync_loss = events.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::SyncLoss)
            )
        });
        assert!(has_sync_loss, "should report sync loss");
    }

    #[test]
    fn chunked_input() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let table_events = muxer.write_tables();
        let mut ts_data = Vec::new();
        for ev in &table_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xCC]),
        );
        frame.flags = FrameFlags::KEY;
        let frame_events = muxer.push_frame(&frame);
        for ev in &frame_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        // Feed in small chunks
        let mut demuxer = MpegTsDemuxer::default();
        let mut all_events = Vec::new();
        for chunk in ts_data.chunks(50) {
            all_events.extend(demuxer.push(chunk));
        }
        all_events.extend(demuxer.flush());

        let frames = all_events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();
        assert!(frames >= 1, "chunked input should produce frames");
    }

    #[test]
    fn malformed_pes_pts_header_does_not_decode_payload_as_timestamp() {
        let mut track = DemuxTrackState {
            pid: 0x0100,
            track_id: TrackId(1),
            codec: CodecId::H264,
            media_kind: MediaKind::Video,
            clock_rate: 90_000,
            pes_buf: vec![
                0x00, 0x00, 0x01, 0xe0, // PES start
                0x00, 0x00, // PES packet length
                0x80, // marker bits
                0x80, // PTS only
                0x00, // invalid: PTS flag requires five header bytes
                0x00, 0x00, 0x01, 0x65, 0xaa, // payload must not become timestamp bytes
            ],
            pes_started: true,
            expected_cc: None,
            parameter_sets: ParameterSetCache::default(),
            aac_asc: None,
            sample_rate: None,
            channels: None,
            av1_sequence_header: None,
            pending_private_probe: false,
            pts_wrap_offset: 0,
            last_raw_pts: None,
        };

        let events = parse_pes_to_frames(&mut track);

        assert!(events.is_empty(), "malformed PES should be dropped");
        assert!(
            track.last_raw_pts.is_none(),
            "payload bytes must not update timestamp unwrap state"
        );
    }

    #[test]
    fn h264_parameter_sets_emit_ready_track_update() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(&data);
            }
        }

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x05, 0xbb,
                0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0x20, 0xf1, 0x83,
                0x19, 0x60, 0x00, 0x00, 0x00, 0x01, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0x00, 0x00,
                0x00, 0x01, 0x65, 0x88, 0x84,
            ]),
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(&data);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer
            .push(&ts_data)
            .into_iter()
            .chain(demuxer.flush())
            .collect::<Vec<_>>();

        let ready_track = events.iter().find_map(|event| {
            let MpegTsDemuxEvent::TrackFound(track) = event else {
                return None;
            };
            (track.codec == CodecId::H264 && track.is_ready()).then_some(track)
        });

        assert!(
            ready_track.is_some(),
            "H264 SPS/PPS should emit a ready track update"
        );
    }

    #[test]
    fn continuity_counter_gap_emits_diagnostic() {
        // Build valid TS with PAT+PMT, then manually corrupt CC
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let table_events = muxer.write_tables();
        let mut ts_data = Vec::new();
        for ev in &table_events {
            if let MpegTsMuxEvent::Packet(data) = ev {
                ts_data.extend_from_slice(data);
            }
        }

        // Write two video frames
        for i in 0..2 {
            let mut frame = AVFrame::new(
                TrackId(1),
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                90_000 * (i + 1),
                90_000 * (i + 1),
                Timebase::new(1, 90_000),
                Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xDD]),
            );
            frame.flags = FrameFlags::KEY;
            for ev in muxer.push_frame(&frame) {
                if let MpegTsMuxEvent::Packet(data) = ev {
                    ts_data.extend_from_slice(&data);
                }
            }
        }

        // Corrupt CC of the last video packet: find last packet with video PID 0x0100
        let pkt_count = ts_data.len() / 188;
        for i in (0..pkt_count).rev() {
            let off = i * 188;
            let pid = ((ts_data[off + 1] as u16 & 0x1F) << 8) | ts_data[off + 2] as u16;
            if pid == 0x0100 {
                // Corrupt CC by adding 5 (skip several)
                ts_data[off + 3] = (ts_data[off + 3] & 0xF0) | ((ts_data[off + 3] + 5) & 0x0F);
                break;
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let has_cc_gap = all.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::ContinuityGap { .. })
            )
        });
        assert!(has_cc_gap, "should detect continuity counter gap");
    }

    #[test]
    fn pts_dts_roundtrip_through_mux_demux() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Use values that roundtrip cleanly through us->90kHz conversion
        // 90000 ticks at 1/90000 = 1000000 us -> 90000 in 90kHz (exact)
        // 180000 ticks at 1/90000 = 2000000 us -> 180000 in 90kHz (exact)
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            180_000, // pts (2 seconds)
            90_000,  // dts (1 second)
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xEE]),
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let demuxed_frame = all.iter().find_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                Some(f)
            } else {
                None
            }
        });
        assert!(demuxed_frame.is_some(), "should produce a frame");
        let f = demuxed_frame.unwrap();
        // PTS/DTS preserved through mux->demux (90kHz ticks)
        assert_eq!(f.pts, 180_000);
        assert_eq!(f.dts, 90_000);
    }

    #[test]
    fn pat_pmt_crc_is_valid() {
        // Mux PAT+PMT and verify CRC by re-parsing
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // PAT is first packet, PMT is second
        assert_eq!(ts_data.len(), 376);
        for pkt_idx in 0..2 {
            let pkt = &ts_data[pkt_idx * 188..(pkt_idx + 1) * 188];
            assert_eq!(pkt[0], 0x47);
            // pointer field at byte 4 (PUSI set)
            let pointer = pkt[4] as usize;
            let section_start = 5 + pointer;
            let section_len =
                ((pkt[section_start + 1] as usize & 0x0F) << 8) | pkt[section_start + 2] as usize;
            let section_end = section_start + 3 + section_len;
            assert!(section_end <= 188);
            // Verify CRC: last 4 bytes of section are CRC, rest is data
            let section_data = &pkt[section_start..section_end - 4];
            let stored_crc = u32::from_be_bytes([
                pkt[section_end - 4],
                pkt[section_end - 3],
                pkt[section_end - 2],
                pkt[section_end - 1],
            ]);
            let computed_crc = crate::crc32_mpeg2(section_data);
            assert_eq!(stored_crc, computed_crc, "CRC mismatch in packet {pkt_idx}");
        }
    }

    #[test]
    fn pcr_present_on_keyframe() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xFF]),
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Find the first video PES packet (PID 0x0100, PUSI set)
        let pkt_count = ts_data.len() / 188;
        let mut found_pcr = false;
        for i in 0..pkt_count {
            let off = i * 188;
            let pid = ((ts_data[off + 1] as u16 & 0x1F) << 8) | ts_data[off + 2] as u16;
            let pusi = ts_data[off + 1] & 0x40 != 0;
            let af_control = (ts_data[off + 3] >> 4) & 0x03;
            if pid == 0x0100 && pusi && (af_control == 0x03 || af_control == 0x02) {
                let af_flags = ts_data[off + 5];
                if af_flags & 0x10 != 0 {
                    found_pcr = true;
                }
                break;
            }
        }
        assert!(found_pcr, "keyframe packet should contain PCR");
    }

    #[test]
    fn null_packets_are_ignored() {
        // Build a stream with a null packet (PID 0x1FFF) injected
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Insert a null packet
        let mut null_pkt = [0xFF_u8; 188];
        null_pkt[0] = 0x47;
        null_pkt[1] = 0x1F;
        null_pkt[2] = 0xFF;
        null_pkt[3] = 0x10;
        ts_data.extend_from_slice(&null_pkt);

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        // Should not crash or produce errors from null packet
        let errors = events.iter().filter(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::UnknownStreamType { .. })
            )
        });
        assert_eq!(errors.count(), 0, "null packets should be silently ignored");
    }

    #[test]
    fn pes_overflow_protection() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Create a frame that's large but within limits
        let big_payload = vec![0x65; 1000];
        let mut annexb = vec![0x00, 0x00, 0x00, 0x01];
        annexb.extend_from_slice(&big_payload);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from(annexb),
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Use a very small max_reassembly_bytes to trigger overflow
        let config = MpegTsDemuxerConfig {
            max_reassembly_bytes: 100,
            strict_crc: false,
        };
        let mut demuxer = MpegTsDemuxer::new(config);
        let events = demuxer.push(&ts_data);

        let has_overflow = events.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::PesOverflow { .. })
            )
        });
        assert!(has_overflow, "should detect PES overflow");
    }

    #[test]
    fn multi_track_roundtrip() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
            TrackInfo::new(TrackId(3), MediaKind::Audio, CodecId::MP2, 90_000),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Write one frame per track
        let mut vf = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAA]),
        );
        vf.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&vf) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let af = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            48_000,
            48_000,
            Timebase::new(1, 48_000),
            Bytes::from_static(&[0xDE, 0xAD]),
        );
        for ev in muxer.push_frame(&af) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mp2f = AVFrame::new(
            TrackId(3),
            MediaKind::Audio,
            CodecId::MP2,
            FrameFormat::Mp2Frame,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0xBE, 0xEF]),
        );
        for ev in muxer.push_frame(&mp2f) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let track_count = all
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
            .count();
        assert_eq!(track_count, 3, "should find all 3 tracks");

        let frame_count = all
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::Frame(_)))
            .count();
        assert_eq!(frame_count, 3, "should produce 3 frames");
    }

    #[test]
    fn aac_adts_wrap_and_strip_roundtrip() {
        use crate::track::CodecExtradata;
        // ASC: AAC-LC, 48kHz, stereo
        let asc_bytes = Bytes::from_static(&[0x11, 0x90]); // object_type=2, freq_idx=3, ch=2
        let mut track_info = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000);
        track_info.extradata = CodecExtradata::AAC {
            asc: asc_bytes.clone(),
        };

        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            track_info,
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Push raw AAC frame
        let raw_aac = Bytes::from_static(&[0x01, 0x02, 0x03, 0x04, 0x05]);
        let af = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            raw_aac.clone(),
        );
        for ev in muxer.push_frame(&af) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Demux should strip ADTS and return raw AAC
        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let ready_track = all.iter().find_map(|e| {
            let MpegTsDemuxEvent::TrackFound(track) = e else {
                return None;
            };
            (track.codec == CodecId::AAC && track.is_ready()).then_some(track)
        });
        assert!(
            ready_track.is_some(),
            "AAC ADTS should emit a ready track update with ASC"
        );

        let frame = all.iter().find_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                if f.codec == CodecId::AAC {
                    Some(f)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(frame.is_some(), "should produce AAC frame");
        let f = frame.unwrap();
        assert_eq!(f.format, FrameFormat::AacRaw);
        assert_eq!(
            &f.payload[..],
            &raw_aac[..],
            "demuxed AAC should be raw (ADTS stripped)"
        );
    }

    #[test]
    fn h266_keyframe_prepends_aud_vps_sps_pps() {
        use crate::track::CodecExtradata;
        let vps = Bytes::from_static(&[0x00, 0x70, 0x01]);
        let sps = Bytes::from_static(&[0x00, 0x78, 0x01]);
        let pps = Bytes::from_static(&[0x00, 0x80, 0x01]);
        let mut track_info = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H266, 90_000);
        track_info.extradata = CodecExtradata::H266 {
            vps: vec![vps],
            sps: vec![sps],
            pps: vec![pps],
        };

        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &[track_info]);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H266,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x38, 0x01, 0xAA]),
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let all: Vec<_> = demuxer
            .push(&ts_data)
            .into_iter()
            .chain(demuxer.flush())
            .collect();
        let video_frame = all.iter().find_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                (f.codec == CodecId::H266).then_some(f)
            } else {
                None
            }
        });
        let vf = video_frame.expect("should produce H266 frame");
        assert!(vf.payload.windows(7).any(|w| w == crate::AUD_H266));
        assert!(vf.payload.windows(7).any(|w| w == [0, 0, 0, 1, 0, 0x70, 1]));
        assert!(vf.payload.windows(7).any(|w| w == [0, 0, 0, 1, 0, 0x78, 1]));
        assert!(vf.payload.windows(7).any(|w| w == [0, 0, 0, 1, 0, 0x80, 1]));
    }

    #[test]
    fn private_descriptor_codecs_roundtrip() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::MJPEG, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::ADPCM, 90_000),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        for (track_id, media_kind, codec, format, payload) in [
            (
                TrackId(1),
                MediaKind::Video,
                CodecId::MJPEG,
                FrameFormat::MjpegFrame,
                Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
            ),
            (
                TrackId(2),
                MediaKind::Audio,
                CodecId::ADPCM,
                FrameFormat::AdpcmPacket,
                Bytes::from_static(&[0x11, 0x22, 0x33, 0x44]),
            ),
        ] {
            let frame = AVFrame::new(
                track_id,
                media_kind,
                codec,
                format,
                90_000,
                90_000,
                Timebase::new(1, 90_000),
                payload,
            );
            for ev in muxer.push_frame(&frame) {
                if let MpegTsMuxEvent::Packet(d) = ev {
                    ts_data.extend_from_slice(&d);
                }
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let all: Vec<_> = demuxer
            .push(&ts_data)
            .into_iter()
            .chain(demuxer.flush())
            .collect();
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track) if track.codec == CodecId::MJPEG
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track) if track.codec == CodecId::ADPCM
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame)
                if frame.codec == CodecId::MJPEG && frame.format == FrameFormat::MjpegFrame
        )));
        assert!(all.iter().any(|e| matches!(
        e,
        MpegTsDemuxEvent::Frame(frame)
            if frame.codec == CodecId::ADPCM && frame.format == FrameFormat::AdpcmPacket
            )));
    }

    #[test]
    fn unknown_private_video_stream_infers_vp8_from_payload() {
        let all = demux_unknown_private_video_payload(
            CodecId::VP8,
            FrameFormat::CanonicalVp8Frame,
            Bytes::from_static(&[0x50, 0x31, 0x00, 0x9d, 0x01, 0x2a, 0xa0, 0x00, 0x5a, 0x00]),
        );
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track)
                if track.codec == CodecId::VP8 && track.media_kind == MediaKind::Video
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame)
                if frame.codec == CodecId::VP8 && frame.format == FrameFormat::CanonicalVp8Frame
        )));
    }

    #[test]
    fn unknown_private_video_stream_infers_vp9_from_payload() {
        let all = demux_unknown_private_video_payload(
            CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            Bytes::from_static(&[0xa2, 0x49, 0x83, 0x42, 0xe0, 0x09, 0xf0, 0x05]),
        );
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track)
                if track.codec == CodecId::VP9 && track.media_kind == MediaKind::Video
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame)
                if frame.codec == CodecId::VP9 && frame.format == FrameFormat::CanonicalVp9Frame
        )));
    }

    #[test]
    fn unknown_private_video_stream_infers_av1_from_payload() {
        let all = demux_unknown_private_video_payload(
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            Bytes::from_static(&[
                0x12, 0x00, 0x0a, 0x0d, 0x20, 0x00, 0x00, 0x03, 0xb4, 0xfd, 0x93, 0x6b, 0xe4, 0x80,
                0x86, 0x80, 0x10,
            ]),
        );
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track)
                if track.codec == CodecId::AV1
                    && track.media_kind == MediaKind::Video
                    && track.is_ready()
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame)
                if frame.codec == CodecId::AV1 && frame.format == FrameFormat::CanonicalAv1Obu
        )));
    }

    #[test]
    fn unknown_private_video_stream_does_not_infer_av1_from_header_only_payload() {
        let all = demux_unknown_private_video_payload(
            CodecId::Unknown,
            FrameFormat::Unknown,
            Bytes::from_static(&[0x08, 0xaa, 0xbb, 0xcc]),
        );
        assert!(!all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track) if track.codec == CodecId::AV1
        )));
        assert!(!all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame) if frame.codec == CodecId::AV1
        )));
    }

    fn demux_unknown_private_video_payload(
        codec: CodecId,
        format: FrameFormat,
        payload: Bytes,
    ) -> Vec<MpegTsDemuxEvent> {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::Unknown,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            codec,
            format,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            payload,
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        demuxer
            .push(&ts_data)
            .into_iter()
            .chain(demuxer.flush())
            .collect()
    }

    #[test]
    fn mpeg_audio_stream_type_refines_mp3_from_frame_header() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Audio,
            CodecId::MP2,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Audio,
            CodecId::MP3,
            FrameFormat::Mp3Frame,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0xFF, 0xFB, 0x90, 0x64, 0x00, 0x00]),
        );
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let all: Vec<_> = demuxer
            .push(&ts_data)
            .into_iter()
            .chain(demuxer.flush())
            .collect();

        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::TrackFound(track)
                if track.codec == CodecId::MP3
                    && track.sample_rate == Some(44_100)
                    && track.channels == Some(2)
                    && track.is_ready()
        )));
        assert!(all.iter().any(|e| matches!(
            e,
            MpegTsDemuxEvent::Frame(frame)
                if frame.codec == CodecId::MP3 && frame.format == FrameFormat::Mp3Frame
        )));
    }

    #[test]
    fn h264_keyframe_prepends_sps_pps() {
        use crate::track::CodecExtradata;
        let sps = Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E]); // fake SPS
        let pps = Bytes::from_static(&[0x68, 0xCE, 0x38, 0x80]); // fake PPS
        let mut track_info = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track_info.extradata = CodecExtradata::H264 {
            sps: vec![sps.clone()],
            pps: vec![pps.clone()],
            avcc: None,
        };

        let tracks = vec![track_info];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Push keyframe
        let idr = Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB]);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000,
            90_000,
            Timebase::new(1, 90_000),
            idr,
        );
        frame.flags = FrameFlags::KEY;
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Demux and verify SPS/PPS are present in the output payload
        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let video_frame = all.iter().find_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                if f.codec == CodecId::H264 {
                    Some(f)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(video_frame.is_some(), "should produce H264 frame");
        let vf = video_frame.unwrap();
        assert!(vf.flags.contains(FrameFlags::KEY));
        // Payload should contain SPS NALU (0x67)
        let has_sps = vf.payload.windows(5).any(|w| {
            w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x00 && w[3] == 0x01 && (w[4] & 0x1F) == 7
        });
        assert!(has_sps, "keyframe should contain SPS");
    }

    #[test]
    fn non_keyframe_does_not_prepend_params() {
        use crate::track::CodecExtradata;
        let sps = Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E]);
        let pps = Bytes::from_static(&[0x68, 0xCE, 0x38, 0x80]);
        let mut track_info = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track_info.extradata = CodecExtradata::H264 {
            sps: vec![sps.clone()],
            pps: vec![pps.clone()],
            avcc: None,
        };

        let tracks = vec![track_info];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Push non-keyframe (no KEY flag)
        let slice = Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x41, 0xCC, 0xDD]);
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            180_000,
            180_000,
            Timebase::new(1, 90_000),
            slice,
        );
        for ev in muxer.push_frame(&frame) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let video_frame = all.iter().find_map(|e| {
            if let MpegTsDemuxEvent::Frame(f) = e {
                if f.codec == CodecId::H264 {
                    Some(f)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(video_frame.is_some());
        let vf = video_frame.unwrap();
        // Should NOT contain SPS (NAL type 7)
        let has_sps = vf.payload.windows(5).any(|w| {
            w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x00 && w[3] == 0x01 && (w[4] & 0x1F) == 7
        });
        assert!(!has_sps, "non-keyframe should NOT contain SPS");
    }

    #[test]
    fn crc_error_strict_mode_rejects_pat() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Corrupt PAT CRC (last 4 bytes of PAT section in first packet)
        // PAT is at offset 0, pointer field at byte 4 = 0, section starts at byte 5
        // section_len = 13, so section ends at 5 + 3 + 13 = 21, CRC at bytes 17..21
        ts_data[17] ^= 0xFF;

        // Strict mode: should reject PAT and not discover PMT
        let config = MpegTsDemuxerConfig {
            max_reassembly_bytes: 4 * 1024 * 1024,
            strict_crc: true,
        };
        let mut demuxer = MpegTsDemuxer::new(config);
        let events = demuxer.push(&ts_data);

        let has_crc_error = events.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::CrcError { .. })
            )
        });
        assert!(has_crc_error, "should report CRC error");

        // In strict mode, no tracks should be found (PAT rejected -> no PMT PID)
        let track_count = events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
            .count();
        assert_eq!(track_count, 0, "strict CRC should reject bad PAT");
    }

    #[test]
    fn crc_error_loose_mode_continues() {
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Corrupt PAT CRC
        ts_data[17] ^= 0xFF;

        // Loose mode (default): should diagnose but continue
        let config = MpegTsDemuxerConfig {
            max_reassembly_bytes: 4 * 1024 * 1024,
            strict_crc: false,
        };
        let mut demuxer = MpegTsDemuxer::new(config);
        let events = demuxer.push(&ts_data);

        let has_crc_error = events.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::CrcError { .. })
            )
        });
        assert!(has_crc_error, "should report CRC error");

        // In loose mode, tracks should still be found (PAT accepted despite bad CRC)
        let track_count = events
            .iter()
            .filter(|e| matches!(e, MpegTsDemuxEvent::TrackFound(_)))
            .count();
        assert!(track_count >= 1, "loose CRC should still discover tracks");
    }

    #[test]
    fn adaptation_field_overflow_emits_diagnostic() {
        // Build a TS packet with adaptation field length exceeding packet bounds
        let mut pkt = [0xFF_u8; 188];
        pkt[0] = 0x47;
        pkt[1] = 0x01; // PID = 0x0100
        pkt[2] = 0x00;
        pkt[3] = 0x30; // AF + payload
        pkt[4] = 200; // AF length > 183 (impossible)

        // Need PAT/PMT first so the demuxer knows about PID 0x0100
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }
        ts_data.extend_from_slice(&pkt);

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);

        let has_af_overflow = events.iter().any(|e| {
            matches!(
                e,
                MpegTsDemuxEvent::Diagnostic(MpegTsDemuxDiagnostic::AdaptationFieldOverflow { .. })
            )
        });
        assert!(has_af_overflow, "should detect adaptation field overflow");
    }

    #[test]
    fn pts_33bit_wrap_produces_monotonic_output() {
        // Simulate two frames: one near the 33-bit max, one after wrap
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Frame 1: PTS near max (2^33 - 90000)
        let pts_near_max: i64 = (0x1_FFFF_FFFF_u64 - 90_000) as i64;
        let mut frame1 = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            pts_near_max,
            pts_near_max,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA]),
        );
        frame1.flags = FrameFlags::KEY;
        // Override pts_us to match the 90kHz value
        frame1.pts_us = pts_near_max * 100 / 9;
        frame1.dts_us = pts_near_max * 100 / 9;
        for ev in muxer.push_frame(&frame1) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Frame 2: PTS after wrap (small value, simulating wrap)
        // The muxer will encode this as (pts_us * 9 / 100) & 0x1_FFFF_FFFF
        // We need to simulate a wrapped PTS. Since the muxer uses us_to_90k which masks,
        // let's directly construct the second frame with a PTS that wraps.
        let pts_after_wrap: i64 = 90_000; // small value after wrap
        let mut frame2 = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            pts_after_wrap,
            pts_after_wrap,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x41, 0xBB]),
        );
        frame2.pts_us = pts_after_wrap * 100 / 9;
        frame2.dts_us = pts_after_wrap * 100 / 9;
        for ev in muxer.push_frame(&frame2) {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        let mut demuxer = MpegTsDemuxer::default();
        let events = demuxer.push(&ts_data);
        let flush = demuxer.flush();
        let all: Vec<_> = events.into_iter().chain(flush).collect();

        let frames: Vec<_> = all
            .iter()
            .filter_map(|e| {
                if let MpegTsDemuxEvent::Frame(f) = e {
                    Some(f)
                } else {
                    None
                }
            })
            .collect();

        assert!(frames.len() >= 2, "should produce at least 2 frames");
        // After unwrapping, frame2.pts should be > frame1.pts (monotonic)
        assert!(
            frames[1].pts > frames[0].pts,
            "PTS should be monotonic after wrap unwrap: {} > {}",
            frames[1].pts,
            frames[0].pts
        );
    }
}
