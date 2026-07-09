//! Shared MPEG-TS muxer for all protocol modules.
//!
//! Produces 188-byte aligned TS packets from `AVFrame` + `TrackInfo`.
//! Supports multi-track, PAT/PMT, PES, PCR, AUD injection, and parameter set prepend.

use bytes::{BufMut, Bytes, BytesMut};

use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};
use crate::ts_common::{
    crc32_mpeg2, encode_timestamp, registration_descriptor, stream_type_for_codec,
    write_pes_packets, AUD_H264, AUD_H265, AUD_H266, PAT_PID, PMT_PID, TS_PACKET_SIZE,
};
use crate::AVFrame;

/// Configuration for the shared TS muxer.
#[derive(Debug, Clone)]
pub struct MpegTsMuxerConfig {
    /// Video PID base (default 0x0100).
    pub video_pid_base: u16,
    /// Audio PID base (default 0x0110).
    pub audio_pid_base: u16,
}

impl Default for MpegTsMuxerConfig {
    fn default() -> Self {
        Self {
            video_pid_base: 0x0100,
            audio_pid_base: 0x0110,
        }
    }
}

/// Events emitted by the muxer.
#[derive(Debug, Clone)]
pub enum MpegTsMuxEvent {
    /// One or more 188-byte TS packets.
    Packet(Bytes),
    /// Diagnostic message (non-fatal).
    Diagnostic(MpegTsDiagnostic),
}

/// Diagnostic messages from the muxer.
#[derive(Debug, Clone)]
pub enum MpegTsDiagnostic {
    /// Codec not supported for TS muxing; frame skipped.
    UnsupportedCodec { track_id: TrackId, codec: CodecId },
}

/// Track entry maintained by the muxer.
struct MuxTrackEntry {
    track_id: TrackId,
    pid: u16,
    stream_id: u8,
    codec: CodecId,
    media_kind: MediaKind,
    cc: u8,
    extradata: crate::track::CodecExtradata,
}

/// Shared multi-track MPEG-TS muxer.
pub struct MpegTsMuxer {
    tracks: Vec<MuxTrackEntry>,
    pat_cc: u8,
    pmt_cc: u8,
    pcr_pid: u16,
}

impl MpegTsMuxer {
    /// Create a new muxer from track info and config.
    pub fn new(config: &MpegTsMuxerConfig, tracks: &[TrackInfo]) -> Self {
        // Sort tracks by media_kind (video first) then by TrackId for stable PID assignment
        let mut sorted_tracks: Vec<&TrackInfo> = tracks
            .iter()
            .filter(|t| t.media_kind == MediaKind::Video || t.media_kind == MediaKind::Audio)
            .collect();
        sorted_tracks.sort_by_key(|t| (t.media_kind != MediaKind::Video, t.track_id));

        let mut entries = Vec::with_capacity(sorted_tracks.len());
        let mut next_video_pid = config.video_pid_base;
        let mut next_audio_pid = config.audio_pid_base;
        let mut next_video_stream_id: u8 = 0xE0;
        let mut next_audio_stream_id: u8 = 0xC0;

        for track in sorted_tracks {
            let (pid, stream_id) = match track.media_kind {
                MediaKind::Video => {
                    let p = next_video_pid;
                    next_video_pid += 1;
                    let s = next_video_stream_id;
                    next_video_stream_id = next_video_stream_id.wrapping_add(1);
                    (p, s)
                }
                MediaKind::Audio => {
                    let p = next_audio_pid;
                    next_audio_pid += 1;
                    let s = next_audio_stream_id;
                    next_audio_stream_id = next_audio_stream_id.wrapping_add(1);
                    (p, s)
                }
                _ => continue,
            };
            entries.push(MuxTrackEntry {
                track_id: track.track_id,
                pid,
                stream_id,
                codec: track.codec,
                media_kind: track.media_kind,
                cc: 0,
                extradata: track.extradata.clone(),
            });
        }

        // PCR PID: first video track, or first audio track, or fallback
        let pcr_pid = entries
            .iter()
            .find(|t| t.media_kind == MediaKind::Video)
            .or_else(|| entries.first())
            .map(|t| t.pid)
            .unwrap_or(config.video_pid_base);

        Self {
            tracks: entries,
            pat_cc: 0,
            pmt_cc: 0,
            pcr_pid,
        }
    }

    /// Write PAT + PMT tables.
    pub fn write_tables(&mut self) -> Vec<MpegTsMuxEvent> {
        let mut buf = BytesMut::with_capacity(2 * TS_PACKET_SIZE);
        self.write_pat(&mut buf);
        self.write_pmt(&mut buf);
        vec![MpegTsMuxEvent::Packet(buf.freeze())]
    }

    /// Mux a single AVFrame into TS packets.
    pub fn push_frame(&mut self, frame: &AVFrame) -> Vec<MpegTsMuxEvent> {
        let Some(idx) = self
            .tracks
            .iter()
            .position(|t| t.track_id == frame.track_id)
        else {
            return Vec::new();
        };

        let codec = self.tracks[idx].codec;
        let media_kind = self.tracks[idx].media_kind;
        let pid = self.tracks[idx].pid;
        let stream_id = self.tracks[idx].stream_id;
        let is_video = media_kind == MediaKind::Video;
        let is_keyframe = frame.flags.contains(crate::FrameFlags::KEY);

        // Convert timestamps to 90kHz from microseconds
        let pts_90k = us_to_90k(frame.pts_us);
        let dts_90k = us_to_90k(frame.dts_us);

        // Build payload with codec-specific processing
        let mut prepended = Vec::new();
        let payload: &[u8] = match codec {
            CodecId::H264 if is_video => {
                prepended.reserve(AUD_H264.len() + frame.payload.len() + 128);
                prepended.extend_from_slice(AUD_H264);
                if is_keyframe {
                    prepend_h264_params(&self.tracks[idx].extradata, &mut prepended);
                }
                prepended.extend_from_slice(&frame.payload);
                &prepended
            }
            CodecId::H265 if is_video => {
                prepended.reserve(AUD_H265.len() + frame.payload.len() + 128);
                prepended.extend_from_slice(AUD_H265);
                if is_keyframe {
                    prepend_h265_params(&self.tracks[idx].extradata, &mut prepended);
                }
                prepended.extend_from_slice(&frame.payload);
                &prepended
            }
            CodecId::H266 if is_video => {
                prepended.reserve(AUD_H266.len() + frame.payload.len() + 128);
                prepended.extend_from_slice(AUD_H266);
                if is_keyframe {
                    prepend_h266_params(&self.tracks[idx].extradata, &mut prepended);
                }
                prepended.extend_from_slice(&frame.payload);
                &prepended
            }
            CodecId::AAC => {
                // Wrap raw AAC in ADTS for TS transport
                if let Some(adts_data) = wrap_aac_adts(&frame.payload, &self.tracks[idx].extradata)
                {
                    prepended = adts_data;
                    &prepended
                } else {
                    &frame.payload
                }
            }
            _ => &frame.payload,
        };

        // Build PES
        let pes = build_pes(
            stream_id,
            payload,
            Some(pts_90k),
            if is_video { Some(dts_90k) } else { None },
        );

        let pcr = if is_video && is_keyframe {
            Some(dts_90k)
        } else {
            None
        };

        let mut buf = BytesMut::with_capacity(pes.len() + TS_PACKET_SIZE);
        let mut cc = self.tracks[idx].cc;
        write_pes_packets(&mut buf, pid, &pes, &mut cc, is_keyframe && is_video, pcr);
        self.tracks[idx].cc = cc;

        vec![MpegTsMuxEvent::Packet(buf.freeze())]
    }

    /// Flush (no-op for muxer; included for API symmetry).
    pub fn flush(&mut self) -> Vec<MpegTsMuxEvent> {
        Vec::new()
    }

    /// Number of tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    fn write_pat(&mut self, buf: &mut BytesMut) {
        let mut payload = BytesMut::with_capacity(17);
        payload.put_u8(0x00); // table_id
        payload.put_u16(0xB000 | 13); // section_syntax + length
        payload.put_u16(0x0001); // transport_stream_id
        payload.put_u8(0xC1); // version/current
        payload.put_u8(0x00); // section_number
        payload.put_u8(0x00); // last_section_number
        payload.put_u16(0x0001); // program_number
        payload.put_u16(0xE000 | PMT_PID);
        let crc = crc32_mpeg2(&payload);
        payload.put_u32(crc);
        write_single_ts_packet(buf, PAT_PID, &payload, &mut self.pat_cc, true);
    }

    fn write_pmt(&mut self, buf: &mut BytesMut) {
        let mut payload = BytesMut::with_capacity(64);
        payload.put_u8(0x02); // table_id
        let section_len_pos = payload.len();
        payload.put_u16(0); // placeholder
        payload.put_u16(0x0001); // program_number
        payload.put_u8(0xC1); // version/current
        payload.put_u8(0x00); // section_number
        payload.put_u8(0x00); // last_section_number
        payload.put_u16(0xE000 | self.pcr_pid);
        payload.put_u16(0xF000); // program_info_length = 0

        for track in &self.tracks {
            payload.put_u8(stream_type_for_codec(track.codec));
            payload.put_u16(0xE000 | track.pid);
            let desc = registration_descriptor(track.codec);
            payload.put_u16(0xF000 | desc.len() as u16);
            payload.extend_from_slice(desc);
        }

        let section_len = (payload.len() - 3 + 4) as u16;
        let len_bytes = (0xB000 | section_len).to_be_bytes();
        payload[section_len_pos] = len_bytes[0];
        payload[section_len_pos + 1] = len_bytes[1];
        let crc = crc32_mpeg2(&payload);
        payload.put_u32(crc);
        write_single_ts_packet(buf, PMT_PID, &payload, &mut self.pmt_cc, true);
    }
}

fn prepend_h264_params(extradata: &crate::track::CodecExtradata, buf: &mut Vec<u8>) {
    if let crate::track::CodecExtradata::H264 { sps, pps, .. } = extradata {
        for s in sps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(s);
        }
        for p in pps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(p);
        }
    }
}

fn prepend_h265_params(extradata: &crate::track::CodecExtradata, buf: &mut Vec<u8>) {
    if let crate::track::CodecExtradata::H265 { vps, sps, pps, .. } = extradata {
        for v in vps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(v);
        }
        for s in sps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(s);
        }
        for p in pps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(p);
        }
    }
}

fn prepend_h266_params(extradata: &crate::track::CodecExtradata, buf: &mut Vec<u8>) {
    if let crate::track::CodecExtradata::H266 { vps, sps, pps } = extradata {
        for v in vps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(v);
        }
        for s in sps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(s);
        }
        for p in pps {
            buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            buf.extend_from_slice(p);
        }
    }
}

fn wrap_aac_adts(payload: &[u8], extradata: &crate::track::CodecExtradata) -> Option<Vec<u8>> {
    // Only wrap if we have ASC info; if payload already has ADTS sync word, pass through
    if payload.len() >= 2 && payload[0] == 0xFF && (payload[1] & 0xF0) == 0xF0 {
        return None; // Already ADTS
    }
    let asc = match extradata {
        crate::track::CodecExtradata::AAC { asc } => {
            crate::audio::AacAudioSpecificConfig::from_bytes(asc)?
        }
        _ => return None,
    };
    let frame_len = (payload.len() + 7).min(0xFFFF) as u16;
    let header = crate::audio::AdtsHeader {
        profile: asc.audio_object_type.saturating_sub(1) & 0x03,
        sampling_frequency_index: asc.sampling_frequency_index,
        channel_configuration: asc.channel_configuration,
        frame_length: frame_len,
    }
    .build();
    let mut out = Vec::with_capacity(frame_len as usize);
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    Some(out)
}

fn us_to_90k(us: i64) -> u64 {
    if us >= 0 {
        (us as u64) * 9 / 100
    } else {
        0
    }
}

fn build_pes(stream_id: u8, payload: &[u8], pts: Option<u64>, dts: Option<u64>) -> Vec<u8> {
    let has_pts = pts.is_some();
    let has_dts = dts.is_some() && dts != pts;

    let header_data_len: u8 = match (has_pts, has_dts) {
        (true, true) => 10,
        (true, false) => 5,
        _ => 0,
    };

    let pts_dts_flags: u8 = match (has_pts, has_dts) {
        (true, true) => 0xC0,
        (true, false) => 0x80,
        _ => 0x00,
    };

    let pes_packet_len: u16 = if stream_id >= 0xE0 {
        0 // video: unbounded
    } else {
        let len = 3u32 + header_data_len as u32 + payload.len() as u32;
        if len > 0xFFFF {
            0
        } else {
            len as u16
        }
    };

    let mut pes = Vec::with_capacity(9 + header_data_len as usize + payload.len());
    pes.extend_from_slice(&[0x00, 0x00, 0x01]);
    pes.push(stream_id);
    pes.extend_from_slice(&pes_packet_len.to_be_bytes());
    pes.push(0x80);
    pes.push(pts_dts_flags);
    pes.push(header_data_len);

    if let Some(pts_val) = pts {
        let marker = if has_dts { 0x03 } else { 0x02 };
        encode_timestamp(&mut pes, marker, pts_val);
    }
    if has_dts {
        if let Some(dts_val) = dts {
            encode_timestamp(&mut pes, 0x01, dts_val);
        }
    }

    pes.extend_from_slice(payload);
    pes
}

fn write_single_ts_packet(buf: &mut BytesMut, pid: u16, payload: &[u8], cc: &mut u8, pusi: bool) {
    let mut offset = 0;
    let mut first = true;

    while offset < payload.len() || (payload.is_empty() && first) {
        let mut pkt = [0xFF_u8; TS_PACKET_SIZE];
        pkt[0] = 0x47;
        let use_pusi = first && pusi;
        let pusi_bit: u16 = if use_pusi { 0x4000 } else { 0 };
        let pid_bytes = (pusi_bit | pid).to_be_bytes();
        pkt[1] = pid_bytes[0];
        pkt[2] = pid_bytes[1];
        pkt[3] = 0x10 | (*cc & 0x0F);

        let header_len = if use_pusi { 5 } else { 4 };
        if use_pusi {
            pkt[4] = 0x00;
        }

        let available = TS_PACKET_SIZE - header_len;
        let remaining = payload.len().saturating_sub(offset);
        let copy_len = remaining.min(available);
        if copy_len > 0 {
            pkt[header_len..header_len + copy_len]
                .copy_from_slice(&payload[offset..offset + copy_len]);
        }
        offset += copy_len;

        buf.extend_from_slice(&pkt);
        *cc = (*cc + 1) & 0x0F;
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameFlags;
    use crate::time::Timebase;
    use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};

    fn make_track(id: u32, kind: MediaKind, codec: CodecId) -> TrackInfo {
        TrackInfo::new(TrackId(id), kind, codec, 90_000)
    }

    #[test]
    fn muxer_produces_aligned_pat_pmt() {
        let tracks = vec![
            make_track(1, MediaKind::Video, CodecId::H264),
            make_track(2, MediaKind::Audio, CodecId::AAC),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let events = muxer.write_tables();
        assert_eq!(events.len(), 1);
        if let MpegTsMuxEvent::Packet(data) = &events[0] {
            assert_eq!(data.len(), 2 * TS_PACKET_SIZE);
            assert_eq!(data[0], 0x47);
            assert_eq!(data[188], 0x47);
        } else {
            panic!("expected Packet event");
        }
    }

    #[test]
    fn muxer_push_frame_produces_aligned_packets() {
        let tracks = vec![make_track(1, MediaKind::Video, CodecId::H264)];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            crate::FrameFormat::CanonicalH26x,
            90_000, // 1 second at 90kHz
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB]),
        );
        frame.flags = FrameFlags::KEY;

        let events = muxer.push_frame(&frame);
        assert_eq!(events.len(), 1);
        if let MpegTsMuxEvent::Packet(data) = &events[0] {
            assert_eq!(data.len() % TS_PACKET_SIZE, 0);
            for chunk in data.chunks(TS_PACKET_SIZE) {
                assert_eq!(chunk[0], 0x47);
            }
        }
    }

    #[test]
    fn multi_track_muxer() {
        let tracks = vec![
            make_track(1, MediaKind::Video, CodecId::H265),
            make_track(2, MediaKind::Audio, CodecId::AAC),
            make_track(3, MediaKind::Audio, CodecId::MP2),
        ];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        assert_eq!(muxer.track_count(), 3);

        let events = muxer.write_tables();
        if let MpegTsMuxEvent::Packet(data) = &events[0] {
            assert_eq!(data.len() % TS_PACKET_SIZE, 0);
        }
    }

    #[test]
    fn descriptor_heavy_pmt_spans_multiple_ts_packets_without_truncation() {
        let tracks: Vec<_> = (0..32)
            .map(|idx| make_track(idx + 1, MediaKind::Audio, CodecId::Opus))
            .collect();
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);

        let events = muxer.write_tables();
        let MpegTsMuxEvent::Packet(data) = &events[0] else {
            panic!("expected Packet event");
        };

        assert!(
            data.len() > 2 * TS_PACKET_SIZE,
            "descriptor-heavy PMT should require continuation packets"
        );
        assert_eq!(data.len() % TS_PACKET_SIZE, 0);

        let pmt_section = collect_section_from_pid(data, PMT_PID);
        let section_len = ((pmt_section[1] as usize & 0x0F) << 8) | pmt_section[2] as usize;
        let section_end = 3 + section_len;
        assert_eq!(pmt_section.len(), section_end);

        let stored_crc = u32::from_be_bytes([
            pmt_section[section_end - 4],
            pmt_section[section_end - 3],
            pmt_section[section_end - 2],
            pmt_section[section_end - 1],
        ]);
        let computed_crc = crc32_mpeg2(&pmt_section[..section_end - 4]);
        assert_eq!(stored_crc, computed_crc);

        let program_info_len = ((pmt_section[10] as usize & 0x0F) << 8) | pmt_section[11] as usize;
        let mut pos = 12 + program_info_len;
        let entries_end = section_end - 4;
        let mut stream_count = 0;
        while pos + 5 <= entries_end {
            assert_eq!(
                pmt_section[pos], 0x06,
                "Opus should use private PES stream type"
            );
            let es_info_len =
                ((pmt_section[pos + 3] as usize & 0x0F) << 8) | pmt_section[pos + 4] as usize;
            pos += 5 + es_info_len;
            stream_count += 1;
        }
        assert_eq!(stream_count, tracks.len());
        assert_eq!(pos, entries_end);
    }

    fn collect_section_from_pid(data: &[u8], pid: u16) -> Vec<u8> {
        let mut section = Vec::new();
        let mut target_len = None;

        for pkt in data.chunks(TS_PACKET_SIZE) {
            let pkt_pid = ((pkt[1] as u16 & 0x1F) << 8) | pkt[2] as u16;
            if pkt_pid != pid {
                continue;
            }
            let pusi = pkt[1] & 0x40 != 0;
            let mut offset = 4;
            if pusi {
                offset += 1 + pkt[4] as usize;
                section.clear();
                target_len = None;
            }
            if offset >= TS_PACKET_SIZE {
                continue;
            }
            section.extend_from_slice(&pkt[offset..]);
            if section.len() >= 3 && target_len.is_none() {
                let section_len = ((section[1] as usize & 0x0F) << 8) | section[2] as usize;
                target_len = Some(3 + section_len);
            }
            if let Some(len) = target_len {
                if section.len() >= len {
                    section.truncate(len);
                    return section;
                }
            }
        }

        panic!("section for pid {pid:#x} not found or incomplete");
    }
}
