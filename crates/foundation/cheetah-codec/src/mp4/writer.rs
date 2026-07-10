//! Classic ISO BMFF MP4 writer (`ftyp + mdat + moov`).
//!
//! Targets VOD/record file output. Not fragmented; emits a single `moov` after
//! all samples are written. The driver layer is responsible for actually
//! writing the bytes to disk; this module returns the byte plan as
//! `Mp4WriteEvent`.
//!
//! Supports the project's documented codec matrix:
//! H264/H265/AAC/G711/Opus/MP3/MJPEG/VP8/VP9/AV1.

use crate::prelude::*;
use bytes::{BufMut, Bytes, BytesMut};

use crate::track::{CodecId, MediaKind, TrackInfo};

use super::box_parser::{write_box, write_full_box};
use super::sample_table::{TrackBuilder, TrackSampleRecord};
use super::Mp4Error;

/// Configuration for the classic MP4 writer.
#[derive(Debug, Clone)]
pub struct Mp4WriterConfig {
    /// Reserved for the future faststart layout (`moov` before `mdat`).
    /// Not yet implemented; setting this to `true` causes `finalize` to
    /// return `Mp4Error::UnsupportedTrack`.
    pub faststart: bool,
    /// Preferred 4cc for the major brand in `ftyp`.
    pub major_brand: [u8; 4],
}

impl Default for Mp4WriterConfig {
    fn default() -> Self {
        Self {
            faststart: false,
            major_brand: *b"isom",
        }
    }
}

/// Events emitted by the writer.
#[derive(Debug, Clone)]
pub enum Mp4WriteEvent {
    /// The complete file as a single contiguous `Bytes`.
    File(Bytes),
}

/// Sample-table aware writer.
pub struct Mp4Writer {
    /// `config` field of type `Mp4WriterConfig`.
    /// `config` 字段，类型为 `Mp4WriterConfig`.
    config: Mp4WriterConfig,
    /// `tracks` field.
    /// `tracks` 字段.
    tracks: Vec<TrackBuilder>,
    /// `payload` field of type `BytesMut`.
    /// `payload` 字段，类型为 `BytesMut`.
    payload: BytesMut,
}

impl Mp4Writer {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(config: Mp4WriterConfig, tracks: &[TrackInfo]) -> Result<Self, Mp4Error> {
        if tracks.is_empty() {
            return Err(Mp4Error::UnsupportedTrack("no tracks"));
        }
        Ok(Self {
            config,
            tracks: tracks.iter().map(TrackBuilder::new).collect(),
            payload: BytesMut::new(),
        })
    }

    /// Append a sample for the given track. Sample data is buffered; the
    /// final byte plan is produced by `finalize`.
    pub fn push_sample(
        &mut self,
        track_id: u32,
        dts_us: i64,
        pts_us: i64,
        is_sync: bool,
        data: &[u8],
    ) -> Result<(), Mp4Error> {
        if data.len() > u32::MAX as usize {
            return Err(Mp4Error::InvalidSampleTable("sample size exceeds 4GiB"));
        }
        let tb = self
            .tracks
            .iter_mut()
            .find(|t| t.track_id.0 == track_id)
            .ok_or(Mp4Error::UnsupportedTrack("unknown track id"))?;

        // Patch previous sample's duration once the current dts is known.
        if let Some(prev) = tb.samples.last_mut() {
            if prev.duration_us == 0 {
                prev.duration_us = (dts_us - prev.dts_us).max(0);
            }
        }
        // Buffer payload and record sample metadata. The duration is left
        // at 0 here and patched on the next `push_sample` call (or in
        // `finalize` for the last sample).
        let absolute_offset = self.payload.len() as u64;
        self.payload.extend_from_slice(data);
        tb.samples.push(TrackSampleRecord {
            data_offset: absolute_offset,
            size: data.len() as u32,
            dts_us,
            pts_us,
            duration_us: 0,
            is_sync,
        });
        Ok(())
    }

    /// Finalize the file and return the encoded MP4 byte stream.
    pub fn finalize(mut self) -> Result<Mp4WriteEvent, Mp4Error> {
        // Faststart layout (`moov` before `mdat`) requires a two-pass
        // pre-compute of chunk offsets. The current implementation only
        // ships the standard mdat-first layout; reject the configuration
        // explicitly rather than silently emit an MP4 with wrong stco
        // offsets.
        if self.config.faststart {
            return Err(Mp4Error::UnsupportedTrack(
                "faststart=true not yet supported by Mp4Writer; use mdat-first layout",
            ));
        }

        // Compute the last sample's duration heuristically from the previous
        // delta, defaulting to 33 ms for video and 23 ms for audio.
        for tb in self.tracks.iter_mut() {
            let len = tb.samples.len();
            if len == 0 {
                continue;
            }
            if tb.samples[len - 1].duration_us == 0 {
                let default = match tb.media_kind {
                    MediaKind::Video => 33_000,
                    MediaKind::Audio => 23_000,
                    _ => 33_000,
                };
                let dur = if len >= 2 {
                    tb.samples[len - 1]
                        .dts_us
                        .saturating_sub(tb.samples[len - 2].dts_us)
                        .max(default)
                } else {
                    default
                };
                tb.samples[len - 1].duration_us = dur;
            }
        }

        let mut out = BytesMut::with_capacity(self.payload.len() + 4096);
        write_ftyp(&mut out, &self.config.major_brand);

        // mdat first, then moov.
        let payload_len = self.payload.len() as u64;
        let mdat_header_size: u64 = if payload_len + 8 > u32::MAX as u64 {
            16
        } else {
            8
        };
        if mdat_header_size == 16 {
            out.put_u32(1);
            out.extend_from_slice(b"mdat");
            out.put_u64(payload_len + 16);
        } else {
            out.put_u32(payload_len as u32 + 8);
            out.extend_from_slice(b"mdat");
        }
        let mdat_payload_start = out.len() as u64;
        out.extend_from_slice(&self.payload);
        // moov: each track has one chunk per sample; absolute offsets
        // are mdat_payload_start + sample.data_offset.
        write_moov(&mut out, &self.tracks, mdat_payload_start);

        Ok(Mp4WriteEvent::File(out.freeze()))
    }
}

fn write_ftyp(buf: &mut BytesMut, major_brand: &[u8; 4]) {
    write_box(buf, b"ftyp", |buf| {
        buf.extend_from_slice(major_brand);
        buf.put_u32(0x200); // minor version
        buf.extend_from_slice(b"isom");
        buf.extend_from_slice(b"mp42");
        buf.extend_from_slice(b"avc1");
    });
}

fn write_moov(buf: &mut BytesMut, tracks: &[TrackBuilder], chunk_base: u64) {
    write_box(buf, b"moov", |buf| {
        write_mvhd(buf, tracks);
        for (i, tb) in tracks.iter().enumerate() {
            let track_id = (i + 1) as u32;
            write_trak(buf, tb, track_id, chunk_base);
        }
    });
}

fn write_mvhd(buf: &mut BytesMut, tracks: &[TrackBuilder]) {
    write_full_box(buf, b"mvhd", 0, 0, |buf| {
        buf.put_u32(0); // creation_time
        buf.put_u32(0); // modification_time
        buf.put_u32(1000); // timescale
        let dur = tracks
            .iter()
            .map(|t| (t.duration_us() / 1000).max(0).min(u32::MAX as i64) as u32)
            .max()
            .unwrap_or(0);
        buf.put_u32(dur); // duration in 1000 timescale (ms)
        buf.put_u32(0x00010000); // rate
        buf.put_u16(0x0100); // volume
        buf.extend_from_slice(&[0u8; 10]);
        for v in [0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            buf.put_u32(v);
        }
        buf.extend_from_slice(&[0u8; 24]);
        buf.put_u32(tracks.len() as u32 + 1);
    });
}

fn write_trak(buf: &mut BytesMut, tb: &TrackBuilder, track_id: u32, chunk_base: u64) {
    write_box(buf, b"trak", |buf| {
        write_tkhd(buf, tb, track_id);
        write_mdia(buf, tb, chunk_base);
    });
}

fn write_tkhd(buf: &mut BytesMut, tb: &TrackBuilder, track_id: u32) {
    write_full_box(buf, b"tkhd", 0, 0x000003, |buf| {
        buf.put_u32(0);
        buf.put_u32(0);
        buf.put_u32(track_id);
        buf.put_u32(0);
        let dur_ms = (tb.duration_us() / 1000).max(0).min(u32::MAX as i64) as u32;
        buf.put_u32(dur_ms);
        buf.extend_from_slice(&[0u8; 8]);
        buf.put_u16(0);
        buf.put_u16(0);
        buf.put_u16(if tb.media_kind == MediaKind::Audio {
            0x0100
        } else {
            0
        });
        buf.put_u16(0);
        for v in [0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000] {
            buf.put_u32(v);
        }
        buf.put_u32((tb.width as u32) << 16);
        buf.put_u32((tb.height as u32) << 16);
    });
}

fn write_mdia(buf: &mut BytesMut, tb: &TrackBuilder, chunk_base: u64) {
    write_box(buf, b"mdia", |buf| {
        write_mdhd(buf, tb);
        write_hdlr(buf, tb);
        write_minf(buf, tb, chunk_base);
    });
}

fn write_mdhd(buf: &mut BytesMut, tb: &TrackBuilder) {
    write_full_box(buf, b"mdhd", 0, 0, |buf| {
        buf.put_u32(0);
        buf.put_u32(0);
        buf.put_u32(tb.timescale);
        let dur_ts = us_to_timescale(tb.duration_us(), tb.timescale)
            .max(0)
            .min(u32::MAX as i64) as u32;
        buf.put_u32(dur_ts);
        buf.put_u16(0x55C4);
        buf.put_u16(0);
    });
}

fn write_hdlr(buf: &mut BytesMut, tb: &TrackBuilder) {
    write_full_box(buf, b"hdlr", 0, 0, |buf| {
        buf.put_u32(0);
        let handler: &[u8; 4] = match tb.media_kind {
            MediaKind::Video => b"vide",
            MediaKind::Audio => b"soun",
            _ => b"meta",
        };
        buf.extend_from_slice(handler);
        buf.extend_from_slice(&[0u8; 12]);
        buf.extend_from_slice(b"Cheetah\0");
    });
}

fn write_minf(buf: &mut BytesMut, tb: &TrackBuilder, chunk_base: u64) {
    write_box(buf, b"minf", |buf| {
        match tb.media_kind {
            MediaKind::Video => {
                write_full_box(buf, b"vmhd", 0, 1, |buf| {
                    buf.put_u16(0);
                    buf.extend_from_slice(&[0u8; 6]);
                });
            }
            MediaKind::Audio => {
                write_full_box(buf, b"smhd", 0, 0, |buf| {
                    buf.put_u16(0);
                    buf.put_u16(0);
                });
            }
            _ => {
                write_full_box(buf, b"nmhd", 0, 0, |_buf| {});
            }
        }
        write_box(buf, b"dinf", |buf| {
            write_full_box(buf, b"dref", 0, 0, |buf| {
                buf.put_u32(1);
                write_full_box(buf, b"url ", 0, 1, |_buf| {});
            });
        });
        write_stbl(buf, tb, chunk_base);
    });
}

fn write_stbl(buf: &mut BytesMut, tb: &TrackBuilder, chunk_base: u64) {
    write_box(buf, b"stbl", |buf| {
        write_stsd(buf, tb);
        write_stts(buf, tb);
        write_ctts_if_needed(buf, tb);
        write_stss_if_needed(buf, tb);
        write_stsc(buf, tb);
        write_stsz(buf, tb);
        write_stco_or_co64(buf, tb, chunk_base);
    });
}

fn write_stsd(buf: &mut BytesMut, tb: &TrackBuilder) {
    write_full_box(buf, b"stsd", 0, 0, |buf| {
        buf.put_u32(1); // entry_count
        match tb.media_kind {
            MediaKind::Video => write_video_sample_entry(buf, tb),
            MediaKind::Audio => write_audio_sample_entry(buf, tb),
            _ => {}
        }
    });
}

fn write_video_sample_entry(buf: &mut BytesMut, tb: &TrackBuilder) {
    let codec_box: &[u8; 4] = match tb.codec {
        CodecId::H264 => b"avc1",
        CodecId::H265 => b"hvc1",
        CodecId::H266 => b"vvc1",
        CodecId::VP8 => b"vp08",
        CodecId::VP9 => b"vp09",
        CodecId::AV1 => b"av01",
        CodecId::MJPEG => b"mp4v",
        _ => b"mp4v",
    };
    write_box(buf, codec_box, |buf| {
        buf.extend_from_slice(&[0u8; 6]);
        buf.put_u16(1);
        buf.extend_from_slice(&[0u8; 16]);
        buf.put_u16(tb.width);
        buf.put_u16(tb.height);
        buf.put_u32(0x00480000);
        buf.put_u32(0x00480000);
        buf.put_u32(0);
        buf.put_u16(1);
        buf.extend_from_slice(&[0u8; 32]);
        buf.put_u16(0x0018);
        buf.put_i16(-1);
        match tb.codec {
            CodecId::H264 => write_box(buf, b"avcC", |b| b.extend_from_slice(&tb.extradata)),
            CodecId::H265 => write_box(buf, b"hvcC", |b| b.extend_from_slice(&tb.extradata)),
            CodecId::H266 => write_box(buf, b"vvcC", |b| b.extend_from_slice(&tb.extradata)),
            CodecId::VP8 | CodecId::VP9 => {
                write_box(buf, b"vpcC", |b| b.extend_from_slice(&tb.extradata))
            }
            CodecId::AV1 => write_box(buf, b"av1C", |b| b.extend_from_slice(&tb.extradata)),
            CodecId::MJPEG => write_esds_video(buf, 0x6C),
            _ => {}
        }
    });
}

fn write_audio_sample_entry(buf: &mut BytesMut, tb: &TrackBuilder) {
    let codec_box: &[u8; 4] = match tb.codec {
        CodecId::Opus => b"Opus",
        CodecId::G711A => b"alaw",
        CodecId::G711U => b"ulaw",
        _ => b"mp4a",
    };
    write_box(buf, codec_box, |buf| {
        buf.extend_from_slice(&[0u8; 6]);
        buf.put_u16(1);
        buf.extend_from_slice(&[0u8; 8]);
        buf.put_u16(tb.channels as u16);
        buf.put_u16(16);
        buf.put_u16(0);
        buf.put_u16(0);
        buf.put_u32(tb.sample_rate.min(65535) << 16);
        match tb.codec {
            CodecId::AAC => write_esds_audio(buf, 0x40, &tb.extradata),
            CodecId::MP3 => write_esds_audio(buf, 0x69, &tb.extradata),
            CodecId::MP2 => write_esds_audio(buf, 0x6B, &tb.extradata),
            CodecId::Opus => write_box(buf, b"dOps", |b| b.extend_from_slice(&tb.extradata)),
            _ => {}
        }
    });
}

fn write_esds_audio(buf: &mut BytesMut, object_type: u8, dsi: &[u8]) {
    let dsi = if dsi.len() > 127 { &dsi[..127] } else { dsi };
    write_full_box(buf, b"esds", 0, 0, |buf| {
        let dsi_desc_len = 2 + dsi.len();
        let dec_cfg_len = 2 + 13 + dsi_desc_len;
        // ES_Descriptor
        buf.put_u8(0x03);
        buf.put_u8((3 + dec_cfg_len + 3) as u8);
        buf.put_u16(1);
        buf.put_u8(0);
        // DecoderConfigDescriptor
        buf.put_u8(0x04);
        buf.put_u8((13 + dsi_desc_len) as u8);
        buf.put_u8(object_type);
        buf.put_u8(0x15);
        buf.extend_from_slice(&[0u8; 3]);
        buf.put_u32(0);
        buf.put_u32(0);
        // DecoderSpecificInfo
        buf.put_u8(0x05);
        buf.put_u8(dsi.len() as u8);
        buf.extend_from_slice(dsi);
        // SLConfigDescriptor
        buf.put_u8(0x06);
        buf.put_u8(0x01);
        buf.put_u8(0x02);
    });
}

fn write_esds_video(buf: &mut BytesMut, object_type: u8) {
    write_full_box(buf, b"esds", 0, 0, |buf| {
        let dec_cfg_len = 2 + 13;
        // ES_Descriptor
        buf.put_u8(0x03);
        buf.put_u8((3 + dec_cfg_len + 3) as u8);
        buf.put_u16(1);
        buf.put_u8(0);
        // DecoderConfigDescriptor
        buf.put_u8(0x04);
        buf.put_u8(13u8);
        buf.put_u8(object_type);
        buf.put_u8(0x21);
        buf.extend_from_slice(&[0u8; 3]);
        buf.put_u32(0);
        buf.put_u32(0);
        // SLConfigDescriptor
        buf.put_u8(0x06);
        buf.put_u8(0x01);
        buf.put_u8(0x02);
    });
}

fn write_stts(buf: &mut BytesMut, tb: &TrackBuilder) {
    // RLE-compress same-duration runs.
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for s in &tb.samples {
        let dur = us_to_timescale(s.duration_us, tb.timescale)
            .max(0)
            .min(u32::MAX as i64) as u32;
        if let Some(last) = runs.last_mut() {
            if last.1 == dur {
                last.0 += 1;
                continue;
            }
        }
        runs.push((1, dur));
    }
    write_full_box(buf, b"stts", 0, 0, |buf| {
        buf.put_u32(runs.len() as u32);
        for (count, delta) in runs {
            buf.put_u32(count);
            buf.put_u32(delta);
        }
    });
}

fn write_ctts_if_needed(buf: &mut BytesMut, tb: &TrackBuilder) {
    let has_b = tb.samples.iter().any(|s| s.pts_us != s.dts_us);
    if !has_b {
        return;
    }
    let mut runs: Vec<(u32, i32)> = Vec::new();
    for s in &tb.samples {
        let delta = s.pts_us.saturating_sub(s.dts_us);
        let cts = super::compat::clamp_composition_offset(us_to_timescale(delta, tb.timescale));
        if let Some(last) = runs.last_mut() {
            if last.1 == cts {
                last.0 += 1;
                continue;
            }
        }
        runs.push((1, cts));
    }
    write_full_box(buf, b"ctts", 1, 0, |buf| {
        buf.put_u32(runs.len() as u32);
        for (count, off) in runs {
            buf.put_u32(count);
            buf.put_i32(off);
        }
    });
}

fn write_stss_if_needed(buf: &mut BytesMut, tb: &TrackBuilder) {
    if tb.media_kind != MediaKind::Video {
        return;
    }
    let sync: Vec<u32> = tb
        .samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if s.is_sync {
                Some((i + 1) as u32)
            } else {
                None
            }
        })
        .collect();
    write_full_box(buf, b"stss", 0, 0, |buf| {
        buf.put_u32(sync.len() as u32);
        for n in sync {
            buf.put_u32(n);
        }
    });
}

fn write_stsc(buf: &mut BytesMut, _tb: &TrackBuilder) {
    // 1 sample per chunk, single entry: first_chunk=1, samples_per_chunk=1, sd_idx=1
    write_full_box(buf, b"stsc", 0, 0, |buf| {
        buf.put_u32(1);
        buf.put_u32(1);
        buf.put_u32(1);
        buf.put_u32(1);
    });
}

fn write_stsz(buf: &mut BytesMut, tb: &TrackBuilder) {
    write_full_box(buf, b"stsz", 0, 0, |buf| {
        buf.put_u32(0);
        buf.put_u32(tb.samples.len() as u32);
        for s in &tb.samples {
            buf.put_u32(s.size);
        }
    });
}

fn write_stco_or_co64(buf: &mut BytesMut, tb: &TrackBuilder, chunk_base: u64) {
    let max_offset = tb
        .samples
        .iter()
        .map(|s| chunk_base + s.data_offset)
        .max()
        .unwrap_or(0);
    if max_offset > u32::MAX as u64 {
        write_full_box(buf, b"co64", 0, 0, |buf| {
            buf.put_u32(tb.samples.len() as u32);
            for s in &tb.samples {
                buf.put_u64(chunk_base + s.data_offset);
            }
        });
    } else {
        write_full_box(buf, b"stco", 0, 0, |buf| {
            buf.put_u32(tb.samples.len() as u32);
            for s in &tb.samples {
                buf.put_u32((chunk_base + s.data_offset) as u32);
            }
        });
    }
}

fn us_to_timescale(us: i64, timescale: u32) -> i64 {
    let ts = timescale as i128;
    let v = (us as i128) * ts / 1_000_000;
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};
    use bytes::Bytes;

    fn h264_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        t.width = Some(640);
        t.height = Some(360);
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
    fn writes_minimal_h264_mp4() {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).expect("writer");
        let payload = b"FAKE_H264_AU";
        w.push_sample(1, 0, 0, true, payload).unwrap();
        w.push_sample(1, 33_333, 33_333, false, payload).unwrap();
        let Mp4WriteEvent::File(buf) = w.finalize().expect("finalize");
        assert!(buf.windows(4).any(|w| w == b"ftyp"));
        assert!(buf.windows(4).any(|w| w == b"moov"));
        assert!(buf.windows(4).any(|w| w == b"mdat"));
        assert!(buf.windows(4).any(|w| w == b"avc1"));
        assert!(buf.windows(4).any(|w| w == b"avcC"));
        assert!(buf.windows(4).any(|w| w == b"stsd"));
        assert!(buf.windows(4).any(|w| w == b"stss"));
    }
}
