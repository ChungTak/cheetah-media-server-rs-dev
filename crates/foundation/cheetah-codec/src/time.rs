#[cfg(test)]
use crate::prelude::*;
use core::fmt;
use smallvec::SmallVec;

/// `Timebase` data structure.
/// `Timebase` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timebase {
    pub num: u32,
    pub den: u32,
}

impl Timebase {
    /// Creates a new `Timebase` instance.
    /// 创建新的 `Timebase` 实例。
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// Converts to `micros` representation.
    /// 转换为 `micros` 表示。
    pub fn to_micros(tb: Timebase, value: i64) -> i64 {
        let num = i128::from(tb.num);
        let den = i128::from(tb.den.max(1));
        let v = i128::from(value);
        ((v * num * 1_000_000_i128) / den) as i64
    }

    /// Creates `micros` from input.
    /// 从输入创建 `micros`。
    pub fn from_micros(tb: Timebase, micros: i64) -> i64 {
        let num = i128::from(tb.num.max(1));
        let den = i128::from(tb.den);
        let us = i128::from(micros);
        ((us * den) / (num * 1_000_000_i128)) as i64
    }
}

/// `MonoTime` data structure.
/// `MonoTime` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonoTime {
    micros: u64,
}

impl MonoTime {
    /// Creates `micros` from input.
    /// 从输入创建 `micros`。
    pub const fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    /// `as_micros` function of `MonoTime`.
    /// `MonoTime` 的 `as_micros` 函数。
    pub const fn as_micros(self) -> u64 {
        self.micros
    }
}

/// Error returned by `Timestamp` operations.
/// `Timestamp` 操作返回的错误。
#[derive(Debug, thiserror::Error)]
pub enum TimestampError {
    #[error("invalid wrap width: {0}")]
    InvalidWrapWidth(u8),
}

/// `WrapUnwrapper` data structure.
/// `WrapUnwrapper` 数据结构。
#[derive(Clone)]
pub struct WrapUnwrapper {
    mask: u64,
    half: u64,
    high: u64,
    last_raw: Option<u64>,
}

impl fmt::Debug for WrapUnwrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrapUnwrapper")
            .field("mask", &self.mask)
            .field("half", &self.half)
            .field("high", &self.high)
            .field("last_raw", &self.last_raw)
            .finish()
    }
}

impl WrapUnwrapper {
    /// Creates a new `WrapUnwrapper` instance.
    /// 创建新的 `WrapUnwrapper` 实例。
    pub fn new(bits: u8) -> Result<Self, TimestampError> {
        if bits == 0 || bits > 63 {
            return Err(TimestampError::InvalidWrapWidth(bits));
        }
        let mask = (1_u64 << bits) - 1;
        let half = 1_u64 << (bits - 1);
        Ok(Self {
            mask,
            half,
            high: 0,
            last_raw: None,
        })
    }

    /// `unwrap` function of `WrapUnwrapper`.
    /// `WrapUnwrapper` 的 `unwrap` 函数。
    pub fn unwrap(&mut self, raw: u64) -> u64 {
        let raw = raw & self.mask;
        if let Some(last) = self.last_raw {
            if raw < last && (last - raw) > self.half {
                self.high = self.high.wrapping_add(self.mask + 1);
            } else if raw > last && (raw - last) > self.half {
                self.high = self.high.wrapping_sub(self.mask + 1);
            }
        }
        self.last_raw = Some(raw);
        self.high + raw
    }

    /// Resets the state to its initial value.
    /// 将状态重置为初始值。
    pub fn reset(&mut self) {
        self.high = 0;
        self.last_raw = None;
    }
}

/// Mode selecting `Stamp Adjust` behavior.
/// 选择 `Stamp Adjust` 行为的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampAdjustMode {
    Source,
    Arrival,
    SampleCount,
    FrameRateGuess,
}

/// `StampAdjust` data structure.
/// `StampAdjust` 数据结构。
#[derive(Debug, Clone)]
pub struct StampAdjust {
    mode: StampAdjustMode,
    last_pts_us: Option<i64>,
    step_us: i64,
}

impl StampAdjust {
    /// Creates a new `StampAdjust` instance.
    /// 创建新的 `StampAdjust` 实例。
    pub fn new(mode: StampAdjustMode, step_us: i64) -> Self {
        Self {
            mode,
            last_pts_us: None,
            step_us: step_us.max(1),
        }
    }

    /// `adjust` function of `StampAdjust`.
    /// `StampAdjust` 的 `adjust` 函数。
    pub fn adjust(&mut self, source_pts_us: Option<i64>, now: MonoTime) -> i64 {
        let mut value = match self.mode {
            StampAdjustMode::Source => source_pts_us.unwrap_or_else(|| now.as_micros() as i64),
            StampAdjustMode::Arrival => now.as_micros() as i64,
            StampAdjustMode::SampleCount | StampAdjustMode::FrameRateGuess => self
                .last_pts_us
                .map_or(now.as_micros() as i64, |v| v + self.step_us),
        };

        if let Some(last) = self.last_pts_us {
            if value <= last {
                value = last + self.step_us;
            }
        }
        self.last_pts_us = Some(value);
        value
    }
}

/// `DtsGenerator` data structure.
/// `DtsGenerator` 数据结构。
#[derive(Debug, Default, Clone)]
pub struct DtsGenerator {
    last_dts: Option<i64>,
    last_source_pts: Option<i64>,
    smoothed_step: Option<i64>,
    pts_reorder_seen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PtsOnlyDtsGeneration {
    dts: i64,
    reordered: bool,
    discontinuity: bool,
}

impl DtsGenerator {
    /// Generates the `monotonic from pts`.
    /// 生成 `monotonic from pts`。
    pub fn generate_monotonic_from_pts(&mut self, pts: i64) -> i64 {
        let mut dts = pts;
        if let Some(last) = self.last_dts {
            if dts <= last {
                dts = last + 1;
            }
        }
        self.last_dts = Some(dts);
        dts
    }

    fn generate_from_pts_only(
        &mut self,
        pts: i64,
        fallback_step: i64,
        frame_duration: Option<i64>,
        max_gap_ticks: i64,
    ) -> PtsOnlyDtsGeneration {
        let fallback_step = fallback_step.max(1);
        let expected_step = self.smoothed_step.unwrap_or(fallback_step).max(1);
        let jump_threshold = max_gap_ticks
            .max(expected_step.saturating_mul(8))
            .max(fallback_step.saturating_mul(8))
            .max(1);

        let mut discontinuity = false;
        let mut reordered = false;
        let mut step = frame_duration.unwrap_or(0).max(0);
        let mut observed_step_for_smoothing = None;

        if let Some(last_pts) = self.last_source_pts {
            let delta = pts.saturating_sub(last_pts);
            if delta.saturating_abs() > jump_threshold {
                discontinuity = true;
                self.smoothed_step = None;
                if delta > 0 && step == 0 {
                    step = delta;
                    observed_step_for_smoothing = Some(delta);
                }
            } else if delta < 0 {
                reordered = true;
                self.pts_reorder_seen = true;
                self.smoothed_step = Some(fallback_step);
            } else if delta > 0 && step == 0 {
                if self.pts_reorder_seen {
                    step = expected_step;
                    observed_step_for_smoothing = Some(step);
                } else {
                    step = delta;
                    observed_step_for_smoothing = Some(delta);
                }
            }
        }

        if step == 0 {
            step = self.smoothed_step.unwrap_or(fallback_step).max(1);
        }

        let dts = match self.last_dts {
            Some(last_dts) => last_dts.saturating_add(step),
            None => pts,
        };

        let observed_step = if let Some(last_pts) = self.last_source_pts {
            let delta = pts.saturating_sub(last_pts);
            if let Some(value) = observed_step_for_smoothing {
                value
            } else if delta > 0 {
                delta
            } else {
                step
            }
        } else {
            step
        }
        .max(1);
        let blended_step = match self.smoothed_step {
            Some(previous) => ((i128::from(previous) * 3 + i128::from(observed_step)) / 4)
                .clamp(i128::from(1), i128::from(i64::MAX)) as i64,
            None => observed_step,
        };

        self.smoothed_step = Some(blended_step.max(1));
        self.last_source_pts = Some(pts);
        self.last_dts = Some(dts);
        PtsOnlyDtsGeneration {
            dts,
            reordered,
            discontinuity,
        }
    }

    /// Resets the state to its initial value.
    /// 将状态重置为初始值。
    pub fn reset(&mut self) {
        self.last_dts = None;
        self.last_source_pts = None;
        self.smoothed_step = None;
        self.pts_reorder_seen = false;
    }
}

/// `TimebaseConverter` data structure.
/// `TimebaseConverter` 数据结构。
pub struct TimebaseConverter;

impl TimebaseConverter {
    /// Converts the value into another representation.
    /// 将值转换为另一种表示。
    pub fn convert(value: i64, src: Timebase, dst: Timebase) -> i64 {
        let us = Timebase::to_micros(src, value);
        Timebase::from_micros(dst, us)
    }
}

/// `DiscontinuityJudge` data structure.
/// `DiscontinuityJudge` 数据结构。
#[derive(Debug, Clone)]
pub struct DiscontinuityJudge {
    max_gap_us: i64,
    last_pts_us: Option<i64>,
}

impl DiscontinuityJudge {
    /// Creates a new `DiscontinuityJudge` instance.
    /// 创建新的 `DiscontinuityJudge` 实例。
    pub fn new(max_gap_us: i64) -> Self {
        Self {
            max_gap_us: max_gap_us.max(0),
            last_pts_us: None,
        }
    }

    /// `observe` function of `DiscontinuityJudge`.
    /// `DiscontinuityJudge` 的 `observe` 函数。
    pub fn observe(&mut self, pts_us: i64) -> bool {
        let is_discontinuity = match self.last_pts_us {
            Some(last) => pts_us < last || (pts_us - last) > self.max_gap_us,
            None => false,
        };
        self.last_pts_us = Some(pts_us);
        is_discontinuity
    }
}

/// `TimestampValue` enumeration.
/// `TimestampValue` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampValue {
    Unwrapped(i64),
    Wrapped(u64),
}

/// Configuration for `Timestamp Normalizer`.
/// `Timestamp Normalizer` 的配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampNormalizerConfig {
    pub input_timebase: Timebase,
    pub output_timebase: Timebase,
    pub wrap_bits: Option<u8>,
    pub max_forward_gap_us: i64,
    pub default_fallback_step: i64,
    pub allow_negative_composition: bool,
}

impl TimestampNormalizerConfig {
    /// Creates a new `TimestampNormalizerConfig` instance.
    /// 创建新的 `TimestampNormalizerConfig` 实例。
    pub fn new(
        input_timebase: Timebase,
        output_timebase: Timebase,
        wrap_bits: Option<u8>,
    ) -> Result<Self, TimestampNormalizerConfigError> {
        if input_timebase.num == 0 || input_timebase.den == 0 {
            return Err(TimestampNormalizerConfigError::InvalidInputTimebase {
                num: input_timebase.num,
                den: input_timebase.den,
            });
        }
        if output_timebase.num == 0 || output_timebase.den == 0 {
            return Err(TimestampNormalizerConfigError::InvalidOutputTimebase {
                num: output_timebase.num,
                den: output_timebase.den,
            });
        }
        if let Some(bits) = wrap_bits {
            if bits == 0 || bits > 63 {
                return Err(TimestampNormalizerConfigError::InvalidWrapWidth(bits));
            }
        }
        Ok(Self {
            input_timebase,
            output_timebase,
            wrap_bits,
            max_forward_gap_us: 2_000_000,
            default_fallback_step: 1,
            allow_negative_composition: true,
        })
    }

    /// Returns a copy with `max forward gap us` set.
    /// 返回将 `max forward gap us` 设置后的副本。
    pub fn with_max_forward_gap_us(mut self, max_forward_gap_us: i64) -> Self {
        self.max_forward_gap_us = max_forward_gap_us.max(0);
        self
    }

    /// Returns a copy with `default fallback step` set.
    /// 返回将 `default fallback step` 设置后的副本。
    pub fn with_default_fallback_step(mut self, default_fallback_step: i64) -> Self {
        self.default_fallback_step = default_fallback_step.max(1);
        self
    }

    /// Returns a copy with `negative composition allowed` set.
    /// 返回将 `negative composition allowed` 设置后的副本。
    pub fn with_negative_composition_allowed(mut self, allow: bool) -> Self {
        self.allow_negative_composition = allow;
        self
    }
}

/// Error returned by `Timestamp Normalizer Config` operations.
/// `Timestamp Normalizer Config` 操作返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TimestampNormalizerConfigError {
    #[error("invalid input timebase {num}/{den}")]
    InvalidInputTimebase { num: u32, den: u32 },
    #[error("invalid output timebase {num}/{den}")]
    InvalidOutputTimebase { num: u32, den: u32 },
    #[error("invalid wrap width: {0}")]
    InvalidWrapWidth(u8),
}

/// Error returned by `Timestamp Normalize` operations.
/// `Timestamp Normalize` 操作返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TimestampNormalizeError {
    #[error("wrapped timestamp provided without configured wrap_bits")]
    WrappedTimestampWithoutConfig,
    #[error("wrapped timestamp {value} overflowed i64 after unwrap")]
    UnwrappedTimestampOverflow { value: u64 },
}

/// `TimestampAlert` enumeration.
/// `TimestampAlert` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampAlert {
    MissingDtsUsedFallback,
    MissingPtsDerivedFromDts,
    NonMonotonicDtsRepaired,
    PtsReorderObserved,
    TimelineDiscontinuityDetected,
    NegativeCompositionClamped,
    ResetApplied,
}

/// Mode selecting `Timestamp Normalize` behavior.
/// 选择 `Timestamp Normalize` 行为的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampNormalizeMode {
    NoTimestamp,
    DtsPts {
        dts: TimestampValue,
        pts: TimestampValue,
    },
    DtsWithCompositionOffset {
        dts: TimestampValue,
        composition_offset: Option<i64>,
    },
    PtsOnly {
        pts: TimestampValue,
    },
}

/// `TimestampNormalizeInput` data structure.
/// `TimestampNormalizeInput` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampNormalizeInput {
    pub mode: TimestampNormalizeMode,
    /// Optional frame duration in `input_timebase` ticks.
    /// Used by `PtsOnly` mode as the highest-priority DTS cadence hint.
    pub frame_duration: Option<i64>,
    /// Optional fallback step in `input_timebase` ticks for missing source
    /// timestamps.
    pub fallback_step: Option<i64>,
    pub is_video: bool,
    pub force_discontinuity: bool,
}

/// `TimestampNormalizeOutput` data structure.
/// `TimestampNormalizeOutput` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampNormalizeOutput {
    pub pts: i64,
    pub dts: i64,
    pub pts_us: i64,
    pub dts_us: i64,
    pub discontinuity: bool,
    pub alerts: SmallVec<[TimestampAlert; 4]>,
}

/// `TimestampNormalizer` data structure.
/// `TimestampNormalizer` 数据结构。
#[derive(Debug, Clone)]
pub struct TimestampNormalizer {
    config: TimestampNormalizerConfig,
    dts_unwrapper: Option<WrapUnwrapper>,
    pts_unwrapper: Option<WrapUnwrapper>,
    epoch_offset: Option<i64>,
    last_dts: Option<i64>,
    next_fallback_dts: Option<i64>,
    dts_generator: DtsGenerator,
    pending_reset: bool,
}

impl TimestampNormalizer {
    /// Creates a new `TimestampNormalizer` instance.
    /// 创建新的 `TimestampNormalizer` 实例。
    pub fn new(config: TimestampNormalizerConfig) -> Self {
        let dts_unwrapper = config.wrap_bits.map(|bits| match WrapUnwrapper::new(bits) {
            Ok(unwrapper) => unwrapper,
            Err(_) => unreachable!("wrap_bits is validated in config constructor"),
        });
        let pts_unwrapper = config.wrap_bits.map(|bits| match WrapUnwrapper::new(bits) {
            Ok(unwrapper) => unwrapper,
            Err(_) => unreachable!("wrap_bits is validated in config constructor"),
        });
        Self {
            config,
            dts_unwrapper,
            pts_unwrapper,
            epoch_offset: None,
            last_dts: None,
            next_fallback_dts: None,
            dts_generator: DtsGenerator::default(),
            pending_reset: false,
        }
    }

    /// `config` function of `TimestampNormalizer`.
    /// `TimestampNormalizer` 的 `config` 函数。
    pub fn config(&self) -> &TimestampNormalizerConfig {
        &self.config
    }

    /// Resets the state to its initial value.
    /// 将状态重置为初始值。
    pub fn reset(&mut self) {
        self.epoch_offset = None;
        self.last_dts = None;
        self.next_fallback_dts = None;
        self.pending_reset = true;
        self.dts_generator.reset();
        if let Some(unwrapper) = self.dts_unwrapper.as_mut() {
            unwrapper.reset();
        }
        if let Some(unwrapper) = self.pts_unwrapper.as_mut() {
            unwrapper.reset();
        }
    }

    /// Normalizes the value to a canonical form.
    /// 将值归一化为标准形式。
    pub fn normalize(
        &mut self,
        input: TimestampNormalizeInput,
    ) -> Result<TimestampNormalizeOutput, TimestampNormalizeError> {
        let fallback_step_raw = input
            .fallback_step
            .unwrap_or(self.config.default_fallback_step);
        let fallback_step = TimebaseConverter::convert(
            fallback_step_raw.max(1),
            self.config.input_timebase,
            self.config.output_timebase,
        )
        .max(1);
        let frame_duration = input.frame_duration.map(|value| {
            TimebaseConverter::convert(
                value.max(1),
                self.config.input_timebase,
                self.config.output_timebase,
            )
            .max(1)
        });
        let max_gap_ticks =
            Timebase::from_micros(self.config.output_timebase, self.config.max_forward_gap_us)
                .max(1);
        let is_pts_only_mode = matches!(input.mode, TimestampNormalizeMode::PtsOnly { .. });
        let (source_dts, source_pts, composition_offset) = match input.mode {
            TimestampNormalizeMode::NoTimestamp => (None, None, None),
            TimestampNormalizeMode::DtsPts { dts, pts } => (
                self.resolve_ticks(Some(dts), true)?,
                self.resolve_ticks(Some(pts), false)?,
                None,
            ),
            TimestampNormalizeMode::DtsWithCompositionOffset {
                dts,
                composition_offset,
            } => (
                self.resolve_ticks(Some(dts), true)?,
                None,
                composition_offset.map(|value| {
                    TimebaseConverter::convert(
                        value,
                        self.config.input_timebase,
                        self.config.output_timebase,
                    )
                }),
            ),
            TimestampNormalizeMode::PtsOnly { pts } => {
                (None, self.resolve_ticks(Some(pts), false)?, None)
            }
        };

        let mut alerts = SmallVec::<[TimestampAlert; 4]>::new();
        let mut discontinuity = input.force_discontinuity;
        if self.pending_reset {
            push_alert(&mut alerts, TimestampAlert::ResetApplied);
            discontinuity = true;
            self.pending_reset = false;
        }

        let mut dts = match source_dts {
            Some(value) => value,
            None => {
                push_alert(&mut alerts, TimestampAlert::MissingDtsUsedFallback);
                match source_pts {
                    Some(pts) if is_pts_only_mode => {
                        let generated = self.dts_generator.generate_from_pts_only(
                            pts,
                            fallback_step,
                            frame_duration,
                            max_gap_ticks,
                        );
                        if generated.reordered {
                            push_alert(&mut alerts, TimestampAlert::PtsReorderObserved);
                        }
                        if generated.discontinuity {
                            discontinuity = true;
                            push_alert(&mut alerts, TimestampAlert::TimelineDiscontinuityDetected);
                        }
                        generated.dts
                    }
                    Some(pts) => self.dts_generator.generate_monotonic_from_pts(pts),
                    None => self.next_fallback_dts.unwrap_or(0),
                }
            }
        };
        let epoch_offset = if source_dts.is_some() || source_pts.is_some() {
            *self.epoch_offset.get_or_insert(dts)
        } else {
            0
        };
        dts = dts.saturating_sub(epoch_offset);

        if let Some(last) = self.last_dts {
            if dts <= last {
                dts = last.saturating_add(1);
                push_alert(&mut alerts, TimestampAlert::NonMonotonicDtsRepaired);
            } else {
                let gap =
                    Timebase::to_micros(self.config.output_timebase, dts.saturating_sub(last));
                if gap > self.config.max_forward_gap_us {
                    discontinuity = true;
                    push_alert(&mut alerts, TimestampAlert::TimelineDiscontinuityDetected);
                }
            }
        }

        let mut pts = match source_pts {
            Some(value) => value.saturating_sub(epoch_offset),
            None => {
                push_alert(&mut alerts, TimestampAlert::MissingPtsDerivedFromDts);
                dts.saturating_add(composition_offset.unwrap_or(0))
            }
        };
        if source_pts.is_none() {
            pts = dts.saturating_add(composition_offset.unwrap_or(0));
        }
        if input.is_video && !self.config.allow_negative_composition && pts < dts {
            pts = dts;
            push_alert(&mut alerts, TimestampAlert::NegativeCompositionClamped);
        }

        self.last_dts = Some(dts);
        self.next_fallback_dts = Some(dts.saturating_add(fallback_step));

        let dts_us = Timebase::to_micros(self.config.output_timebase, dts);
        let pts_us = Timebase::to_micros(self.config.output_timebase, pts);
        Ok(TimestampNormalizeOutput {
            pts,
            dts,
            pts_us,
            dts_us,
            discontinuity,
            alerts,
        })
    }

    fn resolve_ticks(
        &mut self,
        value: Option<TimestampValue>,
        is_dts: bool,
    ) -> Result<Option<i64>, TimestampNormalizeError> {
        let Some(value) = value else {
            return Ok(None);
        };
        let raw_ticks = match value {
            TimestampValue::Unwrapped(value) => value,
            TimestampValue::Wrapped(raw) => {
                let unwrapper = if is_dts {
                    self.dts_unwrapper.as_mut()
                } else {
                    self.pts_unwrapper.as_mut()
                };
                let Some(unwrapper) = unwrapper else {
                    return Err(TimestampNormalizeError::WrappedTimestampWithoutConfig);
                };
                let unwrapped = unwrapper.unwrap(raw);
                i64::try_from(unwrapped).map_err(|_| {
                    TimestampNormalizeError::UnwrappedTimestampOverflow { value: unwrapped }
                })?
            }
        };
        Ok(Some(TimebaseConverter::convert(
            raw_ticks,
            self.config.input_timebase,
            self.config.output_timebase,
        )))
    }
}

fn push_alert(alerts: &mut SmallVec<[TimestampAlert; 4]>, alert: TimestampAlert) {
    if !alerts.contains(&alert) {
        alerts.push(alert);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timebase_roundtrip() {
        let tb = Timebase::new(1, 90_000);
        let raw = 9000;
        let us = Timebase::to_micros(tb, raw);
        assert_eq!(us, 100_000);
        assert_eq!(Timebase::from_micros(tb, us), raw);
    }

    #[test]
    fn unwraps_u32_wraparound() {
        let mut unwrap = WrapUnwrapper::new(32).expect("valid");
        let v1 = unwrap.unwrap(u32::MAX as u64 - 3);
        let v2 = unwrap.unwrap(2);
        assert!(v2 > v1);
    }

    #[test]
    fn dts_is_monotonic() {
        let mut gen = DtsGenerator::default();
        let d1 = gen.generate_monotonic_from_pts(1000);
        let d2 = gen.generate_monotonic_from_pts(1000);
        let d3 = gen.generate_monotonic_from_pts(999);
        assert!(d2 > d1);
        assert!(d3 > d2);
    }

    #[test]
    fn timestamp_normalizer_unwraps_wraparound_and_converts_timebase() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 1_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);
        let first = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts: TimestampValue::Wrapped((u32::MAX - 10) as u64),
                    composition_offset: None,
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("first");
        let second = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts: TimestampValue::Wrapped(20),
                    composition_offset: None,
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("second");
        assert!(second.dts > first.dts);
        assert!(second.dts_us > first.dts_us);
    }

    #[test]
    fn timestamp_normalizer_removes_random_rtp_epoch_per_track() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 1_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);

        let first = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Wrapped(3_895_818_000),
                    pts: TimestampValue::Wrapped(3_895_819_800),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("first");
        let second = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Wrapped(3_895_820_970),
                    pts: TimestampValue::Wrapped(3_895_822_770),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("second");

        assert_eq!(first.dts, 0);
        assert_eq!(first.pts, 20);
        assert_eq!(first.pts - first.dts, 20);
        assert_eq!(second.dts, 33);
        assert_eq!(second.pts - second.dts, 20);
    }

    #[test]
    fn timestamp_normalizer_repairs_non_monotonic_dts_without_discontinuity() {
        let config =
            TimestampNormalizerConfig::new(Timebase::new(1, 1_000), Timebase::new(1, 1_000), None)
                .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);
        let a = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Unwrapped(100),
                    pts: TimestampValue::Unwrapped(100),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("a");
        let b = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Unwrapped(99),
                    pts: TimestampValue::Unwrapped(99),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("b");
        assert!(!a.discontinuity);
        assert!(!b.discontinuity);
        assert!(b.dts > a.dts);
        assert!(b.alerts.contains(&TimestampAlert::NonMonotonicDtsRepaired));
    }

    #[test]
    fn timestamp_normalizer_keeps_or_clamps_negative_composition_by_policy() {
        let mut allow_negative = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(Timebase::new(1, 1_000), Timebase::new(1, 1_000), None)
                .expect("valid"),
        );
        let out_allow = allow_negative
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Unwrapped(1_000),
                    pts: TimestampValue::Unwrapped(980),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("allow");
        assert!(out_allow.pts < out_allow.dts);

        let mut clamp_negative = TimestampNormalizer::new(
            TimestampNormalizerConfig::new(Timebase::new(1, 1_000), Timebase::new(1, 1_000), None)
                .expect("valid")
                .with_negative_composition_allowed(false),
        );
        let out_clamp = clamp_negative
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsPts {
                    dts: TimestampValue::Unwrapped(1_000),
                    pts: TimestampValue::Unwrapped(980),
                },
                frame_duration: None,
                fallback_step: None,
                is_video: true,
                force_discontinuity: false,
            })
            .expect("clamp");
        assert_eq!(out_clamp.pts, out_clamp.dts);
        assert!(out_clamp
            .alerts
            .contains(&TimestampAlert::NegativeCompositionClamped));
    }

    #[test]
    fn timestamp_normalizer_supports_reset_and_fallback_generation() {
        let config =
            TimestampNormalizerConfig::new(Timebase::new(1, 1_000), Timebase::new(1, 1_000), None)
                .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);

        let first = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts: TimestampValue::Unwrapped(10),
                    composition_offset: None,
                },
                frame_duration: None,
                fallback_step: Some(40),
                is_video: false,
                force_discontinuity: false,
            })
            .expect("first");
        assert_eq!(first.pts, first.dts);

        let second = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::NoTimestamp,
                frame_duration: None,
                fallback_step: Some(40),
                is_video: false,
                force_discontinuity: false,
            })
            .expect("second");
        assert_eq!(second.dts, first.dts + 40);
        assert!(second
            .alerts
            .contains(&TimestampAlert::MissingDtsUsedFallback));

        normalizer.reset();
        let after_reset = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts: TimestampValue::Unwrapped(5),
                    composition_offset: None,
                },
                frame_duration: None,
                fallback_step: None,
                is_video: false,
                force_discontinuity: false,
            })
            .expect("after_reset");
        assert!(after_reset.discontinuity);
        assert!(after_reset.alerts.contains(&TimestampAlert::ResetApplied));
    }

    #[test]
    fn timestamp_normalizer_pts_only_mode_derives_monotonic_dts_for_video() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 90_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);

        let first = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::PtsOnly {
                    pts: TimestampValue::Wrapped(9_000),
                },
                frame_duration: Some(3_000),
                fallback_step: Some(3_000),
                is_video: true,
                force_discontinuity: false,
            })
            .expect("first");
        let second = normalizer
            .normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::PtsOnly {
                    pts: TimestampValue::Wrapped(3_000),
                },
                frame_duration: None,
                fallback_step: Some(3_000),
                is_video: true,
                force_discontinuity: false,
            })
            .expect("second");

        assert!(first
            .alerts
            .contains(&TimestampAlert::MissingDtsUsedFallback));
        assert!(second
            .alerts
            .contains(&TimestampAlert::MissingDtsUsedFallback));
        assert!(
            second.dts > first.dts,
            "pts-only mode must keep dts monotonic"
        );
    }

    #[test]
    fn timestamp_normalizer_pts_only_reorder_keeps_smooth_decode_cadence() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 90_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);
        let pts_in_decode_arrival_order = [0_i64, 9_000, 3_000, 6_000, 12_000];
        let mut out = Vec::new();

        for pts in pts_in_decode_arrival_order {
            let normalized = normalizer
                .normalize(TimestampNormalizeInput {
                    mode: TimestampNormalizeMode::PtsOnly {
                        pts: TimestampValue::Unwrapped(pts),
                    },
                    frame_duration: Some(3_000),
                    fallback_step: Some(3_000),
                    is_video: true,
                    force_discontinuity: false,
                })
                .expect("pts-only normalized");
            out.push(normalized);
        }

        assert_eq!(out[0].dts, 0);
        let first_step = out[1].dts - out[0].dts;
        let second_step = out[2].dts - out[1].dts;
        assert!(
            (2_990..=3_010).contains(&first_step),
            "expected smooth decode cadence near 3000 ticks, got {first_step}"
        );
        assert!(
            (2_990..=3_010).contains(&second_step),
            "expected smooth decode cadence near 3000 ticks, got {second_step}"
        );
        assert!(
            out[2].alerts.contains(&TimestampAlert::PtsReorderObserved),
            "small backward reorder should be observable but not discontinuity"
        );
        assert!(!out[2].discontinuity);
    }

    #[test]
    fn timestamp_normalizer_pts_only_forward_reorder_jump_does_not_stretch_dts() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 90_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);
        let pts_in_decode_arrival_order =
            [0_i64, 9_000, 3_000, 6_000, 12_000, 21_000, 15_000, 18_000];
        let mut out = Vec::new();

        for pts in pts_in_decode_arrival_order {
            out.push(
                normalizer
                    .normalize(TimestampNormalizeInput {
                        mode: TimestampNormalizeMode::PtsOnly {
                            pts: TimestampValue::Unwrapped(pts),
                        },
                        frame_duration: None,
                        fallback_step: Some(3_000),
                        is_video: true,
                        force_discontinuity: false,
                    })
                    .expect("pts-only normalized"),
            );
        }

        let reorder_index = out
            .iter()
            .position(|value| value.alerts.contains(&TimestampAlert::PtsReorderObserved))
            .expect("reorder should be observed");
        for window in out[reorder_index..].windows(2) {
            let step = window[1].dts - window[0].dts;
            assert!(
                (2_900..=3_100).contains(&step),
                "B-frame PTS jumps must not stretch decode cadence: step={step}, dts={:?}",
                out.iter().map(|value| value.dts).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn timestamp_normalizer_pts_only_large_jump_marks_discontinuity() {
        let config = TimestampNormalizerConfig::new(
            Timebase::new(1, 90_000),
            Timebase::new(1, 90_000),
            Some(32),
        )
        .expect("valid config");
        let mut normalizer = TimestampNormalizer::new(config);
        let pts_sequence = [0_i64, 3_000, 6_000, 300_000];
        let mut out = Vec::new();

        for pts in pts_sequence {
            out.push(
                normalizer
                    .normalize(TimestampNormalizeInput {
                        mode: TimestampNormalizeMode::PtsOnly {
                            pts: TimestampValue::Unwrapped(pts),
                        },
                        frame_duration: Some(3_000),
                        fallback_step: Some(3_000),
                        is_video: true,
                        force_discontinuity: false,
                    })
                    .expect("pts-only normalized"),
            );
        }

        let jumped = out.last().expect("jumped frame");
        assert!(jumped.discontinuity);
        assert!(jumped
            .alerts
            .contains(&TimestampAlert::TimelineDiscontinuityDetected));
        assert!(jumped.dts > out[2].dts);
    }
}
