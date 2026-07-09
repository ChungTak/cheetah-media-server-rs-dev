//! Shared fMP4 muxer for ISO BMFF fragmented MP4 generation.
//!
//! Generates init segments (ftyp+moov) and media segments (styp+moof+mdat).
//! Supports H264/H265/H266/VP8/VP9/AV1/MJPEG video and AAC/G711A/G711U/MP3/MP2/Opus audio.

use bytes::{BufMut, Bytes, BytesMut};

use crate::track::{CodecExtradata, CodecId, MediaKind, TrackInfo};

/// Configuration for the fMP4 muxer.
#[derive(Debug, Clone)]
pub struct Fmp4MuxerConfig {
    pub include_styp: bool,
    pub include_sidx: bool,
    pub max_samples_per_fragment: usize,
}

impl Default for Fmp4MuxerConfig {
    fn default() -> Self {
        Self {
            include_styp: true,
            include_sidx: false,
            max_samples_per_fragment: 8192,
        }
    }
}

/// Events produced by the muxer.
#[derive(Debug, Clone)]
pub enum Fmp4MuxEvent {
    InitSegment(Bytes),
    MediaSegment { data: Bytes, keyframe: bool },
    Diagnostic(Fmp4Diagnostic),
}

/// Diagnostic messages from the muxer.
#[derive(Debug, Clone)]
pub enum Fmp4Diagnostic {
    UnsupportedCodec { codec: CodecId, track_id: u32 },
    MaxSamplesReached { track_id: u32 },
}

/// Track descriptor for the muxer (derived from TrackInfo).
#[derive(Debug, Clone)]
struct MuxTrack {
    track_id: u32,
    codec: CodecId,
    media_kind: MediaKind,
    timescale: u32,
    extradata: Bytes,
    width: u16,
    height: u16,
    sample_rate: u32,
    channels: u8,
}

impl MuxTrack {
    fn from_track_info(track: &TrackInfo) -> Self {
        let extradata = extract_extradata(track);
        Self {
            track_id: track.track_id.0,
            codec: track.codec,
            media_kind: track.media_kind,
            timescale: track.clock_rate,
            extradata,
            width: track.width.unwrap_or(0) as u16,
            height: track.height.unwrap_or(0) as u16,
            sample_rate: track.sample_rate.unwrap_or(track.clock_rate),
            channels: track
                .channels
                .unwrap_or(if track.media_kind == MediaKind::Audio {
                    1
                } else {
                    0
                }),
        }
    }
}

/// A sample to be muxed.
#[derive(Debug, Clone)]
pub struct Fmp4MuxSample {
    pub track_id: u32,
    pub dts_us: i64,
    pub pts_us: i64,
    pub is_keyframe: bool,
    pub data: Bytes,
}

/// Shared fMP4 muxer.
pub struct Fmp4Muxer {
    config: Fmp4MuxerConfig,
    tracks: Vec<MuxTrack>,
    sequence_number: u32,
    init_segment: Option<Bytes>,
}

impl Fmp4Muxer {
    pub fn new(config: Fmp4MuxerConfig, tracks: &[TrackInfo]) -> Self {
        let mux_tracks = tracks.iter().map(MuxTrack::from_track_info).collect();
        Self {
            config,
            tracks: mux_tracks,
            sequence_number: 0,
            init_segment: None,
        }
    }

    /// Generate init segment events.
    pub fn init_segment(&mut self) -> Vec<Fmp4MuxEvent> {
        if let Some(ref cached) = self.init_segment {
            return vec![Fmp4MuxEvent::InitSegment(cached.clone())];
        }
        let mut buf = BytesMut::with_capacity(4096);
        write_ftyp(&mut buf);
        write_moov(&mut buf, &self.tracks);
        let result = buf.freeze();
        self.init_segment = Some(result.clone());
        vec![Fmp4MuxEvent::InitSegment(result)]
    }

    /// Mux samples into a media segment.
    pub fn write_segment(&mut self, samples: &[Fmp4MuxSample]) -> Vec<Fmp4MuxEvent> {
        if samples.is_empty() {
            return Vec::new();
        }
        self.sequence_number += 1;
        let keyframe = samples.iter().any(|s| s.is_keyframe);
        let mut buf =
            BytesMut::with_capacity(samples.iter().map(|s| s.data.len()).sum::<usize>() + 1024);
        if self.config.include_styp {
            write_styp(&mut buf);
        }
        if self.config.include_sidx {
            // Write sidx as placeholder, patch after moof+mdat are written
            let sidx_pos = buf.len();
            write_sidx_placeholder(&mut buf, &self.tracks, samples);
            let moof_mdat_start = buf.len();
            write_moof_mdat(&mut buf, &self.tracks, samples, self.sequence_number);
            let moof_mdat_size = (buf.len() - moof_mdat_start) as u32;
            let fragment_duration = compute_fragment_duration(samples, &self.tracks);
            patch_sidx(&mut buf, sidx_pos, moof_mdat_size, fragment_duration);
        } else {
            write_moof_mdat(&mut buf, &self.tracks, samples, self.sequence_number);
        }
        vec![Fmp4MuxEvent::MediaSegment {
            data: buf.freeze(),
            keyframe,
        }]
    }

    /// Write a partial segment (no styp) for LL-HLS parts.
    pub fn write_part(&mut self, samples: &[Fmp4MuxSample]) -> Vec<Fmp4MuxEvent> {
        if samples.is_empty() {
            return Vec::new();
        }
        self.sequence_number += 1;
        let keyframe = samples.iter().any(|s| s.is_keyframe);
        let mut buf =
            BytesMut::with_capacity(samples.iter().map(|s| s.data.len()).sum::<usize>() + 512);
        write_moof_mdat(&mut buf, &self.tracks, samples, self.sequence_number);
        vec![Fmp4MuxEvent::MediaSegment {
            data: buf.freeze(),
            keyframe,
        }]
    }

    pub fn sequence_number(&self) -> u32 {
        self.sequence_number
    }
}

// ─── Extradata extraction ───

fn extract_extradata(track: &TrackInfo) -> Bytes {
    match (&track.codec, &track.extradata) {
        (
            CodecId::H264,
            CodecExtradata::H264 {
                avcc: Some(avcc), ..
            },
        ) => avcc.clone(),
        (CodecId::H264, CodecExtradata::H264 { sps, pps, .. }) => {
            build_h264_avcc(sps.as_slice(), pps.as_slice())
        }
        (
            CodecId::H265,
            CodecExtradata::H265 {
                hvcc: Some(hvcc), ..
            },
        ) => hvcc.clone(),
        (CodecId::H265, CodecExtradata::H265 { vps, sps, pps, .. }) => {
            build_h265_hvcc(vps.as_slice(), sps.as_slice(), pps.as_slice())
        }
        (CodecId::AAC, CodecExtradata::AAC { asc }) => asc.clone(),
        (
            CodecId::Opus,
            CodecExtradata::Opus {
                channel_mapping, ..
            },
        ) => channel_mapping.clone().unwrap_or_default(),
        (CodecId::AV1, CodecExtradata::AV1 { codec_config, .. }) => {
            codec_config.clone().unwrap_or_default()
        }
        (CodecId::VP8, CodecExtradata::VP8 { config }) => config.clone().unwrap_or_default(),
        (CodecId::VP9, CodecExtradata::VP9 { config }) => config.clone().unwrap_or_default(),
        _ => Bytes::new(),
    }
}

fn build_h264_avcc(sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    let Some(first_sps) = sps.first() else {
        return Bytes::new();
    };
    if first_sps.len() < 4 || pps.is_empty() {
        return Bytes::new();
    }

    let sps_count = sps.len().min(31);
    let pps_count = pps.len().min(255);
    let mut out = vec![
        1,
        first_sps[1],
        first_sps[2],
        first_sps[3],
        0xff,
        0xe0 | sps_count as u8,
    ];
    for unit in sps.iter().take(sps_count) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
    out.push(pps_count as u8);
    for unit in pps.iter().take(pps_count) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
    Bytes::from(out)
}

fn build_h265_hvcc(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    if vps.is_empty() || sps.is_empty() || pps.is_empty() {
        return Bytes::new();
    }

    let first_sps = &sps[0];
    let (profile_byte, compat_flags, constraint_flags, level_idc) = if first_sps.len() >= 2 + 1 + 12
    {
        let ptl = &first_sps[3..];
        let profile_byte = ptl[0];
        let compat_flags = u32::from_be_bytes([ptl[1], ptl[2], ptl[3], ptl[4]]);
        let mut constraint_flags = [0u8; 6];
        constraint_flags.copy_from_slice(&ptl[5..11]);
        (profile_byte, compat_flags, constraint_flags, ptl[11])
    } else {
        (0x01, 0x60000000_u32, [0x90u8, 0, 0, 0, 0, 0], 120)
    };

    let mut out = Vec::new();
    out.push(1);
    out.push(profile_byte);
    out.extend_from_slice(&compat_flags.to_be_bytes());
    out.extend_from_slice(&constraint_flags);
    out.push(level_idc);
    out.extend_from_slice(&0xf000_u16.to_be_bytes());
    out.push(0xfc);
    out.push(0xfc);
    out.push(0xf8);
    out.push(0xf8);
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.push(0x0f);
    out.push(3);

    append_hvcc_array(&mut out, 32, vps);
    append_hvcc_array(&mut out, 33, sps);
    append_hvcc_array(&mut out, 34, pps);

    Bytes::from(out)
}

fn append_hvcc_array(out: &mut Vec<u8>, nal_unit_type: u8, units: &[Bytes]) {
    out.push(0x80 | (nal_unit_type & 0x3f));
    out.extend_from_slice(&(units.len().min(u16::MAX as usize) as u16).to_be_bytes());
    for unit in units.iter().take(u16::MAX as usize) {
        let unit = &unit[..unit.len().min(u16::MAX as usize)];
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
}

// ─── Box writing helpers ───

fn write_box(buf: &mut BytesMut, box_type: &[u8; 4], content: &[u8]) {
    buf.put_u32(8 + content.len() as u32);
    buf.extend_from_slice(box_type);
    buf.extend_from_slice(content);
}

fn write_full_box_header(
    buf: &mut BytesMut,
    box_type: &[u8; 4],
    size: u32,
    version: u8,
    flags: u32,
) {
    buf.put_u32(size);
    buf.extend_from_slice(box_type);
    buf.put_u8(version);
    buf.put_u8((flags >> 16) as u8);
    buf.put_u8((flags >> 8) as u8);
    buf.put_u8(flags as u8);
}

fn write_ftyp(buf: &mut BytesMut) {
    let mut content = BytesMut::with_capacity(28);
    content.extend_from_slice(b"iso6");
    content.put_u32(0);
    content.extend_from_slice(b"iso6");
    content.extend_from_slice(b"mp42");
    content.extend_from_slice(b"avc1");
    content.extend_from_slice(b"dash");
    content.extend_from_slice(b"hlsf");
    write_box(buf, b"ftyp", &content);
}

fn write_styp(buf: &mut BytesMut) {
    let mut content = BytesMut::with_capacity(12);
    content.extend_from_slice(b"msdh");
    content.put_u32(0);
    content.extend_from_slice(b"msdh");
    content.extend_from_slice(b"msix");
    write_box(buf, b"styp", &content);
}

// ─── moov ───

fn write_moov(buf: &mut BytesMut, tracks: &[MuxTrack]) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"moov");
    write_mvhd(buf, tracks.len() as u32);
    for track in tracks {
        write_trak(buf, track);
    }
    write_mvex(buf, tracks);
    patch_size(buf, start);
}

fn write_mvhd(buf: &mut BytesMut, next_track_id: u32) {
    write_full_box_header(buf, b"mvhd", 108, 0, 0);
    buf.put_u32(0); // creation_time
    buf.put_u32(0); // modification_time
    buf.put_u32(1000); // timescale
    buf.put_u32(0); // duration
    buf.put_u32(0x00010000); // rate 1.0
    buf.put_u16(0x0100); // volume 1.0
    buf.extend_from_slice(&[0u8; 10]);
    for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
        buf.put_u32(v);
    }
    buf.extend_from_slice(&[0u8; 24]);
    buf.put_u32(next_track_id + 1);
}

fn write_mvex(buf: &mut BytesMut, tracks: &[MuxTrack]) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"mvex");
    for track in tracks {
        write_trex(buf, track.track_id);
    }
    patch_size(buf, start);
}

fn write_trex(buf: &mut BytesMut, track_id: u32) {
    write_full_box_header(buf, b"trex", 32, 0, 0);
    buf.put_u32(track_id);
    buf.put_u32(1); // default_sample_description_index
    buf.put_u32(0); // default_sample_duration
    buf.put_u32(0); // default_sample_size
    buf.put_u32(0); // default_sample_flags
}

// ─── trak ───

fn write_trak(buf: &mut BytesMut, track: &MuxTrack) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"trak");
    write_tkhd(buf, track);
    write_mdia(buf, track);
    patch_size(buf, start);
}

fn write_tkhd(buf: &mut BytesMut, track: &MuxTrack) {
    write_full_box_header(buf, b"tkhd", 92, 0, 0x000003);
    buf.put_u32(0); // creation_time
    buf.put_u32(0); // modification_time
    buf.put_u32(track.track_id);
    buf.put_u32(0); // reserved
    buf.put_u32(0); // duration
    buf.extend_from_slice(&[0u8; 8]);
    buf.put_u16(0); // layer
    buf.put_u16(0); // alternate_group
    buf.put_u16(if track.media_kind == MediaKind::Audio {
        0x0100
    } else {
        0
    });
    buf.put_u16(0);
    for &v in &[0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
        buf.put_u32(v);
    }
    buf.put_u32((track.width as u32) << 16);
    buf.put_u32((track.height as u32) << 16);
}

fn write_mdia(buf: &mut BytesMut, track: &MuxTrack) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"mdia");
    write_mdhd(buf, track.timescale);
    write_hdlr(buf, track.media_kind);
    write_minf(buf, track);
    patch_size(buf, start);
}

fn write_mdhd(buf: &mut BytesMut, timescale: u32) {
    write_full_box_header(buf, b"mdhd", 32, 0, 0);
    buf.put_u32(0);
    buf.put_u32(0);
    buf.put_u32(timescale);
    buf.put_u32(0);
    buf.put_u16(0x55C4); // und
    buf.put_u16(0);
}

fn write_hdlr(buf: &mut BytesMut, kind: MediaKind) {
    let handler = match kind {
        MediaKind::Video => b"vide",
        MediaKind::Audio => b"soun",
        _ => b"meta",
    };
    let name = b"Cheetah\0";
    let size = 12 + 20 + name.len() as u32;
    write_full_box_header(buf, b"hdlr", size, 0, 0);
    buf.put_u32(0);
    buf.extend_from_slice(handler);
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(name);
}

fn write_minf(buf: &mut BytesMut, track: &MuxTrack) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"minf");
    match track.media_kind {
        MediaKind::Video => {
            write_full_box_header(buf, b"vmhd", 20, 0, 1);
            buf.put_u16(0);
            buf.extend_from_slice(&[0u8; 6]);
        }
        MediaKind::Audio => {
            write_full_box_header(buf, b"smhd", 16, 0, 0);
            buf.put_u16(0);
            buf.put_u16(0);
        }
        _ => {}
    }
    write_dinf(buf);
    write_stbl(buf, track);
    patch_size(buf, start);
}

fn write_dinf(buf: &mut BytesMut) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"dinf");
    write_full_box_header(buf, b"dref", 28, 0, 0);
    buf.put_u32(1);
    write_full_box_header(buf, b"url ", 12, 0, 1);
    patch_size(buf, start);
}

fn write_stbl(buf: &mut BytesMut, track: &MuxTrack) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"stbl");
    write_stsd(buf, track);
    // stts empty
    write_full_box_header(buf, b"stts", 16, 0, 0);
    buf.put_u32(0);
    // stsc empty
    write_full_box_header(buf, b"stsc", 16, 0, 0);
    buf.put_u32(0);
    // stsz empty
    write_full_box_header(buf, b"stsz", 20, 0, 0);
    buf.put_u32(0);
    buf.put_u32(0);
    // stco empty
    write_full_box_header(buf, b"stco", 16, 0, 0);
    buf.put_u32(0);
    patch_size(buf, start);
}

fn write_stsd(buf: &mut BytesMut, track: &MuxTrack) {
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"stsd");
    buf.put_u8(0);
    buf.extend_from_slice(&[0u8; 3]);
    buf.put_u32(1);
    match track.media_kind {
        MediaKind::Video => write_video_sample_entry(buf, track),
        MediaKind::Audio => write_audio_sample_entry(buf, track),
        _ => {}
    }
    patch_size(buf, start);
}

fn write_video_sample_entry(buf: &mut BytesMut, track: &MuxTrack) {
    let codec_box: &[u8; 4] = match track.codec {
        CodecId::H264 => b"avc1",
        CodecId::H265 => b"hvc1",
        CodecId::H266 => b"vvc1",
        CodecId::VP8 => b"vp08",
        CodecId::VP9 => b"vp09",
        CodecId::AV1 => b"av01",
        CodecId::MJPEG => b"mp4v",
        _ => b"mp4v",
    };

    let entry_start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(codec_box);
    buf.extend_from_slice(&[0u8; 6]);
    buf.put_u16(1); // data_reference_index
    buf.extend_from_slice(&[0u8; 16]);
    buf.put_u16(track.width);
    buf.put_u16(track.height);
    buf.put_u32(0x00480000); // 72 dpi
    buf.put_u32(0x00480000);
    buf.put_u32(0);
    buf.put_u16(1); // frame_count
    buf.extend_from_slice(&[0u8; 32]); // compressorname
    buf.put_u16(0x0018); // depth
    buf.put_i16(-1);

    match track.codec {
        CodecId::H264 => write_box(buf, b"avcC", &track.extradata),
        CodecId::H265 => write_box(buf, b"hvcC", &track.extradata),
        CodecId::H266 => write_box(buf, b"vvcC", &track.extradata),
        CodecId::VP8 | CodecId::VP9 => write_box(buf, b"vpcC", &track.extradata),
        CodecId::AV1 => write_box(buf, b"av1C", &track.extradata),
        CodecId::MJPEG => write_esds_video(buf, 0x6C),
        _ => {}
    }

    patch_size(buf, entry_start);
}

fn write_audio_sample_entry(buf: &mut BytesMut, track: &MuxTrack) {
    let codec_box: &[u8; 4] = match track.codec {
        CodecId::Opus => b"Opus",
        CodecId::G711A => b"alaw",
        CodecId::G711U => b"ulaw",
        _ => b"mp4a",
    };

    let entry_start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(codec_box);
    buf.extend_from_slice(&[0u8; 6]);
    buf.put_u16(1); // data_reference_index
    buf.extend_from_slice(&[0u8; 8]);
    buf.put_u16(track.channels as u16);
    buf.put_u16(16); // sample_size bits
    buf.put_u16(0);
    buf.put_u16(0);
    buf.put_u32(track.sample_rate.min(65535) << 16);

    match track.codec {
        CodecId::AAC => write_esds_audio(buf, 0x40, &track.extradata),
        CodecId::MP3 => write_esds_audio(buf, 0x69, &track.extradata),
        CodecId::MP2 => write_esds_audio(buf, 0x6B, &track.extradata),
        CodecId::Opus => write_box(buf, b"dOps", &track.extradata),
        _ => {} // G711 needs no config box
    }

    patch_size(buf, entry_start);
}

fn write_esds_audio(buf: &mut BytesMut, object_type: u8, dsi: &[u8]) {
    // Cap DSI to 127 bytes (single-byte descriptor size limit)
    let dsi = if dsi.len() > 127 { &dsi[..127] } else { dsi };
    let dsi_len = dsi.len();
    let dsi_desc_len = 2 + dsi_len;
    let dec_cfg_len = 2 + 13 + dsi_desc_len;
    let es_desc_len = 2 + 3 + dec_cfg_len + 3;
    let esds_size = 12 + es_desc_len as u32;

    write_full_box_header(buf, b"esds", esds_size, 0, 0);
    // ES_Descriptor
    buf.put_u8(0x03);
    buf.put_u8((3 + dec_cfg_len + 3) as u8);
    buf.put_u16(1); // ES_ID
    buf.put_u8(0);
    // DecoderConfigDescriptor
    buf.put_u8(0x04);
    buf.put_u8((13 + dsi_desc_len) as u8);
    buf.put_u8(object_type);
    buf.put_u8(0x15); // AudioStream(5) << 2 | 0x01
    buf.extend_from_slice(&[0u8; 3]);
    buf.put_u32(0); // maxBitrate
    buf.put_u32(0); // avgBitrate
                    // DecoderSpecificInfo
    buf.put_u8(0x05);
    buf.put_u8(dsi_len as u8);
    buf.extend_from_slice(dsi);
    // SLConfigDescriptor
    buf.put_u8(0x06);
    buf.put_u8(0x01);
    buf.put_u8(0x02);
}

fn write_esds_video(buf: &mut BytesMut, object_type: u8) {
    // Minimal esds for video (MJPEG) with no decoder specific info
    let dec_cfg_len = 2 + 13;
    let es_desc_len = 2 + 3 + dec_cfg_len + 3;
    let esds_size = 12 + es_desc_len as u32;

    write_full_box_header(buf, b"esds", esds_size, 0, 0);
    buf.put_u8(0x03);
    buf.put_u8((3 + dec_cfg_len + 3) as u8);
    buf.put_u16(1);
    buf.put_u8(0);
    buf.put_u8(0x04);
    buf.put_u8(13u8);
    buf.put_u8(object_type);
    buf.put_u8(0x21); // VisualStream(4) << 2 | 0x01
    buf.extend_from_slice(&[0u8; 3]);
    buf.put_u32(0);
    buf.put_u32(0);
    buf.put_u8(0x06);
    buf.put_u8(0x01);
    buf.put_u8(0x02);
}

// ─── sidx ───

/// Write a sidx box with placeholder reference_size and subsegment_duration.
/// These are patched after moof+mdat are written.
fn write_sidx_placeholder(buf: &mut BytesMut, tracks: &[MuxTrack], samples: &[Fmp4MuxSample]) {
    // Use first track's timescale for sidx
    let reference_id = tracks.first().map(|t| t.track_id).unwrap_or(1);
    let timescale = tracks.first().map(|t| t.timescale).unwrap_or(90_000);
    let earliest_pts_us = samples.iter().map(|s| s.pts_us).min().unwrap_or(0);
    let earliest_pts = us_to_timescale(earliest_pts_us, timescale) as u64;

    // sidx: version=1, flags=0, 1 reference
    // size = 12 (full box header) + 8 (ref_id + timescale) + 8 (earliest_pts) + 4 (first_offset=0) + 2 (reserved) + 2 (ref_count=1) + 12 (reference entry)
    let size: u32 = 12 + 8 + 8 + 4 + 2 + 2 + 12;
    write_full_box_header(buf, b"sidx", size, 1, 0);
    buf.put_u32(reference_id);
    buf.put_u32(timescale);
    buf.put_u64(earliest_pts); // earliest_presentation_time
    buf.put_u32(0); // first_offset (0 = immediately follows)
    buf.put_u16(0); // reserved
    buf.put_u16(1); // reference_count
                    // Reference entry (12 bytes): reference_type(1 bit) + referenced_size(31 bits) + subsegment_duration(32) + SAP(32)
    buf.put_u32(0); // placeholder: referenced_size (patched later)
    buf.put_u32(0); // placeholder: subsegment_duration (patched later)
    let sap: u32 = if samples.iter().any(|s| s.is_keyframe) {
        0x90000000 // starts_with_SAP=1, SAP_type=1
    } else {
        0
    };
    buf.put_u32(sap);
}

/// Patch sidx referenced_size and subsegment_duration after moof+mdat are known.
fn patch_sidx(buf: &mut BytesMut, sidx_pos: usize, moof_mdat_size: u32, duration: u32) {
    // Reference entry starts at sidx_pos + box_size - 12 (last 12 bytes are the reference entry)
    let ref_entry_pos = sidx_pos + 12 + 8 + 8 + 4 + 2 + 2; // after full_box_header + ref_id + ts + earliest + first_offset + reserved + count
    buf[ref_entry_pos..ref_entry_pos + 4].copy_from_slice(&moof_mdat_size.to_be_bytes());
    buf[ref_entry_pos + 4..ref_entry_pos + 8].copy_from_slice(&duration.to_be_bytes());
}

/// Compute fragment duration in the first track's timescale.
fn compute_fragment_duration(samples: &[Fmp4MuxSample], tracks: &[MuxTrack]) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let timescale = tracks.first().map(|t| t.timescale).unwrap_or(90_000);
    let first_dts = samples.first().unwrap().dts_us;
    let last_dts = samples.last().unwrap().dts_us;
    // Estimate last sample duration from second-to-last gap or default
    let last_dur = if samples.len() >= 2 {
        samples.last().unwrap().dts_us - samples[samples.len() - 2].dts_us
    } else {
        33_333 // ~30fps default
    };
    us_to_timescale(last_dts - first_dts + last_dur, timescale) as u32
}

// ─── moof + mdat ───

fn write_moof_mdat(buf: &mut BytesMut, tracks: &[MuxTrack], samples: &[Fmp4MuxSample], seq: u32) {
    // Group samples by track
    let mut track_samples: Vec<Vec<&Fmp4MuxSample>> = vec![Vec::new(); tracks.len()];
    for sample in samples {
        if let Some(idx) = tracks.iter().position(|t| t.track_id == sample.track_id) {
            track_samples[idx].push(sample);
        }
    }

    let mdat_payload_size: usize = track_samples
        .iter()
        .flat_map(|ts| ts.iter())
        .map(|s| s.data.len())
        .sum();

    // moof
    let moof_start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(b"moof");

    // mfhd
    write_full_box_header(buf, b"mfhd", 16, 0, 0);
    buf.put_u32(seq);

    // traf per track with samples
    let mut data_offset_positions: Vec<usize> = Vec::new();
    for (idx, track) in tracks.iter().enumerate() {
        let ts = &track_samples[idx];
        if ts.is_empty() {
            continue;
        }
        let traf_start = buf.len();
        buf.put_u32(0);
        buf.extend_from_slice(b"traf");

        // tfhd: default-base-is-moof
        write_full_box_header(buf, b"tfhd", 16, 0, 0x020000);
        buf.put_u32(track.track_id);

        // tfdt version=1
        let base_dts = us_to_timescale(ts[0].dts_us, track.timescale).max(0);
        write_full_box_header(buf, b"tfdt", 20, 1, 0);
        buf.put_u64(base_dts as u64);

        // trun
        let sample_count = ts.len() as u32;
        let is_video = track.media_kind == MediaKind::Video;
        if is_video {
            // version=1 with duration+size+flags+cts_offset
            let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400 | 0x000800;
            let trun_size = 12 + 4 + 4 + sample_count * 16;
            write_full_box_header(buf, b"trun", trun_size, 1, trun_flags);
            buf.put_u32(sample_count);
            data_offset_positions.push(buf.len());
            buf.put_i32(0); // placeholder

            for (i, s) in ts.iter().enumerate() {
                let next_dts = if i + 1 < ts.len() {
                    ts[i + 1].dts_us
                } else if i > 0 {
                    s.dts_us + (s.dts_us - ts[i - 1].dts_us)
                } else {
                    s.dts_us + 33_333 // ~30fps
                };
                let duration = us_to_timescale(next_dts - s.dts_us, track.timescale) as u32;
                let size = s.data.len() as u32;
                let flags: u32 = if s.is_keyframe {
                    0x02000000
                } else {
                    0x01010000
                };
                let cts = us_to_timescale(s.pts_us - s.dts_us, track.timescale) as i32;
                buf.put_u32(duration);
                buf.put_u32(size);
                buf.put_u32(flags);
                buf.put_i32(cts);
            }
        } else {
            // version=0 with duration+size
            let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200;
            let trun_size = 12 + 4 + 4 + sample_count * 8;
            write_full_box_header(buf, b"trun", trun_size, 0, trun_flags);
            buf.put_u32(sample_count);
            data_offset_positions.push(buf.len());
            buf.put_i32(0); // placeholder

            for (i, s) in ts.iter().enumerate() {
                let next_dts = if i + 1 < ts.len() {
                    ts[i + 1].dts_us
                } else if i > 0 {
                    s.dts_us + (s.dts_us - ts[i - 1].dts_us)
                } else {
                    s.dts_us + 23_220 // ~44100 Hz
                };
                let duration = us_to_timescale(next_dts - s.dts_us, track.timescale) as u32;
                let size = s.data.len() as u32;
                buf.put_u32(duration);
                buf.put_u32(size);
            }
        }

        patch_size(buf, traf_start);
    }

    patch_size(buf, moof_start);
    let moof_size = (buf.len() - moof_start) as u32;

    // Patch data_offset: from moof start to sample data in mdat
    let mut mdat_track_offset: u32 = 0;
    let mut dop_idx = 0;
    for (idx, _) in tracks.iter().enumerate() {
        let ts = &track_samples[idx];
        if ts.is_empty() {
            continue;
        }
        let data_offset = (moof_size + 8 + mdat_track_offset) as i32;
        let pos = data_offset_positions[dop_idx];
        buf[pos..pos + 4].copy_from_slice(&data_offset.to_be_bytes());
        mdat_track_offset += ts.iter().map(|s| s.data.len() as u32).sum::<u32>();
        dop_idx += 1;
    }

    // mdat
    buf.put_u32(8 + mdat_payload_size as u32);
    buf.extend_from_slice(b"mdat");
    for (idx, _) in tracks.iter().enumerate() {
        for s in &track_samples[idx] {
            buf.extend_from_slice(&s.data);
        }
    }
}

fn us_to_timescale(us: i64, timescale: u32) -> i64 {
    let ts = timescale as i64;
    (us / 1_000_000) * ts + (us % 1_000_000) * ts / 1_000_000
}

fn patch_size(buf: &mut BytesMut, start: usize) {
    let size = (buf.len() - start) as u32;
    buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{CodecExtradata, TrackId};

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
    fn init_segment_starts_with_ftyp() {
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[h264_track(), aac_track()]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        assert_eq!(&data[4..8], b"ftyp");
    }

    #[test]
    fn init_segment_contains_moov() {
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[h264_track()]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        assert!(data.windows(4).any(|w| w == b"moov"));
    }

    #[test]
    fn h264_init_segment_builds_avcc_from_parameter_sets() {
        let mut track = h264_track();
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1E])],
            pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38])],
            avcc: None,
        };
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[track]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        let avcc_pos = data.windows(4).position(|w| w == b"avcC").unwrap();
        let avcc = &data[avcc_pos + 4..];
        assert_eq!(avcc[0], 1);
        assert!(avcc.windows(4).any(|w| w == [0x67, 0x42, 0x00, 0x1E]));
        assert!(avcc.windows(3).any(|w| w == [0x68, 0xCE, 0x38]));
    }

    #[test]
    fn media_segment_contains_moof_mdat() {
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[h264_track()]);
        let samples = vec![Fmp4MuxSample {
            track_id: 1,
            dts_us: 0,
            pts_us: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65, 0xAA, 0xBB]),
        }];
        let events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, keyframe } = &events[0] else {
            panic!()
        };
        assert!(*keyframe);
        assert!(data.windows(4).any(|w| w == b"moof"));
        assert!(data.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn segment_starts_with_styp_when_configured() {
        let mut muxer = Fmp4Muxer::new(
            Fmp4MuxerConfig {
                include_styp: true,
                ..Default::default()
            },
            &[h264_track()],
        );
        let samples = vec![Fmp4MuxSample {
            track_id: 1,
            dts_us: 0,
            pts_us: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65]),
        }];
        let events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &events[0] else {
            panic!()
        };
        assert_eq!(&data[4..8], b"styp");
    }

    #[test]
    fn part_has_no_styp() {
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[h264_track()]);
        let samples = vec![Fmp4MuxSample {
            track_id: 1,
            dts_us: 0,
            pts_us: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65]),
        }];
        let events = muxer.write_part(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &events[0] else {
            panic!()
        };
        assert_eq!(&data[4..8], b"moof");
    }

    #[test]
    fn segment_with_sidx() {
        let mut muxer = Fmp4Muxer::new(
            Fmp4MuxerConfig {
                include_styp: true,
                include_sidx: true,
                ..Default::default()
            },
            &[h264_track()],
        );
        let samples = vec![
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 0,
                pts_us: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01]),
            },
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 33_333,
                pts_us: 33_333,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41, 0x02]),
            },
        ];
        let events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &events[0] else {
            panic!()
        };
        // Should contain styp, sidx, moof, mdat in order
        assert_eq!(&data[4..8], b"styp");
        let styp_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        assert_eq!(&data[styp_size + 4..styp_size + 8], b"sidx");
        let sidx_size = u32::from_be_bytes([
            data[styp_size],
            data[styp_size + 1],
            data[styp_size + 2],
            data[styp_size + 3],
        ]) as usize;
        let after_sidx = styp_size + sidx_size;
        assert_eq!(&data[after_sidx + 4..after_sidx + 8], b"moof");
        // sidx referenced_size should equal moof+mdat total
        let ref_entry_offset = styp_size + 12 + 8 + 8 + 4 + 2 + 2;
        let ref_size = u32::from_be_bytes([
            data[ref_entry_offset],
            data[ref_entry_offset + 1],
            data[ref_entry_offset + 2],
            data[ref_entry_offset + 3],
        ]);
        let moof_mdat_actual = data.len() - after_sidx;
        assert_eq!(ref_size as usize, moof_mdat_actual);
    }

    #[test]
    fn sidx_reference_id_uses_first_track_id() {
        let mut track = h264_track();
        track.track_id = TrackId(42);
        let mut muxer = Fmp4Muxer::new(
            Fmp4MuxerConfig {
                include_styp: false,
                include_sidx: true,
                ..Default::default()
            },
            &[track],
        );
        let samples = vec![Fmp4MuxSample {
            track_id: 42,
            dts_us: 0,
            pts_us: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65, 0x01]),
        }];

        let events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &events[0] else {
            panic!()
        };

        assert_eq!(&data[4..8], b"sidx");
        let reference_id = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
        assert_eq!(reference_id, 42);
    }

    #[test]
    fn multi_track_segment() {
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[h264_track(), aac_track()]);
        let samples = vec![
            Fmp4MuxSample {
                track_id: 1,
                dts_us: 0,
                pts_us: 33_333,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01]),
            },
            Fmp4MuxSample {
                track_id: 2,
                dts_us: 0,
                pts_us: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0xFF, 0xF1, 0x50]),
            },
        ];
        let events = muxer.write_segment(&samples);
        let Fmp4MuxEvent::MediaSegment { data, .. } = &events[0] else {
            panic!()
        };
        // Should contain both track data
        assert!(data.windows(4).any(|w| w == b"traf"));
        // mdat should contain both payloads
        let mdat_pos = data.windows(4).position(|w| w == b"mdat").unwrap();
        let mdat_payload = &data[mdat_pos + 4..];
        assert_eq!(mdat_payload.len(), 5); // 2 + 3
    }

    #[test]
    fn mjpeg_init_segment_has_esds() {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::MJPEG, 90_000);
        t.width = Some(640);
        t.height = Some(480);
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        assert!(data.windows(4).any(|w| w == b"mp4v"));
        assert!(data.windows(4).any(|w| w == b"esds"));
    }

    #[test]
    fn g711a_init_segment() {
        let t = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::G711A, 8_000);
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        assert!(data.windows(4).any(|w| w == b"alaw"));
    }

    #[test]
    fn opus_init_segment() {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::Opus, 48_000);
        t.sample_rate = Some(48_000);
        t.channels = Some(2);
        t.extradata = crate::track::CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: Some(Bytes::from_static(&[0x01, 0x02])),
        };
        let mut muxer = Fmp4Muxer::new(Fmp4MuxerConfig::default(), &[t]);
        let events = muxer.init_segment();
        let Fmp4MuxEvent::InitSegment(data) = &events[0] else {
            panic!()
        };
        assert!(data.windows(4).any(|w| w == b"Opus"));
        assert!(data.windows(4).any(|w| w == b"dOps"));
    }
}
