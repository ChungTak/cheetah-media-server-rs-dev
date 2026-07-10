use bytes::Bytes;

use crate::prelude::*;
use crate::{
    frame_composition_time_ms, frame_dts_to_rtmp_timestamp_ms, media_ts_to_rtp_ticks,
    select_egress_timestamps, AVFrame, CodecConfigError, CodecConfigView, CodecId, FrameFlags,
    FrameTimingError, MediaKind, ParameterSetCache, ParameterSetRequirement, Timebase,
    TimestampNormalizeOutput, TrackId, TrackInfo, TrackInfoError,
};

/// Origin of the timeline used by an ingress frame.
///
/// `TimestampNormalizer` means the frame went through `TimestampNormalizer` and has
/// a canonical timeline. `PassthroughLegacy` is the legacy path where normalization
/// is skipped and the caller promises the values are already compatible.
///
/// 入口帧所使用的时间线来源。
///
/// `TimestampNormalizer` 表示帧经过 `TimestampNormalizer` 并具有标准时间线。
/// `PassthroughLegacy` 是跳过归一化的旧路径，调用方保证值已兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineSource {
    TimestampNormalizer,
    PassthroughLegacy,
}

/// Target protocol for the future adapter contract checks.
///
/// SRT and WebRTC have additional ingress/egress contract requirements beyond the
/// generic `AVFrame` contract.
///
/// 未来适配器契约检查的目标协议。
///
/// SRT 和 WebRTC 在通用 `AVFrame` 契约之外还有额外的入口/出口契约要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutureProtocolKind {
    SrtTransport,
    WebRtcRtpRtcp,
}

/// Errors returned when an ingress/egress frame violates the adapter contract.
///
/// 入口/出口帧违反适配器契约时返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdapterContractError {
    #[error("track/frame mismatch for {field}: track={track}, frame={frame}")]
    TrackFrameMismatch {
        field: &'static str,
        track: String,
        frame: String,
    },
    #[error("normalized timestamp mismatch for {field}: frame={frame_value}, normalized={normalized_value}")]
    NormalizedTimestampMismatch {
        field: &'static str,
        frame_value: i64,
        normalized_value: i64,
    },
    #[error(
        "normalized discontinuity mismatch: frame={frame_discontinuity}, normalized={normalized_discontinuity}"
    )]
    NormalizedDiscontinuityMismatch {
        frame_discontinuity: bool,
        normalized_discontinuity: bool,
    },
    #[error("invalid frame timing: {0}")]
    InvalidFrameTiming(#[from] FrameTimingError),
    #[error("invalid track info: {0}")]
    InvalidTrackInfo(#[from] TrackInfoError),
    #[error("invalid codec config: {0}")]
    InvalidCodecConfig(#[from] CodecConfigError),
    #[error("required parameter sets missing for track {track_id:?} codec {codec:?}")]
    MissingRequiredParameterSets { track_id: TrackId, codec: CodecId },
    #[error("srt ingress bypassed timestamp normalization")]
    SrtBypassedMediaNormalization,
    #[error("webrtc ingress bypassed timestamp normalization")]
    WebRtcBypassedMediaNormalization,
    #[error("webrtc video track {track_id:?} missing access unit boundary markers")]
    WebRtcVideoMissingAccessUnitBoundary { track_id: TrackId },
}

/// Ingress-side adapter frame that pairs a `TrackInfo` with a normalized `AVFrame`.
///
/// This type is the boundary between protocol modules and the codec layer. It
/// captures whether the frame went through the timestamp normalizer and whether it
/// marks a random-access or discontinuity point.
///
/// 入口侧适配器帧，将 `TrackInfo` 与归一化的 `AVFrame` 配对。
///
/// 该类型是协议模块与 codec 层之间的边界。它捕获帧是否经过时间戳归一化，
/// 以及是否标记随机访问或不连续点。
#[derive(Debug, Clone)]
pub struct IngressAdapterFrame {
    track: TrackInfo,
    frame: AVFrame,
    timeline_source: TimelineSource,
    random_access: bool,
    discontinuity: bool,
}

impl IngressAdapterFrame {
    /// Build an ingress frame from a timestamp-normalized output.
    ///
    /// Verifies that the frame's `pts`, `dts`, `pts_us`, `dts_us` match the normalized
    /// values and that the discontinuity flag is consistent. This guarantees the codec
    /// layer and the normalizer agree on the timeline.
    ///
    /// 从时间戳归一化输出构建入口帧。
    ///
    /// 校验帧的 `pts`、`dts`、`pts_us`、`dts_us` 与归一化值一致，且不连续标志一致。
    /// 保证 codec 层与归一化器在时间线上达成一致。
    pub fn from_normalized(
        track: TrackInfo,
        frame: AVFrame,
        normalized: &TimestampNormalizeOutput,
    ) -> Result<Self, AdapterContractError> {
        validate_track_and_frame(&track, &frame)?;

        ensure_normalized_match("pts", frame.pts, normalized.pts)?;
        ensure_normalized_match("dts", frame.dts, normalized.dts)?;
        ensure_normalized_match("pts_us", frame.pts_us, normalized.pts_us)?;
        ensure_normalized_match("dts_us", frame.dts_us, normalized.dts_us)?;

        let discontinuity = frame.flags.contains(FrameFlags::DISCONTINUITY);
        if discontinuity != normalized.discontinuity {
            return Err(AdapterContractError::NormalizedDiscontinuityMismatch {
                frame_discontinuity: discontinuity,
                normalized_discontinuity: normalized.discontinuity,
            });
        }

        Ok(Self {
            random_access: frame.flags.contains(FrameFlags::KEY),
            discontinuity,
            track,
            frame,
            timeline_source: TimelineSource::TimestampNormalizer,
        })
    }

    /// Build an ingress frame from a legacy passthrough path.
    ///
    /// Performs track/frame validation but does not require normalized timestamp values.
    ///
    /// 从旧直通路径构建入口帧。
    ///
    /// 执行轨道/帧校验，但不要求归一化时间戳值。
    pub fn from_passthrough(
        track: TrackInfo,
        frame: AVFrame,
    ) -> Result<Self, AdapterContractError> {
        validate_track_and_frame(&track, &frame)?;
        Ok(Self {
            random_access: frame.flags.contains(FrameFlags::KEY),
            discontinuity: frame.flags.contains(FrameFlags::DISCONTINUITY),
            track,
            frame,
            timeline_source: TimelineSource::PassthroughLegacy,
        })
    }

    pub fn track(&self) -> &TrackInfo {
        &self.track
    }

    pub fn frame(&self) -> &AVFrame {
        &self.frame
    }

    pub fn codec(&self) -> CodecId {
        self.frame.codec
    }

    pub fn timebase(&self) -> Timebase {
        self.frame.timebase
    }

    pub fn pts(&self) -> i64 {
        self.frame.pts
    }

    pub fn dts(&self) -> i64 {
        self.frame.dts
    }

    pub fn duration(&self) -> i64 {
        self.frame.duration
    }

    pub fn random_access(&self) -> bool {
        self.random_access
    }

    pub fn discontinuity(&self) -> bool {
        self.discontinuity
    }

    pub fn timeline_source(&self) -> TimelineSource {
        self.timeline_source
    }
}

/// Marks whether a frame is at the start or end of an access unit.
///
/// Some protocols (e.g. WebRTC) require complete access units and use these flags
/// to detect packet boundaries.
///
/// 标记帧是否处于 access unit 的起始或结束。
///
/// 某些协议（如 WebRTC）需要完整 access unit，使用这些标志检测包边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentBoundary {
    pub start_of_access_unit: bool,
    pub end_of_access_unit: bool,
}

/// Pre-computed timestamps in the wire formats used by RTMP and RTP.
///
/// These are derived from the canonical `AVFrame` timing so each protocol egress can
/// read them directly without re-doing timebase conversions.
///
/// RTMP 和 RTP 使用的线格式预计算时间戳。
///
/// 它们从标准 `AVFrame` 时间派生，因此每个协议出口可以直接读取，无需重新进行
/// timebase 转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncapsulationTimestamps {
    pub rtmp_timestamp_ms: u32,
    pub composition_time_ms: i32,
    pub rtp_timestamp_ticks: u32,
}

/// Parameter-set status and payload needed for a keyframe at egress.
///
/// Carries the `ParameterSetRequirement` and, if present, the cached SPS/PPS/VPS
/// that should be prepended before the keyframe payload.
///
/// 关键帧出口所需的参数集状态和负载。
///
/// 携带 `ParameterSetRequirement` 以及若存在应前置到关键帧负载前的缓存 SPS/PPS/VPS。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterSetReplay {
    pub requirement: ParameterSetRequirement,
    pub units: Vec<Bytes>,
}

/// Egress-side view of a frame, with all protocol-relevant timestamps and metadata.
///
/// This is the canonical payload produced by the codec layer for consumption by
/// protocol modules. It includes the codec config, parameter-set replay, fragment
/// boundary flags and pre-computed RTMP/RTP timestamps.
///
/// 出口侧帧视图，包含所有协议相关时间戳和元数据。
///
/// 这是 codec 层为协议模块消费生成的标准负载。包含编解码器配置、参数集重放、
/// 分片边界标志以及预计算的 RTMP/RTP 时间戳。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAdapterView {
    track_id: TrackId,
    media_kind: MediaKind,
    codec: CodecId,
    timebase: Timebase,
    random_access: bool,
    discontinuity: bool,
    pts: i64,
    dts: i64,
    duration: i64,
    fragment_boundary: FragmentBoundary,
    encapsulation_timestamps: EncapsulationTimestamps,
    codec_config: CodecConfigView,
    parameter_set_replay: ParameterSetReplay,
}

impl EgressAdapterView {
    /// Build an egress view from a track, frame, and cached parameter sets.
    ///
    /// Validates the track/frame, computes random-access status, selects parameter-set
    /// replay units, extracts codec config, and computes the RTMP/RTP encapsulation timestamps.
    ///
    /// 从轨道、帧和缓存参数集构建出口视图。
    ///
    /// 校验轨道/帧、计算随机访问状态、选择参数集重放单元、提取编解码器配置，
    /// 并计算 RTMP/RTP 封装时间戳。
    pub fn build(
        track: &TrackInfo,
        frame: &AVFrame,
        parameter_sets: &ParameterSetCache,
    ) -> Result<Self, AdapterContractError> {
        validate_track_and_frame(track, frame)?;

        let random_access = frame.flags.contains(FrameFlags::KEY);
        let requirement = parameter_sets.requirement_for_frame(frame.codec, random_access);
        let replay_units = parameter_set_units_for_codec(parameter_sets, frame.codec);
        if matches!(
            requirement,
            ParameterSetRequirement::RequiredMissing | ParameterSetRequirement::RequiredPresent
        ) && replay_units.is_empty()
        {
            return Err(AdapterContractError::MissingRequiredParameterSets {
                track_id: frame.track_id,
                codec: frame.codec,
            });
        }

        let codec_config = track.codec_config_view()?;
        let fragment_boundary = FragmentBoundary {
            start_of_access_unit: frame.flags.contains(FrameFlags::START_OF_AU),
            end_of_access_unit: frame.flags.contains(FrameFlags::END_OF_AU),
        };
        let (primary, secondary) = select_egress_timestamps(frame.media_kind, frame.pts, frame.dts);
        let encapsulation_timestamps = EncapsulationTimestamps {
            rtmp_timestamp_ms: frame_dts_to_rtmp_timestamp_ms(frame),
            composition_time_ms: frame_composition_time_ms(frame),
            rtp_timestamp_ticks: media_ts_to_rtp_ticks(
                primary,
                secondary,
                frame.timebase,
                track.clock_rate,
            ),
        };

        Ok(Self {
            track_id: frame.track_id,
            media_kind: frame.media_kind,
            codec: frame.codec,
            timebase: frame.timebase,
            random_access,
            discontinuity: frame.flags.contains(FrameFlags::DISCONTINUITY),
            pts: frame.pts,
            dts: frame.dts,
            duration: frame.duration,
            fragment_boundary,
            encapsulation_timestamps,
            codec_config,
            parameter_set_replay: ParameterSetReplay {
                requirement,
                units: replay_units,
            },
        })
    }

    pub fn track_id(&self) -> TrackId {
        self.track_id
    }

    pub fn media_kind(&self) -> MediaKind {
        self.media_kind
    }

    pub fn codec(&self) -> CodecId {
        self.codec
    }

    pub fn timebase(&self) -> Timebase {
        self.timebase
    }

    pub fn random_access(&self) -> bool {
        self.random_access
    }

    pub fn discontinuity(&self) -> bool {
        self.discontinuity
    }

    pub fn pts(&self) -> i64 {
        self.pts
    }

    pub fn dts(&self) -> i64 {
        self.dts
    }

    pub fn duration(&self) -> i64 {
        self.duration
    }

    pub fn fragment_boundary(&self) -> FragmentBoundary {
        self.fragment_boundary
    }

    pub fn encapsulation_timestamps(&self) -> EncapsulationTimestamps {
        self.encapsulation_timestamps
    }

    pub fn codec_config(&self) -> &CodecConfigView {
        &self.codec_config
    }

    pub fn parameter_set_replay(&self) -> &ParameterSetReplay {
        &self.parameter_set_replay
    }
}

/// Egress contract view for SRT encapsulation.
///
/// Contains RTMP-style millisecond timestamps and composition time, which the SRT module
/// uses for its wire format.
///
/// SRT 封装的出口契约视图。
///
/// 包含 RTMP 风格的毫秒时间戳和合成时间，SRT 模块用于其线格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtEgressContractView {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub random_access: bool,
    pub discontinuity: bool,
    pub dts_ms: u32,
    pub composition_time_ms: i32,
    pub codec_config: CodecConfigView,
    pub parameter_set_replay: ParameterSetReplay,
}

/// Egress contract view for WebRTC RTP/RTCP encapsulation.
///
/// Contains RTP timestamp and access-unit boundary flags, which the WebRTC module uses
/// when packetizing frames for RTP.
///
/// WebRTC RTP/RTCP 封装的出口契约视图。
///
/// 包含 RTP 时间戳和 access unit 边界标志，WebRTC 模块用于将帧打包为 RTP。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcEgressContractView {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub random_access: bool,
    pub discontinuity: bool,
    pub fragment_boundary: FragmentBoundary,
    pub rtp_timestamp_ticks: u32,
    pub codec_config: CodecConfigView,
    pub parameter_set_replay: ParameterSetReplay,
}

/// Ingress-side view describing a WebRTC RTP/RTCP packet hand-off.
///
/// Phase 03 + 04 of the WebRTC plan use this as the canonical "what does
/// the codec layer know about an incoming WebRTC packet" type. The
/// `Optional` per-packet metadata maps onto well-known RTP header
/// extensions; codec-side helpers can use this view for stats, RID
/// tracking and timestamp-normalizer wiring without having to reach into
/// `cheetah-webrtc-core` types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcIngressContractView {
    pub track_id: TrackId,
    pub codec: CodecId,
    pub rtp_timestamp_ticks: u32,
    pub sequence_number: u16,
    pub marker: bool,
    pub rid: Option<String>,
    pub repaired_rid: Option<String>,
    pub twcc_sequence: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FutureProtocolEgressContractView {
    Srt(SrtEgressContractView),
    WebRtc(WebRtcEgressContractView),
}

/// Enforce protocol-specific ingress contract requirements.
///
/// SRT and WebRTC ingress must pass through `TimestampNormalizer`. WebRTC video must
/// also have both start and end access-unit boundary markers.
///
/// 强制执行协议特定的入口契约要求。
///
/// SRT 和 WebRTC 入口必须经过 `TimestampNormalizer`。WebRTC 视频还必须同时具有
/// access unit 起始和结束边界标记。
pub fn enforce_future_protocol_ingress(
    protocol: FutureProtocolKind,
    ingress: &IngressAdapterFrame,
) -> Result<(), AdapterContractError> {
    match protocol {
        FutureProtocolKind::SrtTransport => {
            if ingress.timeline_source() != TimelineSource::TimestampNormalizer {
                return Err(AdapterContractError::SrtBypassedMediaNormalization);
            }
        }
        FutureProtocolKind::WebRtcRtpRtcp => {
            if ingress.timeline_source() != TimelineSource::TimestampNormalizer {
                return Err(AdapterContractError::WebRtcBypassedMediaNormalization);
            }
            ensure_webrtc_video_has_access_unit_boundary(ingress.track.track_id, &ingress.frame)?;
        }
    }
    Ok(())
}

/// Enforce protocol-specific egress contract requirements.
///
/// WebRTC video egress requires complete access units (`START_OF_AU` and `END_OF_AU`).
///
/// 强制执行协议特定的出口契约要求。
///
/// WebRTC 视频出口需要完整 access unit（`START_OF_AU` 和 `END_OF_AU`）。
pub fn enforce_future_protocol_egress(
    protocol: FutureProtocolKind,
    egress: &EgressAdapterView,
) -> Result<(), AdapterContractError> {
    if matches!(protocol, FutureProtocolKind::WebRtcRtpRtcp)
        && matches!(egress.media_kind, MediaKind::Video)
        && (!egress.fragment_boundary.start_of_access_unit
            || !egress.fragment_boundary.end_of_access_unit)
    {
        return Err(AdapterContractError::WebRtcVideoMissingAccessUnitBoundary {
            track_id: egress.track_id,
        });
    }
    Ok(())
}

/// Build the protocol-specific egress contract view from a generic egress view.
///
/// 从通用出口视图构建协议特定的出口契约视图。
pub fn build_future_protocol_egress_contract_view(
    protocol: FutureProtocolKind,
    egress: &EgressAdapterView,
) -> Result<FutureProtocolEgressContractView, AdapterContractError> {
    enforce_future_protocol_egress(protocol, egress)?;
    let encapsulation_ts = egress.encapsulation_timestamps();
    match protocol {
        FutureProtocolKind::SrtTransport => Ok(FutureProtocolEgressContractView::Srt(
            SrtEgressContractView {
                track_id: egress.track_id(),
                media_kind: egress.media_kind(),
                codec: egress.codec(),
                random_access: egress.random_access(),
                discontinuity: egress.discontinuity(),
                dts_ms: encapsulation_ts.rtmp_timestamp_ms,
                composition_time_ms: encapsulation_ts.composition_time_ms,
                codec_config: egress.codec_config().clone(),
                parameter_set_replay: egress.parameter_set_replay().clone(),
            },
        )),
        FutureProtocolKind::WebRtcRtpRtcp => Ok(FutureProtocolEgressContractView::WebRtc(
            WebRtcEgressContractView {
                track_id: egress.track_id(),
                media_kind: egress.media_kind(),
                codec: egress.codec(),
                random_access: egress.random_access(),
                discontinuity: egress.discontinuity(),
                fragment_boundary: egress.fragment_boundary(),
                rtp_timestamp_ticks: encapsulation_ts.rtp_timestamp_ticks,
                codec_config: egress.codec_config().clone(),
                parameter_set_replay: egress.parameter_set_replay().clone(),
            },
        )),
    }
}

fn ensure_webrtc_video_has_access_unit_boundary(
    track_id: TrackId,
    frame: &AVFrame,
) -> Result<(), AdapterContractError> {
    if matches!(frame.media_kind, MediaKind::Video)
        && (!frame.flags.contains(FrameFlags::START_OF_AU)
            || !frame.flags.contains(FrameFlags::END_OF_AU))
    {
        return Err(AdapterContractError::WebRtcVideoMissingAccessUnitBoundary { track_id });
    }
    Ok(())
}

fn ensure_normalized_match(
    field: &'static str,
    frame_value: i64,
    normalized_value: i64,
) -> Result<(), AdapterContractError> {
    if frame_value != normalized_value {
        return Err(AdapterContractError::NormalizedTimestampMismatch {
            field,
            frame_value,
            normalized_value,
        });
    }
    Ok(())
}

/// Validate that a `TrackInfo` and `AVFrame` describe the same logical track.
///
/// Checks `track_id`, `media_kind`, `codec`, and verifies the track has a valid
/// timebase and the frame has valid timing.
///
/// 校验 `TrackInfo` 和 `AVFrame` 描述同一逻辑轨道。
///
/// 检查 `track_id`、`media_kind`、`codec`，并验证轨道具有有效 timebase、帧具有有效时间。
fn validate_track_and_frame(
    track: &TrackInfo,
    frame: &AVFrame,
) -> Result<(), AdapterContractError> {
    ensure_track_frame_match(
        "track_id",
        format!("{}", track.track_id.0),
        format!("{}", frame.track_id.0),
    )?;
    ensure_track_frame_match(
        "media_kind",
        format!("{:?}", track.media_kind),
        format!("{:?}", frame.media_kind),
    )?;
    ensure_track_frame_match(
        "codec",
        format!("{:?}", track.codec),
        format!("{:?}", frame.codec),
    )?;

    let _ = track.media_timebase()?;
    frame.validate_media_timing()?;
    Ok(())
}

fn ensure_track_frame_match(
    field: &'static str,
    track: String,
    frame: String,
) -> Result<(), AdapterContractError> {
    if track != frame {
        return Err(AdapterContractError::TrackFrameMismatch {
            field,
            track,
            frame,
        });
    }
    Ok(())
}

/// Collect the cached parameter-set units for a codec in egress order.
///
/// 按出口顺序收集某编解码器的缓存参数集单元。
fn parameter_set_units_for_codec(parameter_sets: &ParameterSetCache, codec: CodecId) -> Vec<Bytes> {
    let mut units = Vec::new();
    match codec {
        CodecId::H264 => {
            if let Some(sps) = &parameter_sets.sps {
                units.push(sps.clone());
            }
            if let Some(pps) = &parameter_sets.pps {
                units.push(pps.clone());
            }
        }
        CodecId::H265 | CodecId::H266 => {
            if let Some(vps) = &parameter_sets.vps {
                units.push(vps.clone());
            }
            if let Some(sps) = &parameter_sets.sps {
                units.push(sps.clone());
            }
            if let Some(pps) = &parameter_sets.pps {
                units.push(pps.clone());
            }
        }
        _ => {}
    }
    units
}
