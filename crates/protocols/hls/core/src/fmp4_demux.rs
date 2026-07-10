//! Fragmented MP4 (fMP4) demuxer for HLS pull scenarios.
//!
//! fMP4（Fragmented MP4）解复用器，用于 HLS 拉流场景。
//! 包装 `cheetah_codec::Fmp4Demuxer` 并维护 HLS 专用接口：毫秒时间戳、
//! 独立的 init 分段解析与 segment 分段解析。

use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind};

/// Track info extracted from an init segment (ftyp + moov).
///
/// 从 init 分段（ftyp + moov）中提取的轨道信息。
#[derive(Debug, Clone)]
pub struct Fmp4DemuxTrack {
    pub track_id: u32,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub timescale: u32,
    pub extradata: Bytes,
}

/// Events produced by the fMP4 demuxer.
///
/// fMP4 解复用器产生的事件。
#[derive(Debug, Clone)]
pub enum Fmp4DemuxEvent {
    /// Track metadata discovered from the init segment.
    ///
    /// 从 init 分段发现的轨道元数据。
    TrackInfo(Vec<Fmp4DemuxTrack>),
    /// A decoded media frame with timing and keyframe information.
    ///
    /// 携带时间戳与关键帧信息的解码媒体帧。
    Frame {
        track_id: u32,
        media_kind: MediaKind,
        pts_ms: u64,
        dts_ms: u64,
        keyframe: bool,
        data: Bytes,
    },
}

/// Error from fMP4 demuxing.
///
/// fMP4 解复用错误。
#[derive(Debug, Clone)]
pub enum Fmp4DemuxError {
    /// A box could not be parsed (size/type mismatch).
    ///
    /// Box 解析失败（大小/类型不匹配）。
    InvalidBox,
    /// No `moov` box found in the init segment.
    ///
    /// init 分段中未找到 `moov` box。
    NoMoov,
    /// No `mdat` box found in the media segment.
    ///
    /// 媒体分段中未找到 `mdat` box。
    NoMdat,
    /// `parse_segment` was called before `parse_init`.
    ///
    /// 在 `parse_init` 之前调用了 `parse_segment`。
    InitNotParsed,
}

/// fMP4 demuxer state — delegates to `cheetah_codec::Fmp4Demuxer`.
///
/// fMP4 解复用器状态 — 委托给 `cheetah_codec::Fmp4Demuxer`。
pub struct Fmp4Demuxer {
    inner: cheetah_codec::Fmp4Demuxer,
    init_parsed: bool,
}

impl Fmp4Demuxer {
    /// Create a new demuxer with default codec configuration.
    ///
    /// 使用默认编解码器配置创建新的解复用器。
    pub fn new() -> Self {
        Self {
            inner: cheetah_codec::Fmp4Demuxer::new(cheetah_codec::Fmp4DemuxerConfig::default()),
            init_parsed: false,
        }
    }

    /// Parse init segment (ftyp + moov) to extract track info.
    ///
    /// The codec-level demuxer emits track metadata once the moov box is seen.
    /// We translate the raw track descriptors into the HLS-specific `Fmp4DemuxTrack`.
    ///
    /// 解析 init 分段（ftyp + moov）以提取轨道信息。
    /// 当看到 moov box 时，codec 层解复用器会输出轨道元数据；
    /// 我们将其转换为 HLS 专用的 `Fmp4DemuxTrack`。
    pub fn parse_init(&mut self, data: &[u8]) -> Result<Vec<Fmp4DemuxEvent>, Fmp4DemuxError> {
        let events = self.inner.push(data);
        let mut result = Vec::new();
        let mut found_tracks = false;
        for event in events {
            if let cheetah_codec::Fmp4DemuxEvent::TrackInfo(tracks) = event {
                found_tracks = true;
                result.push(Fmp4DemuxEvent::TrackInfo(
                    tracks.into_iter().map(convert_track).collect(),
                ));
            }
        }
        if !found_tracks {
            return Err(Fmp4DemuxError::NoMoov);
        }
        self.init_parsed = true;
        Ok(result)
    }

    /// Parse media segment (moof + mdat) to extract frames.
    ///
    /// Requires that the init segment has already been parsed. The actual moof/mdat
    /// parsing is done against the track list from the init segment.
    ///
    /// 解析媒体分段（moof + mdat）以提取帧。
    /// 要求已经解析过 init 分段；实际的 moof/mdat 解析基于 init 中的轨道列表。
    pub fn parse_segment(&self, data: &[u8]) -> Result<Vec<Fmp4DemuxEvent>, Fmp4DemuxError> {
        if !self.init_parsed {
            return Err(Fmp4DemuxError::InitNotParsed);
        }
        let events = parse_segment_with_tracks(data, self.inner.tracks());
        Ok(events)
    }
}

impl Default for Fmp4Demuxer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a codec-level track descriptor into the HLS wrapper type.
///
/// 将 codec 层轨道描述符转换为 HLS 包装类型。
fn convert_track(t: cheetah_codec::Fmp4DemuxTrack) -> Fmp4DemuxTrack {
    Fmp4DemuxTrack {
        track_id: t.track_id,
        codec: t.codec,
        media_kind: t.media_kind,
        timescale: t.timescale,
        extradata: t.extradata,
    }
}

/// Parse a media segment given known tracks (for the `&self` `parse_segment` API).
///
/// Walks the top-level boxes to locate the `moof` and `mdat` boxes, then delegates
/// per-fragment parsing to `parse_traf_boxes`.
///
/// 在已知轨道列表的前提下解析媒体分段（供 `parse_segment` 调用）。
/// 遍历顶层 box 定位 `moof` 与 `mdat`，再按每个 fragment 委托给 `parse_traf_boxes`。
fn parse_segment_with_tracks(
    data: &[u8],
    tracks: &[cheetah_codec::Fmp4DemuxTrack],
) -> Vec<Fmp4DemuxEvent> {
    // Find moof and mdat boxes
    let mut offset = 0;
    let mut moof_data: Option<&[u8]> = None;
    let mut mdat_data: Option<&[u8]> = None;

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
        let box_type = &data[offset + 4..offset + 8];
        match box_type {
            b"moof" => moof_data = Some(&data[offset + 8..offset + size]),
            b"mdat" => mdat_data = Some(&data[offset + 8..offset + size]),
            _ => {}
        }
        offset += size;
    }

    let Some(moof) = moof_data else {
        return Vec::new();
    };
    let Some(mdat) = mdat_data else {
        return Vec::new();
    };
    let moof_box_size = moof.len() + 8;

    let mut events = Vec::new();
    parse_traf_boxes(moof, mdat, moof_box_size, tracks, &mut events);
    events
}

/// Parse every `traf` box inside a `moof`.
///
/// A `moof` may contain multiple `traf` boxes (one per track). This function iterates
/// over all of them and forwards each to `parse_single_traf`.
///
/// 解析 `moof` 中的所有 `traf` box。
/// 一个 `moof` 可能包含多个 `traf`（每轨道一个），此处遍历并逐个转发给 `parse_single_traf`。
fn parse_traf_boxes(
    moof: &[u8],
    mdat: &[u8],
    moof_box_size: usize,
    tracks: &[cheetah_codec::Fmp4DemuxTrack],
    events: &mut Vec<Fmp4DemuxEvent>,
) {
    let mut offset = 0;
    while offset + 8 <= moof.len() {
        let size = u32::from_be_bytes([
            moof[offset],
            moof[offset + 1],
            moof[offset + 2],
            moof[offset + 3],
        ]) as usize;
        if size < 8 || offset + size > moof.len() {
            break;
        }
        if &moof[offset + 4..offset + 8] == b"traf" {
            parse_single_traf(
                &moof[offset + 8..offset + size],
                mdat,
                moof_box_size,
                tracks,
                events,
            );
        }
        offset += size;
    }
}

/// Parse a single `traf` (track fragment) into frame events.
///
/// The algorithm reads `tfhd` (track id), `tfdt` (base decode time), and `trun`
/// (sample runs) to compute each sample's byte offset within `mdat`, then extracts
/// the bytes and converts decode/compose timestamps to milliseconds.
///
/// 将单个 `traf`（轨道 fragment）解析为帧事件。
/// 算法读取 `tfhd`（轨道 id）、`tfdt`（基准解码时间）和 `trun`（sample run），
/// 计算每个 sample 在 `mdat` 中的字节偏移，提取数据并将 DTS/CTS 转换为毫秒。
fn parse_single_traf(
    data: &[u8],
    mdat: &[u8],
    moof_box_size: usize,
    tracks: &[cheetah_codec::Fmp4DemuxTrack],
    events: &mut Vec<Fmp4DemuxEvent>,
) {
    let mut track_id = 0u32;
    let mut base_decode_time: u64 = 0;
    let mut data_offset: i32 = 0;
    let mut samples: Vec<(u32, u32, u32, i32)> = Vec::new(); // (duration, size, flags, cts)

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
        let box_type = &data[offset + 4..offset + 8];
        let inner = &data[offset + 8..offset + size];
        match box_type {
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
                let (off, s) = parse_trun_compat(inner);
                data_offset = off;
                samples = s;
            }
            _ => {}
        }
        offset += size;
    }

    let track = match tracks.iter().find(|t| t.track_id == track_id) {
        Some(t) => t,
        None => return,
    };

    let mdat_base = if data_offset > 0 {
        (data_offset as usize).saturating_sub(moof_box_size + 8)
    } else {
        0
    };

    let mut mdat_offset = mdat_base;
    let mut current_dts = base_decode_time;

    for (duration, size, flags, cts_offset) in &samples {
        let size = *size as usize;
        if mdat_offset + size > mdat.len() {
            break;
        }
        let frame_data = &mdat[mdat_offset..mdat_offset + size];
        let dts_ms = current_dts * 1000 / track.timescale as u64;
        let pts_ticks = current_dts as i64 + *cts_offset as i64;
        let pts_ms = (pts_ticks as u64) * 1000 / track.timescale as u64;
        let keyframe = is_keyframe_flags(*flags);

        events.push(Fmp4DemuxEvent::Frame {
            track_id,
            media_kind: track.media_kind,
            pts_ms,
            dts_ms,
            keyframe,
            data: Bytes::copy_from_slice(frame_data),
        });

        mdat_offset += size;
        current_dts += *duration as u64;
    }
}

/// Parse an `trun` box in a compatibility manner.
///
/// `trun` carries per-sample flags for data offset, duration, size, flags, and CTS.
/// The returned tuple is `(data_offset, Vec<(duration, size, flags, cts)>)`.
///
/// 以兼容方式解析 `trun` box。
/// `trun` 携带每个 sample 的数据偏移、时长、大小、标志和 CTS 标志。
/// 返回 `(data_offset, Vec<(duration, size, flags, cts)>)`。
fn parse_trun_compat(data: &[u8]) -> (i32, Vec<(u32, u32, u32, i32)>) {
    if data.len() < 8 {
        return (0, Vec::new());
    }
    let flags = u32::from_be_bytes([0, data[1], data[2], data[3]]);
    let sample_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;

    let has_data_offset = flags & 0x000001 != 0;
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

    let mut samples = Vec::with_capacity(sample_count.min(4096));
    for _ in 0..sample_count {
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
        samples.push((duration, size, sample_flags, cts_offset));
    }
    (data_offset, samples)
}

/// Determine whether sample flags indicate a keyframe.
///
/// For fMP4, the upper two bits of `sample_flags` give `sample_depends_on`:
/// 2 means this sample does not depend on others (sync sample). When `depends_on` is 0,
/// the `sample_is_non_sync_sample` bit must be 0 for it to be a keyframe.
///
/// 判断 sample flags 是否表示关键帧。
/// 对 fMP4，`sample_flags` 的高两位表示 `sample_depends_on`：
/// 2 表示不依赖其他 sample（同步样本）。若 `depends_on` 为 0，则 `sample_is_non_sync_sample` 位为 0 时才是关键帧。
fn is_keyframe_flags(flags: u32) -> bool {
    let depends_on = (flags >> 24) & 0x03;
    let is_non_sync = (flags >> 16) & 0x01;
    depends_on == 2 || (depends_on == 0 && is_non_sync == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmp4_mux::{Fmp4Muxer, Fmp4Sample, Fmp4TrackDesc};

    #[test]
    fn roundtrip_init_segment() {
        let tracks = vec![Fmp4TrackDesc {
            track_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Video,
            timescale: 90000,
            extradata: Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ]),
            width: 1920,
            height: 1080,
            sample_rate: 0,
            channels: 0,
        }];
        let mut muxer = Fmp4Muxer::new(tracks);
        let init = muxer.init_segment();

        let mut demuxer = Fmp4Demuxer::new();
        let events = demuxer.parse_init(&init).unwrap();
        assert_eq!(events.len(), 1);
        if let Fmp4DemuxEvent::TrackInfo(tracks) = &events[0] {
            assert_eq!(tracks.len(), 1);
            assert_eq!(tracks[0].codec, CodecId::H264);
        }
    }

    #[test]
    fn roundtrip_media_segment() {
        let tracks = vec![Fmp4TrackDesc {
            track_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Video,
            timescale: 90000,
            extradata: Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ]),
            width: 1920,
            height: 1080,
            sample_rate: 0,
            channels: 0,
        }];
        let mut muxer = Fmp4Muxer::new(tracks);
        let init = muxer.init_segment();

        let samples = vec![
            Fmp4Sample {
                track_id: 1,
                pts_ms: 33,
                dts_ms: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01, 0x02]),
            },
            Fmp4Sample {
                track_id: 1,
                pts_ms: 66,
                dts_ms: 33,
                is_keyframe: false,
                data: Bytes::from_static(&[0x41, 0x03]),
            },
        ];
        let seg = muxer.write_segment(&samples);

        let mut demuxer = Fmp4Demuxer::new();
        demuxer.parse_init(&init).unwrap();
        let events = demuxer.parse_segment(&seg).unwrap();

        let frames: Vec<_> = events
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
}
