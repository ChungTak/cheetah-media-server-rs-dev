//! Program Stream (PS) demuxer.
//!
//! 节目流（PS）解复用器。

use crate::frame::{AVFrame, FrameFormat};
use crate::prelude::*;
use crate::ps::diagnostic::{PsDemuxDiagnostic, PsDemuxEvent, PsDemuxerConfig};
use crate::ps::{default_frame_format, is_ps_stream_id, parse_pts_dts};
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};
use crate::video_payload_is_random_access;
use bytes::Bytes;

/// Program Stream (PS) demuxer.
///
/// Parses PS packets into `TrackInfo` discovery events and `AVFrame` media frames.
/// Buffers partial data between `push` calls and emits frames on `flush`.
///
/// 节目流（PS）解复用器。
/// 将 PS 包解析为 `TrackInfo` 发现事件与 `AVFrame` 媒体帧。
/// 在 `push` 调用之间缓冲不完整数据，并在 `flush` 时输出帧。
pub struct PsDemuxer {
    config: PsDemuxerConfig,
    remain_buffer: Vec<u8>,
    tracks: HashMap<u8, TrackInfo>,
    video_buffer: Vec<u8>,
    last_video_pts: Option<i64>,
    video_dts: Option<i64>,
    last_audio_pts: Option<i64>,
    audio_es_id: u8,
    new_ps: bool,
    probe_pack_count: u32,
    probe_exceeded: bool,
    tracks_ever_found: bool,
}

impl PsDemuxer {
    /// Create a new PS demuxer with the given configuration.
    ///
    /// 使用给定配置创建新的 PS 解复用器。
    pub fn new(config: PsDemuxerConfig) -> Self {
        Self {
            config,
            remain_buffer: Vec::new(),
            tracks: HashMap::new(),
            video_buffer: Vec::new(),
            last_video_pts: None,
            video_dts: None,
            last_audio_pts: None,
            audio_es_id: 0,
            new_ps: false,
            probe_pack_count: 0,
            probe_exceeded: false,
            tracks_ever_found: false,
        }
    }

    /// Push raw PS bytes. Returns parsed events.
    ///
    /// 压入原始 PS 字节，返回解析出的事件。
    pub fn push(&mut self, data: &[u8]) -> Vec<PsDemuxEvent> {
        let mut events = Vec::new();
        if self.remain_buffer.len() + data.len() > self.config.max_reassembly_bytes {
            self.remain_buffer.clear();
            events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::BufferOverflow));
            return events;
        }

        self.remain_buffer.extend_from_slice(data);

        let mut cursor = 0;
        while cursor + 4 <= self.remain_buffer.len() {
            if self.remain_buffer[cursor..cursor + 3] != [0x00, 0x00, 0x01] {
                if let Some(pos) = self.remain_buffer[cursor..]
                    .windows(3)
                    .position(|w| w == [0x00, 0x00, 0x01])
                {
                    cursor += pos;
                } else {
                    cursor = self.remain_buffer.len().saturating_sub(2);
                    break;
                }
            }

            if cursor + 4 > self.remain_buffer.len() {
                break;
            }

            let stream_id = self.remain_buffer[cursor + 3];
            match stream_id {
                0xBA => {
                    if cursor + 14 > self.remain_buffer.len() {
                        break;
                    }
                    let stuffing_len = (self.remain_buffer[cursor + 13] & 0x07) as usize;
                    let total_len = 14 + stuffing_len;
                    if cursor + total_len > self.remain_buffer.len() {
                        break;
                    }
                    self.new_ps = true;
                    self.probe_pack_count = self.probe_pack_count.saturating_add(1);
                    if !self.tracks_ever_found
                        && self.probe_pack_count > self.config.max_probe_packets
                    {
                        if !self.probe_exceeded {
                            self.probe_exceeded = true;
                            events.push(PsDemuxEvent::Diagnostic(
                                PsDemuxDiagnostic::LimitExceeded {
                                    resource: "probe_packets".to_string(),
                                },
                            ));
                        }
                        cursor += total_len;
                        continue;
                    }
                    cursor += total_len;
                }
                0xBB => {
                    if cursor + 6 > self.remain_buffer.len() {
                        break;
                    }
                    let header_len = u16::from_be_bytes([
                        self.remain_buffer[cursor + 4],
                        self.remain_buffer[cursor + 5],
                    ]) as usize;
                    let total_len = 6 + header_len;
                    if cursor + total_len > self.remain_buffer.len() {
                        break;
                    }
                    cursor += total_len;
                    self.probe_pack_count = 0;
                    self.probe_exceeded = false;
                }
                0xBC => {
                    if cursor + 6 > self.remain_buffer.len() {
                        break;
                    }
                    let psm_len = u16::from_be_bytes([
                        self.remain_buffer[cursor + 4],
                        self.remain_buffer[cursor + 5],
                    ]) as usize;
                    let total_len = 6 + psm_len;
                    if cursor + total_len > self.remain_buffer.len() {
                        break;
                    }
                    let psm_payload = self.remain_buffer[cursor + 6..cursor + total_len].to_vec();
                    self.parse_psm(&psm_payload, &mut events);
                    cursor += total_len;
                    self.probe_pack_count = 0;
                    self.probe_exceeded = false;
                }
                0xBD | 0xC0..=0xDF | 0xE0..=0xEF => {
                    if cursor + 6 > self.remain_buffer.len() {
                        break;
                    }
                    let pes_len = u16::from_be_bytes([
                        self.remain_buffer[cursor + 4],
                        self.remain_buffer[cursor + 5],
                    ]) as usize;

                    let total_len = if pes_len == 0 {
                        // PES_packet_length == 0 means "unbounded video PES".
                        // We must scan for the next PS-layer start code, but H.264 Annex-B
                        // NALU start codes (`00 00 01` / `00 00 00 01`) inside the payload
                        // also match the 3-byte triplet, so we additionally require that the
                        // byte after the prefix is a valid PS-layer stream id; otherwise we
                        // would truncate a video frame mid-NALU.
                        let max_payload = self.config.max_pes_packet_size.saturating_sub(6);
                        let search_end = (cursor + 6 + max_payload).min(self.remain_buffer.len());
                        let scan = &self.remain_buffer[cursor + 6..search_end];
                        let mut found: Option<usize> = None;
                        let mut probe = 0usize;
                        while probe + 4 <= scan.len() {
                            if scan[probe] == 0x00
                                && scan[probe + 1] == 0x00
                                && scan[probe + 2] == 0x01
                                && is_ps_stream_id(scan[probe + 3])
                            {
                                found = Some(probe);
                                break;
                            }
                            probe += 1;
                        }
                        if let Some(pos) = found {
                            6 + pos
                        } else if self.remain_buffer.len() - (cursor + 6) > max_payload {
                            // No valid start code within the configured PES size limit.
                            self.remain_buffer.clear();
                            events.push(PsDemuxEvent::Diagnostic(
                                PsDemuxDiagnostic::LimitExceeded {
                                    resource: "pes_packet_size".to_string(),
                                },
                            ));
                            return events;
                        } else {
                            // Wait for more bytes to disambiguate.
                            break;
                        }
                    } else {
                        6 + pes_len
                    };

                    if total_len > self.config.max_pes_packet_size {
                        self.remain_buffer.clear();
                        events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                            resource: "pes_packet_size".to_string(),
                        }));
                        return events;
                    }

                    if cursor + total_len > self.remain_buffer.len() {
                        break;
                    }

                    let pes_payload = self.remain_buffer[cursor..cursor + total_len].to_vec();
                    self.parse_pes(stream_id, &pes_payload, &mut events);
                    cursor += total_len;
                    self.probe_pack_count = 0;
                    self.probe_exceeded = false;
                }
                _ => {
                    cursor += 4;
                }
            }
        }

        if cursor > 0 {
            self.remain_buffer.drain(..cursor);
        }

        events
    }

    /// Flush any buffered video data and return remaining events.
    ///
    /// 刷新所有缓冲的视频数据并返回剩余事件。
    pub fn flush(&mut self) -> Vec<PsDemuxEvent> {
        let mut events = Vec::new();
        self.emit_video_frame(&mut events);
        events
    }

    fn parse_psm(&mut self, payload: &[u8], events: &mut Vec<PsDemuxEvent>) {
        if payload.len() < 10 {
            events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::PsmParseError));
            return;
        }
        let psm_length = payload.len();
        let mut buf = &payload[2..];
        if buf.len() < 2 {
            return;
        }
        let ps_info_length = u16::from_be_bytes([buf[0], buf[1]]) as usize;
        buf = &buf[2..];
        if buf.len() < ps_info_length + 2 {
            return;
        }
        buf = &buf[ps_info_length + 2..];

        let mut es_map_length = psm_length.saturating_sub(ps_info_length + 10);
        let mut new_tracks = Vec::new();

        while es_map_length >= 4 && buf.len() >= 4 {
            let es_type = buf[0];
            let es_id = buf[1];
            let es_info_length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            if buf.len() < 4 + es_info_length {
                break;
            }
            buf = &buf[4 + es_info_length..];
            es_map_length = es_map_length.saturating_sub(4 + es_info_length);

            let codec = match es_type {
                0x1B => CodecId::H264,
                0x24 => CodecId::H265,
                0x0F | 0x11 => CodecId::AAC,
                0x90 | 0x07 => CodecId::G711A,
                0x91 | 0x08 => CodecId::G711U,
                0x03 => CodecId::MP3,
                0x04 => CodecId::MP3,
                0x80 => CodecId::Opus,
                _ => continue,
            };

            let media_kind = if (0xE0..=0xEF).contains(&es_id) {
                MediaKind::Video
            } else {
                MediaKind::Audio
            };

            let track_id = TrackId(es_id as u32);
            let timescale = if media_kind == MediaKind::Video {
                90_000
            } else {
                8_000
            };

            let track_info = TrackInfo::new(track_id, media_kind, codec, timescale);
            new_tracks.push(track_info);
        }

        if !new_tracks.is_empty() {
            let new_keys = new_tracks
                .iter()
                .filter(|t| !self.tracks.contains_key(&(t.track_id.0 as u8)))
                .count();
            if self.tracks.len() + new_keys > self.config.max_tracks {
                events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                    resource: "tracks".to_string(),
                }));
                return;
            }
            for track in &new_tracks {
                let track_key = track.track_id.0 as u8;
                self.tracks.insert(track_key, track.clone());
            }
            self.tracks_ever_found = true;
            events.push(PsDemuxEvent::TrackInfo(new_tracks));
        }
    }

    fn parse_pes(&mut self, stream_id: u8, pes_packet: &[u8], events: &mut Vec<PsDemuxEvent>) {
        if pes_packet.len() < 9 {
            events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::PesParseError));
            return;
        }

        let length = u16::from_be_bytes([pes_packet[4], pes_packet[5]]) as usize;
        let info1 = pes_packet[7];
        let stuffing_len = pes_packet[8] as usize;
        let data_start = 9 + stuffing_len;
        if pes_packet.len() < data_start {
            events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::PesParseError));
            return;
        }

        let payload_len = if length == 0 {
            pes_packet.len() - data_start
        } else {
            (6 + length)
                .saturating_sub(data_start)
                .min(pes_packet.len() - data_start)
        };

        if payload_len == 0 {
            return;
        }

        let pts_dts_flags = (info1 & 0xC0) >> 6;
        let mut pts = None;
        let mut dts = None;
        let mut cursor = 9;
        if (pts_dts_flags & 2) != 0 && cursor + 5 <= data_start {
            pts = parse_pts_dts(&pes_packet[cursor..cursor + 5]);
            cursor += 5;
        }
        if (pts_dts_flags & 1) != 0 && cursor + 5 <= data_start {
            dts = parse_pts_dts(&pes_packet[cursor..cursor + 5]);
        }

        let is_video = (0xE0..=0xEF).contains(&stream_id);
        let is_audio = (0xC0..=0xDF).contains(&stream_id) || stream_id == self.audio_es_id;

        let payload = &pes_packet[data_start..data_start + payload_len];

        if is_video {
            if let Some(_track) = self
                .tracks
                .values()
                .find(|t| t.media_kind == MediaKind::Video)
            {
                if self.new_ps && !self.video_buffer.is_empty() {
                    self.new_ps = false;
                    self.emit_video_frame(events);
                }

                if self.video_buffer.len() + payload.len() > self.config.max_access_unit_size {
                    self.video_buffer.clear();
                    self.last_video_pts = None;
                    self.video_dts = None;
                    events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                        resource: "access_unit".to_string(),
                    }));
                    return;
                }

                self.video_buffer.extend_from_slice(payload);
                if pts.is_some() {
                    self.last_video_pts = pts;
                    if self.video_dts.is_none() {
                        self.video_dts = dts.or(pts);
                    }
                }
            } else {
                if !self.tracks.contains_key(&stream_id)
                    && self.tracks.len() >= self.config.max_tracks
                {
                    events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                        resource: "tracks".to_string(),
                    }));
                    return;
                }
                if self.video_buffer.len() + payload.len() > self.config.max_access_unit_size {
                    self.video_buffer.clear();
                    self.last_video_pts = None;
                    self.video_dts = None;
                    events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                        resource: "access_unit".to_string(),
                    }));
                    return;
                }
                let track_id = TrackId(stream_id as u32);
                let track_info = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
                self.tracks.insert(stream_id, track_info.clone());
                self.tracks_ever_found = true;
                events.push(PsDemuxEvent::TrackInfo(vec![track_info]));

                self.video_buffer.extend_from_slice(payload);
                self.last_video_pts = pts;
                self.video_dts = dts.or(pts);
            }
        } else if is_audio {
            if let Some(track) = self
                .tracks
                .values()
                .find(|t| t.media_kind == MediaKind::Audio)
            {
                let pts_val = pts.unwrap_or(self.last_audio_pts.unwrap_or(0));
                let dts_val = dts.or(pts).unwrap_or(pts_val);

                let track_clock = track.clock_rate.max(1) as i128;
                let pts_converted = (pts_val as i128 * track_clock / 90_000) as i64;
                let dts_converted = (dts_val as i128 * track_clock / 90_000) as i64;

                let frame = AVFrame::new(
                    track.track_id,
                    track.media_kind,
                    track.codec,
                    default_frame_format(track.codec),
                    pts_converted,
                    dts_converted,
                    Timebase::new(1, track.clock_rate.max(1)),
                    Bytes::copy_from_slice(payload),
                );
                events.push(PsDemuxEvent::Frame(Box::new(frame)));
                self.last_audio_pts = pts;
            } else {
                if !self.tracks.contains_key(&stream_id)
                    && self.tracks.len() >= self.config.max_tracks
                {
                    events.push(PsDemuxEvent::Diagnostic(PsDemuxDiagnostic::LimitExceeded {
                        resource: "tracks".to_string(),
                    }));
                    return;
                }
                let track_id = TrackId(stream_id as u32);
                let track_info = TrackInfo::new(track_id, MediaKind::Audio, CodecId::G711A, 8_000);
                self.tracks.insert(stream_id, track_info.clone());
                self.audio_es_id = stream_id;
                self.tracks_ever_found = true;
                events.push(PsDemuxEvent::TrackInfo(vec![track_info]));

                let pts_val = pts.unwrap_or(0);
                let dts_val = dts.or(pts).unwrap_or(pts_val);
                let pts_converted = (pts_val as i128 * 8_000 / 90_000) as i64;
                let dts_converted = (dts_val as i128 * 8_000 / 90_000) as i64;

                let frame = AVFrame::new(
                    track_id,
                    MediaKind::Audio,
                    CodecId::G711A,
                    FrameFormat::G711Packet,
                    pts_converted,
                    dts_converted,
                    Timebase::new(1, 8_000),
                    Bytes::copy_from_slice(payload),
                );
                events.push(PsDemuxEvent::Frame(Box::new(frame)));
                self.last_audio_pts = pts;
            }
        }
    }

    fn emit_video_frame(&mut self, events: &mut Vec<PsDemuxEvent>) {
        if self.video_buffer.is_empty() {
            return;
        }

        if let Some(track) = self
            .tracks
            .values()
            .find(|t| t.media_kind == MediaKind::Video)
        {
            let pts = self.last_video_pts.unwrap_or(0);
            let dts = self.video_dts.unwrap_or(pts);

            let is_keyframe = video_payload_is_random_access(
                track.codec,
                default_frame_format(track.codec),
                &self.video_buffer,
            );

            let track_clock = track.clock_rate.max(1) as i128;
            let pts_converted = (pts as i128 * track_clock / 90_000) as i64;
            let dts_converted = (dts as i128 * track_clock / 90_000) as i64;

            let mut frame = AVFrame::new(
                track.track_id,
                track.media_kind,
                track.codec,
                default_frame_format(track.codec),
                pts_converted,
                dts_converted,
                Timebase::new(1, track.clock_rate.max(1)),
                Bytes::from(core::mem::take(&mut self.video_buffer)),
            );
            if is_keyframe {
                frame.flags.insert(crate::frame::FrameFlags::KEY);
            }
            events.push(PsDemuxEvent::Frame(Box::new(frame)));
        } else {
            self.video_buffer.clear();
        }
        self.video_dts = None;
    }
}
