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
    /// H.264/H.265/H.266 Annex-B start-code form.
    /// H.264/H.265/H.266 Annex-B 起始码形式。
    CanonicalH26x,
    /// AV1 Open Bitstream Unit sequence.
    /// AV1 开放比特流单元序列。
    CanonicalAv1Obu,
    /// VP8 raw frame (whole frame, not partitioned).
    /// VP8 原始帧（完整帧，未分片）。
    CanonicalVp8Frame,
    /// VP9 raw frame (whole frame, not super-frame).
    /// VP9 原始帧（完整帧，非 super-frame）。
    CanonicalVp9Frame,
    /// Motion JPEG frame.
    /// Motion JPEG 帧。
    MjpegFrame,
    /// AAC audio access unit without ADTS/LOAS header.
    /// 无 ADTS/LOAS 头的 AAC 音频访问单元。
    AacRaw,
    /// ADPCM audio packet.
    /// ADPCM 音频包。
    AdpcmPacket,
    /// Opus audio packet.
    /// Opus 音频包。
    OpusPacket,
    /// G.711 A-law or mu-law audio packet.
    /// G.711 A-law 或 mu-law 音频包。
    G711Packet,
    /// MPEG-1/2 Layer II audio frame.
    /// MPEG-1/2 Layer II 音频帧。
    Mp2Frame,
    /// MPEG-1/2 Layer III audio frame.
    /// MPEG-1/2 Layer III 音频帧。
    Mp3Frame,
    /// Non-media data packet (e.g. SEI, metadata).
    /// 非媒体数据包（如 SEI、元数据）。
    DataPacket,
    /// Fallback used when the codec has not been normalized yet.
    /// 编解码器尚未归一化时的回退格式。
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
    /// Ingested from a protocol ingress path.
    /// 从协议入口路径接入。
    Ingest,
    /// Relayed from another stream without re-normalization.
    /// 从其他流转发且未重新归一化。
    Relay,
    /// Generated internally, e.g. silence or black filler.
    /// 内部生成，例如静音或黑帧填充。
    Generated,
    /// Recovered from a loss concealment or retransmission path.
    /// 从丢包隐藏或重传路径恢复。
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
    /// Last SR NTP timestamp's middle 32 bits as defined by RFC 3550.
    /// RFC 3550 中定义的最后一个 SR NTP 时间戳的中间 32 位。
    pub lsr: u32,
    /// Local Unix receive time in microseconds when the RTCP SR was received.
    /// 收到 RTCP SR 时的本地 Unix 接收时间（微秒）。
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
    /// Raw 32-bit wire value, including wrap-around.
    /// 包含回绕的原始 32 位线值。
    pub raw_timestamp: u32,
    /// 64-bit unwrapped counter, monotonic within the session.
    /// 会话内单调的 64 位解绕计数器。
    pub unwrapped_timestamp: u64,
    /// Difference between `unwrapped_timestamp` and `raw_timestamp` clamped to `i64`.
    /// `unwrapped_timestamp` 与 `raw_timestamp` 之差， clamp 到 `i64`。
    pub epoch_offset: i64,
    /// Optional RTP sequence number captured when the frame was packetized.
    /// 帧被分包时捕获的可选 RTP 序列号。
    pub sequence_number: Option<u16>,
    /// Optional RTCP SR mapping used for wall-clock synchronization.
    /// 用于墙上时间同步的可选 RTCP SR 映射。
    pub rtcp_mapping: Option<RtpRtcpMapping>,
}

impl RtpTimestamp {
    /// Create a new timestamp from the raw wire value and the already-unwrapped value.
    /// 使用原始线值和已解绕值创建新的时间戳。
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

/// RTMP/FLV timestamp with the raw 32-bit millisecond value and the unwrapped value.
///
/// RTMP timestamps are also 32-bit and can wrap or jump backwards when a stream
/// reconnects. The unwrapped value is the canonical timeline.
///
/// 包含原始 32 位毫秒值和解绕值的 RTMP/FLV 时间戳。
///
/// RTMP 时间戳同样是 32 位，在流重连时可能回绕或向后跳变。
/// 解绕后的值是规范时间线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtmpTimestamp {
    /// Raw 32-bit millisecond value as it appears on the wire.
    /// 线路上出现的原始 32 位毫秒值。
    pub raw_timestamp_ms: u32,
    /// 64-bit unwrapped millisecond counter, monotonic within the session.
    /// 会话内单调的 64 位解绕毫秒计数器。
    pub unwrapped_timestamp_ms: u64,
    /// Difference between the unwrapped and raw values in milliseconds.
    /// 解绕值与原始值之间的毫秒差。
    pub epoch_offset_ms: i64,
}

impl RtmpTimestamp {
    /// Create a new timestamp from the raw wire value and the already-unwrapped value.
    /// 使用原始线值和已解绕值创建新的时间戳。
    pub fn new(raw_timestamp_ms: u32, unwrapped_timestamp_ms: u64) -> Self {
        Self {
            raw_timestamp_ms,
            unwrapped_timestamp_ms,
            epoch_offset_ms: saturating_epoch_offset(unwrapped_timestamp_ms, raw_timestamp_ms),
        }
    }
}

/// Source timestamp carrier for a frame.
///
/// Keeps the protocol-specific timestamp representation so that the egress path
/// can reconstruct the original clock domain.
///
/// 帧的源时间戳载体。
///
/// 保留协议相关的时间戳表示，以便出口路径能够重建原始时钟域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTimestamp {
    /// RTP timestamp (32-bit raw plus unwrapped 64-bit value).
    /// RTP 时间戳（32 位原始值加 64 位解绕值）。
    Rtp(RtpTimestamp),
    /// RTMP/FLV timestamp (32-bit raw plus unwrapped 64-bit value).
    /// RTMP/FLV 时间戳（32 位原始值加 64 位解绕值）。
    Rtmp(RtmpTimestamp),
}

/// Optional side information attached to an [`AVFrame`].
///
/// Used to carry per-frame metadata that is not part of the payload itself.
///
/// 附加到 [`AVFrame`] 的可选附加信息。
///
/// 用于携带不属于负载本身的每帧元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameSideData {
    /// Digest of parameter sets carried by this frame.
    /// 本帧所携带参数集的摘要。
    ParameterSetDigest(u64),
    /// Arbitrary sequence number for ordering/debugging.
    /// 用于排序或调试的任意序列号。
    SequenceNumber(u64),
    /// Original source timestamp before protocol normalization.
    /// 协议归一化前的原始源时间戳。
    SourceTimestamp(SourceTimestamp),
    /// Key/value metadata produced by the codec or ingress.
    /// 编解码器或入口产生的键/值元数据。
    Metadata { key: String, value: String },
    /// Opaque binary blob for protocol-specific extensions.
    /// 协议特定扩展的不透明二进制数据。
    Opaque(Bytes),
}

bitflags! {
    /// Bit flags describing the role and quality of a frame.
    ///
    /// Frames may be key frames, configuration data, start/end of an access unit,
    /// B-frames, discontinuities, generated fillers, corrupted, or droppable.
    ///
    /// 描述帧角色与质量的位标志。
    ///
    /// 帧可以是关键帧、配置数据、访问单元起止、B 帧、断点、
    /// 生成填充、损坏或可被丢弃。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FrameFlags: u32 {
        /// Random access point (IDR/key frame).
        /// 随机访问点（IDR/关键帧）。
        const KEY = 1 << 0;
        /// Codec configuration data (e.g. SPS/PPS/VPS).
        /// 编解码器配置数据（如 SPS/PPS/VPS）。
        const CONFIG = 1 << 1;
        /// First packet of an access unit.
        /// 一个访问单元的第一个包。
        const START_OF_AU = 1 << 2;
        /// Last packet of an access unit.
        /// 一个访问单元的最后一个包。
        const END_OF_AU = 1 << 3;
        /// Bidirectional predicted frame.
        /// 双向预测帧（B 帧）。
        const B_FRAME = 1 << 4;
        /// Non-picture data (e.g. SEI, filler data).
        /// 非图像数据（如 SEI、填充数据）。
        const NON_PICTURE = 1 << 5;
        /// Discontinuity in the timeline (e.g. stream reset).
        /// 时间线上的断点（如流重置）。
        const DISCONTINUITY = 1 << 6;
        /// Generated internally (e.g. silence/blank frame).
        /// 内部生成（如静音/黑帧）。
        const GENERATED = 1 << 7;
        /// Payload may contain errors.
        /// 负载可能包含错误。
        const CORRUPTED = 1 << 8;
        /// Safe to drop by a slow subscriber without re-encoding.
        /// 慢订阅者可直接丢弃而无需重新编码。
        const DROPPABLE = 1 << 9;
    }
}

/// Timing validation errors for frame operations.
///
/// These errors are returned when the timebase is invalid, durations are
/// negative, or composition timing is impossible without a B-frame flag.
///
/// 帧操作的时间校验错误。
///
/// 当 timebase 无效、持续时间为负，或没有 B 帧标志但出现不可能的组合时间时返回。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FrameTimingError {
    /// Track timebase has a zero numerator or denominator.
    /// 轨道 timebase 的分子或分母为零。
    #[error("track {track_id:?} has invalid timebase {timebase_num}/{timebase_den}")]
    InvalidTimebase {
        track_id: TrackId,
        timebase_num: u32,
        timebase_den: u32,
    },
    /// Frame duration is negative.
    /// 帧持续时间为负。
    #[error("track {track_id:?} has negative duration {duration}")]
    NegativeDuration { track_id: TrackId, duration: i64 },
    /// Duration conversion to microseconds overflowed.
    /// 持续时间转换为微秒时溢出。
    #[error("track {track_id:?} duration micros overflow for duration {duration}")]
    DurationMicrosOverflow { track_id: TrackId, duration: i64 },
    /// Video `pts` is smaller than `dts` without a B-frame flag.
    /// 视频 `pts` 小于 `dts` 但没有 B 帧标志。
    #[error("video track {track_id:?} has pts < dts without B-frame flag: pts={pts}, dts={dts}")]
    NegativeCompositionWithoutBFrame {
        track_id: TrackId,
        pts: i64,
        dts: i64,
    },
}

/// Unified internal representation of one audio, video, or data sample.
///
/// All protocol ingress paths normalize to `AVFrame` before the engine sees
/// the media. Egress paths then convert `AVFrame` back to protocol-specific
/// payloads. The canonical timeline is `pts`/`dts` in `timebase` ticks plus
/// the pre-computed microsecond values `pts_us`/`dts_us`.
///
/// 统一的音频、视频或数据样本内部表示。
///
/// 所有协议入口路径在引擎看到媒体之前都会归一化为 `AVFrame`。
/// 出口路径随后将 `AVFrame` 转换回协议特定的负载。
/// 规范时间线由 `timebase` 刻度下的 `pts`/`dts`，以及预计算出的微秒值
/// `pts_us`/`dts_us` 共同组成。
#[derive(Debug, Clone)]
pub struct AVFrame {
    /// Track this frame belongs to.
    /// 本帧所属的轨道。
    pub track_id: TrackId,
    /// Whether this is audio, video, data, or subtitle media.
    /// 媒体类型：音频、视频、数据或字幕。
    pub media_kind: MediaKind,
    /// Normalized codec identifier.
    /// 归一化后的编解码器标识。
    pub codec: CodecId,
    /// Normalized payload layout (Annex-B, OBU, raw access unit, etc.).
    /// 归一化后的负载布局（Annex-B、OBU、原始访问单元等）。
    pub format: FrameFormat,
    /// Presentation timestamp in `timebase` ticks.
    /// Invariant: for non-B video frames, `pts >= dts`.
    /// 以 `timebase` 刻度表示的显示时间戳。
    /// 不变式：非 B 帧视频必须满足 `pts >= dts`。
    pub pts: i64,
    /// Decoding timestamp in `timebase` ticks.
    /// 以 `timebase` 刻度表示的解码时间戳。
    pub dts: i64,
    /// Canonical media time unit for this frame. Must be non-zero.
    /// 本帧的规范媒体时间单位。必须非零。
    pub timebase: Timebase,
    /// `pts` converted to microseconds for cross-protocol scheduling/logging.
    /// `pts` 转换为微秒，用于跨协议调度与日志。
    pub pts_us: i64,
    /// `dts` converted to microseconds.
    /// `dts` 转换为微秒。
    pub dts_us: i64,
    /// Frame duration in `timebase` ticks; zero means "unknown/unspecified".
    /// 以 `timebase` 刻度表示的帧持续时间；零表示“未知/未指定”。
    pub duration: i64,
    /// Frame duration in microseconds.
    /// 以微秒表示的帧持续时间。
    pub duration_us: i64,
    /// Key/random-access, config frame, discontinuity and corruption semantics.
    /// 关键/随机访问、配置帧、断点、损坏等语义标志。
    pub flags: FrameFlags,
    /// Raw payload bytes in the canonical `format`.
    /// 以规范 `format` 组织的原始负载字节。
    pub payload: Bytes,
    /// Optional per-frame side data (timestamps, metadata, opaque data).
    /// 可选的每帧附加数据（时间戳、元数据、不透明数据）。
    pub side_data: SmallVec<[FrameSideData; 4]>,
    /// Origin of this frame inside the pipeline.
    /// 本帧在流水线中的来源。
    pub origin: FrameOrigin,
}

impl AVFrame {
    /// Create a new frame with `pts`/`dts` and the pre-computed microsecond values.
    /// 使用 `pts`/`dts` 和预计算的微秒值创建新帧。
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

    /// Returns `true` if this frame is a key/random access point.
    /// 返回本帧是否为关键/随机访问点。
    pub fn is_key_frame(&self) -> bool {
        self.flags.contains(FrameFlags::KEY)
    }

    /// Mark this frame as a timeline discontinuity.
    /// 将本帧标记为时间线断点。
    pub fn mark_discontinuity(&mut self) {
        self.flags.insert(FrameFlags::DISCONTINUITY);
    }

    /// Store or replace the source timestamp side data entry.
    /// 存储或替换源时间戳附加数据项。
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

    /// Return the source timestamp side data, if present.
    /// 返回可能存在的源时间戳附加数据。
    pub fn source_timestamp(&self) -> Option<SourceTimestamp> {
        self.side_data.iter().find_map(|entry| {
            let FrameSideData::SourceTimestamp(source) = entry else {
                return None;
            };
            Some(*source)
        })
    }

    /// Return a copy of this frame with `duration` set.
    /// 返回一个设置好 `duration` 的帧副本。
    pub fn with_duration(mut self, duration: i64) -> Result<Self, FrameTimingError> {
        self.set_duration(duration)?;
        Ok(self)
    }

    /// Set the frame duration and update the microsecond value.
    /// 设置帧持续时间并更新微秒值。
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

    /// Difference between `pts` and `dts` in `timebase` ticks.
    /// 以 `timebase` 刻度表示的 `pts` 与 `dts` 之差。
    pub fn composition_time(&self) -> i64 {
        self.pts.saturating_sub(self.dts)
    }

    /// Difference between `pts_us` and `dts_us` in microseconds.
    /// 以微秒表示的 `pts_us` 与 `dts_us` 之差。
    pub fn composition_time_us(&self) -> i64 {
        self.pts_us.saturating_sub(self.dts_us)
    }

    /// Validate `duration`, `timebase`, and non-B composition timing.
    /// 验证 `duration`、`timebase` 以及非 B 帧的组合时间。
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
