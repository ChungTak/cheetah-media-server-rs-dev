//! Fragmented MP4 (fMP4) muxer for HLS segment generation.
//!
//! fMP4 复用器，用于 HLS 分片生成。
//! 包装 `cheetah_codec::Fmp4Muxer` 并维护 HLS 专用接口：毫秒时间戳、
//! `Fmp4TrackDesc`/`Fmp4Sample` 类型，以及 init/segment/part 三种输出。

use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind};

/// fMP4 track description (HLS-specific wrapper).
///
/// fMP4 轨道描述（HLS 专用包装）。
#[derive(Debug, Clone)]
pub struct Fmp4TrackDesc {
    pub track_id: u32,
    pub codec: CodecId,
    pub media_kind: MediaKind,
    pub timescale: u32,
    /// Codec-specific extradata (avcC for H264, hvcC for H265, esds for AAC, etc.)
    ///
    /// 编解码器专用 extradata（H264 为 avcC，H265 为 hvcC，AAC 为 esds 等）。
    pub extradata: Bytes,
    pub width: u16,
    pub height: u16,
    pub sample_rate: u32,
    pub channels: u8,
}

/// A single sample to be written into a media segment.
///
/// 要写入媒体分段的单个 sample。
#[derive(Debug, Clone)]
pub struct Fmp4Sample {
    pub track_id: u32,
    pub pts_ms: u64,
    pub dts_ms: u64,
    pub is_keyframe: bool,
    pub data: Bytes,
}

/// fMP4 muxer for HLS — delegates to `cheetah_codec::Fmp4Muxer`.
///
/// HLS 用 fMP4 复用器 — 委托给 `cheetah_codec::Fmp4Muxer`。
pub struct Fmp4Muxer {
    inner: cheetah_codec::Fmp4Muxer,
}

impl Fmp4Muxer {
    /// Create a muxer for the given track descriptions.
    ///
    /// Converts each `Fmp4TrackDesc` into the codec's `TrackInfo` and enables `styp`
    /// (segment type) boxes for full segments.
    ///
    /// 根据给定轨道描述创建复用器。
    /// 将每个 `Fmp4TrackDesc` 转换为 codec 的 `TrackInfo`，并启用完整分段的 `styp` box。
    pub fn new(tracks: Vec<Fmp4TrackDesc>) -> Self {
        let track_infos: Vec<_> = tracks.iter().map(desc_to_track_info).collect();
        let inner = cheetah_codec::Fmp4Muxer::new(
            cheetah_codec::Fmp4MuxerConfig {
                include_styp: true,
                include_sidx: false,
                ..Default::default()
            },
            &track_infos,
        );
        Self { inner }
    }

    /// Generate (or return cached) init segment: ftyp + moov.
    ///
    /// 生成（或返回缓存的）init 分段：ftyp + moov。
    pub fn init_segment(&mut self) -> Bytes {
        let events = self.inner.init_segment();
        match &events[0] {
            cheetah_codec::Fmp4MuxEvent::InitSegment(data) => data.clone(),
            _ => Bytes::new(),
        }
    }

    /// Generate a media segment: styp + moof + mdat.
    ///
    /// 生成媒体分段：styp + moof + mdat。
    pub fn write_segment(&mut self, samples: &[Fmp4Sample]) -> Bytes {
        let mux_samples = self.convert_samples(samples);
        let events = self.inner.write_segment(&mux_samples);
        match events.first() {
            Some(cheetah_codec::Fmp4MuxEvent::MediaSegment { data, .. }) => data.clone(),
            _ => Bytes::new(),
        }
    }

    /// Generate a partial segment (part) for LL-HLS: moof + mdat only (no styp).
    ///
    /// 为 LL-HLS 生成部分分段（part）：仅 moof + mdat（无 styp）。
    pub fn write_part(&mut self, samples: &[Fmp4Sample]) -> Bytes {
        let mux_samples = self.convert_samples(samples);
        let events = self.inner.write_part(&mux_samples);
        match events.first() {
            Some(cheetah_codec::Fmp4MuxEvent::MediaSegment { data, .. }) => data.clone(),
            _ => Bytes::new(),
        }
    }

    /// Current sequence number (incremented per segment).
    ///
    /// 当前序列号（每生成一个 segment 递增）。
    pub fn sequence_number(&self) -> u32 {
        self.inner.sequence_number()
    }

    /// Convert HLS samples into codec samples (ms -> microseconds).
    ///
    /// 将 HLS sample 转换为 codec sample（毫秒转微秒）。
    fn convert_samples(&self, samples: &[Fmp4Sample]) -> Vec<cheetah_codec::Fmp4MuxSample> {
        samples
            .iter()
            .map(|s| cheetah_codec::Fmp4MuxSample {
                track_id: s.track_id,
                dts_us: s.dts_ms as i64 * 1000,
                pts_us: s.pts_ms as i64 * 1000,
                is_keyframe: s.is_keyframe,
                data: s.data.clone(),
            })
            .collect()
    }
}

/// Convert an `Fmp4TrackDesc` into a codec `TrackInfo` with proper extradata.
///
/// The `codec_config` is stored inside the codec-specific `CodecExtradata` variant.
/// For H.264/H.265 this is the raw avcC/hvcC box; for AAC it is the AudioSpecificConfig.
///
/// 将 `Fmp4TrackDesc` 转换为 codec `TrackInfo` 并设置正确的 extradata。
/// codec 配置存放在对应 `CodecExtradata` 变体中；
/// H.264/H.265 为原始 avcC/hvcC box，AAC 为 AudioSpecificConfig。
fn desc_to_track_info(desc: &Fmp4TrackDesc) -> cheetah_codec::TrackInfo {
    use cheetah_codec::track::{CodecExtradata, TrackId};

    let mut t = cheetah_codec::TrackInfo::new(
        TrackId(desc.track_id),
        desc.media_kind,
        desc.codec,
        desc.timescale,
    );
    t.width = if desc.width > 0 {
        Some(desc.width as u32)
    } else {
        None
    };
    t.height = if desc.height > 0 {
        Some(desc.height as u32)
    } else {
        None
    };
    t.sample_rate = if desc.sample_rate > 0 {
        Some(desc.sample_rate)
    } else {
        None
    };
    t.channels = if desc.channels > 0 {
        Some(desc.channels)
    } else {
        None
    };

    // Set extradata based on codec
    t.extradata = match desc.codec {
        CodecId::H264 => CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(desc.extradata.clone()),
        },
        CodecId::H265 => CodecExtradata::H265 {
            vps: vec![],
            sps: vec![],
            pps: vec![],
            hvcc: Some(desc.extradata.clone()),
        },
        CodecId::AAC => CodecExtradata::AAC {
            asc: desc.extradata.clone(),
        },
        CodecId::Opus => CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: Some(desc.extradata.clone()),
        },
        CodecId::AV1 => CodecExtradata::AV1 {
            sequence_header: None,
            codec_config: Some(desc.extradata.clone()),
        },
        CodecId::VP8 => CodecExtradata::VP8 {
            config: Some(desc.extradata.clone()),
        },
        CodecId::VP9 => CodecExtradata::VP9 {
            config: Some(desc.extradata.clone()),
        },
        CodecId::MP3 => CodecExtradata::MP3 { side_info: None },
        _ => CodecExtradata::None,
    };
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_h264_track() -> Fmp4TrackDesc {
        Fmp4TrackDesc {
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
        }
    }

    fn make_aac_track() -> Fmp4TrackDesc {
        Fmp4TrackDesc {
            track_id: 2,
            codec: CodecId::AAC,
            media_kind: MediaKind::Audio,
            timescale: 44100,
            extradata: Bytes::from_static(&[0x12, 0x10]),
            width: 0,
            height: 0,
            sample_rate: 44100,
            channels: 2,
        }
    }

    #[test]
    fn init_segment_starts_with_ftyp() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track(), make_aac_track()]);
        let init = muxer.init_segment();
        assert!(init.len() > 8);
        assert_eq!(&init[4..8], b"ftyp");
    }

    #[test]
    fn init_segment_contains_moov() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        let init = muxer.init_segment();
        let moov_pos = init.windows(4).position(|w| w == b"moov");
        assert!(moov_pos.is_some());
    }

    #[test]
    fn init_segment_is_cached() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        let a = muxer.init_segment();
        let b = muxer.init_segment();
        assert_eq!(a, b);
    }

    #[test]
    fn media_segment_starts_with_styp() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        let samples = vec![Fmp4Sample {
            track_id: 1,
            pts_ms: 0,
            dts_ms: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65, 0xAA, 0xBB]),
        }];
        let seg = muxer.write_segment(&samples);
        assert_eq!(&seg[4..8], b"styp");
    }

    #[test]
    fn media_segment_contains_moof_and_mdat() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        let samples = vec![Fmp4Sample {
            track_id: 1,
            pts_ms: 0,
            dts_ms: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65, 0xAA, 0xBB]),
        }];
        let seg = muxer.write_segment(&samples);
        assert!(seg.windows(4).any(|w| w == b"moof"));
        assert!(seg.windows(4).any(|w| w == b"mdat"));
    }

    #[test]
    fn part_has_moof_but_no_styp() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        let samples = vec![Fmp4Sample {
            track_id: 1,
            pts_ms: 0,
            dts_ms: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65]),
        }];
        let part = muxer.write_part(&samples);
        assert_eq!(&part[4..8], b"moof");
        assert!(!part.windows(4).any(|w| w == b"styp"));
    }

    #[test]
    fn sequence_number_increments() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track()]);
        assert_eq!(muxer.sequence_number(), 0);
        let samples = vec![Fmp4Sample {
            track_id: 1,
            pts_ms: 0,
            dts_ms: 0,
            is_keyframe: true,
            data: Bytes::from_static(&[0x65]),
        }];
        muxer.write_segment(&samples);
        assert_eq!(muxer.sequence_number(), 1);
        muxer.write_part(&samples);
        assert_eq!(muxer.sequence_number(), 2);
    }

    #[test]
    fn multi_track_segment() {
        let mut muxer = Fmp4Muxer::new(vec![make_h264_track(), make_aac_track()]);
        let samples = vec![
            Fmp4Sample {
                track_id: 1,
                pts_ms: 33,
                dts_ms: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0x65, 0x01]),
            },
            Fmp4Sample {
                track_id: 2,
                pts_ms: 0,
                dts_ms: 0,
                is_keyframe: true,
                data: Bytes::from_static(&[0xFF, 0xF1, 0x50]),
            },
        ];
        let seg = muxer.write_segment(&samples);
        assert!(seg.windows(4).any(|w| w == b"traf"));
    }
}
