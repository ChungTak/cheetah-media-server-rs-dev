use crate::prelude::*;
use crate::time::Timebase;
use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
    Data,
    Subtitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecId {
    H264,
    H265,
    H266,
    AV1,
    VP8,
    VP9,
    MJPEG,
    AAC,
    ADPCM,
    Opus,
    G711A,
    G711U,
    MP2,
    MP3,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackReadiness {
    NotReady,
    PendingConfig,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AacRtpPacketization {
    #[default]
    Mpeg4Generic,
    Latm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational32 {
    pub num: u32,
    pub den: u32,
}

impl Rational32 {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CodecExtradata {
    #[default]
    None,
    H264 {
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        avcc: Option<Bytes>,
    },
    H265 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        hvcc: Option<Bytes>,
    },
    H266 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
    },
    AAC {
        asc: Bytes,
    },
    AV1 {
        sequence_header: Option<Bytes>,
        codec_config: Option<Bytes>,
    },
    VP8 {
        config: Option<Bytes>,
    },
    VP9 {
        config: Option<Bytes>,
    },
    MP3 {
        side_info: Option<Bytes>,
    },
    Opus {
        fmtp: Option<String>,
        channel_mapping: Option<Bytes>,
    },
    Raw(Bytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecConfigRequirement {
    Required,
    Optional,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecConfigPayload {
    H264 {
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        avcc: Option<Bytes>,
    },
    H265 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
        hvcc: Option<Bytes>,
    },
    H266 {
        vps: Vec<Bytes>,
        sps: Vec<Bytes>,
        pps: Vec<Bytes>,
    },
    AAC {
        asc: Bytes,
    },
    AV1 {
        sequence_header: Option<Bytes>,
        codec_config: Option<Bytes>,
    },
    VP8 {
        config: Option<Bytes>,
    },
    VP9 {
        config: Option<Bytes>,
    },
    Opus {
        fmtp: Option<String>,
        channel_mapping: Option<Bytes>,
    },
    MP3 {
        side_info: Option<Bytes>,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecConfigView {
    pub requirement: CodecConfigRequirement,
    pub payload: CodecConfigPayload,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CodecConfigError {
    #[error("track {track_id:?} codec {codec:?} missing required codec config: {detail}")]
    MissingRequiredConfig {
        track_id: TrackId,
        codec: CodecId,
        detail: &'static str,
    },
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TrackInfoError {
    #[error("track {track_id:?} has invalid clock_rate 0")]
    InvalidClockRate { track_id: TrackId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    pub aac_rtp_packetization: AacRtpPacketization,
    pub aac_latm_config_in_band: bool,
    pub payload_type: Option<u8>,
    pub clock_rate: u32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<Rational32>,
    pub bitrate: Option<u32>,
    pub extradata: CodecExtradata,
    pub readiness: TrackReadiness,
}

impl TrackInfo {
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

    pub fn is_ready(&self) -> bool {
        self.readiness == TrackReadiness::Ready
    }

    /// Returns canonical media timebase derived from RTP/codec clock rate.
    /// A zero clock rate is invalid and must be rejected by callers.
    pub fn media_timebase(&self) -> Result<Timebase, TrackInfoError> {
        if self.clock_rate == 0 {
            return Err(TrackInfoError::InvalidClockRate {
                track_id: self.track_id,
            });
        }
        Ok(Timebase::new(1, self.clock_rate))
    }

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

    #[test]
    fn codec_config_view_rejects_missing_required_h266_parameter_sets() {
        let mut track = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::H266, 90_000);
        track.extradata = CodecExtradata::H266 {
            vps: vec![],
            sps: vec![Bytes::from_static(&[0x78, 0x01])],
            pps: vec![Bytes::from_static(&[0x80, 0x01])],
        };

        let err = track
            .codec_config_view()
            .expect_err("missing h266 vps must be rejected");
        assert!(matches!(
            err,
            CodecConfigError::MissingRequiredConfig {
                codec: CodecId::H266,
                ..
            }
        ));
    }
}
