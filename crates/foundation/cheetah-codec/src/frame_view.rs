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

/// Supported payload views that can be produced from a canonical `AVFrame`.
///
/// `Native` returns the payload unchanged; `AnnexB` and `H26xLengthPrefixed` convert
/// H.26x NALUs; `Adts` wraps AAC raw frames with an ADTS header.
///
/// 可从标准 `AVFrame` 生成的支持负载视图。
///
/// `Native` 返回原负载；`AnnexB` 和 `H26xLengthPrefixed` 转换 H.26x NALU；
/// `Adts` 用 ADTS 头包装 AAC 原始帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameViewKind {
    Native,
    AnnexB,
    Avcc,
    H26xLengthPrefixed,
    Adts,
}

/// Lazy cache for the different payload views of a single frame.
///
/// Each view is computed once on first access and then reused. This avoids repeated
/// format conversion while keeping the canonical `AVFrame` payload untouched.
///
/// 单帧不同 payload 视图的惰性缓存。
///
/// 每种视图首次访问时计算并复用。避免重复格式转换，同时保持标准 `AVFrame` 负载不变。
#[derive(Debug, Default)]
pub struct FrameViewCache {
    annexb: LazyCell<Bytes>,
    h26x_length_prefixed: LazyCell<Bytes>,
    adts: LazyCell<Bytes>,
}

impl FrameViewCache {
    /// Return the payload in its native canonical format.
    ///
    /// 以原生标准格式返回负载。
    pub fn native(frame: &AVFrame) -> Bytes {
        frame.payload.clone()
    }

    /// Return an Annex-B view of the frame payload, converting if necessary.
    ///
    /// 返回帧负载的 Annex-B 视图，必要时转换。
    pub fn annexb(&self, frame: &AVFrame) -> Bytes {
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            return frame.payload.clone();
        }
        self.annexb
            .get_or_init(|| convert_to_annexb(frame.payload.clone()))
            .clone()
    }

    /// Alias for `h26x_length_prefixed` for AVCC compatibility.
    ///
    /// `h26x_length_prefixed` 的别名，用于 AVCC 兼容。
    pub fn avcc(&self, frame: &AVFrame) -> Bytes {
        self.h26x_length_prefixed(frame)
    }

    /// Return an H.26x length-prefixed view of the frame payload, converting if necessary.
    ///
    /// 返回帧负载的 H.26x 长度前缀视图，必要时转换。
    pub fn h26x_length_prefixed(&self, frame: &AVFrame) -> Bytes {
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            return frame.payload.clone();
        }
        self.h26x_length_prefixed
            .get_or_init(|| convert_to_h26x_length_prefixed(frame.payload.clone()))
            .clone()
    }

    /// Return an ADTS-wrapped view for AAC audio, if an ASC is provided.
    ///
    /// 若提供 ASC，返回 AAC 音频的 ADTS 包装视图。
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

/// Convert an H.26x payload to length-prefixed form.
///
/// 将 H.26x 负载转换为长度前缀形式。
pub fn h26x_length_prefixed_from_payload(payload: Bytes) -> Bytes {
    convert_to_h26x_length_prefixed(payload)
}

/// Convert an H.26x payload to Annex-B form.
///
/// 将 H.26x 负载转换为 Annex-B 形式。
pub fn annexb_from_payload(payload: Bytes) -> Bytes {
    convert_to_annexb(payload)
}

/// Convert a length-prefixed H.26x payload to Annex-B.
///
/// 将长度前缀 H.26x 负载转换为 Annex-B。
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

/// Convert an Annex-B or raw H.26x payload to length-prefixed form.
///
/// 将 Annex-B 或原始 H.26x 负载转换为长度前缀形式。
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

/// Heuristic: check whether the payload is already length-prefixed H.26x.
///
/// 启发式检查：负载是否已经是长度前缀 H.26x。
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

/// Heuristic: check whether the payload starts with an Annex-B start code.
///
/// 启发式检查：负载是否以 Annex-B 起始码开头。
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
