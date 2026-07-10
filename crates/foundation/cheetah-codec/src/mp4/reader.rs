//! Sans-I/O classic MP4 reader.
//!
//! 无 I/O 经典 MP4 读取器。
//!
//! The reader is driven by the runtime layer through a request/response
//! pattern: it asks for byte ranges via `Mp4ReadRequest`, the runtime fulfils
//! them via `feed_bytes`, and the reader then emits parsed track info,
//! sample frames, and EOF events through `step`.
//!
//! Bounded memory: the reader never asks for more than `max_box_bytes` at a
//! time, and it never linearly scans more than `max_top_level_scan` bytes
//! when looking for the `moov` box at the end of the file.

use crate::prelude::*;
use bytes::Bytes;

use crate::frame::{AVFrame, FrameFlags, FrameFormat};
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};

use super::box_parser::{
    read_box_header, read_u16, read_u32, read_u64, BoxIter, ChildBox, DEFAULT_MAX_BOX_SIZE,
};
use super::sample_entry::{codec_id_from_sample_entry, extradata_from_sample_entry};
use super::sample_table::{SampleIndex, SampleTable};
use super::Mp4Error;

/// Configuration for the MP4 reader.
///
/// MP4 读取器配置。
#[derive(Debug, Clone)]
pub struct Mp4ReaderConfig {
    pub max_box_bytes: u64,
    /// Maximum bytes the reader is allowed to scan past the file head when
    /// looking for `moov` (handles `moov`-at-the-end inputs without a full
    /// linear scan). 0 disables the search.
    pub max_top_level_scan: u64,
}

impl Default for Mp4ReaderConfig {
    fn default() -> Self {
        Self {
            max_box_bytes: DEFAULT_MAX_BOX_SIZE,
            max_top_level_scan: 8 * 1024 * 1024,
        }
    }
}

/// Reader output event.
///
/// 读取器输出事件。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Mp4ReadEvent {
    /// File header parsed; reader can advertise its track list and seek index.
    Tracks(Vec<TrackInfo>),
    /// One sample emitted as a canonical media frame.
    Frame(AVFrame),
    /// Reader is waiting for more bytes from the runtime.
    NeedBytes(Mp4ReadRequest),
    /// Reader has emitted all available frames given current position.
    Idle,
    /// End of stream reached.
    Eof,
    /// Non-fatal diagnostic.
    Diagnostic(Mp4ReadDiagnostic),
}

/// Non-fatal diagnostic emitted while reading an MP4 file.
///
/// 读取 MP4 文件时发出的非致命诊断。
#[derive(Debug, Clone)]
pub enum Mp4ReadDiagnostic {
    UnknownBoxSkipped {
        fourcc: String,
        size: u64,
    },
    OversizeBoxSkipped {
        fourcc: String,
        size: u64,
    },
    SampleOutOfBounds {
        track_id: u32,
        offset: u64,
        size: u32,
    },
    MissingStss {
        track_id: u32,
    },
}

/// Request for a byte range that the runtime must provide.
///
/// 运行时必须提供的字节范围请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4ReadRequest {
    /// Absolute file offset of the requested byte range.
    pub offset: u64,
    /// Number of bytes the reader needs.
    pub length: u64,
}

/// Fulfilled read request returned by the runtime.
///
/// 运行时返回的已满足读取请求。
#[derive(Debug, Clone)]
pub struct Mp4ReadResult {
    pub offset: u64,
    pub data: Bytes,
}

/// Sans-I/O classic MP4 reader.
///
/// 无 I/O 经典 MP4 读取器。
pub struct Mp4Reader {
    config: Mp4ReaderConfig,
    state: ReaderState,
    /// File total length (set via `set_file_size`).
    file_size: u64,
    /// Buffered fulfilled reads, keyed by absolute offset.
    pending_reads: Vec<Mp4ReadResult>,
    /// Outstanding request waiting for runtime to fulfil.
    outstanding: Option<Mp4ReadRequest>,
    tracks: Vec<TrackInfo>,
    indices: Vec<SampleIndex>,
    /// Per-track playback cursor (next sample to emit).
    cursors: Vec<usize>,
    /// Optional seek target (dts in microseconds, applied at next `step`).
    seek_request_us: Option<i64>,
    /// True after the reader has emitted `Tracks` once.
    tracks_emitted: bool,
}

#[derive(Debug, Clone)]
enum ReaderState {
    /// Need to read the file head to discover top-level boxes.
    NeedHead,
    /// Scanning the tail backwards to find a moov box.
    TailScan { offset: u64, remaining: u64 },
    /// moov found and parsed; reader emits frames.
    Streaming,
    /// Fatal error.
    Failed(Mp4Error),
}

impl Mp4Reader {
    /// Create a new reader in the `NeedHead` state.
    ///
    /// 创建处于 `NeedHead` 状态的新读取器。
    pub fn new(config: Mp4ReaderConfig) -> Self {
        Self {
            config,
            state: ReaderState::NeedHead,
            file_size: 0,
            pending_reads: Vec::new(),
            outstanding: None,
            tracks: Vec::new(),
            indices: Vec::new(),
            cursors: Vec::new(),
            seek_request_us: None,
            tracks_emitted: false,
        }
    }

    /// Set the total file size so the reader can compute tail-scan ranges.
    ///
    /// 设置文件总大小，以便读取器计算尾部扫描范围。
    pub fn set_file_size(&mut self, file_size: u64) {
        self.file_size = file_size;
    }

    /// Provide bytes the reader previously requested via `Mp4ReadEvent::NeedBytes`.
    ///
    /// 提供读取器之前通过 `Mp4ReadEvent::NeedBytes` 请求的字节。
    pub fn feed_bytes(&mut self, result: Mp4ReadResult) {
        self.pending_reads.push(result);
        if let Some(out) = &self.outstanding {
            if self.find_buffered(out.offset, out.length).is_some() {
                self.outstanding = None;
            }
        }
        // Bound memory: discard old per-sample buffers once we are past
        // moov parsing and the streaming loop is consuming frames.
        if matches!(self.state, ReaderState::Streaming) {
            self.evict_old_pending();
        }
    }

    /// Get the parsed track list.
    ///
    /// 获取已解析的轨道列表。
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Get the per-track sample indices.
    ///
    /// 获取每个轨道的样本索引。
    pub fn indices(&self) -> &[SampleIndex] {
        &self.indices
    }

    /// Total duration across all tracks in microseconds.
    ///
    /// 所有轨道中的最大时长（微秒）。
    pub fn duration_us(&self) -> i64 {
        self.indices
            .iter()
            .map(|i| i.duration_us())
            .max()
            .unwrap_or(0)
    }

    /// Request a logical seek to the given microsecond timestamp. The seek is
    /// applied at the next `step()` call and will rewind every track's
    /// cursor to the nearest sync sample.
    ///
    /// 请求按给定微秒时间戳逻辑定位。定位在下次 `step()` 调用时生效，
    /// 并会将每个轨道的光标回退到最近的同步样本。
    pub fn seek(&mut self, position_us: i64) {
        self.seek_request_us = Some(position_us.max(0));
    }

    /// Advance the reader. Returns the next available event.
    ///
    /// 推进读取器。返回下一个可用事件。
    pub fn step(&mut self) -> Mp4ReadEvent {
        if let Some(ev) = self.handle_seek_request() {
            return ev;
        }
        loop {
            match &self.state {
                ReaderState::NeedHead => match self.try_parse_head() {
                    Some(ev) => return ev,
                    None => continue,
                },
                ReaderState::TailScan { .. } => {
                    if let Some(ev) = self.try_parse_tail() {
                        return ev;
                    }
                }
                ReaderState::Streaming => {
                    if !self.tracks_emitted {
                        self.tracks_emitted = true;
                        return Mp4ReadEvent::Tracks(self.tracks.clone());
                    }
                    return self.step_streaming();
                }
                ReaderState::Failed(e) => {
                    let cloned = e.clone();
                    return Mp4ReadEvent::Diagnostic(Mp4ReadDiagnostic::UnknownBoxSkipped {
                        fourcc: format!("err:{cloned}"),
                        size: 0,
                    });
                }
            }
        }
    }

    fn handle_seek_request(&mut self) -> Option<Mp4ReadEvent> {
        let target = self.seek_request_us.take()?;
        if !matches!(self.state, ReaderState::Streaming) {
            // Defer seek until tracks are loaded.
            self.seek_request_us = Some(target);
            return None;
        }
        for (i, idx) in self.indices.iter().enumerate() {
            let target_ticks = us_to_ticks(target, idx.timescale);
            self.cursors[i] = idx.seek_to_dts(target_ticks).unwrap_or(0);
        }
        None
    }

    fn try_parse_head(&mut self) -> Option<Mp4ReadEvent> {
        let req = Mp4ReadRequest {
            offset: 0,
            length: self.config.max_top_level_scan.min(self.file_size),
        };
        let Some(slice) = self.find_buffered(req.offset, req.length) else {
            return Some(self.queue_request(req));
        };
        let mut cursor = 0u64;
        let mut found_moov_offset: Option<u64> = None;
        while (cursor as usize) < slice.len() {
            let header = match read_box_header(
                slice,
                cursor as usize,
                slice.len(),
                self.config.max_box_bytes,
            ) {
                Ok(h) => h,
                Err(_) => break,
            };
            if &header.fourcc == b"moov" {
                found_moov_offset = Some(cursor);
                break;
            }
            if &header.fourcc == b"mdat" && header.payload_size() > 0 {
                // moov is likely after mdat — switch to tail scan
                break;
            }
            cursor = cursor.saturating_add(header.size);
        }
        if let Some(off) = found_moov_offset {
            // Parse moov from buffered slice
            let header =
                read_box_header(slice, off as usize, slice.len(), self.config.max_box_bytes)
                    .ok()?;
            let payload_start = off as usize + header.header_size as usize;
            let payload_end = (off + header.size) as usize;
            if payload_end > slice.len() {
                // Need more bytes
                return Some(self.queue_request(Mp4ReadRequest {
                    offset: off,
                    length: header.size,
                }));
            }
            let payload = &slice[payload_start..payload_end];
            let payload_owned = payload.to_vec();
            if let Err(e) = self.parse_moov(&payload_owned) {
                self.state = ReaderState::Failed(e);
                return Some(Mp4ReadEvent::Eof);
            }
            self.state = ReaderState::Streaming;
            return None;
        }
        // moov not in head — switch to tail scan
        let scan_limit = self.config.max_top_level_scan.min(self.file_size);
        let start = self.file_size.saturating_sub(scan_limit);
        self.state = ReaderState::TailScan {
            offset: start,
            remaining: scan_limit,
        };
        None
    }

    fn try_parse_tail(&mut self) -> Option<Mp4ReadEvent> {
        let ReaderState::TailScan { offset, remaining } = self.state else {
            return None;
        };
        let req = Mp4ReadRequest {
            offset,
            length: remaining,
        };
        let Some(slice) = self.find_buffered(req.offset, req.length) else {
            return Some(self.queue_request(req));
        };
        // Walk forward looking for moov.
        let mut cursor = 0u64;
        while (cursor as usize) < slice.len() {
            let header = match read_box_header(
                slice,
                cursor as usize,
                slice.len(),
                self.config.max_box_bytes,
            ) {
                Ok(h) => h,
                Err(_) => break,
            };
            if &header.fourcc == b"moov" {
                let payload_start = cursor as usize + header.header_size as usize;
                let payload_end = (cursor + header.size) as usize;
                if payload_end > slice.len() {
                    return Some(self.queue_request(Mp4ReadRequest {
                        offset: offset + cursor,
                        length: header.size,
                    }));
                }
                let payload = &slice[payload_start..payload_end];
                let payload_owned = payload.to_vec();
                if let Err(e) = self.parse_moov(&payload_owned) {
                    self.state = ReaderState::Failed(e);
                    return Some(Mp4ReadEvent::Eof);
                }
                self.state = ReaderState::Streaming;
                return None;
            }
            cursor = cursor.saturating_add(header.size);
        }
        self.state = ReaderState::Failed(Mp4Error::MissingBox("moov"));
        Some(Mp4ReadEvent::Eof)
    }

    fn step_streaming(&mut self) -> Mp4ReadEvent {
        // Pick the track with the smallest next-sample dts (in microseconds).
        let mut best: Option<(usize, i64)> = None;
        for (i, (idx, cursor)) in self.indices.iter().zip(self.cursors.iter()).enumerate() {
            if *cursor >= idx.samples.len() {
                continue;
            }
            let entry = idx.samples[*cursor];
            let dts_us = ticks_to_us(entry.dts, idx.timescale);
            match best {
                None => best = Some((i, dts_us)),
                Some((_, b)) if dts_us < b => best = Some((i, dts_us)),
                _ => {}
            }
        }
        let Some((track_idx, _)) = best else {
            return Mp4ReadEvent::Eof;
        };
        let track = self.tracks[track_idx].clone();
        let idx = &self.indices[track_idx];
        let entry = idx.samples[self.cursors[track_idx]];
        let req = Mp4ReadRequest {
            offset: entry.offset,
            length: entry.size as u64,
        };
        let Some(buf) = self.find_buffered(req.offset, req.length) else {
            return self.queue_request(req);
        };
        let payload = Bytes::copy_from_slice(&buf[..entry.size as usize]);

        let dts_us = ticks_to_us(entry.dts, idx.timescale);
        let pts_us = ticks_to_us(entry.pts(), idx.timescale);
        let timebase = Timebase::new(1, idx.timescale.max(1));
        let mut frame = AVFrame::new(
            track.track_id,
            track.media_kind,
            track.codec,
            frame_format_for_codec(track.codec),
            us_to_ticks(pts_us, idx.timescale),
            us_to_ticks(dts_us, idx.timescale),
            timebase,
            payload,
        );
        if entry.is_sync {
            frame.flags.insert(FrameFlags::KEY);
        }
        let dur_ticks = entry.duration as i64;
        let _ = frame.set_duration(dur_ticks);

        self.cursors[track_idx] += 1;
        Mp4ReadEvent::Frame(frame)
    }

    fn queue_request(&mut self, req: Mp4ReadRequest) -> Mp4ReadEvent {
        // Cap request to file boundary.
        let length = req.length.min(self.file_size.saturating_sub(req.offset));
        let req = Mp4ReadRequest {
            offset: req.offset,
            length,
        };
        if length == 0 {
            return Mp4ReadEvent::Eof;
        }
        if Some(req) == self.outstanding {
            return Mp4ReadEvent::NeedBytes(req);
        }
        self.outstanding = Some(req);
        Mp4ReadEvent::NeedBytes(req)
    }

    fn find_buffered(&self, offset: u64, length: u64) -> Option<&[u8]> {
        for r in &self.pending_reads {
            if r.offset <= offset && r.offset + r.data.len() as u64 >= offset + length {
                let start = (offset - r.offset) as usize;
                let end = start + length as usize;
                return Some(&r.data[start..end]);
            }
        }
        None
    }

    /// Cap the per-frame buffer so memory does not grow unboundedly during
    /// playback of a long file. `Streaming` mode keeps at most
    /// `MAX_PENDING_READS` recent results plus the moov region (which is
    /// re-used as the anchor for parsed track info). The first entry is
    /// preserved because it covers the moov box; we only evict mid-buffer
    /// entries from the front.
    fn evict_old_pending(&mut self) {
        const MAX_PENDING_READS: usize = 8;
        if self.pending_reads.len() <= MAX_PENDING_READS {
            return;
        }
        let drain = self.pending_reads.len() - MAX_PENDING_READS;
        // Always keep the first slice (moov region) and the most recent
        // `MAX_PENDING_READS - 1` reads. Drain entries from index 1.
        let end = (1 + drain).min(self.pending_reads.len() - 1);
        self.pending_reads.drain(1..end);
    }

    fn parse_moov(&mut self, payload: &[u8]) -> Result<(), Mp4Error> {
        let iter = BoxIter::new(payload, 0, payload.len(), self.config.max_box_bytes);
        let mut next_track_id: u32 = 1;
        for child in iter {
            let child = child?;
            if &child.header.fourcc == b"trak" {
                let (track, index) = parse_trak(&child, next_track_id, self.config.max_box_bytes)?;
                self.tracks.push(track);
                self.indices.push(index);
                self.cursors.push(0);
                next_track_id += 1;
            }
        }
        if self.tracks.is_empty() {
            return Err(Mp4Error::MissingBox("trak"));
        }
        Ok(())
    }
}

fn parse_trak(
    child: &ChildBox<'_>,
    track_id_default: u32,
    max_box_size: u64,
) -> Result<(TrackInfo, SampleIndex), Mp4Error> {
    let mut track_id_box: Option<u32> = None;
    let mut timescale: u32 = 90_000;
    let mut media_kind = MediaKind::Data;
    let mut sample_table: Option<SampleTable> = None;
    let mut codec = CodecId::Unknown;
    let mut extradata = crate::track::CodecExtradata::None;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut sample_rate: Option<u32> = None;
    let mut channels: Option<u8> = None;

    let iter = BoxIter::new(child.payload, 0, child.payload.len(), max_box_size);
    for c in iter {
        let c = c?;
        match &c.header.fourcc {
            b"tkhd" => {
                if c.payload.len() >= 24 {
                    let version = c.payload[0];
                    let id_offset = if version == 1 { 4 + 16 } else { 4 + 8 };
                    if c.payload.len() >= id_offset + 4 {
                        track_id_box = Some(read_u32(c.payload, id_offset)?);
                    }
                    // width/height are at the end (last 8 bytes of the box)
                    if c.payload.len() >= 8 {
                        let wh_off = c.payload.len() - 8;
                        let w = read_u32(c.payload, wh_off)?;
                        let h = read_u32(c.payload, wh_off + 4)?;
                        width = w >> 16;
                        height = h >> 16;
                    }
                }
            }
            b"mdia" => {
                let inner = BoxIter::new(c.payload, 0, c.payload.len(), max_box_size);
                for ic in inner {
                    let ic = ic?;
                    match &ic.header.fourcc {
                        b"mdhd" => {
                            if ic.payload.len() >= 24 {
                                let version = ic.payload[0];
                                if version == 1 {
                                    if ic.payload.len() >= 28 {
                                        timescale = read_u32(ic.payload, 20)?;
                                    }
                                } else if ic.payload.len() >= 16 {
                                    timescale = read_u32(ic.payload, 12)?;
                                }
                            }
                        }
                        b"hdlr" => {
                            if ic.payload.len() >= 12 {
                                let mut handler = [0u8; 4];
                                handler.copy_from_slice(&ic.payload[8..12]);
                                media_kind = match &handler {
                                    b"vide" => MediaKind::Video,
                                    b"soun" => MediaKind::Audio,
                                    _ => MediaKind::Data,
                                };
                            }
                        }
                        b"minf" => {
                            let inner2 =
                                BoxIter::new(ic.payload, 0, ic.payload.len(), max_box_size);
                            for mc in inner2 {
                                let mc = mc?;
                                if &mc.header.fourcc == b"stbl" {
                                    let st = parse_stbl(mc.payload, max_box_size)?;
                                    sample_table = Some(st.0);
                                    if let Some((c_id, c_ed, sr, ch)) = st.1 {
                                        codec = c_id;
                                        extradata = c_ed;
                                        sample_rate = sr;
                                        channels = ch;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let track_id = TrackId(track_id_box.unwrap_or(track_id_default));
    let mut info = TrackInfo::new(track_id, media_kind, codec, timescale.max(1));
    info.width = if width > 0 { Some(width) } else { None };
    info.height = if height > 0 { Some(height) } else { None };
    info.sample_rate = sample_rate;
    info.channels = channels;
    info.extradata = extradata;
    info.refresh_readiness();
    let st = sample_table.ok_or(Mp4Error::MissingBox("stbl"))?;
    let mut idx = st.build_index(timescale.max(1))?;
    idx.track_id = track_id;
    Ok((info, idx))
}

type StsdInfo = Option<(
    CodecId,
    crate::track::CodecExtradata,
    Option<u32>,
    Option<u8>,
)>;

fn parse_stbl(payload: &[u8], max_box_size: u64) -> Result<(SampleTable, StsdInfo), Mp4Error> {
    let mut st = SampleTable::default();
    let mut info: StsdInfo = None;
    let iter = BoxIter::new(payload, 0, payload.len(), max_box_size);
    for c in iter {
        let c = c?;
        match &c.header.fourcc {
            b"stsd" => info = parse_stsd(c.payload, max_box_size)?,
            b"stts" => st.stts = parse_stts(c.payload)?,
            b"ctts" => st.ctts = parse_ctts(c.payload)?,
            b"stss" => st.stss = Some(parse_stss(c.payload)?),
            b"stsc" => st.stsc = parse_stsc(c.payload)?,
            b"stsz" => {
                let (def, sizes) = parse_stsz(c.payload)?;
                st.stsz_default = def;
                st.stsz_sizes = sizes;
            }
            b"stco" => st.stco = parse_stco(c.payload)?,
            b"co64" => st.stco = parse_co64(c.payload)?,
            _ => {}
        }
    }
    Ok((st, info))
}

fn parse_stsd(payload: &[u8], max_box_size: u64) -> Result<StsdInfo, Mp4Error> {
    if payload.len() < 8 {
        return Ok(None);
    }
    // skip 4 (version+flags) + 4 (entry_count)
    let iter = BoxIter::new(payload, 8, payload.len(), max_box_size);
    // We only care about the first sample entry per stsd; classic MP4 tracks
    // have a single description and the iterator always yields at most one
    // hit. Allow `clippy::never_loop` since the early return is intentional.
    #[allow(clippy::never_loop)]
    for c in iter {
        let c = c?;
        let codec = codec_id_from_sample_entry(&c.header.fourcc);
        // Determine if this is a video or audio entry by the sample entry layout
        let inner_offset = match codec {
            CodecId::H264
            | CodecId::H265
            | CodecId::H266
            | CodecId::VP8
            | CodecId::VP9
            | CodecId::AV1
            | CodecId::MJPEG => 78,
            CodecId::AAC | CodecId::G711A | CodecId::G711U | CodecId::Opus | CodecId::MP3 => 28,
            _ => 0,
        };
        let mut sample_rate: Option<u32> = None;
        let mut channels: Option<u8> = None;
        if matches!(
            codec,
            CodecId::AAC | CodecId::G711A | CodecId::G711U | CodecId::Opus | CodecId::MP3
        ) && c.payload.len() >= 28
        {
            // Audio sample entry layout: 6 reserved, 2 dref_idx, 8 reserved,
            // 2 channels, 2 sample_size, 4 reserved, 4 sample_rate (16.16)
            channels = Some(read_u16(c.payload, 16)? as u8);
            sample_rate = Some(read_u32(c.payload, 24)? >> 16);
        }
        // child config box (avcC, hvcC, esds, vpcC, av1C, dOps)
        let mut extradata = crate::track::CodecExtradata::None;
        if inner_offset > 0 && c.payload.len() > inner_offset {
            let inner = BoxIter::new(c.payload, inner_offset, c.payload.len(), max_box_size);
            for ic in inner {
                let Ok(ic) = ic else { break };
                if matches!(
                    &ic.header.fourcc,
                    b"avcC" | b"hvcC" | b"vvcC" | b"vpcC" | b"av1C" | b"esds" | b"dOps"
                ) {
                    extradata =
                        extradata_from_sample_entry(codec, Some(ic.header.fourcc), ic.payload);
                    break;
                }
            }
        }
        return Ok(Some((codec, extradata, sample_rate, channels)));
    }
    Ok(None)
}

fn parse_stts(payload: &[u8]) -> Result<Vec<(u32, u32)>, Mp4Error> {
    if payload.len() < 8 {
        return Err(Mp4Error::InvalidSampleTable("stts truncated"));
    }
    let count = read_u32(payload, 4)? as usize;
    let mut runs = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 8;
        if off + 8 > payload.len() {
            break;
        }
        let n = read_u32(payload, off)?;
        let d = read_u32(payload, off + 4)?;
        runs.push((n, d));
    }
    Ok(runs)
}

fn parse_ctts(payload: &[u8]) -> Result<Vec<(u32, i32)>, Mp4Error> {
    if payload.len() < 8 {
        return Ok(Vec::new());
    }
    let version = payload[0];
    let count = read_u32(payload, 4)? as usize;
    let mut runs = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 8;
        if off + 8 > payload.len() {
            break;
        }
        let n = read_u32(payload, off)?;
        let raw = read_u32(payload, off + 4)?;
        // ISO/IEC 14496-12 box layout:
        //   version=0 → unsigned int(32) sample_offset
        //   version=1 → signed int(32) sample_offset
        // For v0 we clamp to i32::MAX to avoid wrap-around; for v1 we
        // bit-cast preserving the sign bit.
        let v = if version == 1 {
            raw as i32
        } else {
            super::compat::clamp_composition_offset(raw as i64)
        };
        runs.push((n, v));
    }
    Ok(runs)
}

fn parse_stss(payload: &[u8]) -> Result<Vec<u32>, Mp4Error> {
    if payload.len() < 8 {
        return Ok(Vec::new());
    }
    let count = read_u32(payload, 4)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 4;
        if off + 4 > payload.len() {
            break;
        }
        entries.push(read_u32(payload, off)?);
    }
    Ok(entries)
}

fn parse_stsc(payload: &[u8]) -> Result<Vec<(u32, u32, u32)>, Mp4Error> {
    if payload.len() < 8 {
        return Err(Mp4Error::InvalidSampleTable("stsc truncated"));
    }
    let count = read_u32(payload, 4)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 12;
        if off + 12 > payload.len() {
            break;
        }
        entries.push((
            read_u32(payload, off)?,
            read_u32(payload, off + 4)?,
            read_u32(payload, off + 8)?,
        ));
    }
    Ok(entries)
}

fn parse_stsz(payload: &[u8]) -> Result<(u32, Vec<u32>), Mp4Error> {
    if payload.len() < 12 {
        return Err(Mp4Error::InvalidSampleTable("stsz truncated"));
    }
    let default = read_u32(payload, 4)?;
    let count = read_u32(payload, 8)? as usize;
    if default != 0 {
        return Ok((default, Vec::new()));
    }
    let mut sizes = Vec::with_capacity(count);
    for i in 0..count {
        let off = 12 + i * 4;
        if off + 4 > payload.len() {
            break;
        }
        sizes.push(read_u32(payload, off)?);
    }
    Ok((default, sizes))
}

fn parse_stco(payload: &[u8]) -> Result<Vec<u64>, Mp4Error> {
    if payload.len() < 8 {
        return Err(Mp4Error::InvalidSampleTable("stco truncated"));
    }
    let count = read_u32(payload, 4)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 4;
        if off + 4 > payload.len() {
            break;
        }
        entries.push(read_u32(payload, off)? as u64);
    }
    Ok(entries)
}

fn parse_co64(payload: &[u8]) -> Result<Vec<u64>, Mp4Error> {
    if payload.len() < 8 {
        return Err(Mp4Error::InvalidSampleTable("co64 truncated"));
    }
    let count = read_u32(payload, 4)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = 8 + i * 8;
        if off + 8 > payload.len() {
            break;
        }
        entries.push(read_u64(payload, off)?);
    }
    Ok(entries)
}

fn frame_format_for_codec(codec: CodecId) -> FrameFormat {
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
        CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
        CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
        CodecId::MJPEG => FrameFormat::MjpegFrame,
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::MP2 => FrameFormat::Mp2Frame,
        _ => FrameFormat::Unknown,
    }
}

fn ticks_to_us(ticks: i64, timescale: u32) -> i64 {
    if timescale == 0 {
        return ticks;
    }
    let v = (ticks as i128) * 1_000_000 / (timescale as i128);
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn us_to_ticks(us: i64, timescale: u32) -> i64 {
    if timescale == 0 {
        return us;
    }
    let v = (us as i128) * (timescale as i128) / 1_000_000;
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mp4::writer::{Mp4WriteEvent, Mp4Writer, Mp4WriterConfig};
    use crate::track::{CodecExtradata, TrackId};

    fn h264_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        t.width = Some(1280);
        t.height = Some(720);
        t.extradata = CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        t
    }

    #[test]
    fn writer_then_reader_roundtrip() {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        let p1 = b"FAKE_AU_1";
        let p2 = b"FAKE_AU_2";
        let p3 = b"FAKE_AU_3";
        w.push_sample(1, 0, 0, true, p1).unwrap();
        w.push_sample(1, 33_333, 33_333, false, p2).unwrap();
        w.push_sample(1, 66_667, 66_667, false, p3).unwrap();
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        let total = buf.len() as u64;

        let mut reader = Mp4Reader::new(Mp4ReaderConfig::default());
        reader.set_file_size(total);
        let mut frames = 0;
        let mut got_tracks = false;
        loop {
            match reader.step() {
                Mp4ReadEvent::NeedBytes(req) => {
                    let end = (req.offset + req.length) as usize;
                    let data = Bytes::copy_from_slice(&buf[req.offset as usize..end]);
                    reader.feed_bytes(Mp4ReadResult {
                        offset: req.offset,
                        data,
                    });
                }
                Mp4ReadEvent::Tracks(tracks) => {
                    assert_eq!(tracks.len(), 1);
                    assert_eq!(tracks[0].codec, CodecId::H264);
                    got_tracks = true;
                }
                Mp4ReadEvent::Frame(frame) => {
                    frames += 1;
                    assert_eq!(frame.codec, CodecId::H264);
                    assert!(!frame.payload.is_empty());
                }
                Mp4ReadEvent::Eof => break,
                Mp4ReadEvent::Idle => break,
                Mp4ReadEvent::Diagnostic(_) => {}
            }
        }
        assert!(got_tracks);
        assert_eq!(frames, 3);
    }
}
