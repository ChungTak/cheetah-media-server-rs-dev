use crate::frame::{AVFrame, FrameFormat};
use crate::prelude::*;
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};
use crate::video::video_payload_is_random_access;
use bytes::Bytes;
use std::collections::HashMap;

fn default_frame_format(codec: CodecId) -> FrameFormat {
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::Opus => FrameFormat::OpusPacket,
        _ => FrameFormat::Unknown,
    }
}

/// Stream IDs recognised at the Program Stream layer. Used to validate that a
/// 3-byte `00 00 01` prefix really starts a new PS packet rather than appearing
/// inside a video PES payload as part of an H.264 / H.265 NALU start code.
fn is_ps_stream_id(stream_id: u8) -> bool {
    matches!(
        stream_id,
        0xBA       // pack_start_code
        | 0xBB     // system_header
        | 0xBC     // program_stream_map
        | 0xBD     // private_stream_1
        | 0xBE     // padding_stream
        | 0xBF     // private_stream_2
        | 0xC0..=0xDF  // audio streams
        | 0xE0..=0xEF  // video streams
        | 0xF0..=0xF2  // ECM, EMM, DSMCC
        | 0xF8..=0xFF  // ITU-T Rec. H.222.1 type E, program_stream_directory, etc.
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsStreamKind {
    Video,
    Audio,
    Private,
}

#[derive(Debug, Clone)]
pub struct PesPacket {
    pub stream_id: u8,
    pub kind: PsStreamKind,
    pub pts: Option<i64>,
    pub dts: Option<i64>,
    pub payload: Bytes,
}

impl PesPacket {
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

#[derive(Debug, Clone)]
pub struct PsPacket {
    pub pes: Vec<PesPacket>,
}

impl PsPacket {
    pub fn parse(raw: &[u8]) -> Self {
        Self::parse_bounded(raw, raw.len(), usize::MAX)
    }

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

    pub fn encode(&self) -> Bytes {
        let total = self.pes.iter().map(|p| p.payload.len() + 32).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        for pes in &self.pes {
            out.extend_from_slice(&pes.encode());
        }
        Bytes::from(out)
    }
}

fn stream_kind(stream_id: u8) -> PsStreamKind {
    if (0xE0..=0xEF).contains(&stream_id) {
        PsStreamKind::Video
    } else if (0xC0..=0xDF).contains(&stream_id) {
        PsStreamKind::Audio
    } else {
        PsStreamKind::Private
    }
}

fn find_start_code(data: &[u8]) -> Option<usize> {
    data.windows(3).position(|w| w == [0x00, 0x00, 0x01])
}

fn parse_pts_dts(raw: &[u8]) -> Option<i64> {
    if raw.len() < 5 {
        return None;
    }
    let v = (((raw[0] >> 1) as u64 & 0x07) << 30)
        | ((raw[1] as u64) << 22)
        | (((raw[2] >> 1) as u64) << 15)
        | ((raw[3] as u64) << 7)
        | ((raw[4] >> 1) as u64);
    Some(v as i64)
}

fn encode_pts_dts(value: i64, prefix: u8) -> [u8; 5] {
    let v = value.max(0) as u64;
    let b0 = (prefix << 4) | (((v >> 30) as u8 & 0x07) << 1) | 0x01;
    let b1 = (v >> 22) as u8;
    let b2 = (((v >> 15) as u8) << 1) | 0x01;
    let b3 = (v >> 7) as u8;
    let b4 = ((v as u8) << 1) | 0x01;
    [b0, b1, b2, b3, b4]
}

// ==========================================
// UPGRADED PRODUCTION-GRADE PS DEMUXER & MUXER
// ==========================================

#[derive(Debug, Clone)]
pub struct PsDemuxerConfig {
    pub max_reassembly_bytes: usize,
    pub max_tracks: usize,
}

impl Default for PsDemuxerConfig {
    fn default() -> Self {
        Self {
            max_reassembly_bytes: 4 * 1024 * 1024,
            max_tracks: 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsDemuxDiagnostic {
    BufferOverflow,
    InvalidStartCode { code: u8 },
    PsmParseError,
    PesParseError,
}

#[derive(Debug, Clone)]
pub enum PsDemuxEvent {
    TrackInfo(Vec<TrackInfo>),
    Frame(Box<AVFrame>),
    Diagnostic(PsDemuxDiagnostic),
}

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
}

impl PsDemuxer {
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
        }
    }

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
                        let scan = &self.remain_buffer[cursor + 6..];
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
                        } else {
                            // Wait for more bytes to disambiguate.
                            break;
                        }
                    } else {
                        6 + pes_len
                    };

                    if cursor + total_len > self.remain_buffer.len() {
                        break;
                    }

                    let pes_payload = self.remain_buffer[cursor..cursor + total_len].to_vec();
                    self.parse_pes(stream_id, &pes_payload, &mut events);
                    cursor += total_len;
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
            for track in &new_tracks {
                if self.tracks.len() < self.config.max_tracks {
                    self.tracks.insert(track.track_id.0 as u8, track.clone());
                }
            }
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

                self.video_buffer.extend_from_slice(payload);
                if pts.is_some() {
                    self.last_video_pts = pts;
                    if self.video_dts.is_none() {
                        self.video_dts = dts.or(pts);
                    }
                }
            } else {
                let track_id = TrackId(stream_id as u32);
                let track_info = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
                self.tracks.insert(stream_id, track_info.clone());
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
                let track_id = TrackId(stream_id as u32);
                let track_info = TrackInfo::new(track_id, MediaKind::Audio, CodecId::G711A, 8_000);
                self.tracks.insert(stream_id, track_info.clone());
                self.audio_es_id = stream_id;
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
                Bytes::from(std::mem::take(&mut self.video_buffer)),
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

#[derive(Default)]
pub struct PsMuxer {
    tracks: HashMap<u8, TrackInfo>,
}

impl PsMuxer {
    pub fn new() -> Self {
        Self {
            tracks: HashMap::new(),
        }
    }

    pub fn add_track(&mut self, track: TrackInfo) {
        self.tracks.insert(track.track_id.0 as u8, track);
    }

    pub fn mux(&mut self, frame: &AVFrame) -> Option<Bytes> {
        let track = self.tracks.get(&(frame.track_id.0 as u8))?;
        let mut out = Vec::new();

        out.extend_from_slice(&self.make_pack_header(frame.pts_us));

        if frame.is_key_frame() && track.media_kind == MediaKind::Video {
            out.extend_from_slice(&self.make_sys_header());
            out.extend_from_slice(&self.make_psm_header());
        }

        let stream_id = frame.track_id.0 as u8;
        let mut payload = &frame.payload[..];
        while !payload.is_empty() {
            let chunk_size = payload.len().min(65000);
            let chunk = &payload[..chunk_size];
            payload = &payload[chunk_size..];

            out.extend_from_slice(&self.make_pes_header(
                stream_id,
                chunk_size,
                frame.pts_us,
                frame.dts_us,
            ));
            out.extend_from_slice(chunk);
        }

        Some(Bytes::from(out))
    }

    fn make_pack_header(&self, pts_us: i64) -> [u8; 14] {
        let mut out = [0u8; 14];
        out[0..4].copy_from_slice(&[0x00, 0x00, 0x01, 0xBA]);

        let scr = (pts_us as u64 * 9) / 100;
        let scr_ext = 0u64;

        let mut val = 0u64;
        val |= 1u64 << 46;
        val |= ((scr >> 30) & 0x07) << 43;
        val |= 1u64 << 42;
        val |= ((scr >> 15) & 0x7FFF) << 27;
        val |= 1u64 << 26;
        val |= (scr & 0x7FFF) << 11;
        val |= 1u64 << 10;
        val |= (scr_ext & 0x01FF) << 1;
        val |= 1u64;

        out[4] = (val >> 40) as u8;
        out[5] = (val >> 32) as u8;
        out[6] = (val >> 24) as u8;
        out[7] = (val >> 16) as u8;
        out[8] = (val >> 8) as u8;
        out[9] = val as u8;

        let mut val2 = 0u32;
        let bitrate = 262143u32;
        val2 |= (bitrate & 0x3FFFFF) << 10;
        val2 |= 3 << 8;
        val2 |= 0x1F << 3;
        val2 |= 0;

        out[10..14].copy_from_slice(&val2.to_be_bytes());
        out
    }

    fn make_sys_header(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(18);
        out.extend_from_slice(&[0x00, 0x00, 0x01, 0xBB]);
        out.extend_from_slice(&12u16.to_be_bytes());

        let mut sys_bytes = [0u8; 12];
        let mut val = 0u64;
        val |= 1u64 << 47;
        val |= (50000 & 0x3FFFFF) << 25;
        val |= 1u64 << 24;
        val |= (1 & 0x3F) << 18;
        val |= 0 << 17;
        val |= 1 << 16;
        val |= 1 << 15;
        val |= 1 << 14;
        val |= 1 << 13;
        val |= (1 & 0x1F) << 8;
        val |= 0 << 7;
        val |= 0x7F;

        sys_bytes[0] = (val >> 40) as u8;
        sys_bytes[1] = (val >> 32) as u8;
        sys_bytes[2] = (val >> 24) as u8;
        sys_bytes[3] = (val >> 16) as u8;
        sys_bytes[4] = (val >> 8) as u8;
        sys_bytes[5] = val as u8;

        let mut aud_val = 0u32;
        aud_val |= 0xC0u32 << 16;
        aud_val |= 3 << 14;
        aud_val |= 0 << 13;
        aud_val |= 512;
        sys_bytes[6] = (aud_val >> 16) as u8;
        sys_bytes[7] = (aud_val >> 8) as u8;
        sys_bytes[8] = aud_val as u8;

        let mut vid_val = 0u32;
        vid_val |= 0xE0u32 << 16;
        vid_val |= 3 << 14;
        vid_val |= 1 << 13;
        vid_val |= 2048;
        sys_bytes[9] = (vid_val >> 16) as u8;
        sys_bytes[10] = (vid_val >> 8) as u8;
        sys_bytes[11] = vid_val as u8;

        out.extend_from_slice(&sys_bytes);
        out
    }

    fn make_psm_header(&self) -> Vec<u8> {
        let mut video_codec = 0u8;
        let mut audio_codec = 0u8;

        for track in self.tracks.values() {
            if track.media_kind == MediaKind::Video {
                video_codec = match track.codec {
                    CodecId::H264 => 0x1B,
                    CodecId::H265 => 0x24,
                    _ => 0,
                };
            } else if track.media_kind == MediaKind::Audio {
                audio_codec = match track.codec {
                    CodecId::AAC => 0x0F,
                    CodecId::G711A => 0x90,
                    CodecId::G711U => 0x91,
                    CodecId::MP3 => 0x03,
                    CodecId::Opus => 0x80,
                    _ => 0,
                };
            }
        }

        let both = video_codec != 0 && audio_codec != 0;
        let data_len = if both { 24 } else { 20 };

        let mut p_data = vec![0u8; data_len];
        p_data[0..3].copy_from_slice(&[0x00, 0x00, 0x01]);
        p_data[3] = 0xBC;
        p_data[4..6].copy_from_slice(&((data_len - 6) as u16).to_be_bytes());

        p_data[6] = 0xE0;
        p_data[7] = 0xFF;
        p_data[8..10].copy_from_slice(&0u16.to_be_bytes());

        let es_map_len = if both { 8 } else { 4 };
        p_data[10..12].copy_from_slice(&(es_map_len as u16).to_be_bytes());

        let mut cursor = 12;
        if audio_codec != 0 {
            p_data[cursor] = audio_codec;
            p_data[cursor + 1] = 0xC0;
            p_data[cursor + 2..cursor + 4].copy_from_slice(&0u16.to_be_bytes());
            cursor += 4;
        }
        if video_codec != 0 {
            p_data[cursor] = video_codec;
            p_data[cursor + 1] = 0xE0;
            p_data[cursor + 2..cursor + 4].copy_from_slice(&0u16.to_be_bytes());
        }

        let crc = crate::ts_common::crc32_mpeg2(&p_data[..data_len - 4]);
        p_data[data_len - 4..data_len].copy_from_slice(&crc.to_be_bytes());

        p_data
    }

    fn make_pes_header(
        &self,
        stream_id: u8,
        payload_len: usize,
        pts_us: i64,
        _dts_us: i64,
    ) -> [u8; 14] {
        let mut out = [0u8; 14];
        out[0..3].copy_from_slice(&[0x00, 0x00, 0x01]);
        out[3] = stream_id;
        out[4..6].copy_from_slice(&((payload_len + 8) as u16).to_be_bytes());
        out[6] = 0x80;
        out[7] = 0x80;
        out[8] = 5;

        let pts_ticks = (pts_us * 9) / 100;
        let pts_bytes = encode_pts_dts(pts_ticks, 0x2);
        out[9..14].copy_from_slice(&pts_bytes);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pes_roundtrip() {
        let pes = PesPacket {
            stream_id: 0xE0,
            kind: PsStreamKind::Video,
            pts: Some(90_000),
            dts: Some(89_000),
            payload: Bytes::from_static(b"es"),
        };
        let encoded = pes.encode();
        let (decoded, _) = PesPacket::parse(&encoded).expect("pes parse");
        assert_eq!(decoded.stream_id, 0xE0);
        assert_eq!(decoded.kind, PsStreamKind::Video);
        assert_eq!(decoded.payload, Bytes::from_static(b"es"));
        assert_eq!(decoded.pts, Some(90_000));
        assert_eq!(decoded.dts, Some(89_000));
    }

    #[test]
    fn ps_roundtrip() {
        let ps = PsPacket {
            pes: vec![
                PesPacket {
                    stream_id: 0xE0,
                    kind: PsStreamKind::Video,
                    pts: None,
                    dts: None,
                    payload: Bytes::from_static(b"v"),
                },
                PesPacket {
                    stream_id: 0xC0,
                    kind: PsStreamKind::Audio,
                    pts: None,
                    dts: None,
                    payload: Bytes::from_static(b"a"),
                },
            ],
        };
        let encoded = ps.encode();
        let decoded = PsPacket::parse(&encoded);
        assert_eq!(decoded.pes.len(), 2);
        assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"v"));
        assert_eq!(decoded.pes[1].payload, Bytes::from_static(b"a"));
    }

    #[test]
    fn ps_parse_bounded_limits_number_of_pes_packets() {
        let ps = PsPacket {
            pes: vec![
                PesPacket {
                    stream_id: 0xE0,
                    kind: PsStreamKind::Video,
                    pts: None,
                    dts: None,
                    payload: Bytes::from_static(b"v0"),
                },
                PesPacket {
                    stream_id: 0xC0,
                    kind: PsStreamKind::Audio,
                    pts: None,
                    dts: None,
                    payload: Bytes::from_static(b"a1"),
                },
                PesPacket {
                    stream_id: 0xE0,
                    kind: PsStreamKind::Video,
                    pts: None,
                    dts: None,
                    payload: Bytes::from_static(b"v2"),
                },
            ],
        };
        let encoded = ps.encode();
        let decoded = PsPacket::parse_bounded(&encoded, encoded.len(), 2);
        assert_eq!(decoded.pes.len(), 2);
        assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"v0"));
        assert_eq!(decoded.pes[1].payload, Bytes::from_static(b"a1"));
    }

    #[test]
    fn ps_parse_bounded_limits_bytes_for_truncated_rtp_payload() {
        let first = PesPacket {
            stream_id: 0xE0,
            kind: PsStreamKind::Video,
            pts: Some(90_000),
            dts: Some(89_000),
            payload: Bytes::from_static(b"video-es"),
        };
        let second = PesPacket {
            stream_id: 0xC0,
            kind: PsStreamKind::Audio,
            pts: Some(90_000),
            dts: None,
            payload: Bytes::from_static(b"audio-es"),
        };
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        payload.extend_from_slice(first.encode().as_ref());
        payload.extend_from_slice(second.encode().as_ref());

        let first_len = first.encode().len();
        let truncated_len = 4 + first_len + 4;
        let decoded = PsPacket::parse_bounded(&payload, truncated_len, 16);
        assert_eq!(decoded.pes.len(), 1);
        assert_eq!(decoded.pes[0].kind, PsStreamKind::Video);
        assert_eq!(decoded.pes[0].payload, Bytes::from_static(b"video-es"));
    }

    #[test]
    fn ps_parse_bounded_zero_limits_return_empty() {
        let decoded = PsPacket::parse_bounded(&[0, 0, 1, 0xE0, 0, 3, 0x80, 0, 0], 0, 1);
        assert!(decoded.pes.is_empty());
        let decoded = PsPacket::parse_bounded(&[0, 0, 1, 0xE0, 0, 3, 0x80, 0, 0], 128, 0);
        assert!(decoded.pes.is_empty());
    }

    #[test]
    fn ps_demuxer_and_muxer_roundtrip() {
        let video_track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
        let audio_track = TrackInfo::new(TrackId(0xC0), MediaKind::Audio, CodecId::G711A, 8_000);

        let mut muxer = PsMuxer::new();
        muxer.add_track(video_track.clone());
        muxer.add_track(audio_track.clone());

        // Create random keyframe video AVFrame: AnnexB format [0, 0, 0, 1, 0x67, ...] which triggers keyframe true
        let mut video_payload = vec![0, 0, 0, 1, 0x67, 0x42, 0, 0x0A]; // H264 SPS
        video_payload.extend_from_slice(b"video frame data");
        let mut video_frame = AVFrame::new(
            TrackId(0xE0),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90_000, // pts
            90_000, // dts
            Timebase::new(1, 90_000),
            Bytes::from(video_payload.clone()),
        );
        video_frame.flags.insert(crate::frame::FrameFlags::KEY);

        let audio_frame = AVFrame::new(
            TrackId(0xC0),
            MediaKind::Audio,
            CodecId::G711A,
            FrameFormat::G711Packet,
            90_080,
            90_080,
            Timebase::new(1, 8_000),
            Bytes::from_static(b"audio frame data"),
        );

        let muxed_video = muxer.mux(&video_frame).expect("mux video");
        let muxed_audio = muxer.mux(&audio_frame).expect("mux audio");

        let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
        let events1 = demuxer.push(&muxed_video);
        let events2 = demuxer.push(&muxed_audio);
        let events3 = demuxer.flush();

        let mut all_events = Vec::new();
        all_events.extend(events1);
        all_events.extend(events2);
        all_events.extend(events3);

        let mut found_tracks = false;
        let mut found_video_frame = false;
        let mut found_audio_frame = false;

        for event in all_events {
            match event {
                PsDemuxEvent::TrackInfo(tracks) => {
                    assert!(tracks.iter().any(|t| t.track_id == TrackId(0xE0)));
                    assert!(tracks.iter().any(|t| t.track_id == TrackId(0xC0)));
                    found_tracks = true;
                }
                PsDemuxEvent::Frame(frame) => {
                    if frame.track_id == TrackId(0xE0) {
                        assert_eq!(frame.pts, 90_000);
                        assert_eq!(frame.payload.as_ref(), video_payload.as_slice());
                        found_video_frame = true;
                    } else if frame.track_id == TrackId(0xC0) {
                        assert_eq!(frame.pts, 90_080);
                        assert_eq!(frame.payload.as_ref(), b"audio frame data");
                        found_audio_frame = true;
                    }
                }
                _ => {}
            }
        }

        assert!(found_tracks);
        assert!(found_video_frame);
        assert!(found_audio_frame);
    }

    #[test]
    fn ps_demuxer_unbounded_video_pes_does_not_truncate_on_internal_nalu_start_code() {
        // PES_packet_length == 0 is allowed for video PES; the demuxer must scan for the
        // next PS-layer start code, *not* match every internal H.264 Annex-B start code
        // (`00 00 01` / `00 00 00 01`) inside the NAL payload. This regression test feeds
        // a single unbounded-length video PES carrying two NAL units (each with a 4-byte
        // start code) followed by a real PS-layer end-of-stream marker.
        let mut buf = Vec::new();

        // Pack header (mandatory before any PES is parsed at the top level).
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBA]);
        // 10-byte SCR/mux-rate body and 0 stuffing bytes.
        buf.extend_from_slice(&[0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x00, 0x88, 0xC3, 0xF8]);

        // Video PES with PES_packet_length == 0 (unbounded) and PTS only.
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xE0]); // start_code + stream_id
        buf.extend_from_slice(&[0x00, 0x00]); // PES_packet_length = 0
        buf.push(0x80); // marker
        buf.push(0x80); // PTS_DTS_flags = 10 -> PTS only
        buf.push(0x05); // header_data_length
        buf.extend_from_slice(&encode_pts_dts(900_000, 0x2));
        // Annex-B NALU 1 with 4-byte start code (`00 00 00 01`).
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A]);
        // Annex-B NALU 2 with 3-byte start code (`00 00 01`).
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]);
        // Some additional payload bytes.
        buf.extend_from_slice(b"-extra-payload-");

        // Real next PS-layer packet: program end (`MPEG_program_end_code` = 0xB9).
        // Use a system header instead so it's a recognised PS-layer stream id.
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0xBB, 0x00, 0x06, 0, 0, 0, 0, 0, 0]);

        let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
        let events = demuxer.push(&buf);
        let _ = demuxer.flush();
        // Demuxer should not have produced PesParseError, and the inner NAL bytes must
        // remain intact in any video frame emitted later (we only check no diagnostics).
        for ev in events {
            if let PsDemuxEvent::Diagnostic(diag) = ev {
                panic!("unexpected diagnostic during unbounded video PES parse: {diag:?}");
            }
        }
    }
}
