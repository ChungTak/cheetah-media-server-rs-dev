//! Fragmented MP4 (fMP4) demuxer for HLS pull scenarios.
//!
//! This is a thin wrapper over `cheetah_codec::Fmp4Demuxer` that preserves the
//! HLS-specific API (ms-based timestamps, separate parse_init/parse_segment).

use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind};

/// Track info extracted from init segment.
#[derive(Debug, Clone)]
pub struct Fmp4DemuxTrack {
    /// `track_id` field of type `u32`.
    /// `track_id` 字段，类型为 `u32`.
    pub track_id: u32,
    /// `codec` field of type `CodecId`.
    /// `codec` 字段，类型为 `CodecId`.
    pub codec: CodecId,
    /// `media_kind` field of type `MediaKind`.
    /// `media_kind` 字段，类型为 `MediaKind`.
    pub media_kind: MediaKind,
    /// `timescale` field of type `u32`.
    /// `timescale` 字段，类型为 `u32`.
    pub timescale: u32,
    /// `extradata` field of type `Bytes`.
    /// `extradata` 字段，类型为 `Bytes`.
    pub extradata: Bytes,
}

/// Events produced by the fMP4 demuxer.
#[derive(Debug, Clone)]
pub enum Fmp4DemuxEvent {
    /// `TrackInfo` variant.
    /// `TrackInfo` 变体.
    TrackInfo(Vec<Fmp4DemuxTrack>),
    /// `Frame` variant.
    /// `Frame` 变体.
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
#[derive(Debug, Clone)]
pub enum Fmp4DemuxError {
    /// `InvalidBox` variant.
    /// `InvalidBox` 变体.
    InvalidBox,
    /// `NoMoov` variant.
    /// `NoMoov` 变体.
    NoMoov,
    /// `NoMdat` variant.
    /// `NoMdat` 变体.
    NoMdat,
    /// `InitNotParsed` variant.
    /// `InitNotParsed` 变体.
    InitNotParsed,
}

/// fMP4 demuxer state — delegates to `cheetah_codec::Fmp4Demuxer`.
pub struct Fmp4Demuxer {
    /// `inner` field.
    /// `inner` 字段.
    inner: cheetah_codec::Fmp4Demuxer,
    /// `init_parsed` field of type `bool`.
    /// `init_parsed` 字段，类型为 `bool`.
    init_parsed: bool,
}

impl Fmp4Demuxer {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            inner: cheetah_codec::Fmp4Demuxer::new(cheetah_codec::Fmp4DemuxerConfig::default()),
            init_parsed: false,
        }
    }

    /// Parse init segment (ftyp + moov) to extract track info.
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

fn convert_track(t: cheetah_codec::Fmp4DemuxTrack) -> Fmp4DemuxTrack {
    Fmp4DemuxTrack {
        track_id: t.track_id,
        codec: t.codec,
        media_kind: t.media_kind,
        timescale: t.timescale,
        extradata: t.extradata,
    }
}

/// Parse a media segment given known tracks (for the &self parse_segment API).
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
