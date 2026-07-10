use crate::prelude::*;
use crate::time::Timebase;
use bytes::Bytes;

/// Opaque track identifier local to a stream.
///
/// `TrackId` is a newtype wrapper around `u32` so that track identifiers are
/// type-safe and distinguishable from arbitrary integers.
///
/// 流内本地轨道标识符。
///
/// `TrackId` 是对 `u32` 的 newtype 包装，使轨道标识符具有类型安全性，
/// 并与普通整数区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub u32);

/// Media category of a track.
///
/// Tracks are classified as video, audio, data, or subtitle so that the engine
/// can apply codec-specific handling and subscriber filtering.
///
/// 轨道的媒体类别。
///
/// 轨道分为视频、音频、数据或字幕，以便引擎应用特定编解码器处理与订阅过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Video track.
    /// 视频轨道。
    Video,
    /// Audio track.
    /// 音频轨道。
    Audio,
    /// Generic data track (e.g. metadata, telemetry).
    /// 通用数据轨道（如元数据、遥测）。
    Data,
    /// Subtitle or text track.
    /// 字幕或文本轨道。
    Subtitle,
}

/// Normalized codec identifier used across the whole engine.
///
/// Every protocol ingress path maps its codec names to a single `CodecId` so
/// that the engine and `cheetah-codec` can operate on a small, canonical enum.
///
/// 整个引擎使用的归一化编解码器标识。
///
/// 每个协议入口路径都将其编解码器名称映射到一个统一的 `CodecId`，
/// 使引擎和 `cheetah-codec` 能在少量规范枚举上操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecId {
    /// H.264 / AVC.
    /// H.264 / AVC。
    H264,
    /// H.265 / HEVC.
    /// H.265 / HEVC。
    H265,
    /// H.266 / VVC.
    /// H.266 / VVC。
    H266,
    /// AOMedia Video 1.
    /// AOMedia Video 1。
    AV1,
    /// WebM/VP8 video.
    /// WebM/VP8 视频。
    VP8,
    /// WebM/VP9 video.
    /// WebM/VP9 视频。
    VP9,
    /// Motion JPEG.
    /// Motion JPEG。
    MJPEG,
    /// Advanced Audio Coding.
    /// 高级音频编码。
    AAC,
    /// Adaptive Differential PCM.
    /// 自适应差分 PCM。
    ADPCM,
    /// Opus interactive audio codec.
    /// Opus 交互式音频编解码器。
    Opus,
    /// G.711 A-law audio.
    /// G.711 A-law 音频。
    G711A,
    /// G.711 mu-law audio.
    /// G.711 mu-law 音频。
    G711U,
    /// MPEG-1/2 Layer II audio.
    /// MPEG-1/2 Layer II 音频。
    MP2,
    /// MPEG-1/2 Layer III audio.
    /// MPEG-1/2 Layer III 音频。
    MP3,
    /// Unknown or unsupported codec.
    /// 未知或不支持的编解码器。
    Unknown,
}

/// Readiness state of a track for decoding.
///
/// A track becomes `Ready` once it has enough configuration data (e.g. SPS/PPS
/// for H.264, ASC for AAC). Until then it is `PendingConfig` or `NotReady`.
///
/// 轨道用于解码的准备状态。
///
/// 当轨道拥有足够配置数据（如 H.264 的 SPS/PPS、AAC 的 ASC）后变为 `Ready`，
/// 在此之前为 `PendingConfig` 或 `NotReady`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackReadiness {
    /// Not enough information to configure a decoder.
    /// 信息不足，无法配置解码器。
    NotReady,
    /// Waiting for required codec configuration.
    /// 等待必需的编解码器配置。
    PendingConfig,
    /// Configuration is complete; the track can be decoded.
    /// 配置完成；轨道可被解码。
    Ready,
}

/// AAC packetization mode when carried over RTP.
///
/// MPEG-4 Generic and LATM are the two common AAC RTP payload formats. They
/// differ in how the AudioSpecificConfig is signaled and how frames are framed.
///
/// 通过 RTP 承载时的 AAC 分包模式。
///
/// MPEG-4 Generic 和 LATM 是两种常见的 AAC RTP 负载格式，它们在
/// AudioSpecificConfig 的协商方式与帧封装方式上不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AacRtpPacketization {
    /// MPEG-4 Generic RTP payload format (RFC 3640).
    /// MPEG-4 Generic RTP 负载格式（RFC 3640）。
    #[default]
    Mpeg4Generic,
    /// Low-overhead MPEG-4 Audio Transport Multiplex (RFC 6416).
    /// 低开销 MPEG-4 音频传输复用（RFC 6416）。
    Latm,
}

/// 32-bit rational number used for frame rate and aspect ratio.
///
/// Represents `num / den` in lowest terms. The denominator must be non-zero.
///
/// 用于帧率和宽高比的 32 位有理数。
///
/// 表示最简形式 `num / den`，分母必须非零。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational32 {
    /// Numerator.
    /// 分子。
    pub num: u32,
    /// Denominator.
    /// 分母。
    pub den: u32,
}

impl Rational32 {
    /// Create a new rational number.
    /// 创建新的有理数。
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
}

/// Codec-specific initialization data needed to configure a decoder.
///
/// This is the codec-layer view of parameter sets, sequence headers, and other
/// configuration that must be delivered before the first media frame.
///
/// 配置解码器所需的编解码器特定初始化数据。
///
/// 这是编解码器层视角的参数集、序列头和其他配置，必须在首帧媒体之前传递。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CodecExtradata {
    /// No extra initialization data.
    /// 无额外初始化数据。
    #[default]
    None,
    /// H.264 parameter sets and optional AVCC extradata.
    /// H.264 参数集与可选 AVCC  extradata。
    H264 {
        /// Sequence parameter sets.
        /// 序列参数集。
        sps: Vec<Bytes>,
        /// Picture parameter sets.
        /// 图像参数集。
        pps: Vec<Bytes>,
        /// Optional AVCDecoderConfigurationRecord.
        /// 可选 AVCDecoderConfigurationRecord。
        avcc: Option<Bytes>,
    },
    /// H.265 parameter sets and optional HVCC extradata.
    /// H.265 参数集与可选 HVCC extradata。
    H265 {
        /// Video parameter set.
        /// 视频参数集。
        vps: Vec<Bytes>,
        /// Sequence parameter sets.
        /// 序列参数集。
        sps: Vec<Bytes>,
        /// Picture parameter sets.
        /// 图像参数集。
        pps: Vec<Bytes>,
        /// Optional HEVCDecoderConfigurationRecord.
        /// 可选 HEVCDecoderConfigurationRecord。
        hvcc: Option<Bytes>,
    },
    /// H.266 parameter sets.
    /// H.266 参数集。
    H266 {
        /// Video parameter set.
        /// 视频参数集。
        vps: Vec<Bytes>,
        /// Sequence parameter sets.
        /// 序列参数集。
        sps: Vec<Bytes>,
        /// Picture parameter sets.
        /// 图像参数集。
        pps: Vec<Bytes>,
    },
    /// AAC AudioSpecificConfig.
    /// AAC AudioSpecificConfig。
    AAC { asc: Bytes },
    /// AV1 sequence header and optional codec configuration.
    /// AV1 序列头与可选编解码器配置。
    AV1 {
        /// AV1 sequence header OBU.
        /// AV1 序列头 OBU。
        sequence_header: Option<Bytes>,
        /// Optional codec configuration OBU.
        /// 可选编解码器配置 OBU。
        codec_config: Option<Bytes>,
    },
    /// VP8 optional configuration.
    /// VP8 可选配置。
    VP8 { config: Option<Bytes> },
    /// VP9 optional configuration.
    /// VP9 可选配置。
    VP9 { config: Option<Bytes> },
    /// MP3 side info.
    /// MP3 边信息。
    MP3 { side_info: Option<Bytes> },
    /// Opus configuration and optional channel mapping.
    /// Opus 配置与可选声道映射。
    Opus {
        /// fmtp parameters from SDP or similar.
        /// 来自 SDP 或类似来源的 fmtp 参数。
        fmtp: Option<String>,
        /// Opus channel mapping.
        /// Opus 声道映射。
        channel_mapping: Option<Bytes>,
    },
    /// Raw opaque bytes for codecs that do not have a structured representation.
    /// 尚未结构化表示的编解码器原始数据。
    Raw(Bytes),
}

/// Whether a codec needs configuration data before it can decode frames.
///
/// `Required` codecs cannot produce frames without SPS/PPS/ASC/etc. `Optional`
/// codecs can use configuration when present. `None` means no config is needed.
///
/// 编解码器在解码帧之前是否需要配置数据。
///
/// `Required` 编解码器没有 SPS/PPS/ASC 等无法解码。`Optional` 编解码器在有配置时
/// 使用。`None` 表示不需要配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecConfigRequirement {
    /// Codec configuration is mandatory.
    /// 编解码器配置为必需。
    Required,
    /// Codec configuration is optional.
    /// 编解码器配置为可选。
    Optional,
    /// No codec configuration is needed.
    /// 不需要编解码器配置。
    None,
}

/// Codec-specific configuration payload that can be delivered to a decoder.
///
/// This is the consumable form of `CodecExtradata` after validation. The engine
/// uses it to set up decoder or muxer parameters for egress.
///
/// 可传递给解码器的编解码器特定配置负载。
///
/// 这是 `CodecExtradata` 经过校验后的可用形式。引擎用它来设置解码器或
/// 出口复用器参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecConfigPayload {
    /// H.264 SPS/PPS and optional AVCC.
    /// H.264 SPS/PPS 与可选 AVCC。
    H264 {
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        avcc: Option<Bytes>,
    },
    /// H.265 VPS/SPS/PPS and optional HVCC.
    /// H.265 VPS/SPS/PPS 与可选 HVCC。
    H265 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        hvcc: Option<Bytes>,
    },
    /// H.266 VPS/SPS/PPS.
    /// H.266 VPS/SPS/PPS。
    H266 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
    },
    /// AAC AudioSpecificConfig.
    /// AAC AudioSpecificConfig。
    AAC { asc: Bytes },
    /// AV1 sequence header and optional codec configuration.
    /// AV1 序列头与可选编解码器配置。
    AV1 {
        sequence_header: Option<Bytes>,
        codec_config: Option<Bytes>,
    },
    /// VP8 optional configuration.
    /// VP8 可选配置。
    VP8 { config: Option<Bytes> },
    /// VP9 optional configuration.
    /// VP9 可选配置。
    VP9 { config: Option<Bytes> },
    /// Opus configuration and optional channel mapping.
    /// Opus 配置与可选声道映射。
    Opus {
        fmtp: Option<String>,
        channel_mapping: Option<Bytes>,
    },
    /// MP3 side info.
    /// MP3 边信息。
    MP3 { side_info: Option<Bytes> },
    /// No configuration payload.
    /// 无配置负载。
    None,
}

/// Validated view of codec configuration for a track.
///
/// Combines `CodecConfigRequirement` (whether the config is needed) and the
/// actual `CodecConfigPayload` so callers can decide how to initialize decoders.
///
/// 轨道编解码器配置经过校验的视图。
///
/// 组合 `CodecConfigRequirement`（配置是否必需）与实际 `CodecConfigPayload`，
/// 使调用方能够决定如何初始化解码器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecConfigView {
    /// Whether the configuration is required, optional, or not needed.
    /// 配置是必需、可选还是不需要。
    pub requirement: CodecConfigRequirement,
    /// The actual configuration payload.
    /// 实际的配置负载。
    pub payload: CodecConfigPayload,
}

/// Error returned when a required codec configuration is missing or invalid.
///
/// 当必需编解码器配置缺失或无效时返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CodecConfigError {
    /// Track is missing the codec configuration required for its codec.
    /// 轨道缺少其编解码器所需的配置。
    #[error("track {track_id:?} codec {codec:?} missing required codec config: {detail}")]
    MissingRequiredConfig {
        track_id: TrackId,
        codec: CodecId,
        detail: &'static str,
    },
}

/// Error returned when track metadata is invalid.
///
/// 轨道元数据无效时返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TrackInfoError {
    /// Clock rate is zero, which cannot produce a valid timebase.
    /// 时钟速率为零，无法生成有效 timebase。
    #[error("track {track_id:?} has invalid clock_rate 0")]
    InvalidClockRate { track_id: TrackId },
}

/// Static metadata describing a single media track.
///
/// `TrackInfo` is carried separately from [`AVFrame`]. It is updated when the
/// publisher changes tracks (e.g. adds audio, switches resolution) and is used
/// by subscribers to configure their decoders.
///
/// 描述单个媒体轨道的静态元数据。
///
/// `TrackInfo` 与 [`AVFrame`] 分开携带。当发布者改变轨道（如添加音频、切换分辨率）
/// 时更新，订阅者用它配置解码器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    /// Track identifier local to the stream.
    /// 流内本地轨道标识。
    pub track_id: TrackId,
    /// Media category (video, audio, data, subtitle).
    /// 媒体类别（视频、音频、数据、字幕）。
    pub media_kind: MediaKind,
    /// Normalized codec identifier.
    /// 归一化后的编解码器标识。
    pub codec: CodecId,
    /// AAC RTP packetization mode.
    /// AAC RTP 分包模式。
    pub aac_rtp_packetization: AacRtpPacketization,
    /// Whether AAC LATM config is carried in-band.
    /// AAC LATM 配置是否带内传输。
    pub aac_latm_config_in_band: bool,
    /// RTP payload type if the track was received over RTP.
    /// 若轨道通过 RTP 接收，则为 RTP 负载类型。
    pub payload_type: Option<u8>,
    /// RTP/codec clock rate in Hz.
    /// RTP/编解码器时钟速率（Hz）。
    pub clock_rate: u32,
    /// Sample rate in Hz if it differs from `clock_rate`.
    /// 若与 `clock_rate` 不同，则为采样率（Hz）。
    pub sample_rate: Option<u32>,
    /// Number of audio channels.
    /// 音频声道数。
    pub channels: Option<u8>,
    /// Video width in pixels.
    /// 视频宽度（像素）。
    pub width: Option<u32>,
    /// Video height in pixels.
    /// 视频高度（像素）。
    pub height: Option<u32>,
    /// Frame rate as a rational number.
    /// 以有理数表示的帧率。
    pub fps: Option<Rational32>,
    /// Bitrate in bits per second.
    /// 比特率（比特/秒）。
    pub bitrate: Option<u32>,
    /// Codec-specific initialization data.
    /// 编解码器特定初始化数据。
    pub extradata: CodecExtradata,
    /// Current readiness state for decoding.
    /// 当前解码准备状态。
    pub readiness: TrackReadiness,
}

impl TrackInfo {
    /// Create a new track with the minimum required fields.
    /// 使用最少必要字段创建新轨道。
    pub fn new(track_id: TrackId, media_kind: MediaKind, codec: CodecId, clock_rate: u32) -> Self {
        Self {
            track_id,
            media_kind,
            codec,
            aac_rtp_packetization: AacRtpPacketization::Mpeg4Generic,
            aac_latm_config_in_band: false,
            payload_type: None,
            clock_rate,
            sample_rate: None,
            channels: None,
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            extradata: CodecExtradata::None,
            readiness: TrackReadiness::NotReady,
        }
    }

    /// Returns `true` if the track has all required configuration to decode.
    /// 返回轨道是否已具备解码所需的全部配置。
    pub fn is_ready(&self) -> bool {
        self.readiness == TrackReadiness::Ready
    }

    /// Returns canonical media timebase derived from RTP/codec clock rate.
    /// A zero clock rate is invalid and must be rejected by callers.
    ///
    /// 返回从 RTP/编解码器时钟速率派生的规范媒体 timebase。
    /// 零时钟速率无效，调用方必须拒绝。
    pub fn media_timebase(&self) -> Result<Timebase, TrackInfoError> {
        if self.clock_rate == 0 {
            return Err(TrackInfoError::InvalidClockRate {
                track_id: self.track_id,
            });
        }
        Ok(Timebase::new(1, self.clock_rate))
    }

    /// Recompute `readiness` from the current `extradata` and `codec`.
    ///
    /// Codecs that need parameter sets (H.264/H.265/H.266/AAC) become `Ready`
    /// once the required data is present. Most audio and simple video codecs
    /// are `Ready` immediately.
    ///
    /// 根据当前 `extradata` 和 `codec` 重新计算 `readiness`。
    ///
    /// 需要参数集的编解码器（H.264/H.265/H.266/AAC）在获得所需数据后变为 `Ready`。
    /// 大多数音频和简单视频编解码器立即 `Ready`。
    pub fn refresh_readiness(&mut self) {
        self.readiness = match self.codec {
            CodecId::H264 | CodecId::H265 | CodecId::H266 => match &self.extradata {
                CodecExtradata::H264 { sps, pps, .. } => {
                    if sps.is_empty() || pps.is_empty() {
                        TrackReadiness::PendingConfig
                    } else {
                        TrackReadiness::Ready
                    }
                }
                CodecExtradata::H265 { vps, sps, pps, .. } => {
                    if vps.is_empty() || sps.is_empty() || pps.is_empty() {
                        TrackReadiness::PendingConfig
                    } else {
                        TrackReadiness::Ready
                    }
                }
                CodecExtradata::H266 { vps, sps, pps } => {
                    if vps.is_empty() || sps.is_empty() || pps.is_empty() {
                        TrackReadiness::PendingConfig
                    } else {
                        TrackReadiness::Ready
                    }
                }
                _ => TrackReadiness::PendingConfig,
            },
            CodecId::AAC => match &self.extradata {
                CodecExtradata::AAC { asc } => {
                    if asc.is_empty() {
                        TrackReadiness::PendingConfig
                    } else {
                        TrackReadiness::Ready
                    }
                }
                _ => TrackReadiness::PendingConfig,
            },
            CodecId::AV1 => match &self.extradata {
                CodecExtradata::AV1 {
                    sequence_header,
                    codec_config,
                } => {
                    if sequence_header.as_ref().is_some_and(|v| !v.is_empty())
                        || codec_config.as_ref().is_some_and(|v| !v.is_empty())
                    {
                        TrackReadiness::Ready
                    } else {
                        TrackReadiness::PendingConfig
                    }
                }
                _ => TrackReadiness::PendingConfig,
            },
            CodecId::VP8 | CodecId::VP9 | CodecId::MJPEG => TrackReadiness::Ready,
            CodecId::Opus
            | CodecId::G711A
            | CodecId::G711U
            | CodecId::MP2
            | CodecId::MP3
            | CodecId::ADPCM => TrackReadiness::Ready,
            _ => self.readiness,
        };
    }

    /// Build a validated codec configuration view for this track.
    ///
    /// Returns `MissingRequiredConfig` when the codec needs configuration data
    /// and it is missing or empty. For optional codecs, `None` or empty data is
    /// accepted.
    ///
    /// 为本轨道构建经过校验的编解码器配置视图。
    ///
    /// 当编解码器需要配置数据但缺失或为空时，返回 `MissingRequiredConfig`。
    /// 对于可选编解码器，允许 `None` 或空数据。
    pub fn codec_config_view(&self) -> Result<CodecConfigView, CodecConfigError> {
        let missing = |detail: &'static str| CodecConfigError::MissingRequiredConfig {
            track_id: self.track_id,
            codec: self.codec,
            detail,
        };

        let view = match self.codec {
            CodecId::H264 => {
                let CodecExtradata::H264 { sps, pps, avcc } = &self.extradata else {
                    return Err(missing("expected H264 extradata"));
                };
                if sps.is_empty() || sps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty SPS list"));
                }
                if pps.is_empty() || pps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty PPS list"));
                }
                CodecConfigView {
                    requirement: CodecConfigRequirement::Required,
                    payload: CodecConfigPayload::H264 {
                        sps: sps.clone(),
                        pps: pps.clone(),
                        avcc: avcc.clone(),
                    },
                }
            }
            CodecId::H265 => {
                let CodecExtradata::H265 {
                    vps,
                    sps,
                    pps,
                    hvcc,
                } = &self.extradata
                else {
                    return Err(missing("expected H265 extradata"));
                };
                if vps.is_empty() || vps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty VPS list"));
                }
                if sps.is_empty() || sps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty SPS list"));
                }
                if pps.is_empty() || pps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty PPS list"));
                }
                CodecConfigView {
                    requirement: CodecConfigRequirement::Required,
                    payload: CodecConfigPayload::H265 {
                        vps: vps.clone(),
                        sps: sps.clone(),
                        pps: pps.clone(),
                        hvcc: hvcc.clone(),
                    },
                }
            }
            CodecId::H266 => {
                let CodecExtradata::H266 { vps, sps, pps } = &self.extradata else {
                    return Err(missing("expected H266 extradata"));
                };
                if vps.is_empty() || vps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty VPS list"));
                }
                if sps.is_empty() || sps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty SPS list"));
                }
                if pps.is_empty() || pps.iter().any(Bytes::is_empty) {
                    return Err(missing("empty PPS list"));
                }
                CodecConfigView {
                    requirement: CodecConfigRequirement::Required,
                    payload: CodecConfigPayload::H266 {
                        vps: vps.clone(),
                        sps: sps.clone(),
                        pps: pps.clone(),
                    },
                }
            }
            CodecId::AAC => {
                let CodecExtradata::AAC { asc } = &self.extradata else {
                    return Err(missing("expected AAC extradata"));
                };
                if asc.is_empty() {
                    return Err(missing("empty AAC ASC"));
                }
                CodecConfigView {
                    requirement: CodecConfigRequirement::Required,
                    payload: CodecConfigPayload::AAC { asc: asc.clone() },
                }
            }
            CodecId::AV1 => {
                let (sequence_header, codec_config) = match &self.extradata {
                    CodecExtradata::AV1 {
                        sequence_header,
                        codec_config,
                    } => (sequence_header.clone(), codec_config.clone()),
                    _ => (None, None),
                };
                CodecConfigView {
                    requirement: CodecConfigRequirement::Optional,
                    payload: CodecConfigPayload::AV1 {
                        sequence_header,
                        codec_config,
                    },
                }
            }
            CodecId::VP8 => {
                let config = match &self.extradata {
                    CodecExtradata::VP8 { config } => config.clone(),
                    _ => None,
                };
                CodecConfigView {
                    requirement: CodecConfigRequirement::Optional,
                    payload: CodecConfigPayload::VP8 { config },
                }
            }
            CodecId::VP9 => {
                let config = match &self.extradata {
                    CodecExtradata::VP9 { config } => config.clone(),
                    _ => None,
                };
                CodecConfigView {
                    requirement: CodecConfigRequirement::Optional,
                    payload: CodecConfigPayload::VP9 { config },
                }
            }
            CodecId::Opus => {
                let (fmtp, channel_mapping) = match &self.extradata {
                    CodecExtradata::Opus {
                        fmtp,
                        channel_mapping,
                    } => (fmtp.clone(), channel_mapping.clone()),
                    _ => (None, None),
                };
                CodecConfigView {
                    requirement: CodecConfigRequirement::Optional,
                    payload: CodecConfigPayload::Opus {
                        fmtp,
                        channel_mapping,
                    },
                }
            }
            CodecId::MP3 => {
                let side_info = match &self.extradata {
                    CodecExtradata::MP3 { side_info } => side_info.clone(),
                    _ => None,
                };
                CodecConfigView {
                    requirement: CodecConfigRequirement::Optional,
                    payload: CodecConfigPayload::MP3 { side_info },
                }
            }
            CodecId::G711A
            | CodecId::G711U
            | CodecId::MP2
            | CodecId::ADPCM
            | CodecId::MJPEG
            | CodecId::Unknown => CodecConfigView {
                requirement: CodecConfigRequirement::None,
                payload: CodecConfigPayload::None,
            },
        };
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn derives_media_timebase_from_clock_rate() {
        let track = TrackInfo::new(TrackId(10), MediaKind::Audio, CodecId::AAC, 48_000);
        let tb = track.media_timebase().expect("valid clock rate");
        assert_eq!(tb, Timebase::new(1, 48_000));
    }

    #[test]
    fn rejects_zero_clock_rate() {
        let track = TrackInfo::new(TrackId(11), MediaKind::Video, CodecId::H264, 0);
        let err = track
            .media_timebase()
            .expect_err("zero clock rate must fail");
        assert_eq!(
            err,
            TrackInfoError::InvalidClockRate {
                track_id: TrackId(11)
            }
        );
    }

    #[test]
    fn codec_config_view_reports_required_and_optional_semantics() {
        let mut h265 = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        h265.extradata = CodecExtradata::H265 {
            vps: vec![Bytes::from_static(&[0x40, 0x01])],
            sps: vec![Bytes::from_static(&[0x42, 0x01])],
            pps: vec![Bytes::from_static(&[0x44, 0x01])],
            hvcc: None,
        };
        let h265_view = h265.codec_config_view().expect("h265 config");
        assert!(matches!(
            h265_view.requirement,
            CodecConfigRequirement::Required
        ));
        assert!(matches!(h265_view.payload, CodecConfigPayload::H265 { .. }));

        let mut av1 = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::AV1, 90_000);
        av1.extradata = CodecExtradata::AV1 {
            sequence_header: None,
            codec_config: Some(Bytes::from_static(&[0x81, 0x00])),
        };
        let av1_view = av1.codec_config_view().expect("av1 config");
        assert!(matches!(
            av1_view.requirement,
            CodecConfigRequirement::Optional
        ));
        assert!(matches!(av1_view.payload, CodecConfigPayload::AV1 { .. }));
    }
}
