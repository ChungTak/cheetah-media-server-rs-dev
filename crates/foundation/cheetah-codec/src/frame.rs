use crate::prelude::*;
use bitflags::bitflags;
use bytes::Bytes;
use smallvec::SmallVec;

use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackId};

/// Canonical payload layout used by [`AVFrame`] after ingress normalization.
///
/// Codecs share a small set of wire formats so the downstream engine does not
/// need to handle protocol-specific encapsulation. For example H.264/5/6 frames
/// are always stored in Annex-B start-code form inside the engine.
///
/// [`AVFrame`] 在入口归一化后使用的标准负载格式。
///
/// 编解码器共享少量内部线性格式，使下游引擎无需处理协议相关的封装。
/// 例如，H.264/5/6 帧在引擎内部统一使用 Annex-B 起始码形式存储。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    CanonicalH26x,
    CanonicalAv1Obu,
    CanonicalVp8Frame,
    CanonicalVp9Frame,
    MjpegFrame,
    AacRaw,
    AdpcmPacket,
    OpusPacket,
    G711Packet,
    Mp2Frame,
    Mp3Frame,
    WebVttPacket,
    DataPacket,
    Unknown,
}

/// Origin of a frame inside the pipeline.
///
/// This distinguishes externally ingested media, internally generated filler
/// frames (e.g. mute audio), recovered frames after packet loss, and relayed
/// frames that did not pass through ingress normalization.
///
/// 帧在流水线中的来源。
///
/// 用于区分外部接入的媒体、内部生成的填充帧（如静音音频）、
/// 丢包后恢复的帧，以及未经过入口归一化的转发帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOrigin {
    Ingest,
    Relay,
    Generated,
    Recovered,
}

/// Cross-reference between an RTP timestamp and an RTCP sender report.
///
/// `lsr` (last sender report) and the local unix receive time let the receiver
/// map RTP clock ticks to wall-clock time for A/V sync and jitter calculations.
///
/// RTP 时间戳与 RTCP 发送者报告之间的交叉引用。
///
/// `lsr`（上一个发送者报告）和本地 Unix 接收时间使接收端能够将 RTP 时钟刻度
/// 映射到墙上时间，用于音视频同步和抖动计算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpRtcpMapping {
    pub lsr: u32,
    pub arrival_unix_micros: u64,
}

/// RTP timestamp with both the raw 32-bit wire value and the unwrapped 64-bit value.
///
/// RTP timestamps wrap at 2^32. The unwrapped value is used for ordering and
/// timebase conversion while the raw value is preserved for egress or RTCP.
///
/// 包含原始 32 位线值和解绕后 64 位值的 RTP 时间戳。
///
/// RTP 时间戳在 2^32 处回绕。解绕后的值用于排序和 timebase 转换，
/// 原始值则保留用于出口或 RTCP。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpTimestamp {
    pub raw_timestamp: u32,
    pub unwrapped_timestamp: u64,
    pub epoch_offset: i64,
    pub sequence_number: Option<u16>,
    pub rtcp_mapping: Option<RtpRtcpMapping>,
}

impl RtpTimestamp {
    /// Build an RTP timestamp from the raw 32-bit wire value and the unwrapped 64-bit value.
    ///
    /// The epoch offset is the saturating difference between the unwrapped and raw values.
    ///
    /// 根据原始 32 位线值和解绕后 64 位值构建 RTP 时间戳。
    ///
    /// epoch 偏移是 unwrapped 与 raw 之间的饱和差值。
    pub fn new(raw_timestamp: u32, unwrapped_timestamp: u64) -> Self {
        Self {
            raw_timestamp,
            unwrapped_timestamp,
            epoch_offset: saturating_epoch_offset(unwrapped_timestamp, raw_timestamp),
            sequence_number: None,
            rtcp_mapping: None,
        }
    }
}

/// RTMP timestamp with the raw 32-bit wire value and the unwrapped 64-bit value.
///
/// RTMP timestamps are in milliseconds and wrap at 2^32. The unwrapped value
/// keeps a linear timeline while the raw value is preserved for egress.
///
/// 包含原始 32 位线值和解绕后 64 位值的 RTMP 时间戳。
///
/// RTMP 时间戳以毫秒为单位，在 2^32 处回绕。解绕后的值保持线性时间线，
/// 原始值则保留用于出口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtmpTimestamp {
    pub raw_timestamp_ms: u32,
    pub unwrapped_timestamp_ms: u64,
    pub epoch_offset_ms: i64,
}

impl RtmpTimestamp {
    /// Build an RTMP timestamp from the raw millisecond wire value and the unwrapped value.
    ///
    /// 根据原始毫秒线值和解绕后值构建 RTMP 时间戳。
    pub fn new(raw_timestamp_ms: u32, unwrapped_timestamp_ms: u64) -> Self {
        Self {
            raw_timestamp_ms,
            unwrapped_timestamp_ms,
            epoch_offset_ms: saturating_epoch_offset(unwrapped_timestamp_ms, raw_timestamp_ms),
        }
    }
}

/// Source-specific timestamp carried as side data of an [`AVFrame`].
///
/// This preserves the original wire timestamp format so the egress path can
/// map the normalized frame back to protocol-specific timing.
///
/// 作为 [`AVFrame`] 边数据携带的源相关时间戳。
///
/// 它保留原始线格式时间戳，使出口路径能够将归一化帧映射回协议相关的时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTimestamp {
    Rtp(RtpTimestamp),
    Rtmp(RtmpTimestamp),
}

/// Additional metadata attached to an [`AVFrame`] that is not part of the payload.
///
/// Side data carries provenance, parameter-set digests, sequence numbers and
/// opaque protocol tags without changing the canonical frame payload.
///
/// 附加到 [`AVFrame`] 的额外元数据，不属于负载本身。
///
/// 边数据携带来源、参数集摘要、序列号和不透明的协议标记，
/// 而无需更改标准帧负载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameSideData {
    ParameterSetDigest(u64),
    SequenceNumber(u64),
    SourceTimestamp(SourceTimestamp),
    Metadata { key: String, value: String },
    Opaque(Bytes),
}

bitflags! {
    /// Bit flags describing the role and state of an [`AVFrame`].
    ///
    /// Flags indicate key frames, parameter-set frames, access-unit boundaries,
    /// discontinuities, generated filler frames and corruption/droppability.
    ///
    /// 描述 [`AVFrame`] 角色与状态的位标志。
    ///
    /// 标志用于指示关键帧、参数集帧、访问单元边界、不连续、
    /// 生成填充帧以及损坏/可丢弃状态。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FrameFlags: u32 {
        const KEY = 1 << 0;
        const CONFIG = 1 << 1;
        const START_OF_AU = 1 << 2;
        const END_OF_AU = 1 << 3;
        const B_FRAME = 1 << 4;
        const NON_PICTURE = 1 << 5;
        const DISCONTINUITY = 1 << 6;
        const GENERATED = 1 << 7;
        const CORRUPTED = 1 << 8;
        const DROPPABLE = 1 << 9;
    }
}

/// Errors raised when an [`AVFrame`] timing or timebase is invalid.
///
/// These errors are checked during ingress normalization and before any
/// timebase conversion to prevent overflow or negative composition times.
///
/// [`AVFrame`] 时间或 timebase 无效时引发的错误。
///
/// 这些错误在入口归一化期间以及任何 timebase 转换之前进行检查，
/// 以防止溢出或负合成时间。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FrameTimingError {
    #[error("track {track_id:?} has invalid timebase {timebase_num}/{timebase_den}")]
    InvalidTimebase {
        track_id: TrackId,
        timebase_num: u32,
        timebase_den: u32,
    },
    #[error("track {track_id:?} has negative duration {duration}")]
    NegativeDuration { track_id: TrackId, duration: i64 },
    #[error("track {track_id:?} duration micros overflow for duration {duration}")]
    DurationMicrosOverflow { track_id: TrackId, duration: i64 },
    #[error("video track {track_id:?} has pts < dts without B-frame flag: pts={pts}, dts={dts}")]
    NegativeCompositionWithoutBFrame {
        track_id: TrackId,
        pts: i64,
        dts: i64,
    },
}

/// Unified media frame exchanged inside the engine.
///
/// `AVFrame` separates protocol-specific ingress from protocol-specific
/// egress by normalizing payloads into canonical formats, storing timing in
/// `timebase` ticks plus microseconds, and carrying side data for provenance.
///
/// 引擎内部统一的媒体帧。
///
/// `AVFrame` 通过将负载归一化为标准格式、以 `timebase` 刻度加微秒存储时间，
/// 并携带来源边数据，将协议相关的入口与协议相关的出口分离开。
#[derive(Debug, Clone)]
pub struct AVFrame {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub format: FrameFormat,
    /// Decoding/presentation timeline expressed in `timebase` ticks.
    /// Invariant: for non-B video frames, `pts >= dts`.
    pub pts: i64,
    pub dts: i64,
    /// Canonical media time unit for this frame. Must be non-zero.
    pub timebase: Timebase,
    /// `pts/dts` converted to microseconds for cross-protocol scheduling/logging.
    pub pts_us: i64,
    pub dts_us: i64,
    /// Frame duration expressed in `timebase` ticks and microseconds.
    /// `duration == 0` means "unknown/unspecified".
    pub duration: i64,
    pub duration_us: i64,
    /// Key/random-access, config frame, discontinuity and corruption semantics.
    pub flags: FrameFlags,
    pub payload: Bytes,
    pub side_data: SmallVec<[FrameSideData; 4]>,
    pub origin: FrameOrigin,
}

impl AVFrame {
    /// Construct a new frame with normalized microsecond timing.
    ///
    /// `pts_us` and `dts_us` are computed eagerly from the supplied `timebase`
    /// so downstream code can compare and schedule frames without repeated conversion.
    ///
    /// 构造一个具有归一化微秒时间的新帧。
    ///
    /// `pts_us` 和 `dts_us` 根据提供的 `timebase` 预先计算，
    /// 以便下游代码无需重复转换即可比较和调度帧。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        track_id: TrackId,
        media_kind: MediaKind,
        codec: CodecId,
        format: FrameFormat,
        pts: i64,
        dts: i64,
        timebase: Timebase,
        payload: Bytes,
    ) -> Self {
        let pts_us = Timebase::to_micros(timebase, pts);
        let dts_us = Timebase::to_micros(timebase, dts);
        Self {
            track_id,
            media_kind,
            codec,
            format,
            pts,
            dts,
            timebase,
            pts_us,
            dts_us,
            duration: 0,
            duration_us: 0,
            flags: FrameFlags::empty(),
            payload,
            side_data: SmallVec::new(),
            origin: FrameOrigin::Ingest,
        }
    }

    pub fn is_key_frame(&self) -> bool {
        self.flags.contains(FrameFlags::KEY)
    }

    pub fn mark_discontinuity(&mut self) {
        self.flags.insert(FrameFlags::DISCONTINUITY);
    }

    /// Store or replace the source timestamp side data for this frame.
    ///
    /// If a `SourceTimestamp` entry already exists it is updated in place;
    /// otherwise a new entry is pushed, keeping the side-data list compact.
    ///
    /// 存储或替换此帧的源时间戳边数据。
    ///
    /// 如果已存在 `SourceTimestamp` 条目则原地更新；否则压入新条目，
    /// 以保持边数据列表紧凑。
    pub fn set_source_timestamp(&mut self, source_timestamp: SourceTimestamp) {
        if let Some(existing) = self
            .side_data
            .iter_mut()
            .find(|entry| matches!(entry, FrameSideData::SourceTimestamp(_)))
        {
            *existing = FrameSideData::SourceTimestamp(source_timestamp);
            return;
        }
        self.side_data
            .push(FrameSideData::SourceTimestamp(source_timestamp));
    }

    /// Look up the source timestamp side data, if any.
    ///
    /// 查找源时间戳边数据（如果存在）。
    pub fn source_timestamp(&self) -> Option<SourceTimestamp> {
        self.side_data.iter().find_map(|entry| {
            let FrameSideData::SourceTimestamp(source) = entry else {
                return None;
            };
            Some(*source)
        })
    }

    pub fn with_duration(mut self, duration: i64) -> Result<Self, FrameTimingError> {
        self.set_duration(duration)?;
        Ok(self)
    }

    /// Set the frame duration and update the microsecond duration.
    ///
    /// Rejects negative durations and validates the timebase before converting
    /// the duration to microseconds so overflow cannot be silently ignored.
    ///
    /// 设置帧时长并更新微秒时长。
    ///
    /// 拒绝负时长，并在将时长转换为微秒之前验证 timebase，
    /// 以避免溢出被静默忽略。
    pub fn set_duration(&mut self, duration: i64) -> Result<(), FrameTimingError> {
        if duration < 0 {
            return Err(FrameTimingError::NegativeDuration {
                track_id: self.track_id,
                duration,
            });
        }
        self.ensure_valid_timebase()?;
        self.duration = duration;
        self.duration_us = self.duration_to_micros_checked(duration)?;
        Ok(())
    }

    pub fn composition_time(&self) -> i64 {
        self.pts.saturating_sub(self.dts)
    }

    pub fn composition_time_us(&self) -> i64 {
        self.pts_us.saturating_sub(self.dts_us)
    }

    /// Verify that the frame timing is internally consistent and safe to process.
    ///
    /// This checks the timebase, rejects negative durations, prevents microsecond
    /// overflow, and enforces `pts >= dts` for non-B-frame video.
    ///
    /// 验证帧时间是否内部一致且可安全处理。
    ///
    /// 检查 timebase、拒绝负时长、防止微秒溢出，
    /// 并对非 B 帧视频强制 `pts >= dts`。
    pub fn validate_media_timing(&self) -> Result<(), FrameTimingError> {
        self.ensure_valid_timebase()?;
        if self.duration < 0 {
            return Err(FrameTimingError::NegativeDuration {
                track_id: self.track_id,
                duration: self.duration,
            });
        }
        let _ = self.duration_to_micros_checked(self.duration)?;
        if self.media_kind == MediaKind::Video
            && self.pts < self.dts
            && !self.flags.contains(FrameFlags::B_FRAME)
        {
            return Err(FrameTimingError::NegativeCompositionWithoutBFrame {
                track_id: self.track_id,
                pts: self.pts,
                dts: self.dts,
            });
        }
        Ok(())
    }

    /// Ensure the timebase numerator and denominator are non-zero.
    ///
    /// A zero timebase would make all downstream duration conversions invalid.
    ///
    /// 确保 timebase 的分子和分母均非零。
    ///
    /// 零 timebase 将使所有下游时长转换无效。
    fn ensure_valid_timebase(&self) -> Result<(), FrameTimingError> {
        if self.timebase.num == 0 || self.timebase.den == 0 {
            return Err(FrameTimingError::InvalidTimebase {
                track_id: self.track_id,
                timebase_num: self.timebase.num,
                timebase_den: self.timebase.den,
            });
        }
        Ok(())
    }

    /// Convert a duration in `timebase` ticks to microseconds with overflow checks.
    ///
    /// Uses 128-bit intermediate arithmetic to detect multiplication and i64
    /// overflow before the final value is stored.
    ///
    /// 将 `timebase` 刻度表示的时长转换为微秒，并检查溢出。
    ///
    /// 使用 128 位中间运算，在最终值存储之前检测乘法和 i64 溢出。
    fn duration_to_micros_checked(&self, duration: i64) -> Result<i64, FrameTimingError> {
        let scaled = i128::from(duration)
            .checked_mul(i128::from(self.timebase.num))
            .and_then(|v| v.checked_mul(1_000_000_i128))
            .ok_or(FrameTimingError::DurationMicrosOverflow {
                track_id: self.track_id,
                duration,
            })?;
        let micros = scaled / i128::from(self.timebase.den);
        i64::try_from(micros).map_err(|_| FrameTimingError::DurationMicrosOverflow {
            track_id: self.track_id,
            duration,
        })
    }
}

/// Compute the saturating offset between an unwrapped and a raw timestamp.
///
/// The result is clamped to `i64` range so minor overflow cannot corrupt the epoch.
///
/// 计算 unwrapped 时间戳与 raw 时间戳之间的饱和偏移。
///
/// 结果被限制在 `i64` 范围内，以避免轻微溢出破坏 epoch。
fn saturating_epoch_offset(unwrapped_timestamp: u64, raw_timestamp: u32) -> i64 {
    let delta = i128::from(unwrapped_timestamp).saturating_sub(i128::from(raw_timestamp));
    delta.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_updates_micros_and_prevents_negative_values() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            10,
            10,
            tb,
            Bytes::from_static(b"payload"),
        )
        .with_duration(33)
        .expect("positive duration should be accepted");
        assert_eq!(frame.duration, 33);
        assert_eq!(frame.duration_us, 33_000);

        let err = frame
            .with_duration(-1)
            .expect_err("negative duration must fail");
        assert!(matches!(err, FrameTimingError::NegativeDuration { .. }));
    }

    #[test]
    fn composition_time_is_non_negative_for_normal_video_paths() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            20,
            10,
            tb,
            Bytes::from_static(b"payload"),
        );
        assert_eq!(frame.composition_time_us(), 10_000);
    }

    #[test]
    fn detects_negative_composition_without_b_frame_flag() {
        let tb = Timebase::new(1, 1_000);
        let frame = AVFrame::new(
            TrackId(3),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            10,
            20,
            tb,
            Bytes::from_static(b"payload"),
        );

        let err = frame
            .validate_media_timing()
            .expect_err("pts < dts without B-frame flag should be rejected");
        assert!(matches!(
            err,
            FrameTimingError::NegativeCompositionWithoutBFrame {
                track_id: TrackId(3),
                ..
            }
        ));
    }

    #[test]
    fn source_timestamp_side_data_can_be_set_and_read() {
        let tb = Timebase::new(1, 1_000);
        let mut frame = AVFrame::new(
            TrackId(7),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            90,
            60,
            tb,
            Bytes::from_static(b"payload"),
        );

        let mut rtp = RtpTimestamp::new(4_321, 4_321);
        rtp.sequence_number = Some(77);
        rtp.rtcp_mapping = Some(RtpRtcpMapping {
            lsr: 0x0011_2233,
            arrival_unix_micros: 123_456,
        });
        frame.set_source_timestamp(SourceTimestamp::Rtp(rtp));

        assert_eq!(frame.source_timestamp(), Some(SourceTimestamp::Rtp(rtp)));
        assert_eq!(
            frame.side_data.len(),
            1,
            "source timestamp should be stored as a single side-data item"
        );
    }

    #[test]
    fn source_timestamp_setter_replaces_existing_entry() {
        let tb = Timebase::new(1, 1_000);
        let mut frame = AVFrame::new(
            TrackId(8),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            10,
            10,
            tb,
            Bytes::from_static(b"payload"),
        );

        frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(30, 30)));
        frame.set_source_timestamp(SourceTimestamp::Rtmp(RtmpTimestamp::new(60, 90)));

        assert_eq!(
            frame.source_timestamp(),
            Some(SourceTimestamp::Rtmp(RtmpTimestamp::new(60, 90)))
        );
        assert_eq!(
            frame.side_data.len(),
            1,
            "setting source timestamp twice must replace the previous entry"
        );
    }
}
