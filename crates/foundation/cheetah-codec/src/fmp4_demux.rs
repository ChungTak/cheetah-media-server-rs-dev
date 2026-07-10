//! Shared fMP4 demuxer for ISO BMFF fragmented MP4 parsing.
//!
//! Supports streaming input (arbitrary chunk sizes), box reassembly,
//! 32-bit/64-bit box sizes, unknown box skip, and all codec sample entries.

use crate::prelude::*;
use bytes::{Bytes, BytesMut};

use crate::track::{CodecId, MediaKind};

/// Configuration for the fMP4 demuxer.
#[derive(Debug, Clone)]
pub struct Fmp4DemuxerConfig {
    pub max_box_bytes: usize,
}

impl Default for Fmp4DemuxerConfig {
    fn default() -> Self {
        Self {
            max_box_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Track info extracted from init segment.
#[derive(Debug, Clone)]
pub struct Fmp4DemuxTrack {
    pub track_id: u32,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub timescale: u32,
    pub extradata: Bytes,
}

/// Events produced by the demuxer.
#[derive(Debug, Clone)]
pub enum Fmp4DemuxEvent {
    TrackInfo(Vec<Fmp4DemuxTrack>),
    Frame {
        track_id: u32,
        media_kind: MediaKind,
        codec: CodecId,
        pts_us: i64,
        dts_us: i64,
        keyframe: bool,
        data: Bytes,
    },
    Diagnostic(Fmp4DemuxDiagnostic),
}

/// Diagnostic messages from the demuxer.
#[derive(Debug, Clone)]
pub enum Fmp4DemuxDiagnostic {
    MalformedBox {
        box_type: [u8; 4],
        size: u64,
        header_size: usize,
    },
    BoxTooLarge {
        box_type: [u8; 4],
        size: u64,
    },
    UnknownBox {
        box_type: [u8; 4],
    },
    MdatOverflow {
        track_id: u32,
    },
    TrackIdNotFound {
        track_id: u32,
    },
    RepeatedInit,
}

/// Streaming fMP4 demuxer with box reassembly.
pub struct Fmp4Demuxer {
    config: Fmp4DemuxerConfig,
    buffer: BytesMut,
    tracks: Vec<Fmp4DemuxTrack>,
    init_parsed: bool,
    pending_moof: Option<Bytes>,
    /// Bytes remaining to skip for an oversized box.
    skip_remaining: u64,
}

impl Fmp4Demuxer {
    pub fn new(config: Fmp4DemuxerConfig) -> Self {
        Self {
            config,
            buffer: BytesMut::new(),
            tracks: Vec::new(),
            init_parsed: false,
            pending_moof: None,
            skip_remaining: 0,
        }
    }

    /// Push bytes into the demuxer. Returns events for any complete boxes parsed.
    pub fn push(&mut self, data: &[u8]) -> Vec<Fmp4DemuxEvent> {
        self.buffer.extend_from_slice(data);
        let mut events = Vec::new();

        // Skip remaining bytes from an oversized box
        if self.skip_remaining > 0 {
            let skip = (self.skip_remaining as usize).min(self.buffer.len());
            let _ = self.buffer.split_to(skip);
            self.skip_remaining -= skip as u64;
            if self.skip_remaining > 0 {
                return events;
            }
        }

        loop {
            let buf = &self.buffer[..];
            if buf.len() < 8 {
                break;
            }
            let (box_size, header_size) = read_box_header(buf);
            if box_size == 0 {
                // size 0 means extends to end - we can't know the end in streaming, skip
                break;
            }
            if header_size > buf.len() {
                break;
            }
            if box_size < header_size as u64 {
                let box_type = [buf[4], buf[5], buf[6], buf[7]];
                events.push(Fmp4DemuxEvent::Diagnostic(
                    Fmp4DemuxDiagnostic::MalformedBox {
                        box_type,
                        size: box_size,
                        header_size,
                    },
                ));
                let _ = self.buffer.split_to(header_size);
                continue;
            }
            if box_size > self.config.max_box_bytes as u64 {
                let box_type = [buf[4], buf[5], buf[6], buf[7]];
                events.push(Fmp4DemuxEvent::Diagnostic(
                    Fmp4DemuxDiagnostic::BoxTooLarge {
                        box_type,
                        size: box_size,
                    },
                ));
                // Skip this entire box - may span multiple push() calls
                let available = self.buffer.len() as u64;
                if box_size <= available {
                    let _ = self.buffer.split_to(box_size as usize);
                } else {
                    self.skip_remaining = box_size - available;
                    self.buffer.clear();
                }
                continue;
            }
            let box_size = box_size as usize;
            if buf.len() < box_size {
                break; // Need more data
            }
            let box_type = [buf[4], buf[5], buf[6], buf[7]];
            let box_data = self.buffer.split_to(box_size);
            let content = &box_data[header_size..];
            self.process_top_level_box(&box_type, content, &mut events);
        }
        events
    }

    /// Flush remaining buffer. Returns any final events.
    pub fn flush(&mut self) -> Vec<Fmp4DemuxEvent> {
        // In streaming mode, incomplete boxes are discarded
        self.buffer.clear();
        Vec::new()
    }

    /// Get current track list.
    pub fn tracks(&self) -> &[Fmp4DemuxTrack] {
        &self.tracks
    }

    fn process_top_level_box(
        &mut self,
        box_type: &[u8; 4],
        content: &[u8],
        events: &mut Vec<Fmp4DemuxEvent>,
    ) {
        match box_type {
            b"ftyp" | b"styp" | b"sidx" | b"free" | b"skip" => {} // Skip
            b"moov" => {
                if self.init_parsed {
                    events.push(Fmp4DemuxEvent::Diagnostic(
                        Fmp4DemuxDiagnostic::RepeatedInit,
                    ));
                    self.tracks.clear();
                }
                self.parse_moov(content);
                self.init_parsed = true;
                events.push(Fmp4DemuxEvent::TrackInfo(self.tracks.clone()));
            }
            b"moof" => {
                // Store moof for pairing with next mdat
                self.pending_moof = Some(Bytes::copy_from_slice(content));
            }
            b"mdat" => {
                if let Some(moof) = self.pending_moof.take() {
                    self.extract_frames(&moof, content, events);
                }
            }
            _ => {
                events.push(Fmp4DemuxEvent::Diagnostic(
                    Fmp4DemuxDiagnostic::UnknownBox {
                        box_type: *box_type,
                    },
                ));
            }
        }
    }

    fn parse_moov(&mut self, data: &[u8]) {
        for_each_child_box(data, |box_type, content| {
            if box_type == b"trak" {
                if let Some(track) = parse_trak(content) {
                    self.tracks.push(track);
                }
            }
        });
    }

    fn extract_frames(&self, moof: &[u8], mdat: &[u8], events: &mut Vec<Fmp4DemuxEvent>) {
        let moof_box_size = moof.len() + 8; // content + header
        for_each_child_box(moof, |box_type, content| {
            if box_type == b"traf" {
                self.parse_traf(content, mdat, moof_box_size, events);
            }
        });
    }

    fn parse_traf(
        &self,
        data: &[u8],
        mdat: &[u8],
        moof_box_size: usize,
        events: &mut Vec<Fmp4DemuxEvent>,
    ) {
        let mut track_id = 0u32;
        let mut base_decode_time: u64 = 0;
        let mut data_offset: i32 = 0;
        let mut samples: Vec<TrunSample> = Vec::new();

        for_each_child_box(data, |box_type, inner| match box_type {
            b"tfhd" if inner.len() >= 8 => {
                track_id = u32::from_be_bytes([inner[4], inner[5], inner[6], inner[7]]);
            }
            b"tfdt" if inner.len() >= 8 => {
                let version = inner[0];
                if version == 1 && inner.len() >= 12 {
                    base_decode_time = u64::from_be_bytes([
                        inner[4], inner[5], inner[6], inner[7], inner[8], inner[9], inner[10],
                        inner[11],
                    ]);
                } else {
                    base_decode_time =
                        u32::from_be_bytes([inner[4], inner[5], inner[6], inner[7]]) as u64;
                }
            }
            b"trun" if inner.len() >= 8 => {
                let (off, s) = parse_trun(inner);
                data_offset = off;
                samples = s;
            }
            _ => {}
        });

        let track = match self.tracks.iter().find(|t| t.track_id == track_id) {
            Some(t) => t,
            None => {
                events.push(Fmp4DemuxEvent::Diagnostic(
                    Fmp4DemuxDiagnostic::TrackIdNotFound { track_id },
                ));
                return;
            }
        };

        // data_offset is relative to moof box start
        let mdat_base = if data_offset > 0 {
            (data_offset as usize).saturating_sub(moof_box_size + 8)
        } else {
            0
        };

        let mut mdat_offset = mdat_base;
        let mut current_dts = base_decode_time;

        for sample in &samples {
            if mdat_offset + sample.size as usize > mdat.len() {
                events.push(Fmp4DemuxEvent::Diagnostic(
                    Fmp4DemuxDiagnostic::MdatOverflow { track_id },
                ));
                break;
            }
            let frame_data = &mdat[mdat_offset..mdat_offset + sample.size as usize];

            let dts_us = timescale_to_us(current_dts, track.timescale);
            let pts_ticks = current_dts as i64 + sample.cts_offset as i64;
            let pts_us = timescale_to_us(pts_ticks as u64, track.timescale);

            let keyframe = is_keyframe_flags(sample.flags);

            events.push(Fmp4DemuxEvent::Frame {
                track_id,
                media_kind: track.media_kind,
                codec: track.codec,
                pts_us,
                dts_us,
                keyframe,
                data: Bytes::copy_from_slice(frame_data),
            });

            mdat_offset += sample.size as usize;
            current_dts += sample.duration as u64;
        }
    }
}

// ─── Helper types and functions ───

struct TrunSample {
    duration: u32,
    size: u32,
    flags: u32,
    cts_offset: i32,
}

fn read_box_header(buf: &[u8]) -> (u64, usize) {
    if buf.len() < 8 {
        return (0, 0);
    }
    let size32 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if size32 == 1 {
        // 64-bit extended size
        if buf.len() < 16 {
            return (0, 0);
        }
        let size64 = u64::from_be_bytes([
            buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
        ]);
        (size64, 16)
    } else if size32 == 0 {
        (0, 8) // extends to end of file
    } else {
        (size32 as u64, 8)
    }
}

fn for_each_child_box(data: &[u8], mut f: impl FnMut(&[u8; 4], &[u8])) {
    let mut offset = 0;
    while offset + 8 <= data.len() {
        let (box_size, header_size) = read_box_header(&data[offset..]);
        if box_size < 8 || box_size == 0 {
            break;
        }
        let box_size = box_size as usize;
        if offset + box_size > data.len() {
            break;
        }
        let box_type: [u8; 4] = [
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ];
        let content = &data[offset + header_size..offset + box_size];
        f(&box_type, content);
        offset += box_size;
    }
}

fn parse_trak(data: &[u8]) -> Option<Fmp4DemuxTrack> {
    let mut track_id = 0u32;
    let mut timescale = 90_000u32;
    let mut handler_type = [0u8; 4];
    let mut codec = CodecId::Unknown;
    let mut extradata = Bytes::new();

    for_each_child_box(data, |box_type, content| {
        match box_type {
            b"tkhd" if content.len() >= 16 => {
                // version(1) + flags(3) + creation(4) + modification(4) + track_id(4)
                track_id = u32::from_be_bytes([
                    content[4 + 4 + 4],
                    content[4 + 4 + 5],
                    content[4 + 4 + 6],
                    content[4 + 4 + 7],
                ]);
            }
            b"mdia" => {
                parse_mdia(
                    content,
                    &mut timescale,
                    &mut handler_type,
                    &mut codec,
                    &mut extradata,
                );
            }
            _ => {}
        }
    });

    let media_kind = match &handler_type {
        b"vide" => MediaKind::Video,
        b"soun" => MediaKind::Audio,
        _ => return None,
    };

    Some(Fmp4DemuxTrack {
        track_id,
        codec,
        media_kind,
        timescale,
        extradata,
    })
}

fn parse_mdia(
    data: &[u8],
    timescale: &mut u32,
    handler: &mut [u8; 4],
    codec: &mut CodecId,
    extradata: &mut Bytes,
) {
    for_each_child_box(data, |box_type, content| match box_type {
        b"mdhd" if content.len() >= 16 => {
            *timescale = u32::from_be_bytes([
                content[4 + 4 + 4],
                content[4 + 4 + 5],
                content[4 + 4 + 6],
                content[4 + 4 + 7],
            ]);
        }
        b"hdlr" if content.len() >= 12 => {
            handler.copy_from_slice(&content[8..12]);
        }
        b"minf" => {
            parse_minf(content, codec, extradata);
        }
        _ => {}
    });
}

fn parse_minf(data: &[u8], codec: &mut CodecId, extradata: &mut Bytes) {
    for_each_child_box(data, |box_type, content| {
        if box_type == b"stbl" {
            parse_stbl(content, codec, extradata);
        }
    });
}

fn parse_stbl(data: &[u8], codec: &mut CodecId, extradata: &mut Bytes) {
    for_each_child_box(data, |box_type, content| {
        if box_type == b"stsd" && content.len() >= 8 {
            // version(1) + flags(3) + entry_count(4) + entries
            parse_stsd_entry(&content[8..], codec, extradata);
        }
    });
}

fn parse_stsd_entry(data: &[u8], codec: &mut CodecId, extradata: &mut Bytes) {
    if data.len() < 8 {
        return;
    }
    let entry_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let box_type = &data[4..8];

    *codec = match box_type {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => CodecId::H264,
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => CodecId::H265,
        b"vvc1" => CodecId::H266,
        b"vp08" => CodecId::VP8,
        b"vp09" => CodecId::VP9,
        b"av01" => CodecId::AV1,
        b"mp4v" | b"jpeg" | b"mjpa" | b"mjpb" => CodecId::MJPEG,
        b"mp4a" => CodecId::AAC, // Will be refined by esds object type
        b"Opus" => CodecId::Opus,
        b"alaw" => CodecId::G711A,
        b"ulaw" => CodecId::G711U,
        _ => CodecId::Unknown,
    };

    if entry_size <= 8 || entry_size > data.len() {
        return;
    }
    let entry_data = &data[8..entry_size];

    // Skip sample entry header to find config box
    let config_offset = match box_type {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" | b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" | b"vvc1"
        | b"vp08" | b"vp09" | b"av01" | b"mp4v" | b"jpeg" | b"mjpa" | b"mjpb" => 78, // video sample entry body
        b"mp4a" | b"Opus" | b"alaw" | b"ulaw" => 28, // audio sample entry body
        _ => return,
    };

    if entry_data.len() <= config_offset {
        return;
    }
    let config_area = &entry_data[config_offset..];

    let config_box_name: &[u8; 4] = match box_type {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => b"avcC",
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => b"hvcC",
        b"vvc1" => b"vvcC",
        b"vp08" | b"vp09" => b"vpcC",
        b"av01" => b"av1C",
        b"mp4a" => b"esds",
        b"Opus" => b"dOps",
        b"mp4v" | b"jpeg" | b"mjpa" | b"mjpb" => b"esds",
        _ => return,
    };

    if let Some(cfg) = find_child_box(config_area, config_box_name) {
        // For esds, refine codec from object type
        if config_box_name == b"esds" {
            if let Some((obj_type, decoder_specific)) = extract_esds_info(cfg) {
                match obj_type {
                    0x40 => *codec = CodecId::AAC,
                    0x69 | 0x6B => {
                        if box_type == b"mp4a" {
                            // 0x69 = MP3, 0x6B = MP2 (or MP3 compat)
                            *codec = if obj_type == 0x6B {
                                CodecId::MP2
                            } else {
                                CodecId::MP3
                            };
                        }
                    }
                    0x6C => *codec = CodecId::MJPEG,
                    _ => {}
                }
                if *codec == CodecId::AAC {
                    *extradata = decoder_specific
                        .map(Bytes::copy_from_slice)
                        .unwrap_or_default();
                    return;
                }
            }
        }
        *extradata = Bytes::copy_from_slice(cfg);
    }
}

fn find_child_box<'a>(data: &'a [u8], target: &[u8; 4]) -> Option<&'a [u8]> {
    let mut offset = 0;
    while offset + 8 <= data.len() {
        let size = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        if size < 8 || offset + size > data.len() {
            break;
        }
        if &data[offset + 4..offset + 8] == target {
            return Some(&data[offset + 8..offset + size]);
        }
        offset += size;
    }
    None
}

fn extract_esds_info(esds: &[u8]) -> Option<(u8, Option<&[u8]>)> {
    // esds content: version(1) + flags(3) + ES_Descriptor
    // ES_Descriptor: tag(1) + size(1+) + ES_ID(2) + flags(1) + DecoderConfigDescriptor
    // DecoderConfigDescriptor: tag(1) + size(1+) + objectTypeIndication(1)
    if esds.len() < 4 {
        return None;
    }
    let desc = &esds[4..]; // skip version+flags
                           // Find ES_Descriptor (tag 0x03)
    if desc.is_empty() || desc[0] != 0x03 {
        return None;
    }
    let (_, after_es_size) = read_descriptor_size(&desc[1..])?;
    let es_content = &desc[1 + after_es_size..];
    if es_content.len() < 3 {
        return None;
    }
    // Skip ES_ID(2) + flags(1)
    let dec_cfg = &es_content[3..];
    // Find DecoderConfigDescriptor (tag 0x04)
    if dec_cfg.is_empty() || dec_cfg[0] != 0x04 {
        return None;
    }
    let (_, after_dc_size) = read_descriptor_size(&dec_cfg[1..])?;
    let dc_content = &dec_cfg[1 + after_dc_size..];
    if dc_content.len() < 13 {
        return None;
    }
    let object_type = dc_content[0];
    let decoder_specific = find_descriptor_payload(&dc_content[13..], 0x05);
    Some((object_type, decoder_specific))
}

fn find_descriptor_payload(data: &[u8], target_tag: u8) -> Option<&[u8]> {
    let mut pos = 0usize;
    while pos + 2 <= data.len() {
        let tag = data[pos];
        let (size, size_len) = read_descriptor_size(&data[pos + 1..])?;
        let payload_start = pos + 1 + size_len;
        let payload_end = payload_start.checked_add(size as usize)?;
        if payload_end > data.len() {
            return None;
        }
        if tag == target_tag {
            return Some(&data[payload_start..payload_end]);
        }
        pos = payload_end;
    }
    None
}

fn read_descriptor_size(data: &[u8]) -> Option<(u32, usize)> {
    let mut size = 0u32;
    let mut i = 0;
    loop {
        if i >= data.len() || i >= 4 {
            return None;
        }
        let b = data[i];
        size = (size << 7) | (b & 0x7F) as u32;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
    }
    Some((size, i))
}

fn parse_trun(data: &[u8]) -> (i32, Vec<TrunSample>) {
    if data.len() < 8 {
        return (0, Vec::new());
    }
    let flags = u32::from_be_bytes([0, data[1], data[2], data[3]]);
    let sample_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;

    let has_data_offset = flags & 0x000001 != 0;
    let has_first_sample_flags = flags & 0x000004 != 0;
    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;
    let has_cts = flags & 0x000800 != 0;

    let mut pos = 8;
    let data_offset = if has_data_offset {
        if pos + 4 > data.len() {
            return (0, Vec::new());
        }
        let v = i32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
        v
    } else {
        0
    };

    let first_sample_flags = if has_first_sample_flags {
        if pos + 4 > data.len() {
            return (data_offset, Vec::new());
        }
        let v = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
        v
    } else {
        0
    };

    let mut samples = Vec::with_capacity(sample_count.min(4096));
    for i in 0..sample_count {
        let duration = if has_duration {
            if pos + 4 > data.len() {
                break;
            }
            let v = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        } else {
            0
        };
        let size = if has_size {
            if pos + 4 > data.len() {
                break;
            }
            let v = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        } else {
            0
        };
        let sample_flags = if has_flags {
            if pos + 4 > data.len() {
                break;
            }
            let v = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        } else if i == 0 && has_first_sample_flags {
            first_sample_flags
        } else {
            0
        };
        let cts_offset = if has_cts {
            if pos + 4 > data.len() {
                break;
            }
            let v = i32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            pos += 4;
            v
        } else {
            0
        };
        samples.push(TrunSample {
            duration,
            size,
            flags: sample_flags,
            cts_offset,
        });
    }

    (data_offset, samples)
}

fn is_keyframe_flags(flags: u32) -> bool {
    // sample_depends_on == 2 (independent) OR no dependency info and not non-sync
    let depends_on = (flags >> 24) & 0x03;
    let is_non_sync = (flags >> 16) & 0x01;
    depends_on == 2 || (depends_on == 0 && is_non_sync == 0)
}

fn timescale_to_us(ticks: u64, timescale: u32) -> i64 {
    if timescale == 0 {
        return 0;
    }
    // Avoid overflow: divide first for large tick values
    let ts = timescale as i64;
    let t = ticks as i64;
    (t / ts) * 1_000_000 + (t % ts) * 1_000_000 / ts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmp4_mux::{Fmp4MuxEvent, Fmp4MuxSample, Fmp4Muxer, Fmp4MuxerConfig};
    use crate::track::{CodecExtradata, TrackId, TrackInfo};

    fn h264_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        t.width = Some(1920);
        t.height = Some(1080);
        t.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E])],
            pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38])],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        t
    }

    fn aac_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 44_100);
        t.sample_rate = Some(44_100);
        t.channels = Some(2);
        t.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x12, 0x10]),
        };
        t
    }

    #[test]
    fn roundtrip_init_segment() {
        let tracks = vec![h264_track(), aac_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &events[0] else {
            panic!()
        };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let demux_events = demuxer.push(init_data);

        let track_event = demux_events
            .iter()
            .find(|e| matches!(e, Fmp4DemuxEvent::TrackInfo(_)));
        assert!(track_event.is_some());
        if let Some(Fmp4DemuxEvent::TrackInfo(tracks)) = track_event {
            assert_eq!(tracks.len(), 2);
            assert_eq!(tracks[0].codec, CodecId::H264);
            assert_eq!(tracks[0].media_kind, MediaKind::Video);
            assert_eq!(tracks[1].codec, CodecId::AAC);
            assert_eq!(tracks[1].media_kind, MediaKind::Audio);
        }
    }

    #[test]
    fn aac_init_segment_exposes_audio_specific_config_not_esds() {
        let tracks = vec![aac_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &events[0] else {
            panic!()
        };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let demux_events = demuxer.push(init_data);
        let track_event = demux_events
            .iter()
            .find(|e| matches!(e, Fmp4DemuxEvent::TrackInfo(_)));

        if let Some(Fmp4DemuxEvent::TrackInfo(tracks)) = track_event {
            assert_eq!(tracks[0].codec, CodecId::AAC);
            assert_eq!(tracks[0].extradata.as_ref(), &[0x12, 0x10]);
        } else {
            panic!("missing track info");
        }
    }

    #[test]
    fn roundtrip_media_segment() {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else {
            panic!()
        };

        let samples = vec![
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 0,
                pts_us: 33_333,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01, 0x02]),
            },
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 33_333,
                pts_us: 66_666,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41, 0x03]),
            },
        ];
        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else {
            panic!()
        };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        demuxer.push(init_data);
        let frame_events = demuxer.push(seg_data);

        let frames: Vec<_> = frame_events
            .iter()
            .filter(|e| matches!(e, Fmp4DemuxEvent::Frame { .. }))
            .collect();
        assert_eq!(frames.len(), 2);
        if let Fmp4DemuxEvent::Frame { keyframe, data, .. } = &frames[0] {
            assert!(*keyframe);
            assert_eq!(data.as_ref(), &[0x65, 0x01, 0x02]);
        }
        if let Fmp4DemuxEvent::Frame { keyframe, data, .. } = &frames[1] {
            assert!(!*keyframe);
            assert_eq!(data.as_ref(), &[0x41, 0x03]);
        }
    }

    #[test]
    fn chunked_input() {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks);
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else {
            panic!()
        };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        // Feed init segment byte by byte
        let mut all_events = Vec::new();
        for byte in init_data.iter() {
            all_events.extend(demuxer.push(&[*byte]));
        }
        let track_event = all_events
            .iter()
            .find(|e| matches!(e, Fmp4DemuxEvent::TrackInfo(_)));
        assert!(track_event.is_some());
    }

    #[test]
    fn unknown_box_skipped() {
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        // Create a fake unknown box
        let mut fake_box = Vec::new();
        fake_box.extend_from_slice(&12u32.to_be_bytes()); // size
        fake_box.extend_from_slice(b"xyzw"); // unknown type
        fake_box.extend_from_slice(&[0u8; 4]); // content
        let events = demuxer.push(&fake_box);
        assert!(events.iter().any(|e| matches!(
            e,
            Fmp4DemuxEvent::Diagnostic(Fmp4DemuxDiagnostic::UnknownBox { .. })
        )));
    }

    #[test]
    fn oversized_box_rejected() {
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig { max_box_bytes: 16 });
        // Create a box that claims to be 100 bytes
        let mut fake_box = Vec::new();
        fake_box.extend_from_slice(&100u32.to_be_bytes());
        fake_box.extend_from_slice(b"moov");
        fake_box.extend_from_slice(&[0u8; 92]);
        let events = demuxer.push(&fake_box);
        assert!(events.iter().any(|e| matches!(
            e,
            Fmp4DemuxEvent::Diagnostic(Fmp4DemuxDiagnostic::BoxTooLarge { .. })
        )));
    }

    #[test]
    fn extended_box_smaller_than_header_is_diagnostic_not_panic() {
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let mut fake_box = Vec::new();
        fake_box.extend_from_slice(&1u32.to_be_bytes());
        fake_box.extend_from_slice(b"free");
        fake_box.extend_from_slice(&8u64.to_be_bytes());

        let events = demuxer.push(&fake_box);

        assert!(events.iter().any(|e| matches!(
            e,
            Fmp4DemuxEvent::Diagnostic(Fmp4DemuxDiagnostic::MalformedBox {
                box_type: [b'f', b'r', b'e', b'e'],
                size: 8,
                header_size: 16,
            })
        )));
    }

    #[test]
    fn roundtrip_with_sidx() {
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(
            Fmp4MuxerConfig {
                include_styp: true,
                include_sidx: true,
                ..Default::default()
            },
            &tracks,
        );
        let init_events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &init_events[0] else {
            panic!()
        };

        let samples = vec![
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 0,
                pts_us: 33_333,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01, 0x02]),
            },
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 33_333,
                pts_us: 66_666,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41, 0x03]),
            },
        ];
        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data: seg_data, .. } = &seg_events[0] else {
            panic!()
        };

        // Verify sidx is present in the segment
        assert!(seg_data.windows(4).any(|w| w == b"sidx"));

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        demuxer.push(init_data);
        let frame_events = demuxer.push(seg_data);

        let frames: Vec<_> = frame_events
            .iter()
            .filter(|e| matches!(e, Fmp4DemuxEvent::Frame { .. }))
            .collect();
        assert_eq!(frames.len(), 2);
        if let Fmp4DemuxEvent::Frame { keyframe, data, .. } = &frames[0] {
            assert!(*keyframe);
            assert_eq!(data.as_ref(), &[0x65, 0x01, 0x02]);
        }
    }

    /// H265 input variants: hev1, dvh1, dvhe should all be recognized as H265.
    #[test]
    fn h265_input_variants_hev1_dvh1_dvhe() {
        // Generate a normal H265 init segment (uses hvc1), then patch to each variant
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        t.width = Some(1920);
        t.height = Some(1080);
        t.extradata = CodecExtradata::H265 {
            vps: vec![],
            sps: vec![],
            pps: vec![],
            hvcc: Some(Bytes::from_static(&[
                0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0xf0,
                0x00, 0xfc, 0xfc, 0xf8, 0xf8, 0x00, 0x00, 0x0f, 0x00,
            ])),
        };
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &events[0] else {
            panic!()
        };

        for variant in [b"hev1", b"dvh1", b"dvhe"] {
            let mut patched = init_data.to_vec();
            // Find "hvc1" in the init segment and replace with variant
            if let Some(pos) = patched.windows(4).position(|w| w == b"hvc1") {
                patched[pos..pos + 4].copy_from_slice(variant);
            }
            let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
            let demux_events = demuxer.push(&patched);
            let tracks = demux_events.iter().find_map(|e| {
                if let Fmp4DemuxEvent::TrackInfo(t) = e {
                    Some(t)
                } else {
                    None
                }
            });
            assert!(
                tracks.is_some(),
                "variant {:?} should produce TrackInfo",
                core::str::from_utf8(variant)
            );
            assert_eq!(
                tracks.unwrap()[0].codec,
                CodecId::H265,
                "variant {:?} should be H265",
                core::str::from_utf8(variant)
            );
        }
    }

    /// MP3 object type 0x69 and 0x6B should both be recognized correctly.
    #[test]
    fn mp3_object_type_0x69_and_0x6b() {
        // Generate an MP3 init segment (uses 0x69), then test 0x6B variant
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::MP3, 44_100);
        t.sample_rate = Some(44_100);
        t.channels = Some(2);
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &events[0] else {
            panic!()
        };

        // Default MP3 uses object type 0x69 — verify demux recognizes it
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let demux_events = demuxer.push(init_data);
        let tracks = demux_events.iter().find_map(|e| {
            if let Fmp4DemuxEvent::TrackInfo(t) = e {
                Some(t)
            } else {
                None
            }
        });
        assert!(tracks.is_some());
        assert_eq!(tracks.unwrap()[0].codec, CodecId::MP3);

        // Patch object type to 0x6B and verify it's recognized as MP2
        let mut patched = init_data.to_vec();
        // Find the esds object type byte (0x69) and replace with 0x6B
        if let Some(pos) = patched.windows(2).position(|w| w == [0x69, 0x15]) {
            patched[pos] = 0x6B;
        }
        let mut demuxer2 = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
        let demux_events2 = demuxer2.push(&patched);
        let tracks2 = demux_events2.iter().find_map(|e| {
            if let Fmp4DemuxEvent::TrackInfo(t) = e {
                Some(t)
            } else {
                None
            }
        });
        assert!(tracks2.is_some());
        // 0x6B maps to MP2 per the demuxer logic
        assert_eq!(tracks2.unwrap()[0].codec, CodecId::MP2);
    }

    /// MJPEG input variants: jpeg, mjpa, mjpb should be recognized.
    #[test]
    fn mjpeg_input_variants_jpeg_mjpa_mjpb() {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::MJPEG, 90_000);
        t.width = Some(640);
        t.height = Some(480);
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(init_data) = &events[0] else {
            panic!()
        };

        // MJPEG muxer outputs "mp4v" — patch to jpeg/mjpa/mjpb variants
        for variant in [b"jpeg", b"mjpa", b"mjpb"] {
            let mut patched = init_data.to_vec();
            if let Some(pos) = patched.windows(4).position(|w| w == b"mp4v") {
                patched[pos..pos + 4].copy_from_slice(variant);
            }
            let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());
            let demux_events = demuxer.push(&patched);
            let tracks = demux_events.iter().find_map(|e| {
                if let Fmp4DemuxEvent::TrackInfo(t) = e {
                    Some(t)
                } else {
                    None
                }
            });
            assert!(
                tracks.is_some(),
                "variant {:?} should produce TrackInfo",
                core::str::from_utf8(variant)
            );
            // jpeg/mjpa/mjpb should map to MJPEG
            assert_eq!(
                tracks.unwrap()[0].codec,
                CodecId::MJPEG,
                "variant {:?} should be MJPEG",
                core::str::from_utf8(variant)
            );
        }
    }

    /// Repeated init segment updates tracks without leaking old state.
    #[test]
    fn repeated_init_updates_tracks_no_leak() {
        let tracks_v1 = vec![h264_track(), aac_track()];
        let mut muxer1 = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks_v1);
        let Fmp4MuxEvent::InitSegment(init1) = &muxer1.init_segment()[0] else {
            panic!()
        };

        // Second init with only video (no audio)
        let tracks_v2 = vec![h264_track()];
        let mut muxer2 = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &tracks_v2);
        let Fmp4MuxEvent::InitSegment(init2) = &muxer2.init_segment()[0] else {
            panic!()
        };

        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig::default());

        // First init: 2 tracks
        let events1 = demuxer.push(init1);
        let t1 = events1.iter().find_map(|e| {
            if let Fmp4DemuxEvent::TrackInfo(t) = e {
                Some(t)
            } else {
                None
            }
        });
        assert_eq!(t1.unwrap().len(), 2);

        // Second init: should replace with 1 track, not accumulate to 3
        let events2 = demuxer.push(init2);
        let t2 = events2.iter().find_map(|e| {
            if let Fmp4DemuxEvent::TrackInfo(t) = e {
                Some(t)
            } else {
                None
            }
        });
        assert_eq!(t2.unwrap().len(), 1);
        assert_eq!(demuxer.tracks().len(), 1);

        // Should also emit RepeatedInit diagnostic
        assert!(events2.iter().any(|e| matches!(
            e,
            Fmp4DemuxEvent::Diagnostic(Fmp4DemuxDiagnostic::RepeatedInit)
        )));
    }

    /// Fragment duration derives from actual sample timestamps, not fixed fps.
    #[test]
    fn fragment_duration_from_real_timestamps() {
        // Use irregular timestamps (not 33ms fixed)
        let tracks = vec![h264_track()];
        let mut muxer = Fmp4Muxer::new(
            Fmp4MuxerConfig {
                include_sidx: true,
                ..Default::default()
            },
            &tracks,
        );
        let _ = muxer.init_segment();

        let samples = vec![
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 0,
                pts_us: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65]),
            },
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 50_000, // 50ms gap (20fps)
                pts_us: 50_000,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41]),
            },
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 120_000, // 70ms gap (irregular)
                pts_us: 120_000,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41]),
            },
        ];
        let seg_events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &seg_events[0] else {
            panic!()
        };

        // Parse sidx to get subsegment_duration
        let styp_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let ref_entry_offset = styp_size + 12 + 8 + 8 + 4 + 2 + 2;
        let duration_ticks = u32::from_be_bytes([
            data[ref_entry_offset + 4],
            data[ref_entry_offset + 5],
            data[ref_entry_offset + 6],
            data[ref_entry_offset + 7],
        ]);
        // Total duration should be ~190ms (0→50→120, last sample duration estimated as 70ms)
        // At 90kHz timescale: 190_000us * 90000 / 1_000_000 = 17100 ticks
        // Allow some tolerance for rounding
        assert!(
            duration_ticks > 15000 && duration_ticks < 19000,
            "duration_ticks={duration_ticks} should reflect real timestamps (~17100)"
        );
    }
}
