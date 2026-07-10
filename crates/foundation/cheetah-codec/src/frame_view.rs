use crate::prelude::*;

#[cfg(not(feature = "std"))]
// SAFETY: In no_std builds, `OnceCell` (cell-based, non-thread-safe) is used
// instead of `OnceLock`. This makes `FrameViewCache` `!Sync` and `!Send`.
// This is only safe when `FrameViewCache` is accessed from a single thread.
// In no_std / WASM targets the runtime is typically single-threaded;
// if multi-threaded access is required, enable the `std` feature to get
// `OnceLock` (which is `Sync` + `Send`) instead.
// NOTE: Storing `FrameViewCache` in `Arc`, `static`, or any `Send`/`Sync`
// boundary will fail to compile in `no_std` mode. This is intentional.
use core::cell::OnceCell as LazyCell;
#[cfg(feature = "std")]
use std::sync::OnceLock as LazyCell;

use crate::video::split_annexb_units;
use bytes::Bytes;

use crate::audio::{adts_wrap, AacAudioSpecificConfig};
use crate::frame::AVFrame;
use crate::track::CodecId;

/// `FrameViewKind` enumeration.
/// `FrameViewKind` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameViewKind {
    /// `Native` variant.
    /// `Native` 变体.
    Native,
    /// `AnnexB` variant.
    /// `AnnexB` 变体.
    AnnexB,
    /// `Avcc` variant.
    /// `Avcc` 变体.
    Avcc,
    /// `H26xLengthPrefixed` variant.
    /// `H26xLengthPrefixed` 变体.
    H26xLengthPrefixed,
    /// `Adts` variant.
    /// `Adts` 变体.
    Adts,
}

/// `FrameViewCache` data structure.
/// `FrameViewCache` 数据结构.
#[derive(Debug, Default)]
pub struct FrameViewCache {
    /// `annexb` field.
    /// `annexb` 字段.
    annexb: LazyCell<Bytes>,
    /// `h26x_length_prefixed` field.
    /// `h26x_length_prefixed` 字段.
    h26x_length_prefixed: LazyCell<Bytes>,
    /// `adts` field.
    /// `adts` 字段.
    adts: LazyCell<Bytes>,
}

impl FrameViewCache {
    /// `native` function.
    /// `native` 函数.
    pub fn native(frame: &AVFrame) -> Bytes {
        frame.payload.clone()
    }

    /// `annexb` function.
    /// `annexb` 函数.
    pub fn annexb(&self, frame: &AVFrame) -> Bytes {
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            return frame.payload.clone();
        }
        self.annexb
            .get_or_init(|| convert_to_annexb(frame.payload.clone()))
            .clone()
    }

    /// `avcc` function.
    /// `avcc` 函数.
    pub fn avcc(&self, frame: &AVFrame) -> Bytes {
        self.h26x_length_prefixed(frame)
    }

    /// `h26x_length_prefixed` function.
    /// `h26x_length_prefixed` 函数.
    pub fn h26x_length_prefixed(&self, frame: &AVFrame) -> Bytes {
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            return frame.payload.clone();
        }
        self.h26x_length_prefixed
            .get_or_init(|| convert_to_h26x_length_prefixed(frame.payload.clone()))
            .clone()
    }

    /// `adts` function.
    /// `adts` 函数.
    pub fn adts(&self, frame: &AVFrame, asc: Option<AacAudioSpecificConfig>) -> Bytes {
        if frame.codec != CodecId::AAC {
            return frame.payload.clone();
        }
        let Some(asc) = asc else {
            return frame.payload.clone();
        };
        self.adts
            .get_or_init(|| adts_wrap(&frame.payload, asc))
            .clone()
    }
}

/// `h26x_length_prefixed_from_payload` function.
/// `h26x_length_prefixed_from_payload` 函数.
pub fn h26x_length_prefixed_from_payload(payload: Bytes) -> Bytes {
    convert_to_h26x_length_prefixed(payload)
}

/// `annexb_from_payload` function.
/// `annexb_from_payload` 函数.
pub fn annexb_from_payload(payload: Bytes) -> Bytes {
    convert_to_annexb(payload)
}

fn convert_to_annexb(payload: Bytes) -> Bytes {
    if looks_like_annexb(&payload) {
        return payload;
    }

    let mut out = Vec::with_capacity(payload.len() + 32);
    let mut cursor = payload.as_ref();
    while cursor.len() >= 4 {
        let size = u32::from_be_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
        cursor = &cursor[4..];
        if size == 0 || cursor.len() < size {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&cursor[..size]);
        cursor = &cursor[size..];
    }
    if out.is_empty() {
        payload
    } else {
        Bytes::from(out)
    }
}

fn convert_to_h26x_length_prefixed(payload: Bytes) -> Bytes {
    if payload.is_empty() {
        return payload;
    }
    if looks_like_h26x_length_prefixed(&payload) {
        return payload;
    }

    if looks_like_annexb(&payload) {
        let mut out = Vec::with_capacity(payload.len() + 16);
        for unit in split_annexb_units(payload.as_ref()) {
            let Ok(size_u32) = u32::try_from(unit.len()) else {
                continue;
            };
            out.extend_from_slice(&size_u32.to_be_bytes());
            out.extend_from_slice(unit);
        }
        if !out.is_empty() {
            return Bytes::from(out);
        }
    }

    let Ok(size_u32) = u32::try_from(payload.len()) else {
        return payload;
    };
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&size_u32.to_be_bytes());
    out.extend_from_slice(&payload);
    Bytes::from(out)
}

fn looks_like_h26x_length_prefixed(data: &[u8]) -> bool {
    let mut cursor = data;
    let mut unit_count = 0usize;
    while cursor.len() >= 4 {
        let size = u32::from_be_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
        cursor = &cursor[4..];
        if size == 0 || cursor.len() < size {
            return false;
        }
        cursor = &cursor[size..];
        unit_count += 1;
    }
    unit_count > 0 && cursor.is_empty()
}

fn looks_like_annexb(data: &[u8]) -> bool {
    if data.len() < 3 {
        return false;
    }
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{AVFrame, FrameFormat, MediaKind, Timebase, TrackId};

    use super::*;

    fn aac_frame(payload: &'static [u8]) -> AVFrame {
        AVFrame::new(
            TrackId(1),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(payload),
        )
    }

    #[test]
    fn adts_conversion_is_not_poisoned_by_missing_asc_first() {
        let cache = FrameViewCache::default();
        let frame = aac_frame(&[1, 2, 3, 4]);
        let raw = cache.adts(&frame, None);
        assert_eq!(raw, Bytes::from_static(&[1, 2, 3, 4]));

        let asc = AacAudioSpecificConfig {
            audio_object_type: 2,
            sampling_frequency_index: 4,
            channel_configuration: 2,
        };
        let wrapped = cache.adts(&frame, Some(asc));
        assert!(wrapped.len() > frame.payload.len());
        assert_eq!(&wrapped[0..2], &[0xff, 0xf1]);
    }

    #[test]
    fn h26x_length_prefixed_wraps_single_raw_nal() {
        let out = h26x_length_prefixed_from_payload(Bytes::from_static(&[0x21, 0x16, 0xc5, 0x23]));

        assert_eq!(out.as_ref(), &[0, 0, 0, 4, 0x21, 0x16, 0xc5, 0x23]);
    }

    #[test]
    fn h26x_length_prefixed_keeps_existing_length_prefixed_payload() {
        let payload = Bytes::from_static(&[0, 0, 0, 3, 0x65, 0xaa, 0xbb]);
        let out = h26x_length_prefixed_from_payload(payload.clone());

        assert_eq!(out, payload);
    }
}
