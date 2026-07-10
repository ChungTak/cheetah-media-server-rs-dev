use crate::prelude::*;
use bitflags::bitflags;
use bytes::Bytes;

use crate::audio::{adts_strip, AacAudioSpecificConfig};
use crate::track::{CodecId, MediaKind};

/// Protocol family for compatibility/profile selection.
///
/// 用于兼容/配置选择的协议族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    /// Real-Time Messaging Protocol.
    ///
    /// 实时消息传输协议。
    Rtmp,
    /// Real-Time Streaming Protocol.
    ///
    /// 实时流协议。
    Rtsp,
    /// HTTP Live Streaming.
    ///
    /// HTTP 直播流协议。
    Hls,
    /// Flash Video.
    ///
    /// Flash 视频。
    Flv,
    /// Web Real-Time Communication.
    ///
    /// Web 实时通信。
    Webrtc,
    /// Secure Reliable Transport.
    ///
    /// 安全可靠传输协议。
    Srt,
    /// GB/T 28181 / GB35114 / JTT1078-style public security surveillance.
    ///
    /// GB/T 28181 / GB35114 / JTT1078  style 公共安全视频监控协议。
    Gb28181,
    /// Unknown or unclassified protocol.
    ///
    /// 未知或未分类协议。
    Unknown,
}

bitflags! {
    /// Compatibility flags that control ingress/egress normalization quirks.
    ///
    /// 控制入站/出站规范化兼容行为的标志位。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CompatFlags: u64 {
        /// Allow both 3-byte and 4-byte H.26x start codes; normalize to 4-byte.
        ///
        /// 允许 3 字节和 4 字节 H.26x 起始码，并统一归一化为 4 字节。
        const RELAX_START_CODE = 1 << 0;
        /// Require an Access Unit Delimiter before emitting frames.
        ///
        /// 要求在每个输出帧前存在 Access Unit Delimiter。
        const REQUIRE_AUD = 1 << 1;
        /// Force AAC AudioSpecificConfig generation even when missing.
        ///
        /// 即使缺失也强制生成 AAC AudioSpecificConfig。
        const FORCE_ASC = 1 << 2;
        /// Use packet arrival time instead of sender timestamps when ambiguous.
        ///
        /// 时间戳不明确时使用包到达时间。
        const TRUST_ARRIVAL_TIME = 1 << 3;
    }
}

/// Vendor/client-specific compatibility profile.
///
/// 厂商/客户端特定的兼容配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatProfile {
    /// Protocol family of the peer.
    ///
    /// 对端协议族。
    pub protocol: ProtocolKind,
    /// Vendor name (e.g. "ZLMediaKit").
    ///
    /// 厂商名称（例如 "ZLMediaKit"）。
    pub vendor: Option<String>,
    /// Device model string.
    ///
    /// 设备型号字符串。
    pub device_model: Option<String>,
    /// Client family / software family.
    ///
    /// 客户端/软件家族。
    pub client_family: Option<String>,
    /// Active compatibility flags.
    ///
    /// 激活的兼容标志。
    pub flags: CompatFlags,
}

/// Apply `CompatProfile` normalization to a codec payload.
///
/// 对编解码器负载应用 `CompatProfile` 规范化。
pub fn apply_compat_profile(profile: &CompatProfile, codec: CodecId, payload: &[u8]) -> Bytes {
    let mut out = Bytes::copy_from_slice(payload);
    if profile.flags.contains(CompatFlags::RELAX_START_CODE)
        && matches!(codec, CodecId::H264 | CodecId::H265 | CodecId::H266)
    {
        out = normalize_h26x_start_codes(&out);
    }
    out
}

/// Normalize 3-byte `00 00 01` H.26x start codes to 4-byte `00 00 00 01`.
///
/// 将 3 字节 `00 00 01` H.26x 起始码归一化为 4 字节 `00 00 00 01`。
pub fn normalize_h26x_start_codes(payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 16);
    let mut i = 0usize;
    while i < payload.len() {
        if i + 3 < payload.len()
            && payload[i] == 0
            && payload[i + 1] == 0
            && payload[i + 2] == 0
            && payload[i + 3] == 1
        {
            out.extend_from_slice(&[0, 0, 0, 1]);
            i += 4;
            continue;
        }
        if i + 2 < payload.len() && payload[i] == 0 && payload[i + 1] == 0 && payload[i + 2] == 1 {
            out.extend_from_slice(&[0, 0, 0, 1]);
            i += 3;
            continue;
        }
        out.push(payload[i]);
        i += 1;
    }
    Bytes::from(out)
}

/// Derive an AAC AudioSpecificConfig from the ADTS header of a frame.
///
/// 从帧的 ADTS 头部推导 AAC AudioSpecificConfig。
pub fn infer_aac_asc_from_adts(frame: &[u8]) -> Option<AacAudioSpecificConfig> {
    let (header, _) = adts_strip(frame)?;
    Some(AacAudioSpecificConfig {
        audio_object_type: header.profile.saturating_add(1),
        sampling_frequency_index: header.sampling_frequency_index,
        channel_configuration: header.channel_configuration,
    })
}

pub const RTMP_AUDIO_CODEC_ID_ADPCM: u8 = 1;
pub const RTMP_AUDIO_CODEC_ID_MP3: u8 = 2;
pub const RTMP_VIDEO_CODEC_ID_H264: u8 = 7;
pub const RTMP_AUDIO_CODEC_ID_G711A: u8 = 7;
pub const RTMP_AUDIO_CODEC_ID_G711U: u8 = 8;
pub const RTMP_AUDIO_CODEC_ID_AAC: u8 = 10;
pub const RTMP_VIDEO_CODEC_ID_H265: u8 = 12;
pub const RTMP_VIDEO_CODEC_ID_AV1: u8 = 13;
pub const RTMP_AUDIO_CODEC_ID_OPUS: u8 = 13;
pub const RTMP_VIDEO_CODEC_ID_H266: u8 = 14;
pub const RTMP_VIDEO_CODEC_ID_VP9: u8 = 16;

// 国内扩展 codec ID (ZLMediaKit / domestic vendor convention)
// These conflict with standard assignments (e.g., 14 = H266 in standard, VP8 in domestic).
pub const DOMESTIC_VIDEO_CODEC_ID_H265: u8 = 12;
pub const DOMESTIC_VIDEO_CODEC_ID_AV1: u8 = 13;
pub const DOMESTIC_VIDEO_CODEC_ID_VP8: u8 = 14;
pub const DOMESTIC_VIDEO_CODEC_ID_VP9: u8 = 15;
pub const DOMESTIC_AUDIO_CODEC_ID_OPUS: u8 = 13;

/// Controls how ambiguous legacy RTMP codec IDs are interpreted.
///
/// 控制对存在歧义的旧版 RTMP codec ID 的解释方式。
///
/// 标准映射中 codec ID 14 对应 H.266（VVC），而国内/ZLMediaKit 约定中 14 对应 VP8。
/// 该模式用于解决此冲突。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DomesticCodecMode {
    /// Standard mapping: 14=H266, 16=VP9. No VP8 legacy ID.
    ///
    /// 标准映射：14=H266，16=VP9，无 VP8 旧 ID。
    #[default]
    Standard,
    /// Domestic/ZLMediaKit mapping: 14=VP8, 15=VP9. No H266 legacy ID.
    ///
    /// 国内/ZLMediaKit 映射：14=VP8，15=VP9，无 H266 旧 ID。
    Domestic,
    /// Auto-detect: attempt to disambiguate based on payload inspection.
    /// Falls back to domestic interpretation since H.266 is rare and typically
    /// uses Enhanced RTMP FourCC instead.
    ///
    /// 自动检测：尝试基于负载检查消除歧义。
    /// 由于 H.266 罕见且通常使用 Enhanced RTMP FourCC，故回退到国内解释。
    Auto,
}

#[doc(hidden)]
#[deprecated(note = "use RTMP_VIDEO_CODEC_ID_H264")]
pub const RTMP_CODEC_ID_H264: u8 = RTMP_VIDEO_CODEC_ID_H264;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_G711A")]
pub const RTMP_CODEC_ID_G711A: u8 = RTMP_AUDIO_CODEC_ID_G711A;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_G711U")]
pub const RTMP_CODEC_ID_G711U: u8 = RTMP_AUDIO_CODEC_ID_G711U;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_AAC")]
pub const RTMP_CODEC_ID_AAC: u8 = RTMP_AUDIO_CODEC_ID_AAC;
#[doc(hidden)]
#[deprecated(note = "use RTMP_VIDEO_CODEC_ID_H265")]
pub const RTMP_CODEC_ID_H265: u8 = RTMP_VIDEO_CODEC_ID_H265;
#[doc(hidden)]
#[deprecated(note = "use RTMP_VIDEO_CODEC_ID_AV1")]
pub const RTMP_CODEC_ID_AV1: u8 = RTMP_VIDEO_CODEC_ID_AV1;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_OPUS")]
pub const RTMP_CODEC_ID_OPUS: u8 = RTMP_AUDIO_CODEC_ID_OPUS;
#[doc(hidden)]
#[deprecated(note = "use RTMP_VIDEO_CODEC_ID_H266")]
pub const RTMP_CODEC_ID_H266: u8 = RTMP_VIDEO_CODEC_ID_H266;
#[doc(hidden)]
#[deprecated(note = "use RTMP_VIDEO_CODEC_ID_VP9")]
pub const RTMP_CODEC_ID_VP9: u8 = RTMP_VIDEO_CODEC_ID_VP9;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_ADPCM")]
pub const RTMP_CODEC_ID_ADPCM: u8 = RTMP_AUDIO_CODEC_ID_ADPCM;
#[doc(hidden)]
#[deprecated(note = "use RTMP_AUDIO_CODEC_ID_MP3")]
pub const RTMP_CODEC_ID_MP3: u8 = RTMP_AUDIO_CODEC_ID_MP3;

const fn fourcc(value: [u8; 4]) -> u32 {
    ((value[0] as u32) << 24)
        | ((value[1] as u32) << 16)
        | ((value[2] as u32) << 8)
        | value[3] as u32
}

pub const RTMP_FOURCC_H265: u32 = fourcc(*b"hvc1");
pub const RTMP_FOURCC_H266: u32 = fourcc(*b"vvc1");
pub const RTMP_FOURCC_H264: u32 = fourcc(*b"avc1");
pub const RTMP_FOURCC_AV1: u32 = fourcc(*b"av01");
pub const RTMP_FOURCC_VP8: u32 = fourcc(*b"vp08");
pub const RTMP_FOURCC_VP9: u32 = fourcc(*b"vp09");

/// Convert a standard RTMP codec ID to a canonical `CodecId`.
///
/// 将标准 RTMP codec ID 转换为规范 `CodecId`。
pub fn codec_from_rtmp_codec_id(media: MediaKind, codec_id: u8) -> Option<CodecId> {
    match media {
        MediaKind::Video => match codec_id {
            RTMP_VIDEO_CODEC_ID_H264 => Some(CodecId::H264),
            RTMP_VIDEO_CODEC_ID_H265 => Some(CodecId::H265),
            RTMP_VIDEO_CODEC_ID_AV1 => Some(CodecId::AV1),
            RTMP_VIDEO_CODEC_ID_H266 => Some(CodecId::H266),
            RTMP_VIDEO_CODEC_ID_VP9 => Some(CodecId::VP9),
            _ => None,
        },
        MediaKind::Audio => match codec_id {
            RTMP_AUDIO_CODEC_ID_AAC => Some(CodecId::AAC),
            RTMP_AUDIO_CODEC_ID_ADPCM => Some(CodecId::ADPCM),
            RTMP_AUDIO_CODEC_ID_G711A => Some(CodecId::G711A),
            RTMP_AUDIO_CODEC_ID_G711U => Some(CodecId::G711U),
            RTMP_AUDIO_CODEC_ID_MP3 => Some(CodecId::MP3),
            RTMP_AUDIO_CODEC_ID_OPUS => Some(CodecId::Opus),
            _ => None,
        },
        _ => None,
    }
}

/// Resolves RTMP codec ID with domestic extension awareness.
///
/// 在考虑国内扩展的情况下解析 RTMP codec ID。
///
/// 在 `Auto` 或 `Domestic` 模式下，codec ID 14 映射为 VP8、15 映射为 VP9（ZLMediaKit 约定）。
/// 在 `Standard` 模式下，14 映射为 H266、16 映射为 VP9。
pub fn codec_from_rtmp_codec_id_with_mode(
    media: MediaKind,
    codec_id: u8,
    mode: DomesticCodecMode,
) -> Option<CodecId> {
    match media {
        MediaKind::Video => match mode {
            DomesticCodecMode::Standard => match codec_id {
                RTMP_VIDEO_CODEC_ID_H264 => Some(CodecId::H264),
                RTMP_VIDEO_CODEC_ID_H265 => Some(CodecId::H265),
                RTMP_VIDEO_CODEC_ID_AV1 => Some(CodecId::AV1),
                RTMP_VIDEO_CODEC_ID_H266 => Some(CodecId::H266),
                RTMP_VIDEO_CODEC_ID_VP9 => Some(CodecId::VP9),
                _ => None,
            },
            DomesticCodecMode::Domestic | DomesticCodecMode::Auto => match codec_id {
                RTMP_VIDEO_CODEC_ID_H264 => Some(CodecId::H264),
                DOMESTIC_VIDEO_CODEC_ID_H265 => Some(CodecId::H265),
                DOMESTIC_VIDEO_CODEC_ID_AV1 => Some(CodecId::AV1),
                DOMESTIC_VIDEO_CODEC_ID_VP8 => Some(CodecId::VP8),
                DOMESTIC_VIDEO_CODEC_ID_VP9 => Some(CodecId::VP9),
                RTMP_VIDEO_CODEC_ID_VP9 => Some(CodecId::VP9),
                _ => None,
            },
        },
        MediaKind::Audio => match codec_id {
            RTMP_AUDIO_CODEC_ID_AAC => Some(CodecId::AAC),
            RTMP_AUDIO_CODEC_ID_ADPCM => Some(CodecId::ADPCM),
            RTMP_AUDIO_CODEC_ID_G711A => Some(CodecId::G711A),
            RTMP_AUDIO_CODEC_ID_G711U => Some(CodecId::G711U),
            RTMP_AUDIO_CODEC_ID_MP3 => Some(CodecId::MP3),
            RTMP_AUDIO_CODEC_ID_OPUS => Some(CodecId::Opus),
            _ => None,
        },
        _ => None,
    }
}

/// Returns the domestic extension codec ID for egress in domestic mode.
///
/// 返回国内扩展模式下输出时使用的 codec ID。
pub fn rtmp_domestic_codec_id_from_codec(codec: CodecId) -> Option<u8> {
    match codec {
        CodecId::H264 => Some(RTMP_VIDEO_CODEC_ID_H264),
        CodecId::H265 => Some(DOMESTIC_VIDEO_CODEC_ID_H265),
        CodecId::AV1 => Some(DOMESTIC_VIDEO_CODEC_ID_AV1),
        CodecId::VP8 => Some(DOMESTIC_VIDEO_CODEC_ID_VP8),
        CodecId::VP9 => Some(DOMESTIC_VIDEO_CODEC_ID_VP9),
        CodecId::AAC => Some(RTMP_AUDIO_CODEC_ID_AAC),
        CodecId::ADPCM => Some(RTMP_AUDIO_CODEC_ID_ADPCM),
        CodecId::G711A => Some(RTMP_AUDIO_CODEC_ID_G711A),
        CodecId::G711U => Some(RTMP_AUDIO_CODEC_ID_G711U),
        CodecId::MP3 => Some(RTMP_AUDIO_CODEC_ID_MP3),
        CodecId::Opus => Some(DOMESTIC_AUDIO_CODEC_ID_OPUS),
        _ => None,
    }
}

/// Convert a canonical `CodecId` to a standard RTMP codec ID.
///
/// 将规范 `CodecId` 转换为标准 RTMP codec ID。
pub fn rtmp_codec_id_from_codec(codec: CodecId) -> Option<u8> {
    match codec {
        CodecId::H264 => Some(RTMP_VIDEO_CODEC_ID_H264),
        CodecId::H265 => Some(RTMP_VIDEO_CODEC_ID_H265),
        CodecId::AV1 => Some(RTMP_VIDEO_CODEC_ID_AV1),
        CodecId::H266 => Some(RTMP_VIDEO_CODEC_ID_H266),
        CodecId::VP9 => Some(RTMP_VIDEO_CODEC_ID_VP9),
        CodecId::AAC => Some(RTMP_AUDIO_CODEC_ID_AAC),
        CodecId::ADPCM => Some(RTMP_AUDIO_CODEC_ID_ADPCM),
        CodecId::G711A => Some(RTMP_AUDIO_CODEC_ID_G711A),
        CodecId::G711U => Some(RTMP_AUDIO_CODEC_ID_G711U),
        CodecId::MP3 => Some(RTMP_AUDIO_CODEC_ID_MP3),
        CodecId::Opus => Some(RTMP_AUDIO_CODEC_ID_OPUS),
        _ => None,
    }
}

/// Convert an Enhanced RTMP FourCC to a canonical `CodecId`.
///
/// 将 Enhanced RTMP FourCC 转换为规范 `CodecId`。
pub fn codec_from_rtmp_fourcc(fourcc: u32) -> Option<CodecId> {
    match fourcc {
        RTMP_FOURCC_H264 => Some(CodecId::H264),
        RTMP_FOURCC_H265 => Some(CodecId::H265),
        RTMP_FOURCC_H266 => Some(CodecId::H266),
        RTMP_FOURCC_AV1 => Some(CodecId::AV1),
        RTMP_FOURCC_VP8 => Some(CodecId::VP8),
        RTMP_FOURCC_VP9 => Some(CodecId::VP9),
        _ => None,
    }
}

/// Convert a canonical `CodecId` to an Enhanced RTMP FourCC.
///
/// 将规范 `CodecId` 转换为 Enhanced RTMP FourCC。
pub fn rtmp_fourcc_from_codec(codec: CodecId) -> Option<u32> {
    match codec {
        CodecId::H264 => Some(RTMP_FOURCC_H264),
        CodecId::H265 => Some(RTMP_FOURCC_H265),
        CodecId::H266 => Some(RTMP_FOURCC_H266),
        CodecId::AV1 => Some(RTMP_FOURCC_AV1),
        CodecId::VP8 => Some(RTMP_FOURCC_VP8),
        CodecId::VP9 => Some(RTMP_FOURCC_VP9),
        _ => None,
    }
}

/// Resolve a `CodecId` from RTMP metadata, trying numeric and text hints.
///
/// 从 RTMP 元数据解析 `CodecId`，依次尝试数值和文本提示。
pub fn codec_from_rtmp_metadata(
    media: MediaKind,
    numeric: Option<f64>,
    text: Option<&str>,
) -> Option<CodecId> {
    if let Some(value) = numeric {
        let as_u32 = value as u32;
        if let Some(codec) = codec_from_rtmp_codec_id(media, as_u32 as u8) {
            return Some(codec);
        }
        if media == MediaKind::Video {
            if let Some(codec) = codec_from_rtmp_fourcc(as_u32) {
                return Some(codec);
            }
        }
    }

    let text = text?;
    let normalized = text.trim().to_ascii_lowercase();
    match media {
        MediaKind::Video => match normalized.as_str() {
            "h264" | "avc" | "avc1" => Some(CodecId::H264),
            "h265" | "hevc" | "hvc1" => Some(CodecId::H265),
            "h266" | "vvc" | "vvc1" => Some(CodecId::H266),
            "av1" | "av01" => Some(CodecId::AV1),
            "vp9" | "vp09" => Some(CodecId::VP9),
            "vp8" => Some(CodecId::VP8),
            "mjpeg" | "jpeg" | "mjpg" => Some(CodecId::MJPEG),
            _ => None,
        },
        MediaKind::Audio => match normalized.as_str() {
            "aac" => Some(CodecId::AAC),
            "adpcma" | "adpcm" => Some(CodecId::ADPCM),
            "g711a" | "pcma" => Some(CodecId::G711A),
            "g711u" | "pcmu" => Some(CodecId::G711U),
            "mp2" => Some(CodecId::MP2),
            "mp3" => Some(CodecId::MP3),
            "opus" => Some(CodecId::Opus),
            _ => None,
        },
        _ => None,
    }
}

/// Build the legacy RTMP audio flag byte for AAC/ADPCM/G.711/MP3/Opus.
///
/// 为 AAC/ADPCM/G.711/MP3/Opus 构建旧版 RTMP 音频标志字节。
pub fn rtmp_audio_flag(
    codec: CodecId,
    sample_rate: u32,
    bits_per_sample: u8,
    channels: u8,
) -> Option<u8> {
    let codec_id = rtmp_codec_id_from_codec(codec)?;
    if !matches!(
        codec,
        CodecId::AAC
            | CodecId::ADPCM
            | CodecId::G711A
            | CodecId::G711U
            | CodecId::MP2
            | CodecId::MP3
            | CodecId::Opus
    ) {
        return None;
    }

    let rate_code = match sample_rate {
        44_100 | 48_000 => 3u8,
        22_050 => 2u8,
        11_025 => 1u8,
        5_512 | 8_000 | 16_000 => 0u8,
        _ => return None,
    };
    let sample_size_flag = if bits_per_sample >= 16 { 1u8 } else { 0u8 };
    let channel_flag = if channels > 1 { 1u8 } else { 0u8 };
    Some((codec_id << 4) | (rate_code << 2) | (sample_size_flag << 1) | channel_flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_mixed_start_codes() {
        let raw = [0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2];
        let out = normalize_h26x_start_codes(&raw);
        assert_eq!(&out[..8], &[0, 0, 0, 1, 0x67, 1, 0, 0]);
    }

    #[test]
    fn parses_video_metadata_codec_from_fourcc() {
        let codec =
            codec_from_rtmp_metadata(MediaKind::Video, Some(RTMP_FOURCC_H265 as f64), Some(""));
        assert_eq!(codec, Some(CodecId::H265));
    }

    #[test]
    fn maps_audio_flag_for_aac() {
        let flag = rtmp_audio_flag(CodecId::AAC, 44_100, 16, 2).expect("aac flag");
        assert_eq!(flag, 0xaf);
    }

    #[test]
    fn maps_vp8_from_fourcc() {
        assert_eq!(codec_from_rtmp_fourcc(RTMP_FOURCC_VP8), Some(CodecId::VP8));
        assert_eq!(rtmp_fourcc_from_codec(CodecId::VP8), Some(RTMP_FOURCC_VP8));
    }

    #[test]
    fn maps_h264_from_fourcc() {
        assert_eq!(
            codec_from_rtmp_fourcc(RTMP_FOURCC_H264),
            Some(CodecId::H264)
        );
        assert_eq!(
            rtmp_fourcc_from_codec(CodecId::H264),
            Some(RTMP_FOURCC_H264)
        );
    }

    #[test]
    fn standard_mode_maps_14_to_h266() {
        assert_eq!(
            codec_from_rtmp_codec_id_with_mode(MediaKind::Video, 14, DomesticCodecMode::Standard),
            Some(CodecId::H266)
        );
    }

    #[test]
    fn domestic_mode_maps_14_to_vp8() {
        assert_eq!(
            codec_from_rtmp_codec_id_with_mode(MediaKind::Video, 14, DomesticCodecMode::Domestic),
            Some(CodecId::VP8)
        );
    }

    #[test]
    fn domestic_mode_maps_15_to_vp9() {
        assert_eq!(
            codec_from_rtmp_codec_id_with_mode(MediaKind::Video, 15, DomesticCodecMode::Domestic),
            Some(CodecId::VP9)
        );
    }

    #[test]
    fn auto_mode_maps_14_to_vp8() {
        assert_eq!(
            codec_from_rtmp_codec_id_with_mode(MediaKind::Video, 14, DomesticCodecMode::Auto),
            Some(CodecId::VP8)
        );
    }

    #[test]
    fn domestic_mode_still_maps_16_to_vp9() {
        // Legacy VP9=16 is also accepted in domestic mode for backward compat
        assert_eq!(
            codec_from_rtmp_codec_id_with_mode(MediaKind::Video, 16, DomesticCodecMode::Domestic),
            Some(CodecId::VP9)
        );
    }

    #[test]
    fn domestic_egress_codec_id_roundtrip() {
        assert_eq!(rtmp_domestic_codec_id_from_codec(CodecId::VP8), Some(14));
        assert_eq!(rtmp_domestic_codec_id_from_codec(CodecId::VP9), Some(15));
        assert_eq!(rtmp_domestic_codec_id_from_codec(CodecId::H265), Some(12));
        assert_eq!(rtmp_domestic_codec_id_from_codec(CodecId::AV1), Some(13));
    }
}
