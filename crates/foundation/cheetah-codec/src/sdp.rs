use crate::prelude::*;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

use crate::track::{CodecExtradata, CodecId, MediaKind, TrackInfo};

/// SDP `m=` line description generated from a track.
///
/// 从轨道生成的 SDP `m=` 行描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpMediaDescription {
    /// Media type, e.g. "video", "audio", "application", "text".
    ///
    /// 媒体类型，例如 "video"、"audio"、"application"、"text"。
    pub media: String,
    /// RTP payload type number.
    ///
    /// RTP 负载类型号。
    pub payload_type: u8,
    /// SDP codec name.
    ///
    /// SDP 编解码器名称。
    pub codec: String,
    /// RTP clock rate.
    ///
    /// RTP 时钟速率。
    pub clock_rate: u32,
    /// Number of audio channels, if applicable.
    ///
    /// 音频通道数（如适用）。
    pub channels: Option<u8>,
    /// `a=fmtp:` attribute value, if any.
    ///
    /// `a=fmtp:` 属性值（如有）。
    pub fmtp: Option<String>,
}

/// Build an SDP `m=` description from a `TrackInfo`.
///
/// 从 `TrackInfo` 构建 SDP `m=` 描述。
pub fn export_media_description(track: &TrackInfo) -> Option<SdpMediaDescription> {
    let payload_type = track
        .payload_type
        .unwrap_or(default_payload_type(track.codec));
    let codec = codec_name(track.codec).to_string();
    let fmtp = export_fmtp(track);
    let media = match track.media_kind {
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Data => "application",
        MediaKind::Subtitle => "text",
    }
    .to_string();

    Some(SdpMediaDescription {
        media,
        payload_type,
        codec,
        clock_rate: track.clock_rate,
        channels: track.channels,
        fmtp,
    })
}

/// Build the `a=fmtp:` parameter string for a track.
///
/// Produces format-specific parameters such as H.264/H.265 sprop parameter sets,
/// AV1 profile/level/tier, AAC config, or Opus fmtp.
///
/// 为轨道构建 `a=fmtp:` 参数字符串。
///
/// 生成格式相关参数，如 H.264/H.265 sprop 参数集、AV1 profile/level/tier、
/// AAC 配置或 Opus fmtp。
pub fn export_fmtp(track: &TrackInfo) -> Option<String> {
    if track.codec == CodecId::AV1 {
        let (profile, level_idx, tier) = match &track.extradata {
            CodecExtradata::AV1 { codec_config, .. } => codec_config
                .as_deref()
                .and_then(av1c_profile_level_tier)
                .unwrap_or((0, 9, 0)),
            _ => (0, 9, 0),
        };
        return Some(format!(
            "profile={profile};level-idx={level_idx};tier={tier}"
        ));
    }

    match (&track.codec, &track.extradata) {
        (CodecId::H264, CodecExtradata::H264 { sps, pps, .. }) => {
            let sps = sps.first()?;
            let pps = pps.first()?;
            Some(format!(
                "packetization-mode=1;sprop-parameter-sets={},{}",
                b64(sps),
                b64(pps)
            ))
        }
        (CodecId::H265, CodecExtradata::H265 { vps, sps, pps, .. }) => {
            let vps = vps.first()?;
            let sps = sps.first()?;
            let pps = pps.first()?;
            Some(format!(
                "sprop-vps={};sprop-sps={};sprop-pps={}",
                b64(vps),
                b64(sps),
                b64(pps)
            ))
        }
        (CodecId::AAC, CodecExtradata::AAC { asc }) => Some(format!(
            "streamtype=5;profile-level-id=1;mode=AAC-hbr;config={};SizeLength=13;IndexLength=3;IndexDeltaLength=3",
            hex(asc)
        )),
        (CodecId::Opus, CodecExtradata::Opus { fmtp, .. }) => fmtp.clone(),
        _ => None,
    }
}

fn av1c_profile_level_tier(codec_config: &[u8]) -> Option<(u8, u8, u8)> {
    if codec_config.len() < 3 {
        return None;
    }
    let profile = (codec_config[1] >> 5) & 0x07;
    let level_idx = codec_config[1] & 0x1f;
    let tier = (codec_config[2] >> 7) & 0x01;
    Some((profile, level_idx, tier))
}

fn codec_name(codec: CodecId) -> &'static str {
    match codec {
        CodecId::H264 => "H264",
        CodecId::H265 => "H265",
        CodecId::H266 => "H266",
        CodecId::AV1 => "AV1",
        CodecId::VP8 => "VP8",
        CodecId::VP9 => "VP9",
        CodecId::MJPEG => "JPEG",
        CodecId::AAC => "MPEG4-GENERIC",
        CodecId::ADPCM => "ADPCM",
        CodecId::Opus => "opus",
        CodecId::G711A => "PCMA",
        CodecId::G711U => "PCMU",
        CodecId::MP2 => "MPA",
        CodecId::MP3 => "MPA",
        CodecId::WebVtt => "WebVTT",
        CodecId::Unknown => "unknown",
    }
}

fn default_payload_type(codec: CodecId) -> u8 {
    match codec {
        CodecId::G711U => 0,
        CodecId::G711A => 8,
        CodecId::MP2 | CodecId::MP3 => 14,
        _ => 96,
    }
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|v| format!("{v:02x}")).collect::<String>()
}

fn b64(data: &[u8]) -> String {
    BASE64_STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::track::{TrackId, TrackReadiness};

    #[test]
    fn exports_h264_fmtp() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42])],
            pps: vec![Bytes::from_static(&[0x68, 0xce])],
            avcc: None,
        };
        track.readiness = TrackReadiness::Ready;
        let fmtp = export_fmtp(&track).expect("fmtp");
        assert_eq!(fmtp, "packetization-mode=1;sprop-parameter-sets=Z0I=,aM4=");
    }

    #[test]
    fn exports_h265_fmtp_with_base64_parameter_sets() {
        let mut track = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::H265, 90_000);
        track.extradata = CodecExtradata::H265 {
            vps: vec![Bytes::from_static(&[0x40, 0x01, 0x0c])],
            sps: vec![Bytes::from_static(&[0x42, 0x01, 0x01])],
            pps: vec![Bytes::from_static(&[0x44, 0x01, 0xc0])],
            hvcc: None,
        };
        track.readiness = TrackReadiness::Ready;
        let fmtp = export_fmtp(&track).expect("fmtp");
        assert_eq!(fmtp, "sprop-vps=QAEM;sprop-sps=QgEB;sprop-pps=RAHA");
    }

    #[test]
    fn exports_aac_fmtp_with_au_header_fields() {
        let mut track = TrackInfo::new(TrackId(3), MediaKind::Audio, CodecId::AAC, 48_000);
        track.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x11, 0x90]),
        };
        track.readiness = TrackReadiness::Ready;
        let fmtp = export_fmtp(&track).expect("fmtp");
        assert_eq!(
            fmtp,
            "streamtype=5;profile-level-id=1;mode=AAC-hbr;config=1190;SizeLength=13;IndexLength=3;IndexDeltaLength=3"
        );
    }

    #[test]
    fn exports_av1_fmtp_defaults_without_codec_config() {
        let mut track = TrackInfo::new(TrackId(4), MediaKind::Video, CodecId::AV1, 90_000);
        track.readiness = TrackReadiness::Ready;
        let fmtp = export_fmtp(&track).expect("fmtp");
        assert_eq!(fmtp, "profile=0;level-idx=9;tier=0");
    }

    #[test]
    fn exports_av1_fmtp_from_av1c_codec_config() {
        let mut track = TrackInfo::new(TrackId(5), MediaKind::Video, CodecId::AV1, 90_000);
        track.extradata = CodecExtradata::AV1 {
            sequence_header: None,
            codec_config: Some(Bytes::from_static(&[0x81, 0x49, 0x00, 0x00])),
        };
        track.readiness = TrackReadiness::Ready;
        let fmtp = export_fmtp(&track).expect("fmtp");
        assert_eq!(fmtp, "profile=2;level-idx=9;tier=0");
    }
}
