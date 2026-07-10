use crate::prelude::*;
use bytes::Bytes;
use core::mem;

use crate::frame::{AVFrame, FrameFlags, FrameFormat, FrameTimingError};
use crate::time::Timebase;
use crate::track::{CodecExtradata, CodecId};

/// A group of one or more NALUs / OBUs / frames that represent a single decode unit.
///
/// `AccessUnit` is the intermediate form used when moving between `AVFrame` and
/// protocol-specific payloads. It carries timing, random-access status, and a
/// parameter-set requirement so the caller can decide whether to prepend SPS/PPS/VPS
/// before egress or decoder hand-off.
///
/// 由一个或多个 NALU/OBU/帧组成的单一解码单元。
///
/// `AccessUnit` 是 `AVFrame` 与协议特定负载之间转换的中间形式，
/// 携带时间戳、随机访问状态和参数集需求，使调用方决定是否在出口或交付解码器前
/// 前置 SPS/PPS/VPS。
#[derive(Debug, Clone)]
pub struct AccessUnit {
    pub units: Vec<Bytes>,
    pub timing: Option<AccessUnitTiming>,
    pub random_access: bool,
    pub parameter_set_requirement: ParameterSetRequirement,
}

impl AccessUnit {
    /// Creates an access unit from raw units without timing or parameter-set metadata.
    ///
    /// 从原始单元创建 access unit，不附加时间戳和参数集元数据。
    pub fn from_units(units: Vec<Bytes>) -> Self {
        Self {
            units,
            timing: None,
            random_access: false,
            parameter_set_requirement: ParameterSetRequirement::NotRequired,
        }
    }

    /// Builds an access unit from a normalized frame and its already-extracted units.
    ///
    /// Validates frame timing, copies the frame's timing into the access unit, and
    /// asks the parameter-set cache whether the keyframe can be decoded with the
    /// currently cached SPS/PPS/VPS.
    ///
    /// 从归一化帧及其已提取的单元构建 access unit。
    ///
    /// 校验帧时间戳，将帧时间复制到 access unit，并询问参数集缓存当前关键帧
    /// 能否用已缓存的 SPS/PPS/VPS 解码。
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

/// Timing information extracted from a frame for an access unit.
///
/// Preserves `pts`, `dts`, `duration` and `timebase` so the codec layer can
/// reason about decode and presentation order independently of the wire format.
///
/// 从帧中提取的 access unit 时间信息。
///
/// 保留 `pts`、`dts`、`duration` 和 `timebase`，使 codec 层能够独立于线格式
/// 处理解码和显示顺序。
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

/// Result of checking whether a keyframe can be decoded with the cached parameter sets.
///
/// `NotRequired` for non-H.26x codecs or non-keyframes; `RequiredPresent` when the
/// keyframe needs parameter sets and they are available; `RequiredMissing` when they
/// are needed but absent, signaling the pipeline to wait or bootstrap.
///
/// 检查关键帧能否用已缓存参数集解码的结果。
///
/// `NotRequired` 表示非 H.26x 编解码器或非关键帧；`RequiredPresent` 表示关键帧需要
/// 参数集且已可用；`RequiredMissing` 表示需要但缺失，提示流水线等待或自举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterSetRequirement {
    #[default]
    NotRequired,
    RequiredPresent,
    RequiredMissing,
}

/// Errors that can occur when constructing an `AccessUnit` from a frame.
///
/// 从帧构建 `AccessUnit` 时可能发生的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AccessUnitBuildError {
    #[error("invalid frame timing when building access unit: {0}")]
    InvalidTiming(#[from] FrameTimingError),
}

/// Errors returned when parsing length-prefixed NALU payloads.
///
/// Length-prefixed formats (AVCC, HVCC, etc.) declare each NALU size with a 4-byte
/// big-endian prefix. This enum captures malformed input: incomplete prefix,
/// zero-length unit, or declared size exceeding remaining bytes.
///
/// 解析长度前缀 NALU 负载时返回的错误。
///
/// 长度前缀格式（AVCC、HVCC 等）用 4 字节大端前缀声明每个 NALU 大小。
/// 该枚举捕获格式错误：前缀不完整、零长度单元或声明大小超过剩余字节。
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

/// Cache for video parameter sets (SPS/PPS/VPS) shared across a stream.
///
/// The cache is updated from Annex-B NALUs, length-prefixed NALUs, or parsed
/// `CodecExtradata`. It can then prepend the required sets to a keyframe access
/// unit or produce a `CodecExtradata` view for track initialization.
///
/// 视频参数集（SPS/PPS/VPS）缓存，在流内共享。
///
/// 缓存从 Annex-B NALU、长度前缀 NALU 或已解析的 `CodecExtradata` 更新，
/// 然后可向关键帧 access unit 前置所需参数集，或生成 `CodecExtradata` 视图用于轨道初始化。
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

    /// Scans an Annex-B payload for SPS/PPS/VPS NALUs and updates the cache.
    ///
    /// Returns `true` if any cached set changed. Only the first matching parameter
    /// set of each type is kept; subsequent identical sets are ignored.
    ///
    /// 扫描 Annex-B 负载中的 SPS/PPS/VPS NALU 并更新缓存。
    ///
    /// 若有任何缓存集发生变化则返回 `true`。每种类型只保留首个匹配参数集，
    /// 后续相同集合被忽略。
    pub fn update_from_annexb(&mut self, codec: CodecId, payload: &[u8]) -> bool {
        let mut changed = false;
        for unit in split_annexb_units(payload) {
            changed |= self.store_nalu(codec, unit);
        }
        changed
    }

    /// Parses a length-prefixed payload and updates the cache from its NALUs.
    ///
    /// Errors are silently ignored; use `update_from_length_prefixed_checked` if
    /// callers need to surface parse failures.
    ///
    /// 解析长度前缀负载并从中更新缓存。
    ///
    /// 错误被静默忽略；如需向调用方暴露解析失败，请使用
    /// `update_from_length_prefixed_checked`。
    pub fn update_from_length_prefixed(&mut self, codec: CodecId, payload: &[u8]) -> bool {
        self.update_from_length_prefixed_checked(codec, payload)
            .unwrap_or(false)
    }

    /// Parses a length-prefixed payload and updates the cache, returning parse errors.
    ///
    /// 解析长度前缀负载并更新缓存，返回解析错误。
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

    /// Seeds the cache from already-parsed codec extradata.
    ///
    /// This is used when a track is initialized from a container (e.g. MP4) that
    /// stores parameter sets outside of the elementary stream.
    ///
    /// 从已解析的编解码器 extradata 初始化缓存。
    ///
    /// 当轨道从将参数集保存在基本流之外的容器（如 MP4）初始化时使用。
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

    /// Prefixes the cached parameter sets to an access unit in the correct order.
    ///
    /// For H.264 the order is SPS then PPS; for H.265/H.266 it is VPS, SPS, then PPS.
    /// If the cache already contains the same parameter sets in the payload, the
    /// prefix is still added because downstream decoders often require them at the
    /// start of a keyframe.
    ///
    /// 将缓存的参数集按正确顺序前置到 access unit。
    ///
    /// H.264 顺序为 SPS 后 PPS；H.265/H.266 为 VPS、SPS、PPS。
    /// 即使负载中已包含相同参数集，仍执行前置，因为下游解码器通常要求关键帧以它们开头。
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

    /// Splits an Annex-B payload into units, prepends cached parameter sets, and
    /// re-serializes the result as Annex-B.
    ///
    /// Returns the original payload unchanged if no start code is found.
    ///
    /// 将 Annex-B 负载拆分为单元，前置缓存参数集，然后重新序列化为 Annex-B。
    ///
    /// 若未找到起始码则返回原始负载不变。
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

    /// Returns `true` when the cache holds the parameter sets required by the codec.
    ///
    /// H.264 requires SPS+PPS; H.265/H.266 require VPS+SPS+PPS. Other codecs are
    /// considered always ready.
    ///
    /// 当缓存持有该编解码器所需参数集时返回 `true`。
    ///
    /// H.264 需要 SPS+PPS；H.265/H.266 需要 VPS+SPS+PPS；其他编解码器视为始终就绪。
    pub fn has_required_sets(&self, codec: CodecId) -> bool {
        match codec {
            CodecId::H264 => self.sps.is_some() && self.pps.is_some(),
            CodecId::H265 | CodecId::H266 => {
                self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
            }
            _ => true,
        }
    }

    /// Determines whether a random-access frame needs parameter sets and whether
    /// the cache can provide them.
    ///
    /// 判断随机访问帧是否需要参数集，以及缓存能否提供。
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

    /// Builds a `CodecExtradata` view from the cached parameter sets.
    ///
    /// Returns `None` when the codec does not require parameter sets or when the
    /// cache is incomplete.
    ///
    /// 从缓存的参数集构建 `CodecExtradata` 视图。
    ///
    /// 当编解码器不需要参数集或缓存不完整时返回 `None`。
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

    /// Discovers and prepends parameter sets for an H.26x keyframe that lacks them.
    ///
    /// First tries to extract SPS/PPS/VPS from the payload itself. If successful,
    /// it prepends the cached sets to the frame payload and returns the newly
    /// discovered `CodecExtradata`. Non-keyframes are left untouched.
    ///
    /// 为缺少参数集的 H.26x 关键帧发现并前置参数集。
    ///
    /// 首先尝试从负载本身提取 SPS/PPS/VPS；若成功，则将缓存集前置到帧负载并返回
    /// 新发现的 `CodecExtradata`。非关键帧保持不变。
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

/// Re-serializes an access unit as an Annex-B byte stream with 4-byte start codes.
///
/// 用 4 字节起始码将 access unit 重新序列化为 Annex-B 字节流。
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

/// Accumulates NALUs/OBUs from one or more ingress packets into a single access unit.
///
/// Supports both Annex-B and length-prefixed input. Once enough data is collected,
/// `take_access_unit` consumes the buffered units and returns an `AccessUnit`.
///
/// 将一个或多个入口包中的 NALU/OBU 累积为单个 access unit。
///
/// 支持 Annex-B 和长度前缀两种输入。一旦收集到足够数据，`take_access_unit` 会消费
/// 缓冲的单元并返回 `AccessUnit`。
#[derive(Debug, Default)]
pub struct AccessUnitAssembler {
    pending: Vec<Bytes>,
}

impl AccessUnitAssembler {
    /// Adds a single unit to the pending buffer.
    ///
    /// 将单个单元添加到待处理缓冲区。
    pub fn push_unit(&mut self, unit: Bytes) {
        self.pending.push(unit);
    }

    /// Splits an Annex-B payload into units and appends them.
    ///
    /// 将 Annex-B 负载拆分为单元并追加。
    pub fn push_annexb(&mut self, payload: &[u8]) {
        for unit in split_annexb_units(payload) {
            self.push_unit(Bytes::copy_from_slice(unit));
        }
    }

    /// Parses a length-prefixed payload and appends its units, ignoring parse errors.
    ///
    /// 解析长度前缀负载并追加其单元，忽略解析错误。
    pub fn push_length_prefixed(&mut self, payload: &[u8]) {
        let _ = self.push_length_prefixed_checked(payload);
    }

    /// Parses a length-prefixed payload and appends its units, returning parse errors.
    ///
    /// 解析长度前缀负载并追加其单元，返回解析错误。
    pub fn push_length_prefixed_checked(
        &mut self,
        payload: &[u8],
    ) -> Result<(), LengthPrefixedParseError> {
        let units = parse_length_prefixed_units(payload)?;
        self.pending.extend(units);
        Ok(())
    }

    /// Consumes the buffered units and returns an access unit.
    ///
    /// The internal buffer is reset, so the assembler can immediately start collecting
    /// the next access unit.
    ///
    /// 消费缓冲的单元并返回 access unit。
    ///
    /// 内部缓冲区被重置，因此该 assembler 可立即开始收集下一个 access unit。
    pub fn take_access_unit(&mut self) -> AccessUnit {
        AccessUnit::from_units(mem::take(&mut self.pending))
    }
}

/// Splits an Annex-B byte stream into NALU slices without copying.
///
/// Walks the payload looking for `0x00 0x00 0x01` or `0x00 0x00 0x00 0x01` start
/// codes. The returned slices point into the original payload and exclude the
/// start code bytes. Empty units between consecutive start codes are skipped.
///
/// 将 Annex-B 字节流拆分为 NALU 切片而不拷贝。
///
/// 遍历负载查找 `0x00 0x00 0x01` 或 `0x00 0x00 0x00 0x01` 起始码。
/// 返回的切片指向原始负载并排除起始码字节。连续起始码之间的空单元被跳过。
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

/// Locates the next Annex-B start code and returns its position and length.
///
/// Returns `(offset, 3)` for `0x00 0x00 0x01` and `(offset, 4)` for
/// `0x00 0x00 0x00 0x01`. Returns `None` if the remaining data is too short
/// or no start code is found.
///
/// 定位下一个 Annex-B 起始码并返回其位置和长度。
///
/// 返回 `(offset, 3)` 表示 `0x00 0x00 0x01`，`(offset, 4)` 表示 `0x00 0x00 0x00 0x01`。
/// 若剩余数据过短或未找到起始码则返回 `None`。
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

/// Parses a length-prefixed NALU stream into owned `Bytes` units.
///
/// Each unit is prefixed by a 4-byte big-endian length. The function validates
/// that lengths are non-zero and that each unit fits within the remaining payload.
/// It returns an error if the payload ends with a partial or malformed prefix.
///
/// 将长度前缀 NALU 流解析为拥有所有权的 `Bytes` 单元。
///
/// 每个单元由 4 字节大端长度前缀。该函数校验长度非零且每个单元都在剩余负载内。
/// 若负载以不完整或格式错误的前缀结尾则返回错误。
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

/// Determines whether a video payload can be decoded without previous frames.
///
/// Dispatches to codec-specific helpers based on `FrameFormat`. For MJPEG every
/// frame is independently decodable; for unknown/unsupported combinations it returns
/// `false`.
///
/// 判断视频负载是否无需参考前一帧即可解码。
///
/// 根据 `FrameFormat` 分派到各编解码器特定辅助函数。MJPEG 每帧独立可解码；
/// 未知/不支持的组合返回 `false`。
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

/// Checks an Annex-B H.26x payload for at least one IDR/random-access NALU.
///
/// 检查 Annex-B H.26x 负载中是否至少包含一个 IDR/随机访问 NALU。
pub fn h26x_annexb_has_random_access(codec: CodecId, payload: &[u8]) -> bool {
    split_annexb_units(payload)
        .into_iter()
        .any(|unit| h26x_nalu_is_random_access(codec, unit))
}

/// Identifies an H.26x NALU as a random-access point by its NAL type.
///
/// H.264 IDR has NAL type 5; H.265 IDR/slice types 16-21 mark random access;
/// H.266 uses types 7-10 for the corresponding slices.
///
/// 通过 NAL 类型判断 H.26x NALU 是否为随机访问点。
///
/// H.264 IDR 的 NAL 类型为 5；H.265 IDR/切片的 16-21 类型标记随机访问；
/// H.266 的对应切片类型为 7-10。
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

/// Walks an AV1 OBU stream and returns `true` if any frame OBU is a keyframe.
///
/// OBUs may include a size field; if absent, the whole remainder is treated as one
/// OBU. The parser handles LEB128 sizes and OBU extensions to skip non-frame OBUs.
///
/// 遍历 AV1 OBU 流，若任何帧 OBU 为关键帧则返回 `true`。
///
/// OBU 可能包含大小字段；若缺失，则将整个剩余部分视为一个 OBU。
/// 解析器处理 LEB128 大小和 OBU 扩展以跳过非帧 OBU。
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

/// Returns `true` if the VP8 frame header indicates a keyframe.
///
/// In VP8 the least-significant bit of the first byte is the frame type: 0 for
/// keyframe, 1 for inter-frame.
///
/// 若 VP8 帧头指示关键帧则返回 `true`。
///
/// VP8 第一个字节的最低位为帧类型：0 表示关键帧，1 表示差分帧。
pub fn vp8_frame_is_keyframe(payload: &[u8]) -> bool {
    payload.first().is_some_and(|byte| (byte & 0x01) == 0)
}

/// Parses the VP9 uncompressed header and returns `true` for keyframes.
///
/// Reads the 2-bit frame marker, profile, and 1-bit frame type from the bitstream.
/// `show_existing_frame` frames are not treated as keyframes.
///
/// 解析 VP9 未压缩头，关键帧返回 `true`。
///
/// 从比特流读取 2 位帧标记、profile 和 1 位帧类型。`show_existing_frame` 帧不视为关键帧。
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
