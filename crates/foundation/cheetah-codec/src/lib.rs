//! `cheetah-codec` is the shared media foundation used by protocol modules.
//!
//! Timeline contract:
//! - `AVFrame.pts/dts/duration` are always canonical media timeline values.
//! - Protocol-native timestamps (for example RTP timestamp or RTMP tag timestamp)
//!   should be preserved as source metadata and must not be treated as canonical
//!   DTS ordering by default.
//! - Protocol egress timestamps are derived from canonical timeline through export
//!   helpers in this crate.
//!
//! This separation keeps ingress compatibility logic, engine scheduling semantics,
//! and protocol encapsulation concerns decoupled.
//!
//! `cheetah-codec` 是各协议模块共享的媒体基础层。
//!
//! 时间线约定：
//! - `AVFrame.pts/dts/duration` 始终是标准媒体时间线值。
//! - 协议原生时间戳（如 RTP 时间戳或 RTMP 标签时间戳）应作为源元数据保留，
//!   默认情况下不能当作标准 DTS 排序依据。
//! - 协议出口时间戳通过本 crate 的导出辅助函数从标准时间线派生。
//!
//! 这种分离将入口兼容逻辑、引擎调度语义与协议封装关注点解耦。
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub(crate) mod prelude {
    #[cfg(not(feature = "std"))]
    pub use alloc::{
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };

    #[cfg(feature = "std")]
    pub use std::{
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };

    // Container types that are available in both `std` and `no_std+alloc` builds.
    pub use alloc::boxed::Box;
    pub use hashbrown::{HashMap, HashSet};

    /// Round an `f64` to the nearest integer. Half-way cases are rounded away from zero.
    #[inline]
    pub fn round_f64(x: f64) -> f64 {
        #[cfg(feature = "std")]
        {
            x.round()
        }
        #[cfg(not(feature = "std"))]
        {
            libm::round(x)
        }
    }

    /// Clamp an `f64` between `min` and `max`.
    #[inline]
    pub fn clamp_f64(x: f64, min: f64, max: f64) -> f64 {
        x.max(min).min(max)
    }
}

pub mod adapter;
pub mod audio;
#[cfg(feature = "cea")]
pub mod cea;
pub mod compat;
pub mod egress;
pub mod flv;
pub mod flv_egress;
pub mod flv_ingress;
pub mod fmp4_demux;
pub mod fmp4_mux;
pub mod frame;
pub mod frame_view;
pub mod image;
pub mod ingress;
pub mod jtt1078;
pub mod mp4;
pub mod mute_audio;
pub mod observability;
pub mod ps;
pub mod record;
pub mod rtp;
pub mod rtp_reorder;
pub mod sdp;
pub mod subtitle;
pub mod time;
pub mod track;
pub mod traits;
pub mod ts_common;
pub mod ts_demux;
pub mod ts_mux;
pub mod video;

pub use adapter::{
    build_future_protocol_egress_contract_view, enforce_future_protocol_egress,
    enforce_future_protocol_ingress, AdapterContractError, EgressAdapterView,
    EncapsulationTimestamps, FragmentBoundary, FutureProtocolEgressContractView,
    FutureProtocolKind, IngressAdapterFrame, ParameterSetReplay, SrtEgressContractView,
    TimelineSource, WebRtcEgressContractView, WebRtcIngressContractView,
};
pub use audio::{
    aac_channel_count_from_asc, aac_channel_count_from_config, adts_strip, adts_wrap,
    AacAudioSpecificConfig, AdtsHeader, AudioParams, AudioSampleLayout,
};
pub use compat::{
    apply_compat_profile, codec_from_rtmp_codec_id, codec_from_rtmp_codec_id_with_mode,
    codec_from_rtmp_fourcc, codec_from_rtmp_metadata, infer_aac_asc_from_adts,
    normalize_h26x_start_codes, rtmp_audio_flag, rtmp_codec_id_from_codec,
    rtmp_domestic_codec_id_from_codec, rtmp_fourcc_from_codec, CompatFlags, CompatProfile,
    DomesticCodecMode, ProtocolKind, DOMESTIC_AUDIO_CODEC_ID_OPUS, DOMESTIC_VIDEO_CODEC_ID_H265,
    DOMESTIC_VIDEO_CODEC_ID_VP8, DOMESTIC_VIDEO_CODEC_ID_VP9, RTMP_AUDIO_CODEC_ID_AAC,
    RTMP_AUDIO_CODEC_ID_ADPCM, RTMP_AUDIO_CODEC_ID_G711A, RTMP_AUDIO_CODEC_ID_G711U,
    RTMP_AUDIO_CODEC_ID_MP3, RTMP_AUDIO_CODEC_ID_OPUS, RTMP_FOURCC_AV1, RTMP_FOURCC_H264,
    RTMP_FOURCC_H265, RTMP_FOURCC_H266, RTMP_FOURCC_VP8, RTMP_FOURCC_VP9, RTMP_VIDEO_CODEC_ID_AV1,
    RTMP_VIDEO_CODEC_ID_H264, RTMP_VIDEO_CODEC_ID_H265, RTMP_VIDEO_CODEC_ID_H266,
    RTMP_VIDEO_CODEC_ID_VP9,
};
pub use egress::{
    audio_rtp_timestamp_step, codec_default_samples_per_frame, codec_rtp_clock_rate,
    compute_rtp_timestamp, dts_to_rtmp_timestamp_ms, frame_composition_time_ms,
    frame_dts_to_rtmp_timestamp_ms, media_ts_to_rtp_ticks, millis_to_rtmp_timestamp_ms,
    repair_monotonic_timestamp, select_egress_timestamps, should_emit_alert_threshold,
    should_sample_timestamp_repair, AvSyncAligner, FrameRateEstimator,
    IncrementalRtpTimestampGenerator, RtpEgressTimestamp, RtpTimestampInput, RtpTimestampMode,
    SortingWindowDtsGenerator, TimestampRepairResult,
};
pub use flv::{
    build_audio_sequence_header, build_video_sequence_header, FlvDemuxEvent, FlvDemuxer, FlvHeader,
    FlvPreviousTagSizeMismatch, FlvStreamError, FlvTag, FlvTagBody, FlvTagType,
};
pub use flv_egress::{
    build_h265_config, build_h266_config, build_metadata, build_track_bootstrap_payloads,
    build_video_config_payload, map_frame_to_rtmp_flv_payload, mute_aac_config_payload,
    mute_aac_frame_payload, rtmp_playback_codec_supported, track_list_has_audio,
    use_enhanced_video_mode, RtmpFlvPayload, RtmpFlvPayloadKind, RtmpFlvPlayMode,
};
pub use flv_ingress::{FlvIngress, FlvIngressError, FlvIngressOutput};
pub use fmp4_demux::{
    Fmp4DemuxDiagnostic, Fmp4DemuxEvent, Fmp4DemuxTrack, Fmp4Demuxer, Fmp4DemuxerConfig,
};
pub use fmp4_mux::{Fmp4Diagnostic, Fmp4MuxEvent, Fmp4MuxSample, Fmp4Muxer, Fmp4MuxerConfig};
pub use frame::{
    AVFrame, FrameFlags, FrameFormat, FrameOrigin, FrameSideData, FrameTimingError, RtmpTimestamp,
    RtpRtcpMapping, RtpTimestamp, SourceTimestamp,
};
pub use frame_view::{
    annexb_from_payload, h26x_length_prefixed_from_payload, FrameViewCache, FrameViewKind,
};
pub use image::{jpeg, png, ImageDimensions};
pub use ingress::{
    fallback_step_for_rtp_ingress, monotonic_dts_min_step, source_timeline_mode_for_rtp_ingress,
};
pub use jtt1078::{
    Jtt1078Diagnostic, Jtt1078Frame, Jtt1078FrameAssembler, Jtt1078FrameType, Jtt1078Header,
    Jtt1078KeepOpenMode, Jtt1078Packetizer, Jtt1078SubPackage, Jtt1078Version,
};
pub use mp4::{
    Mp4ReadEvent, Mp4ReadRequest, Mp4ReadResult, Mp4Reader, Mp4ReaderConfig, Mp4Sample,
    Mp4SampleEntry, Mp4WriteEvent, Mp4Writer, Mp4WriterConfig, SampleIndex, SampleIndexEntry,
    SampleTable,
};
pub use mute_audio::MuteAudioMaker;
pub use observability::{
    classify_timestamp_alert, RepairEventCounters, RepairLayer, RuntimeObservabilityReport,
    RuntimeReportBuilder, REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD,
};
pub use ps::{
    PesPacket, PsDemuxDiagnostic, PsDemuxEvent, PsDemuxer, PsDemuxerConfig, PsMuxer, PsPacket,
    PsStreamKind,
};
pub use record::{
    DynRecordWriter, RecordContainerWriter, RecordDiagnostic, RecordError, RecordFormat,
    RecordWriteEvent,
};
pub use rtp::{
    depacketize_payload, encode_interleaved_rtp_frame, encode_tcp_rtp_frame, packetize_g711,
    packetize_payload, parse_interleaved_rtp_frame, parse_tcp_rtp_frame, parse_tcp_rtp_frame_with,
    probe_rtp_payload, EhomeCodecInfo, EhomeDecoder, EhomeOutput, ParsedTcpRtpFrame, RtpClock,
    RtpHeader, RtpPacket, RtpPayloadMode, RtpTcpFraming,
};
pub use rtp_reorder::{RtpReorderBuffer, RtpReorderSettings, RtpSequenceUnwrapper};
pub use sdp::{export_fmtp, export_media_description, SdpMediaDescription};
pub use time::{
    DiscontinuityJudge, DtsGenerator, MonoTime, StampAdjust, StampAdjustMode, Timebase,
    TimebaseConverter, TimestampAlert, TimestampError, TimestampNormalizeError,
    TimestampNormalizeInput, TimestampNormalizeMode, TimestampNormalizeOutput, TimestampNormalizer,
    TimestampNormalizerConfig, TimestampNormalizerConfigError, TimestampValue, WrapUnwrapper,
};
pub use track::{
    AacRtpPacketization, CodecConfigError, CodecConfigPayload, CodecConfigRequirement,
    CodecConfigView, CodecExtradata, CodecId, MediaKind, Rational32, TrackId, TrackInfo,
    TrackInfoError, TrackReadiness,
};
pub use ts_common::{
    codec_from_stream_type, crc32_mpeg2, decode_timestamp, encode_timestamp, find_sync,
    g711_duration_90k, g711_duration_us, identify_private_stream, registration_descriptor,
    stream_type_for_codec, AUD_H264, AUD_H265, AUD_H266, PAT_PID, PMT_PID, SYNC_BYTE,
    TS_PACKET_SIZE,
};
pub use ts_demux::{MpegTsDemuxDiagnostic, MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig};
pub use ts_mux::{MpegTsDiagnostic, MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig};
pub use video::{
    av1_obu_payload_has_keyframe, h26x_annexb_has_random_access, h26x_nalu_is_random_access,
    video_payload_is_random_access, vp8_frame_is_keyframe, vp9_frame_is_keyframe, AccessUnit,
    AccessUnitAssembler, AccessUnitBuildError, AccessUnitTiming, LengthPrefixedParseError,
    ParameterSetCache, ParameterSetRequirement, PARAMETER_SET_MAX_SIZE,
};
