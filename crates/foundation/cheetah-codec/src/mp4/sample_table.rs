//! Classic MP4 sample table (`stbl`) modelling and seek index construction.
//!
//! 经典 MP4 样本表（`stbl`）建模与索引构建。
//!
//! A track's sample table is built from `stts` (sample durations), `ctts`
//! (composition offsets, optional), `stsc` (samples-per-chunk), `stsz`
//! (sample sizes), `stco`/`co64` (chunk offsets) and optional `stss` (sync
//! sample list). This module materialises the cross-referenced index into a
//! flat per-sample list with absolute file offsets and presentation/decode
//! timestamps in track timescale units.

use crate::prelude::*;
use alloc::collections::BTreeSet;
use bytes::Bytes;

use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};

use super::Mp4Error;

/// Per-sample seek-index entry.
///
/// 每个样本的索引条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleIndexEntry {
    /// Absolute file offset of this sample's payload.
    pub offset: u64,
    /// Sample size in bytes.
    pub size: u32,
    /// Decode time in track timescale ticks.
    pub dts: i64,
    /// Composition (presentation) offset in track timescale ticks.
    pub cts_offset: i32,
    /// Sample duration in track timescale ticks.
    pub duration: u32,
    /// True for sync (random-access) samples.
    pub is_sync: bool,
}

impl SampleIndexEntry {
    /// Presentation timestamp in timescale ticks.
    ///
    /// 以 timescale 刻度表示的显示时间戳。
    pub fn pts(&self) -> i64 {
        self.dts.saturating_add(self.cts_offset as i64)
    }
}

/// Per-track materialised sample index.
///
/// 每个轨道物化的样本索引。
#[derive(Debug, Clone)]
pub struct SampleIndex {
    pub track_id: TrackId,
    pub timescale: u32,
    pub samples: Vec<SampleIndexEntry>,
    /// Total decoded duration in timescale ticks.
    pub duration: i64,
}

impl SampleIndex {
    /// Find the largest sample index whose `dts` is <= the requested
    /// timescale time, then walk backwards to the nearest sync sample.
    /// Returns `None` if the index is empty.
    ///
    /// 查找 `dts` 小于等于请求 timescale 时间的最大样本索引，
    /// 然后回退到最近的同步样本。索引为空时返回 `None`。
    pub fn seek_to_dts(&self, dts: i64) -> Option<usize> {
        if self.samples.is_empty() {
            return None;
        }
        let pos = match self.samples.binary_search_by(|s| s.dts.cmp(&dts)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        // Walk backwards to nearest sync sample. If none, return 0.
        for i in (0..=pos).rev() {
            if self.samples[i].is_sync {
                return Some(i);
            }
        }
        Some(0)
    }

    /// Total decoded duration in microseconds.
    ///
    /// 总解码时长（微秒）。
    pub fn duration_us(&self) -> i64 {
        if self.timescale == 0 {
            return 0;
        }
        let micros = (self.duration as i128) * 1_000_000 / (self.timescale as i128);
        micros.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
}

/// Helper used by the writer to incrementally build a track's sample table.
///
/// 写入器用于逐步构建轨道样本表的辅助结构。
#[derive(Debug, Clone)]
pub struct TrackBuilder {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub timescale: u32,
    pub width: u16,
    pub height: u16,
    pub sample_rate: u32,
    pub channels: u8,
    pub extradata: Bytes,
    pub samples: Vec<TrackSampleRecord>,
}

/// Per-sample record kept by `TrackBuilder` while the writer is buffering.
///
/// `TrackBuilder` 在写入器缓冲期间保留的每个样本记录。
#[derive(Debug, Clone, Copy)]
pub struct TrackSampleRecord {
    /// Sample data offset in the writer's payload buffer (relative).
    pub data_offset: u64,
    pub size: u32,
    pub dts_us: i64,
    pub pts_us: i64,
    pub duration_us: i64,
    pub is_sync: bool,
}

impl TrackBuilder {
    /// Create a builder from a `TrackInfo`.
    ///
    /// 从 `TrackInfo` 创建构建器。
    pub fn new(track: &TrackInfo) -> Self {
        let mut tb = Self {
            track_id: track.track_id,
            media_kind: track.media_kind,
            codec: track.codec,
            timescale: track.clock_rate.max(1),
            width: track.width.unwrap_or(0) as u16,
            height: track.height.unwrap_or(0) as u16,
            sample_rate: track.sample_rate.unwrap_or(track.clock_rate.max(1)),
            channels: track.channels.unwrap_or(match track.media_kind {
                MediaKind::Audio => 1,
                _ => 0,
            }),
            extradata: extract_extradata(track),
            samples: Vec::new(),
        };
        // Anchor at minimum 1000 timescale when the input had clock_rate=0.
        if tb.timescale == 0 {
            tb.timescale = 1000;
        }
        tb
    }

    /// Duration from the first to the last sample plus the last sample duration.
    ///
    /// 从首个样本到末个样本的时长加上末个样本时长。
    pub fn duration_us(&self) -> i64 {
        if self.samples.is_empty() {
            return 0;
        }
        let last = &self.samples[self.samples.len() - 1];
        let first = &self.samples[0];
        last.dts_us
            .saturating_sub(first.dts_us)
            .saturating_add(last.duration_us.max(0))
    }
}

fn extract_extradata(track: &TrackInfo) -> Bytes {
    // Replicates the small extraction logic from `cheetah_codec::fmp4_mux`
    // since the AVCC/HVCC builders there are private. Keeping a local copy
    // avoids reaching into another module's internals.
    match (&track.codec, &track.extradata) {
        (
            CodecId::H264,
            CodecExtradata::H264 {
                avcc: Some(avcc), ..
            },
        ) => avcc.clone(),
        (CodecId::H264, CodecExtradata::H264 { sps, pps, .. }) => build_h264_avcc(sps, pps),
        (
            CodecId::H265,
            CodecExtradata::H265 {
                hvcc: Some(hvcc), ..
            },
        ) => hvcc.clone(),
        (CodecId::H265, CodecExtradata::H265 { vps, sps, pps, .. }) => {
            build_h265_hvcc(vps, sps, pps)
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

/// Parsed `stbl` content.
///
/// 已解析的 `stbl` 内容。
#[derive(Debug, Clone, Default)]
pub struct SampleTable {
    pub stts: Vec<(u32, u32)>,      // (count, delta)
    pub ctts: Vec<(u32, i32)>,      // (count, offset) — version 0 unsigned interpreted as signed
    pub stss: Option<Vec<u32>>,     // 1-based sample numbers
    pub stsc: Vec<(u32, u32, u32)>, // (first_chunk, samples_per_chunk, sample_description_index)
    pub stsz_default: u32,          // 0 if per-sample sizes
    pub stsz_sizes: Vec<u32>,
    pub stco: Vec<u64>,
}

impl SampleTable {
    /// Materialise the per-sample seek index (Vec<SampleIndexEntry>).
    ///
    /// Walks chunks, sample sizes, durations and composition offsets to build
    /// a flat array with absolute offsets and timestamps.
    ///
    /// 物化每个样本的索引（Vec<SampleIndexEntry>）。
    ///
    /// 遍历 chunk、样本大小、时长与合成偏移，构建包含绝对偏移与时间戳的
    /// 扁平数组。
    pub fn build_index(&self, timescale: u32) -> Result<SampleIndex, Mp4Error> {
        // Hard cap so a hostile sample table cannot drive the reader into
        // OOM. 4 million samples is enough for ~37 hours of 30fps video and
        // is well below typical `max_box_bytes` enforcement limits.
        const MAX_SAMPLES: usize = 4 * 1024 * 1024;
        let raw_total = if self.stsz_default == 0 {
            self.stsz_sizes.len()
        } else {
            self.total_samples_from_stsc()
        };
        if raw_total > MAX_SAMPLES {
            return Err(Mp4Error::InvalidSampleTable("sample count exceeds limit"));
        }
        let total_samples = raw_total;
        let mut samples: Vec<SampleIndexEntry> = Vec::with_capacity(total_samples);

        // Build sample size lookup
        let size_for = |sample_idx: usize| -> u32 {
            if self.stsz_default != 0 {
                self.stsz_default
            } else if sample_idx < self.stsz_sizes.len() {
                self.stsz_sizes[sample_idx]
            } else {
                0
            }
        };

        // Build stts iterator: per-sample duration in track timescale ticks
        let mut stts_iter = self.stts.iter().copied();
        let mut stts_remaining = stts_iter.next().unwrap_or((0, 0));

        // Build ctts iterator
        let mut ctts_iter = self.ctts.iter().copied();
        let mut ctts_remaining = ctts_iter.next().unwrap_or((0, 0));

        // Build sync set
        let sync_set: Option<BTreeSet<u32>> =
            self.stss.as_ref().map(|s| s.iter().copied().collect());

        // Walk chunks based on stsc/stco
        let mut sample_idx_global: usize = 0;
        let mut current_dts: i64 = 0;
        // stsc entries describe a run starting at first_chunk; ranges run until next entry's first_chunk
        let chunk_count = self.stco.len();
        for (i, chunk_off) in self.stco.iter().copied().enumerate() {
            let chunk_no = (i + 1) as u32;
            let samples_in_chunk = self.samples_per_chunk_for(chunk_no);
            let mut sample_offset_in_chunk: u64 = 0;
            for _ in 0..samples_in_chunk {
                if sample_idx_global >= total_samples {
                    break;
                }
                let size = size_for(sample_idx_global);
                // Pull duration
                while stts_remaining.0 == 0 {
                    match stts_iter.next() {
                        Some(next) => stts_remaining = next,
                        None => {
                            stts_remaining = (u32::MAX, stts_remaining.1.max(1));
                            break;
                        }
                    }
                }
                let duration = stts_remaining.1;
                stts_remaining.0 = stts_remaining.0.saturating_sub(1);

                // Pull cts offset
                let cts = if !self.ctts.is_empty() {
                    while ctts_remaining.0 == 0 {
                        match ctts_iter.next() {
                            Some(next) => ctts_remaining = next,
                            None => {
                                ctts_remaining = (u32::MAX, 0);
                                break;
                            }
                        }
                    }
                    let v = ctts_remaining.1;
                    ctts_remaining.0 = ctts_remaining.0.saturating_sub(1);
                    v
                } else {
                    0
                };

                let is_sync = match &sync_set {
                    Some(set) => set.contains(&((sample_idx_global + 1) as u32)),
                    None => true, // missing stss → all video frames are sync per spec compat
                };

                samples.push(SampleIndexEntry {
                    offset: chunk_off + sample_offset_in_chunk,
                    size,
                    dts: current_dts,
                    cts_offset: cts,
                    duration,
                    is_sync,
                });
                sample_offset_in_chunk += size as u64;
                current_dts = current_dts.saturating_add(duration as i64);
                sample_idx_global += 1;
            }
            if sample_idx_global >= total_samples {
                break;
            }
        }
        let _ = chunk_count;
        Ok(SampleIndex {
            track_id: TrackId(0),
            timescale,
            duration: current_dts,
            samples,
        })
    }

    fn total_samples_from_stsc(&self) -> usize {
        let mut total = 0usize;
        for i in 0..self.stco.len() {
            let chunk_no = (i + 1) as u32;
            total = total.saturating_add(self.samples_per_chunk_for(chunk_no) as usize);
        }
        total
    }

    /// Look up the samples-per-chunk count for the given 1-based chunk
    /// number based on the `stsc` run-length entries. Returns 0 if the
    /// table is empty or the chunk number falls before the first entry.
    fn samples_per_chunk_for(&self, chunk_no: u32) -> u32 {
        // `stsc` entries are run-length encoded: each `(first_chunk,
        // samples_per_chunk, sample_description_index)` row applies from
        // `first_chunk` until the next entry's `first_chunk` (exclusive),
        // and the last entry extends to the end of the chunk list.
        for window in self.stsc.windows(2) {
            let (first, samples_per_chunk, _) = window[0];
            let (next_first, _, _) = window[1];
            if chunk_no >= first && chunk_no < next_first {
                return samples_per_chunk;
            }
        }
        if let Some((first, samples_per_chunk, _)) = self.stsc.last().copied() {
            if chunk_no >= first {
                return samples_per_chunk;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_table_builds_simple_index() {
        let st = SampleTable {
            stts: vec![(3, 33)],
            ctts: vec![],
            stss: Some(vec![1]),
            stsc: vec![(1, 3, 1)],
            stsz_default: 0,
            stsz_sizes: vec![100, 50, 50],
            stco: vec![1024],
        };
        let idx = st.build_index(1000).expect("idx");
        assert_eq!(idx.samples.len(), 3);
        assert_eq!(idx.samples[0].offset, 1024);
        assert_eq!(idx.samples[1].offset, 1124);
        assert_eq!(idx.samples[2].offset, 1174);
        assert_eq!(idx.samples[0].dts, 0);
        assert_eq!(idx.samples[1].dts, 33);
        assert_eq!(idx.samples[2].dts, 66);
        assert!(idx.samples[0].is_sync);
        assert!(!idx.samples[1].is_sync);
    }

    #[test]
    fn missing_stss_marks_all_sync() {
        let st = SampleTable {
            stts: vec![(2, 1024)],
            ctts: vec![],
            stss: None,
            stsc: vec![(1, 2, 1)],
            stsz_default: 0,
            stsz_sizes: vec![64, 64],
            stco: vec![100],
        };
        let idx = st.build_index(48000).expect("idx");
        assert!(idx.samples.iter().all(|s| s.is_sync));
    }

    #[test]
    fn seek_to_dts_walks_back_to_sync_sample() {
        let mut idx = SampleIndex {
            track_id: TrackId(1),
            timescale: 1000,
            duration: 0,
            samples: vec![
                SampleIndexEntry {
                    offset: 0,
                    size: 1,
                    dts: 0,
                    cts_offset: 0,
                    duration: 33,
                    is_sync: true,
                },
                SampleIndexEntry {
                    offset: 1,
                    size: 1,
                    dts: 33,
                    cts_offset: 0,
                    duration: 33,
                    is_sync: false,
                },
                SampleIndexEntry {
                    offset: 2,
                    size: 1,
                    dts: 66,
                    cts_offset: 0,
                    duration: 33,
                    is_sync: false,
                },
                SampleIndexEntry {
                    offset: 3,
                    size: 1,
                    dts: 100,
                    cts_offset: 0,
                    duration: 33,
                    is_sync: true,
                },
            ],
        };
        idx.duration = 132;
        assert_eq!(idx.seek_to_dts(80), Some(0));
        assert_eq!(idx.seek_to_dts(100), Some(3));
        assert_eq!(idx.seek_to_dts(150), Some(3));
        assert_eq!(idx.seek_to_dts(0), Some(0));
    }
}
