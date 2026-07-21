//! Program Stream (PS) muxer.
//!
//! 节目流（PS）复用器。

use crate::frame::AVFrame;
use crate::ps::encode_pts_dts;
use crate::track::{CodecId, MediaKind, TrackInfo};
use crate::ts_common::crc32_mpeg2;
use bytes::Bytes;
use std::collections::HashMap;

/// Program Stream (PS) muxer.
///
/// 节目流（PS）复用器。
#[derive(Default)]
pub struct PsMuxer {
    tracks: HashMap<u8, TrackInfo>,
}

impl PsMuxer {
    /// Create a new PS muxer.
    ///
    /// 创建新的 PS 复用器。
    pub fn new() -> Self {
        Self {
            tracks: HashMap::new(),
        }
    }

    /// Register a track for muxing.
    ///
    /// 注册待复用的轨道。
    pub fn add_track(&mut self, track: TrackInfo) {
        self.tracks.insert(track.track_id.0 as u8, track);
    }

    /// Mux a single `AVFrame` into a PS packet byte stream.
    ///
    /// 将单个 `AVFrame` 复用为 PS 包字节流。
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

        let crc = crc32_mpeg2(&p_data[..data_len - 4]);
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
