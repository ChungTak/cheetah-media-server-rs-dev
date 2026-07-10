use bytes::Bytes;

use crate::prelude::*;
use crate::{
    frame_composition_time_ms, frame_dts_to_rtmp_timestamp_ms, media_ts_to_rtp_ticks,
    select_egress_timestamps, AVFrame, CodecConfigError, CodecConfigView, CodecId, FrameFlags,
    FrameTimingError, MediaKind, ParameterSetCache, ParameterSetRequirement, Timebase,
    TimestampNormalizeOutput, TrackId, TrackInfo, TrackInfoError,
};

/// Source that produces `Timeline`.
/// 产生 `Timeline` 的 Source。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineSource {
    TimestampNormalizer,
    PassthroughLegacy,
}

/// Kind of `Future Protocol`.
/// `Future Protocol` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutureProtocolKind {
    SrtTransport,
    WebRtcRtpRtcp,
}

/// Error returned by `Adapter Contract` operations.
/// `Adapter Contract` 操作返回的错误。
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

/// Frame for `Ingress Adapter`.
/// `Ingress Adapter` 的帧。
#[derive(Debug, Clone)]
pub struct IngressAdapterFrame {
    track: TrackInfo,
    frame: AVFrame,
    timeline_source: TimelineSource,
    random_access: bool,
    discontinuity: bool,
}

impl IngressAdapterFrame {
    /// Creates `normalized` from input.
    /// 从输入创建 `normalized`。
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

    /// Creates `passthrough` from input.
    /// 从输入创建 `passthrough`。
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

    /// `track` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `track` 函数。
    pub fn track(&self) -> &TrackInfo {
        &self.track
    }

    /// `frame` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `frame` 函数。
    pub fn frame(&self) -> &AVFrame {
        &self.frame
    }

    /// `codec` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `codec` 函数。
    pub fn codec(&self) -> CodecId {
        self.frame.codec
    }

    /// `timebase` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `timebase` 函数。
    pub fn timebase(&self) -> Timebase {
        self.frame.timebase
    }

    /// `pts` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `pts` 函数。
    pub fn pts(&self) -> i64 {
        self.frame.pts
    }

    /// `dts` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `dts` 函数。
    pub fn dts(&self) -> i64 {
        self.frame.dts
    }

    /// `duration` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `duration` 函数。
    pub fn duration(&self) -> i64 {
        self.frame.duration
    }

    /// `random_access` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `random_access` 函数。
    pub fn random_access(&self) -> bool {
        self.random_access
    }

    /// `discontinuity` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `discontinuity` 函数。
    pub fn discontinuity(&self) -> bool {
        self.discontinuity
    }

    /// `timeline_source` function of `IngressAdapterFrame`.
    /// `IngressAdapterFrame` 的 `timeline_source` 函数。
    pub fn timeline_source(&self) -> TimelineSource {
        self.timeline_source
    }
}

/// `FragmentBoundary` data structure.
/// `FragmentBoundary` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentBoundary {
    pub start_of_access_unit: bool,
    pub end_of_access_unit: bool,
}

/// `EncapsulationTimestamps` data structure.
/// `EncapsulationTimestamps` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncapsulationTimestamps {
    pub rtmp_timestamp_ms: u32,
    pub composition_time_ms: i32,
    pub rtp_timestamp_ticks: u32,
}

/// `ParameterSetReplay` data structure.
/// `ParameterSetReplay` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterSetReplay {
    pub requirement: ParameterSetRequirement,
    pub units: Vec<Bytes>,
}

/// View of `Egress Adapter`.
/// `Egress Adapter` 的视图。
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
    /// Builds the output from the accumulated state.
    /// 从累积状态构建输出。
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

    /// `track_id` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `track_id` 函数。
    pub fn track_id(&self) -> TrackId {
        self.track_id
    }

    /// `media_kind` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `media_kind` 函数。
    pub fn media_kind(&self) -> MediaKind {
        self.media_kind
    }

    /// `codec` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `codec` 函数。
    pub fn codec(&self) -> CodecId {
        self.codec
    }

    /// `timebase` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `timebase` 函数。
    pub fn timebase(&self) -> Timebase {
        self.timebase
    }

    /// `random_access` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `random_access` 函数。
    pub fn random_access(&self) -> bool {
        self.random_access
    }

    /// `discontinuity` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `discontinuity` 函数。
    pub fn discontinuity(&self) -> bool {
        self.discontinuity
    }

    /// `pts` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `pts` 函数。
    pub fn pts(&self) -> i64 {
        self.pts
    }

    /// `dts` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `dts` 函数。
    pub fn dts(&self) -> i64 {
        self.dts
    }

    /// `duration` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `duration` 函数。
    pub fn duration(&self) -> i64 {
        self.duration
    }

    /// `fragment_boundary` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `fragment_boundary` 函数。
    pub fn fragment_boundary(&self) -> FragmentBoundary {
        self.fragment_boundary
    }

    /// `encapsulation_timestamps` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `encapsulation_timestamps` 函数。
    pub fn encapsulation_timestamps(&self) -> EncapsulationTimestamps {
        self.encapsulation_timestamps
    }

    /// `codec_config` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `codec_config` 函数。
    pub fn codec_config(&self) -> &CodecConfigView {
        &self.codec_config
    }

    /// `parameter_set_replay` function of `EgressAdapterView`.
    /// `EgressAdapterView` 的 `parameter_set_replay` 函数。
    pub fn parameter_set_replay(&self) -> &ParameterSetReplay {
        &self.parameter_set_replay
    }
}

/// View of `SRT Egress Contract`.
/// `SRT Egress Contract` 的视图。
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

/// View of `Web Rtc Egress Contract`.
/// `Web Rtc Egress Contract` 的视图。
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

/// View of `Future Protocol Egress Contract`.
/// `Future Protocol Egress Contract` 的视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FutureProtocolEgressContractView {
    Srt(SrtEgressContractView),
    WebRtc(WebRtcEgressContractView),
}

/// `enforce_future_protocol_ingress` function.
/// `enforce_future_protocol_ingress` 函数。
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

/// `enforce_future_protocol_egress` function.
/// `enforce_future_protocol_egress` 函数。
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

/// Builds the `future protocol egress contract view`.
/// 构建 `future protocol egress contract view`。
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
