use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_codec::{CodecId, MediaKind, TrackReadiness as CodecTrackReadiness};
use cheetah_media_api::model::{CodecKind, MediaType, TrackReadiness};

/// Current time in milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Convert a codec `MediaKind` to a media-domain `MediaType`.
pub fn media_kind_to_type(k: MediaKind) -> MediaType {
    match k {
        MediaKind::Video => MediaType::Video,
        MediaKind::Audio => MediaType::Audio,
        MediaKind::Data | MediaKind::Subtitle => MediaType::Data,
    }
}

/// Convert a codec `CodecId` to a media-domain `CodecKind`.
pub fn codec_to_api(c: CodecId) -> CodecKind {
    match c {
        CodecId::H264 => CodecKind::H264,
        CodecId::H265 => CodecKind::H265,
        CodecId::H266 => CodecKind::H266,
        CodecId::AV1 => CodecKind::Av1,
        CodecId::VP8 => CodecKind::Vp8,
        CodecId::VP9 => CodecKind::Vp9,
        CodecId::AAC => CodecKind::Aac,
        CodecId::Opus => CodecKind::Opus,
        CodecId::G711A => CodecKind::G711A,
        CodecId::G711U => CodecKind::G711U,
        CodecId::MP3 => CodecKind::Mp3,
        CodecId::MJPEG | CodecId::ADPCM | CodecId::MP2 | CodecId::Unknown => CodecKind::Unknown,
    }
}

/// Convert a codec `TrackReadiness` to a media-domain `TrackReadiness`.
pub fn readiness_to_api(r: CodecTrackReadiness) -> TrackReadiness {
    match r {
        CodecTrackReadiness::NotReady => TrackReadiness::Pending,
        CodecTrackReadiness::PendingConfig => TrackReadiness::Pending,
        CodecTrackReadiness::Ready => TrackReadiness::Ready,
    }
}
