#[cfg(test)]
use crate::prelude::*;
use core::fmt;
use smallvec::SmallVec;

/// Rational timebase for converting ticks to and from microseconds.
///
/// A timebase `num/den` means `value` ticks equals `value * num / den` seconds.
/// The codec layer uses microsecond (`1_000_000`) conversion as the neutral format.
///
/// 用于将刻度转换为微秒以及反向转换的有理 timebase。
///
/// `num/den` 表示 `value` 个刻度等于 `value * num / den` 秒。
/// codec 层以微秒（`1_000_000`）作为中性格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timebase {
    pub num: u32,
    pub den: u32,
}

impl Timebase {
    /// Create a new timebase with `num` seconds per `den` ticks.
    ///
    /// 创建新的 timebase，表示 `den` 个刻度对应 `num` 秒。
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// Convert `value` ticks in `tb` to microseconds.
    ///
    /// Uses 128-bit intermediate arithmetic to avoid overflow during the multiply.
    ///
    /// 将 `tb` 中的 `value` 个刻度转换为微秒。
    ///
    /// 使用 128 位中间运算避免乘法溢出。
    pub fn to_micros(tb: Timebase, value: i64) -> i64 {
        let num = i128::from(tb.num);
        let den = i128::from(tb.den.max(1));
        let v = i128::from(value);
        ((v * num * 1_000_000_i128) / den) as i64
    }

    /// Convert microseconds back to `tb` ticks.
    ///
    /// 将微秒转换回 `tb` 刻度。
    pub fn from_micros(tb: Timebase, micros: i64) -> i64 {
        let num = i128::from(tb.num.max(1));
        let den = i128::from(tb.den);
        let us = i128::from(micros);
        ((us * den) / (num * 1_000_000_i128)) as i64
    }
}

/// Opaque monotonic timestamp in microseconds.
///
/// Used for arrival-time based stamping and for scheduling that does not depend
/// on wall-clock or media timeline.
///
/// 以微秒为单位的单调时间戳。
///
/// 用于基于到达时间的打戳和调度，不依赖墙上时间或媒体时间线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonoTime {
    micros: u64,
}

impl MonoTime {
    pub const fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    pub const fn as_micros(self) -> u64 {
        self.micros
    }
}

/// Errors raised by timestamp unwrap or configuration construction.
///
/// 时间戳解绕或配置构造时产生的错误。
#[derive(Debug, thiserror::Error)]
pub enum TimestampError {
    #[error("invalid wrap width: {0}")]
    InvalidWrapWidth(u8),
}

/// Stateful unwrapper for timestamps that wrap around at a fixed bit width.
///
/// Tracks a high-order counter (`high`) and the last raw value. When a new raw
/// value jumps backward by more than half the wrap range, the counter is incremented
/// by one full period; when it jumps forward by more than half the range, it is
/// decremented. This correctly handles RTP/RTMP 32-bit wraparound and small jitters.
///
/// 在固定位宽回绕的时间戳的有状态解绕器。
///
/// 跟踪高位计数器（`high`）和上一个 raw 值。当新的 raw 值向后跳跃超过半周期时，
/// 计数器增加一个完整周期；向前跳跃超过半周期时减少。正确处理 RTP/RTMP 32 位回绕
/// 和轻微抖动。
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
    /// Create an unwrapper for `bits`-wide timestamps.
    ///
    /// 为 `bits` 位宽的时间戳创建解绕器。
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

    /// Unwrap a raw timestamp into a monotonically increasing 64-bit value.
    ///
    /// 将 raw 时间戳解绕为单调递增的 64 位值。
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

    /// Reset the unwrapper state, discarding the high counter and previous raw value.
    ///
    /// 重置解绕器状态，丢弃高位计数器和上一个 raw 值。
    pub fn reset(&mut self) {
        self.high = 0;
        self.last_raw = None;
    }
}

/// Strategy used to fill in missing timestamps.
///
/// `Source` prefers the original source PTS; `Arrival` uses the wall/monotonic time
/// of arrival; `SampleCount` and `FrameRateGuess` increment by `step_us` each frame.
///
/// 用于填补缺失时间戳的策略。
///
/// `Source` 优先使用原始源 PTS；`Arrival` 使用到达时的单调/墙上时间；
/// `SampleCount` 与 `FrameRateGuess` 每帧按 `step_us` 递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampAdjustMode {
    Source,
    Arrival,
    SampleCount,
    FrameRateGuess,
}

/// Generates monotonic presentation timestamps when the source timestamp is missing.
///
/// The generated value is never smaller than the previous one, so it is safe to use
/// for streams that lack explicit timing or that contain non-monotonic source stamps.
///
/// 在源时间戳缺失时生成单调的展示时间戳。
///
/// 生成的值不会小于上一个值，因此可用于缺少显式时间或源时间戳非单调的流。
#[derive(Debug, Clone)]
pub struct StampAdjust {
    mode: StampAdjustMode,
    last_pts_us: Option<i64>,
    step_us: i64,
}

impl StampAdjust {
    /// Create a new adjuster with the given mode and per-frame step.
    ///
    /// 使用指定模式和每帧步长创建新的 adjuster。
    pub fn new(mode: StampAdjustMode, step_us: i64) -> Self {
        Self {
            mode,
            last_pts_us: None,
            step_us: step_us.max(1),
        }
    }

    /// Produce the next timestamp in microseconds.
    ///
    /// The selected mode determines the base value; if it would not advance past the
    /// previous timestamp, it is bumped by `step_us` to preserve monotonicity.
    ///
    /// 生成下一个微秒时间戳。
    ///
    /// 所选模式决定基础值；若其未超过上一时间戳，则按 `step_us` 递增以保持单调性。
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

/// Generates monotonic decode timestamps when the source only provides PTS.
///
/// For streams with both DTS and PTS, `generate_monotonic_from_pts` ensures the
/// output DTS never decreases. For `PtsOnly` streams, `generate_from_pts_only`
/// estimates frame cadence by smoothing inter-frame gaps and detects reordering
/// and discontinuities.
///
/// 在源只提供 PTS 时生成单调解码时间戳。
///
/// 对于同时有 DTS 和 PTS 的流，`generate_monotonic_from_pts` 保证输出 DTS 不递减。
/// 对于 `PtsOnly` 流，`generate_from_pts_only` 通过平滑帧间间隔估算帧节奏，并检测
/// 重排和不连续。
#[derive(Debug, Default, Clone)]
pub struct DtsGenerator {
    last_dts: Option<i64>,
    last_source_pts: Option<i64>,
    smoothed_step: Option<i64>,
    pts_reorder_seen: bool,
}

/// Internal result of deriving a DTS from a single PTS-only sample.
///
/// `reordered` means the new PTS is behind the previous one; `discontinuity`
/// means the gap is large enough to reset the cadence estimator.
///
/// 从单一样本 PTS 推导 DTS 的内部结果。
///
/// `reordered` 表示新 PTS 位于前一个 PTS 之前；`discontinuity` 表示间隔足够大，
/// 需要重置节奏估算器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PtsOnlyDtsGeneration {
    dts: i64,
    reordered: bool,
    discontinuity: bool,
}

impl DtsGenerator {
    /// Ensures the returned DTS is strictly greater than the previous one.
    ///
    /// If the supplied PTS is not greater than the last DTS, the next value is
    /// `last + 1`. This is used when the source already provides a usable PTS but
    /// a monotonic DTS is required.
    ///
    /// 确保返回的 DTS 严格大于上一个 DTS。
    ///
    /// 若提供的 PTS 不大于上一 DTS，则返回 `last + 1`。
    /// 用于源已提供可用 PTS 但需要单调 DTS 的场景。
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

    /// Derive a DTS from a stream that only provides PTS.
    ///
    /// This is the most complex path: it detects large jumps (discontinuities),
    /// negative deltas (reorders), and smooths the observed frame step. It uses the
    /// provided `frame_duration` as the highest-priority cadence hint when available.
    ///
    /// 从只提供 PTS 的流中推导 DTS。
    ///
    /// 这是最复杂的路径：检测大跳跃（不连续）、负增量（重排）并平滑观察到的帧步长。
    /// 当提供 `frame_duration` 时，它作为最高优先级的节奏提示。
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

    /// Reset the generator state, discarding history and smoothed step.
    ///
    /// 重置生成器状态，丢弃历史和平滑步长。
    pub fn reset(&mut self) {
        self.last_dts = None;
        self.last_source_pts = None;
        self.smoothed_step = None;
        self.pts_reorder_seen = false;
    }
}

/// Stateless helper for converting a value between two timebases.
///
/// It converts to microseconds first, then from microseconds to the destination
/// timebase. This keeps the conversion simple and avoids per-codec factors.
///
/// 用于在两个 timebase 之间转换值的无状态辅助函数。
///
/// 先转换为微秒，再从微秒转换到目标 timebase。保持转换简单并避免每个编解码器的因子。
pub struct TimebaseConverter;

impl TimebaseConverter {
    /// Convert `value` from `src` timebase to `dst` timebase.
    ///
    /// 将 `value` 从 `src` timebase 转换到 `dst` timebase。
    pub fn convert(value: i64, src: Timebase, dst: Timebase) -> i64 {
        let us = Timebase::to_micros(src, value);
        Timebase::from_micros(dst, us)
    }
}

/// Detects timeline discontinuities based on PTS gaps.
///
/// A discontinuity is declared when the current PTS is before the previous one or
/// when the forward gap exceeds `max_gap_us`. This is used by ingress to reset
/// decode state and signal downstream consumers.
///
/// 基于 PTS 间隔检测时间线不连续。
///
/// 当当前 PTS 早于上一个 PTS，或正向间隔超过 `max_gap_us` 时，声明不连续。
/// 入口侧用它重置解码状态并通知下游消费者。
#[derive(Debug, Clone)]
pub struct DiscontinuityJudge {
    max_gap_us: i64,
    last_pts_us: Option<i64>,
}

impl DiscontinuityJudge {
    /// Create a judge with the given maximum forward gap in microseconds.
    ///
    /// 用指定的最大正向间隔（微秒）创建 judge。
    pub fn new(max_gap_us: i64) -> Self {
        Self {
            max_gap_us: max_gap_us.max(0),
            last_pts_us: None,
        }
    }

    /// Observe a new PTS and return whether it represents a discontinuity.
    ///
    /// 观察新的 PTS 并返回其是否表示不连续。
    pub fn observe(&mut self, pts_us: i64) -> bool {
        let is_discontinuity = match self.last_pts_us {
            Some(last) => pts_us < last || (pts_us - last) > self.max_gap_us,
            None => false,
        };
        self.last_pts_us = Some(pts_us);
        is_discontinuity
    }
}

/// A timestamp that may be already unwrapped or still wrapped.
///
/// Wrapped values are passed through the `WrapUnwrapper` configured in the
/// normalizer. Unwrapped values are used directly.
///
/// 可能已经解绕或仍包裹的时间戳。
///
/// 包裹值通过 normalizer 中配置的 `WrapUnwrapper` 解绕；解绕值直接使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampValue {
    Unwrapped(i64),
    Wrapped(u64),
}

/// Configuration for the timestamp normalizer.
///
/// Defines input/output timebases, optional wrap width, the maximum forward gap
/// that is considered a discontinuity, and whether negative composition times
/// (`pts < dts`) are allowed for video.
///
/// 时间戳归一化器配置。
///
/// 定义输入/输出 timebase、可选回绕位宽、被视为不连续的最大正向间隔，
/// 以及是否允许视频出现负合成时间（`pts < dts`）。
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
    /// Create a validated normalizer config.
    ///
    /// Rejects zero timebases and invalid wrap widths. Defaults to a 2-second forward
    /// gap threshold and a 1-tick fallback step.
    ///
    /// 创建经过校验的归一化器配置。
    ///
    /// 拒绝零 timebase 和无效回绕位宽。默认 2 秒正向间隔阈值和 1 刻度回退步长。
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

    pub fn with_max_forward_gap_us(mut self, max_forward_gap_us: i64) -> Self {
        self.max_forward_gap_us = max_forward_gap_us.max(0);
        self
    }

    pub fn with_default_fallback_step(mut self, default_fallback_step: i64) -> Self {
        self.default_fallback_step = default_fallback_step.max(1);
        self
    }

    pub fn with_negative_composition_allowed(mut self, allow: bool) -> Self {
        self.allow_negative_composition = allow;
        self
    }
}

/// Errors from constructing `TimestampNormalizerConfig`.
///
/// 构造 `TimestampNormalizerConfig` 时产生的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TimestampNormalizerConfigError {
    #[error("invalid input timebase {num}/{den}")]
    InvalidInputTimebase { num: u32, den: u32 },
    #[error("invalid output timebase {num}/{den}")]
    InvalidOutputTimebase { num: u32, den: u32 },
    #[error("invalid wrap width: {0}")]
    InvalidWrapWidth(u8),
}

/// Errors returned by `TimestampNormalizer::normalize`.
///
/// `TimestampNormalizer::normalize` 返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TimestampNormalizeError {
    #[error("wrapped timestamp provided without configured wrap_bits")]
    WrappedTimestampWithoutConfig,
    #[error("wrapped timestamp {value} overflowed i64 after unwrap")]
    UnwrappedTimestampOverflow { value: u64 },
}

/// Signals emitted by the normalizer when it repairs or adjusts timestamps.
///
/// Alerts are advisory and do not fail normalization, but they let callers observe
/// fallback behavior, reordering, discontinuities and clamping.
///
/// 归一化器修复或调整时间戳时发出的信号。
///
/// 告警是建议性的，不会导致归一化失败，但让调用方观察回退行为、重排、不连续和裁剪。
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

/// Input shape accepted by the timestamp normalizer.
///
/// `NoTimestamp` produces fallback values; `DtsPts` provides both; `DtsWithCompositionOffset`
/// provides DTS plus a relative composition offset; `PtsOnly` requires DTS generation.
///
/// 时间戳归一器接受的输入形态。
///
/// `NoTimestamp` 生成回退值；`DtsPts` 同时提供 DTS 和 PTS；
/// `DtsWithCompositionOffset` 提供 DTS 加相对合成偏移；`PtsOnly` 需要生成 DTS。
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

/// One input sample to `TimestampNormalizer::normalize`.
///
/// Carries the timestamp mode, optional frame duration, optional fallback step,
/// whether the frame is video, and an explicit discontinuity flag.
///
/// `TimestampNormalizer::normalize` 的单个输入样本。
///
/// 携带时间戳模式、可选帧时长、可选回退步长、是否为视频帧以及显式不连续标志。
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

/// Normalized timestamp result for one frame.
///
/// Contains PTS/DTS in output timebase and microseconds, a discontinuity flag, and
/// any alerts raised during normalization.
///
/// 单帧的归一化时间戳结果。
///
/// 包含输出 timebase 和微秒下的 PTS/DTS、不连续标志以及归一化期间产生的告警。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampNormalizeOutput {
    pub pts: i64,
    pub dts: i64,
    pub pts_us: i64,
    pub dts_us: i64,
    pub discontinuity: bool,
    pub alerts: SmallVec<[TimestampAlert; 4]>,
}

/// Converts protocol-specific timestamps into a normalized timeline.
///
/// Handles timebase conversion, timestamp unwrap, DTS generation from PTS-only
/// sources, monotonicity repair, discontinuity detection, and composition-time
/// clamping. It is the central place where all ingress timing becomes comparable.
///
/// 将协议特定时间戳转换为归一化时间线的组件。
///
/// 处理 timebase 转换、时间戳解绕、从仅 PTS 源生成 DTS、单调性修复、不连续检测
/// 以及合成时间裁剪。所有入口时间戳在此变得可比较。
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
    /// Construct a normalizer from a validated config.
    ///
    /// 从已校验的配置构造归一化器。
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

    /// Return a reference to the underlying config.
    ///
    /// 返回底层配置的引用。
    pub fn config(&self) -> &TimestampNormalizerConfig {
        &self.config
    }

    /// Reset all internal state so the next sample starts a new timeline.
    ///
    /// The next `normalize` call will set `discontinuity` and emit `ResetApplied`.
    ///
    /// 重置所有内部状态，使下一个样本开始新的时间线。
    ///
    /// 下一次 `normalize` 调用会设置 `discontinuity` 并发出 `ResetApplied`。
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

    /// Normalize one frame's timestamps into the configured output timebase.
    ///
    /// The pipeline is:
    /// 1. Resolve wrapped/unwrapped source timestamps and convert to output timebase.
    /// 2. If DTS is missing, derive it from PTS using `DtsGenerator`.
    /// 3. Subtract an epoch offset so the timeline starts near zero.
    /// 4. Enforce monotonic DTS and detect forward gaps.
    /// 5. Compute PTS from DTS + composition offset if needed, clamping for video.
    ///
    /// 将单帧时间戳归一化到配置的输出 timebase。
    ///
    /// 流程：
    /// 1. 解析包裹/解绕源时间戳并转换到输出 timebase。
    /// 2. 若 DTS 缺失，使用 `DtsGenerator` 从 PTS 推导。
    /// 3. 减去 epoch 偏移，使时间线从接近零处开始。
    /// 4. 强制 DTS 单调并检测正向间隔。
    /// 5. 需要时从 DTS + 合成偏移计算 PTS，并对视频进行裁剪。
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

    /// Convert a `TimestampValue` into output timebase ticks, unwrapping if needed.
    ///
    /// 将 `TimestampValue` 转换为输出 timebase 刻度，需要时解绕。
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
