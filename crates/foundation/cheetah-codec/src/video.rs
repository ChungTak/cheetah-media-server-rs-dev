use crate::prelude::*;
use bytes::Bytes;
use core::mem;

use crate::frame::{AVFrame, FrameFlags, FrameFormat, FrameTimingError};
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId};

#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub units: Vec<Bytes>,
    pub timing: Option<AccessUnitTiming>,
    pub random_access: bool,
    pub parameter_set_requirement: ParameterSetRequirement,
}

impl AccessUnit {
    pub fn from_units(units: Vec<Bytes>) -> Self {
        Self {
            units,
            timing: None,
            random_access: false,
            parameter_set_requirement: ParameterSetRequirement::NotRequired,
        }
    }

    pub fn from_frame_units(
        frame: &AVFrame,
        units: Vec<Bytes>,
        parameter_sets: &ParameterSetCache,
    ) -> Result<Self, AccessUnitBuildError> {
        frame.validate_media_timing()?;
        let random_access = frame.flags.contains(FrameFlags::KEY);
        Ok(Self {
            units,
            timing: Some(AccessUnitTiming::from_frame(frame)),
            random_access,
            parameter_set_requirement: parameter_sets
                .requirement_for_frame(frame.codec, random_access),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessUnitTiming {
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub timebase: Timebase,
}

impl AccessUnitTiming {
    fn from_frame(frame: &AVFrame) -> Self {
        Self {
            pts: frame.pts,
            dts: frame.dts,
            duration: frame.duration,
            timebase: frame.timebase,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterSetRequirement {
    #[default]
    NotRequired,
    RequiredPresent,
    RequiredMissing,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AccessUnitBuildError {
    #[error("invalid frame timing when building access unit: {0}")]
    InvalidTiming(#[from] FrameTimingError),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum LengthPrefixedParseError {
    #[error("incomplete NAL length prefix at byte offset {offset}: remaining {remaining_bytes}")]
    IncompleteLengthPrefix {
        offset: usize,
        remaining_bytes: usize,
    },
    #[error("zero-length NAL unit at byte offset {offset}")]
    ZeroLengthUnit { offset: usize },
    #[error(
        "truncated NAL unit at byte offset {offset}: declared {declared_size} bytes, remaining {remaining_bytes}"
    )]
    TruncatedUnit {
        offset: usize,
        declared_size: usize,
        remaining_bytes: usize,
    },
}

/// Maximum size in bytes for a single cached parameter set NALU.
///
/// Real-world SPS/PPS/VPS are typically under 256 bytes. We allow up to 4 KiB
/// to accommodate unusual encoder configurations while preventing unbounded
/// memory growth from malformed or adversarial input.
pub const PARAMETER_SET_MAX_SIZE: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct ParameterSetCache {
    pub vps: Option<Bytes>,
    pub sps: Option<Bytes>,
    pub pps: Option<Bytes>,
}

impl ParameterSetCache {
    pub fn clear(&mut self) {
        self.vps = None;
        self.sps = None;
        self.pps = None;
    }

    pub fn update_from_annexb(&mut self, codec: CodecId, payload: &[u8]) -> bool {
        let mut changed = false;
        for unit in split_annexb_units(payload) {
            changed |= self.store_nalu(codec, unit);
        }
        changed
    }

    pub fn update_from_length_prefixed(&mut self, codec: CodecId, payload: &[u8]) -> bool {
        self.update_from_length_prefixed_checked(codec, payload)
            .unwrap_or(false)
    }

    pub fn update_from_length_prefixed_checked(
        &mut self,
        codec: CodecId,
        payload: &[u8],
    ) -> Result<bool, LengthPrefixedParseError> {
        let mut changed = false;
        let units = parse_length_prefixed_units(payload)?;
        for unit in units {
            changed |= self.store_nalu(codec, unit.as_ref());
        }
        Ok(changed)
    }

    pub fn update_from_extradata(&mut self, extradata: &CodecExtradata) -> bool {
        let mut changed = false;
        match extradata {
            CodecExtradata::H264 { sps, pps, .. } => {
                for unit in sps.iter().chain(pps) {
                    changed |= self.store_nalu(CodecId::H264, unit.as_ref());
                }
            }
            CodecExtradata::H265 { vps, sps, pps, .. } => {
                for unit in vps.iter().chain(sps).chain(pps) {
                    changed |= self.store_nalu(CodecId::H265, unit.as_ref());
                }
            }
            CodecExtradata::H266 { vps, sps, pps } => {
                for unit in vps.iter().chain(sps).chain(pps) {
                    changed |= self.store_nalu(CodecId::H266, unit.as_ref());
                }
            }
            _ => {}
        }
        changed
    }

    pub fn prepend_to_access_unit(&self, codec: CodecId, access_unit: &mut AccessUnit) {
        let mut prefix = Vec::new();
        match codec {
            CodecId::H264 => {
                if let Some(sps) = &self.sps {
                    prefix.push(sps.clone());
                }
                if let Some(pps) = &self.pps {
                    prefix.push(pps.clone());
                }
            }
            CodecId::H265 | CodecId::H266 => {
                if let Some(vps) = &self.vps {
                    prefix.push(vps.clone());
                }
                if let Some(sps) = &self.sps {
                    prefix.push(sps.clone());
                }
                if let Some(pps) = &self.pps {
                    prefix.push(pps.clone());
                }
            }
            _ => {}
        }
        if prefix.is_empty() {
            return;
        }
        prefix.append(&mut access_unit.units);
        access_unit.units = prefix;
    }

    pub fn prepend_to_annexb_access_unit(&self, codec: CodecId, payload: &[u8]) -> Bytes {
        let units = split_annexb_units(payload);
        if units.is_empty() {
            return Bytes::copy_from_slice(payload);
        }

        let mut access_unit = AccessUnit::from_units(
            units
                .into_iter()
                .map(Bytes::copy_from_slice)
                .collect::<Vec<_>>(),
        );
        self.prepend_to_access_unit(codec, &mut access_unit);
        annexb_from_access_unit(&access_unit)
    }

    pub fn has_required_sets(&self, codec: CodecId) -> bool {
        match codec {
            CodecId::H264 => self.sps.is_some() && self.pps.is_some(),
            CodecId::H265 | CodecId::H266 => {
                self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
            }
            _ => true,
        }
    }

    pub fn requirement_for_frame(
        &self,
        codec: CodecId,
        random_access: bool,
    ) -> ParameterSetRequirement {
        let requires_sets = matches!(codec, CodecId::H264 | CodecId::H265 | CodecId::H266);
        if !requires_sets || !random_access {
            return ParameterSetRequirement::NotRequired;
        }
        if self.has_required_sets(codec) {
            ParameterSetRequirement::RequiredPresent
        } else {
            ParameterSetRequirement::RequiredMissing
        }
    }

    pub fn extradata_for_codec(&self, codec: CodecId) -> Option<CodecExtradata> {
        match codec {
            CodecId::H264 => Some(CodecExtradata::H264 {
                sps: vec![self.sps.clone()?],
                pps: vec![self.pps.clone()?],
                avcc: None,
            }),
            CodecId::H265 => Some(CodecExtradata::H265 {
                vps: vec![self.vps.clone()?],
                sps: vec![self.sps.clone()?],
                pps: vec![self.pps.clone()?],
                hvcc: None,
            }),
            CodecId::H266 => Some(CodecExtradata::H266 {
                vps: vec![self.vps.clone()?],
                sps: vec![self.sps.clone()?],
                pps: vec![self.pps.clone()?],
            }),
            _ => None,
        }
    }

    pub fn repair_h26x_keyframe_frame(&mut self, frame: &mut AVFrame) -> Option<CodecExtradata> {
        if frame.format != FrameFormat::CanonicalH26x {
            return None;
        }
        if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            return None;
        }

        let discovered = if self.update_from_annexb(frame.codec, frame.payload.as_ref())
            && self.has_required_sets(frame.codec)
        {
            self.extradata_for_codec(frame.codec)
        } else {
            None
        };

        if frame.flags.contains(FrameFlags::KEY) {
            frame.payload = self.prepend_to_annexb_access_unit(frame.codec, frame.payload.as_ref());
        }
        discovered
    }

    fn store_nalu(&mut self, codec: CodecId, unit: &[u8]) -> bool {
        if unit.is_empty() || unit.len() > PARAMETER_SET_MAX_SIZE {
            return false;
        }

        match codec {
            CodecId::H264 => {
                let h264_type = unit[0] & 0x1f;
                if h264_type == 7 {
                    if self.sps.as_deref() != Some(unit) {
                        self.sps = Some(Bytes::copy_from_slice(unit));
                        return true;
                    }
                    return false;
                }
                if h264_type == 8 {
                    if self.pps.as_deref() != Some(unit) {
                        self.pps = Some(Bytes::copy_from_slice(unit));
                        return true;
                    }
                    return false;
                }
                false
            }
            CodecId::H265 => {
                let h265_type = (unit[0] >> 1) & 0x3f;
                match h265_type {
                    32 => {
                        if self.vps.as_deref() != Some(unit) {
                            self.vps = Some(Bytes::copy_from_slice(unit));
                            return true;
                        }
                    }
                    33 => {
                        if self.sps.as_deref() != Some(unit) {
                            self.sps = Some(Bytes::copy_from_slice(unit));
                            return true;
                        }
                    }
                    34 if self.pps.as_deref() != Some(unit) => {
                        self.pps = Some(Bytes::copy_from_slice(unit));
                        return true;
                    }
                    _ => {}
                }
                false
            }
            CodecId::H266 => {
                if unit.len() < 2 {
                    return false;
                }
                let h266_type = (unit[1] >> 3) & 0x1f;
                match h266_type {
                    14 => {
                        if self.vps.as_deref() != Some(unit) {
                            self.vps = Some(Bytes::copy_from_slice(unit));
                            return true;
                        }
                    }
                    15 => {
                        if self.sps.as_deref() != Some(unit) {
                            self.sps = Some(Bytes::copy_from_slice(unit));
                            return true;
                        }
                    }
                    16 if self.pps.as_deref() != Some(unit) => {
                        self.pps = Some(Bytes::copy_from_slice(unit));
                        return true;
                    }
                    _ => {}
                }
                false
            }
            _ => false,
        }
    }
}

fn annexb_from_access_unit(access_unit: &AccessUnit) -> Bytes {
    let total_len = access_unit
        .units
        .iter()
        .map(|unit| unit.len().saturating_add(4))
        .sum();
    let mut out = Vec::with_capacity(total_len);
    for unit in &access_unit.units {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
    }
    Bytes::from(out)
}

#[derive(Debug, Default)]
pub struct AccessUnitAssembler {
    pending: Vec<Bytes>,
}

impl AccessUnitAssembler {
    pub fn push_unit(&mut self, unit: Bytes) {
        self.pending.push(unit);
    }

    pub fn push_annexb(&mut self, payload: &[u8]) {
        for unit in split_annexb_units(payload) {
            self.push_unit(Bytes::copy_from_slice(unit));
        }
    }

    pub fn push_length_prefixed(&mut self, payload: &[u8]) {
        let _ = self.push_length_prefixed_checked(payload);
    }

    pub fn push_length_prefixed_checked(
        &mut self,
        payload: &[u8],
    ) -> Result<(), LengthPrefixedParseError> {
        let units = parse_length_prefixed_units(payload)?;
        self.pending.extend(units);
        Ok(())
    }

    pub fn take_access_unit(&mut self) -> AccessUnit {
        AccessUnit::from_units(mem::take(&mut self.pending))
    }
}

pub(crate) fn split_annexb_units(mut payload: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((start, code_len)) = find_start_code(payload) {
        payload = &payload[start + code_len..];
        let next_start = find_start_code(payload)
            .map(|(idx, _)| idx)
            .unwrap_or(payload.len());
        if next_start > 0 {
            out.push(&payload[..next_start]);
        }
        payload = &payload[next_start..];
    }
    out
}

pub(crate) fn find_start_code(data: &[u8]) -> Option<(usize, usize)> {
    if data.len() < 3 {
        return None;
    }
    for i in 0..(data.len() - 2) {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
    }
    None
}

fn parse_length_prefixed_units(payload: &[u8]) -> Result<Vec<Bytes>, LengthPrefixedParseError> {
    let mut units = Vec::new();
    let mut offset = 0usize;
    while payload.len().saturating_sub(offset) >= 4 {
        let size = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;
        if size == 0 {
            return Err(LengthPrefixedParseError::ZeroLengthUnit { offset: offset - 4 });
        }
        let remaining = payload.len().saturating_sub(offset);
        if remaining < size {
            return Err(LengthPrefixedParseError::TruncatedUnit {
                offset: offset - 4,
                declared_size: size,
                remaining_bytes: remaining,
            });
        }
        units.push(Bytes::copy_from_slice(&payload[offset..offset + size]));
        offset += size;
    }
    if offset != payload.len() {
        return Err(LengthPrefixedParseError::IncompleteLengthPrefix {
            offset,
            remaining_bytes: payload.len() - offset,
        });
    }
    Ok(units)
}

pub fn video_payload_is_random_access(codec: CodecId, format: FrameFormat, payload: &[u8]) -> bool {
    match (codec, format) {
        (CodecId::H264 | CodecId::H265 | CodecId::H266, FrameFormat::CanonicalH26x) => {
            h26x_annexb_has_random_access(codec, payload)
        }
        (CodecId::AV1, FrameFormat::CanonicalAv1Obu) => av1_obu_payload_has_keyframe(payload),
        (CodecId::VP8, FrameFormat::CanonicalVp8Frame) => vp8_frame_is_keyframe(payload),
        (CodecId::VP9, FrameFormat::CanonicalVp9Frame) => vp9_frame_is_keyframe(payload),
        (CodecId::MJPEG, FrameFormat::MjpegFrame) => true,
        _ => false,
    }
}

pub fn h26x_annexb_has_random_access(codec: CodecId, payload: &[u8]) -> bool {
    split_annexb_units(payload)
        .into_iter()
        .any(|unit| h26x_nalu_is_random_access(codec, unit))
}

pub fn h26x_nalu_is_random_access(codec: CodecId, unit: &[u8]) -> bool {
    match codec {
        CodecId::H264 => unit.first().is_some_and(|header| (header & 0x1f) == 5),
        CodecId::H265 => unit
            .first()
            .is_some_and(|header| (16..=21).contains(&((header >> 1) & 0x3f))),
        CodecId::H266 => unit
            .get(1)
            .is_some_and(|header| matches!((header >> 3) & 0x1f, 7..=10)),
        _ => false,
    }
}

pub fn av1_obu_payload_has_keyframe(payload: &[u8]) -> bool {
    let mut cursor = payload;
    while !cursor.is_empty() {
        let Some(header) = cursor.first().copied() else {
            return false;
        };
        let has_extension = (header & 0x04) != 0;
        let has_size_field = (header & 0x02) != 0;
        let mut offset = 1usize;
        if has_extension {
            offset = match offset.checked_add(1) {
                Some(value) => value,
                None => return false,
            };
        }
        let payload_offset = offset;
        if has_size_field {
            let Some((obu_len, leb_len)) =
                av1_read_leb128(cursor.get(payload_offset..).unwrap_or(&[]))
            else {
                return false;
            };
            offset = match offset.checked_add(leb_len) {
                Some(value) => value,
                None => return false,
            };
            if cursor.len().saturating_sub(offset) < obu_len {
                return false;
            }
            let obu = &cursor[..offset + obu_len];
            if let Some(is_key) = av1_obu_is_keyframe(obu) {
                return is_key;
            }
            cursor = &cursor[offset + obu_len..];
        } else {
            if let Some(is_key) = av1_obu_is_keyframe(cursor) {
                return is_key;
            }
            return false;
        }
    }
    false
}

pub fn vp8_frame_is_keyframe(payload: &[u8]) -> bool {
    payload.first().is_some_and(|byte| (byte & 0x01) == 0)
}

pub fn vp9_frame_is_keyframe(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    let mut bits = BitReader::new(payload);
    let Some(frame_marker) = bits.read_bits(2) else {
        return false;
    };
    if frame_marker != 0b10 {
        return false;
    }
    let Some(profile_low) = bits.read_bit() else {
        return false;
    };
    let Some(profile_high) = bits.read_bit() else {
        return false;
    };
    let profile = profile_low | (profile_high << 1);
    if profile == 3 && bits.read_bit().is_none() {
        return false;
    }
    let Some(show_existing_frame) = bits.read_bit() else {
        return false;
    };
    if show_existing_frame != 0 {
        return false;
    }
    let Some(frame_type) = bits.read_bit() else {
        return false;
    };
    frame_type == 0
}

fn av1_obu_is_keyframe(obu: &[u8]) -> Option<bool> {
    let obu_header = *obu.first()?;
    let obu_type = (obu_header >> 3) & 0x0f;
    let has_extension = (obu_header & 0x04) != 0;
    let has_size_field = (obu_header & 0x02) != 0;
    let mut offset = 1usize;
    if has_extension {
        offset = offset.checked_add(1)?;
    }
    if has_size_field {
        let (_payload_len, leb_len) = av1_read_leb128(obu.get(offset..)?)?;
        offset = offset.checked_add(leb_len)?;
    }
    let payload = obu.get(offset..)?;
    match obu_type {
        3 | 6 | 7 => av1_frame_header_is_keyframe(payload),
        _ => None,
    }
}

fn av1_frame_header_is_keyframe(payload: &[u8]) -> Option<bool> {
    let mut bits = BitReader::new(payload);
    let show_existing_frame = bits.read_bit()?;
    if show_existing_frame != 0 {
        return Some(false);
    }
    let frame_type = bits.read_bits(2)? as u8;
    Some(frame_type == 0)
}

fn av1_read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    let mut shift: u32 = 0;
    for (index, byte) in data.iter().copied().take(8).enumerate() {
        let part = usize::from(byte & 0x7f);
        value |= part.checked_shl(shift)?;
        if (byte & 0x80) == 0 {
            return Some((value, index + 1));
        }
        shift = shift.checked_add(7)?;
    }
    None
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_bit(&mut self) -> Option<u8> {
        self.read_bits(1).map(|value| value as u8)
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        if count > 32 || self.bit_offset.checked_add(count)? > self.data.len().checked_mul(8)? {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..count {
            let byte = self.data[self.bit_offset / 8];
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit_offset += 1;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AVFrame, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId};

    #[test]
    fn cache_extracts_h264_parameter_sets() {
        let mut cache = ParameterSetCache::default();
        let payload = [
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5, 6, 0, 0, 1, 0x65, 9,
        ];
        assert!(cache.update_from_annexb(CodecId::H264, &payload));
        assert_eq!(cache.sps.as_deref(), Some(&[0x67, 1, 2, 3][..]));
        assert_eq!(cache.pps.as_deref(), Some(&[0x68, 4, 5, 6][..]));
    }

    #[test]
    fn cache_extracts_h264_parameter_sets_from_extradata() {
        let mut cache = ParameterSetCache::default();
        let extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 1])],
            pps: vec![Bytes::from_static(&[0x68, 2])],
            avcc: None,
        };

        assert!(cache.update_from_extradata(&extradata));
        assert_eq!(cache.sps.as_deref(), Some(&[0x67, 1][..]));
        assert_eq!(cache.pps.as_deref(), Some(&[0x68, 2][..]));
    }

    #[test]
    fn h265_non_parameter_nalu_does_not_poison_parameter_set_cache() {
        let mut cache = ParameterSetCache::default();
        let payload = [0, 0, 0, 1, 0x28, 0x01, 0xaa, 0xbb];
        assert!(!cache.update_from_annexb(CodecId::H265, &payload));
        assert!(cache.vps.is_none());
        assert!(cache.sps.is_none());
        assert!(cache.pps.is_none());
    }

    #[test]
    fn cache_extracts_h265_parameter_sets() {
        let mut cache = ParameterSetCache::default();
        let payload = [
            0, 0, 1, 0x40, 0x01, 0x0c, 0, 0, 1, 0x42, 0x01, 0x01, 0, 0, 1, 0x44, 0x01, 0xc0,
        ];
        assert!(cache.update_from_annexb(CodecId::H265, &payload));
        assert_eq!(cache.vps.as_deref(), Some(&[0x40, 0x01, 0x0c][..]));
        assert_eq!(cache.sps.as_deref(), Some(&[0x42, 0x01, 0x01][..]));
        assert_eq!(cache.pps.as_deref(), Some(&[0x44, 0x01, 0xc0][..]));
    }

    #[test]
    fn assembler_accepts_annexb() {
        let mut asm = AccessUnitAssembler::default();
        asm.push_annexb(&[0, 0, 1, 0x65, 1, 2, 0, 0, 1, 0x41, 3, 4]);
        let au = asm.take_access_unit();
        assert_eq!(au.units.len(), 2);
        assert_eq!(au.units[0], Bytes::from_static(&[0x65, 1, 2]));
    }

    #[test]
    fn assembler_reports_zero_length_length_prefixed_units() {
        let mut asm = AccessUnitAssembler::default();
        let err = asm
            .push_length_prefixed_checked(&[0, 0, 0, 0])
            .expect_err("zero-length NALU must be rejected");
        assert_eq!(err, LengthPrefixedParseError::ZeroLengthUnit { offset: 0 });
    }

    #[test]
    fn assembler_reports_truncated_length_prefixed_units() {
        let mut asm = AccessUnitAssembler::default();
        let err = asm
            .push_length_prefixed_checked(&[0, 0, 0, 5, 0x65, 1, 2])
            .expect_err("truncated NALU must be rejected");
        assert_eq!(
            err,
            LengthPrefixedParseError::TruncatedUnit {
                offset: 0,
                declared_size: 5,
                remaining_bytes: 3
            }
        );
    }

    #[test]
    fn assembler_reports_incomplete_length_prefix() {
        let mut asm = AccessUnitAssembler::default();
        let err = asm
            .push_length_prefixed_checked(&[0, 0, 0, 1, 0x65, 0x12, 0x34])
            .expect_err("trailing length prefix bytes must be rejected");
        assert_eq!(
            err,
            LengthPrefixedParseError::IncompleteLengthPrefix {
                offset: 5,
                remaining_bytes: 2
            }
        );
    }

    #[test]
    fn prepend_parameter_sets_keeps_prefix_order() {
        let cache = ParameterSetCache {
            sps: Some(Bytes::from_static(&[0x67, 1])),
            pps: Some(Bytes::from_static(&[0x68, 2])),
            ..Default::default()
        };

        let mut au = AccessUnit::from_units(vec![Bytes::from_static(&[0x65, 9])]);
        cache.prepend_to_access_unit(CodecId::H264, &mut au);
        assert_eq!(
            au.units,
            vec![
                Bytes::from_static(&[0x67, 1]),
                Bytes::from_static(&[0x68, 2]),
                Bytes::from_static(&[0x65, 9]),
            ]
        );
    }

    #[test]
    fn prepend_parameter_sets_to_annexb_keyframe_for_bootstrap() {
        let cache = ParameterSetCache {
            sps: Some(Bytes::from_static(&[0x67, 1])),
            pps: Some(Bytes::from_static(&[0x68, 2])),
            ..Default::default()
        };

        let payload = [0, 0, 1, 0x65, 9];
        let out = cache.prepend_to_annexb_access_unit(CodecId::H264, &payload);

        assert_eq!(
            out.as_ref(),
            &[0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9]
        );
    }

    #[test]
    fn repair_h26x_keyframe_frame_discovers_extradata_and_prepends_sets() {
        let mut cache = ParameterSetCache::default();
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            9_000,
            9_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0, 0, 1, 0x67, 1, 2, // SPS
                0, 0, 1, 0x68, 3, 4, // PPS
                0, 0, 1, 0x65, 9, 9, // IDR
            ]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let discovered = cache
            .repair_h26x_keyframe_frame(&mut frame)
            .expect("discover h264 extradata");
        assert!(matches!(discovered, CodecExtradata::H264 { .. }));
        assert!(frame.payload.starts_with(&[0, 0, 0, 1, 0x67]));
        assert!(cache.has_required_sets(CodecId::H264));
    }

    #[test]
    fn extradata_for_codec_requires_complete_parameter_sets() {
        let mut cache = ParameterSetCache {
            sps: Some(Bytes::from_static(&[0x67, 1])),
            ..Default::default()
        };
        assert!(cache.extradata_for_codec(CodecId::H264).is_none());

        cache.pps = Some(Bytes::from_static(&[0x68, 2]));
        assert!(matches!(
            cache.extradata_for_codec(CodecId::H264),
            Some(CodecExtradata::H264 { .. })
        ));
    }

    #[test]
    fn access_unit_from_frame_carries_media_time_random_access_and_parameter_set_requirement() {
        let mut frame = AVFrame::new(
            TrackId(9),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            900,
            800,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x65, 0xaa]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let cache = ParameterSetCache::default();
        let access_unit =
            AccessUnit::from_frame_units(&frame, vec![Bytes::from_static(&[0x65, 0xaa])], &cache)
                .expect("access unit");
        assert!(access_unit.random_access);
        assert!(matches!(
            access_unit.parameter_set_requirement,
            ParameterSetRequirement::RequiredMissing
        ));
        let timing = access_unit.timing.expect("timing");
        assert_eq!(timing.pts, 900);
        assert_eq!(timing.dts, 800);
        assert_eq!(timing.timebase, Timebase::new(1, 90_000));
    }

    #[test]
    fn h266_parameter_set_cache_extracts_vps_sps_pps_from_annexb() {
        let mut cache = ParameterSetCache::default();
        let payload = [
            0, 0, 1, 0x00, 0x70, 0x01, // VVC VPS (type 14)
            0, 0, 1, 0x00, 0x78, 0x01, // VVC SPS (type 15)
            0, 0, 1, 0x00, 0x80, 0x01, // VVC PPS (type 16)
        ];
        assert!(cache.update_from_annexb(CodecId::H266, &payload));
        assert_eq!(cache.vps.as_deref(), Some(&[0x00, 0x70, 0x01][..]));
        assert_eq!(cache.sps.as_deref(), Some(&[0x00, 0x78, 0x01][..]));
        assert_eq!(cache.pps.as_deref(), Some(&[0x00, 0x80, 0x01][..]));
    }

    #[test]
    fn video_payload_random_access_detects_all_supported_video_codecs() {
        assert!(video_payload_is_random_access(
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x65, 0x88]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x41, 0x88]
        ));
        assert!(video_payload_is_random_access(
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x26, 0x01, 0x88]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x02, 0x01, 0x88]
        ));
        assert!(video_payload_is_random_access(
            CodecId::H266,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x00, 0x38, 0x88]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::H266,
            FrameFormat::CanonicalH26x,
            &[0, 0, 1, 0x20, 0x80, 0x88]
        ));
        assert!(video_payload_is_random_access(
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            &[0x1a, 0x01, 0x00]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            &[0x1a, 0x01, 0x40]
        ));
        assert!(video_payload_is_random_access(
            CodecId::VP8,
            FrameFormat::CanonicalVp8Frame,
            &[0x00, 0x9d, 0x01, 0x2a]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::VP8,
            FrameFormat::CanonicalVp8Frame,
            &[0x01, 0x00]
        ));
        assert!(video_payload_is_random_access(
            CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            &[0x82, 0x49, 0x83]
        ));
        assert!(!video_payload_is_random_access(
            CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            &[0x86, 0x49, 0x83]
        ));
    }

    #[test]
    fn cache_reports_length_prefixed_errors_and_preserves_state() {
        let mut cache = ParameterSetCache {
            sps: Some(Bytes::from_static(&[0x67, 1])),
            ..Default::default()
        };

        let err = cache
            .update_from_length_prefixed_checked(CodecId::H264, &[0, 0, 0, 4, 0x68, 1])
            .expect_err("truncated payload must report error");
        assert_eq!(
            err,
            LengthPrefixedParseError::TruncatedUnit {
                offset: 0,
                declared_size: 4,
                remaining_bytes: 2
            }
        );
        assert_eq!(cache.sps.as_deref(), Some(&[0x67, 1][..]));
    }

    #[test]
    fn cache_rejects_oversized_parameter_set_nalu() {
        let mut cache = ParameterSetCache::default();
        // Create an SPS NALU that exceeds PARAMETER_SET_MAX_SIZE
        let mut oversized_sps = vec![0x67]; // H264 SPS type
        oversized_sps.resize(PARAMETER_SET_MAX_SIZE + 1, 0xAA);

        // Build Annex-B payload with oversized SPS
        let mut payload = vec![0, 0, 0, 1];
        payload.extend_from_slice(&oversized_sps);
        payload.extend_from_slice(&[0, 0, 0, 1, 0x68, 1, 2]); // normal PPS

        let changed = cache.update_from_annexb(CodecId::H264, &payload);
        // PPS should be stored, but oversized SPS should be rejected
        assert!(changed);
        assert!(
            cache.sps.is_none(),
            "oversized SPS should be rejected by cache"
        );
        assert!(
            cache.pps.is_some(),
            "normal-sized PPS should still be stored"
        );
    }

    #[test]
    fn cache_rejects_oversized_parameter_set_from_length_prefixed() {
        let mut cache = ParameterSetCache::default();
        // Create an SPS NALU that exceeds PARAMETER_SET_MAX_SIZE
        let mut oversized_sps = vec![0x67]; // H264 SPS type
        oversized_sps.resize(PARAMETER_SET_MAX_SIZE + 1, 0xBB);

        // Build length-prefixed payload
        let size = oversized_sps.len() as u32;
        let mut payload = Vec::new();
        payload.extend_from_slice(&size.to_be_bytes());
        payload.extend_from_slice(&oversized_sps);

        let changed = cache.update_from_length_prefixed(CodecId::H264, &payload);
        assert!(!changed);
        assert!(
            cache.sps.is_none(),
            "oversized SPS should be rejected from length-prefixed input"
        );
    }

    #[test]
    fn h264_cache_extracts_sps_pps_from_length_prefixed() {
        let mut cache = ParameterSetCache::default();
        // Build AVCC-style length-prefixed payload: SPS + PPS + IDR
        let sps = [0x67, 0x64, 0x00, 0x1f];
        let pps = [0x68, 0xeb, 0xef];
        let idr = [0x65, 0x88, 0x80];

        let mut payload = Vec::new();
        payload.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        payload.extend_from_slice(&sps);
        payload.extend_from_slice(&(pps.len() as u32).to_be_bytes());
        payload.extend_from_slice(&pps);
        payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        payload.extend_from_slice(&idr);

        let changed = cache.update_from_length_prefixed(CodecId::H264, &payload);
        assert!(changed);
        assert_eq!(cache.sps.as_deref(), Some(&sps[..]));
        assert_eq!(cache.pps.as_deref(), Some(&pps[..]));
        assert!(cache.has_required_sets(CodecId::H264));
    }

    #[test]
    fn h265_cache_extracts_vps_sps_pps_from_length_prefixed() {
        let mut cache = ParameterSetCache::default();
        // H265 VPS (type 32), SPS (type 33), PPS (type 34)
        let vps = [0x40, 0x01, 0x0c, 0x01];
        let sps = [0x42, 0x01, 0x01, 0x01];
        let pps = [0x44, 0x01, 0xc0];

        let mut payload = Vec::new();
        payload.extend_from_slice(&(vps.len() as u32).to_be_bytes());
        payload.extend_from_slice(&vps);
        payload.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        payload.extend_from_slice(&sps);
        payload.extend_from_slice(&(pps.len() as u32).to_be_bytes());
        payload.extend_from_slice(&pps);

        let changed = cache.update_from_length_prefixed(CodecId::H265, &payload);
        assert!(changed);
        assert_eq!(cache.vps.as_deref(), Some(&vps[..]));
        assert_eq!(cache.sps.as_deref(), Some(&sps[..]));
        assert_eq!(cache.pps.as_deref(), Some(&pps[..]));
        assert!(cache.has_required_sets(CodecId::H265));
    }

    #[test]
    fn parameter_set_max_size_constant_is_reasonable() {
        // Verify the constant is at least large enough for real-world parameter sets
        // (typical SPS is 20-100 bytes) but bounded
        assert!(PARAMETER_SET_MAX_SIZE >= 1024);
        assert!(PARAMETER_SET_MAX_SIZE <= 8192);
    }
}
