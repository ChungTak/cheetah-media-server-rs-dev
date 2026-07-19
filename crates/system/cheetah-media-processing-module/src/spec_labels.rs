//! Shared, sanitized label helpers for processing job specs.

use std::collections::HashSet;

use cheetah_media_api::processing::ProcessingJobSpec;

pub(crate) fn job_kind_label(spec: &ProcessingJobSpec) -> &'static str {
    match spec {
        ProcessingJobSpec::CaptionExtract { .. } => "caption",
        #[cfg(feature = "media-processing-cpu")]
        ProcessingJobSpec::Transcode { .. } => "transcode",
        #[cfg(feature = "media-processing-cpu")]
        ProcessingJobSpec::AbrLadder { .. } => "abr",
        #[cfg(feature = "media-processing-cpu")]
        ProcessingJobSpec::AudioMix { .. } => "mix",
        #[cfg(feature = "media-processing-cpu")]
        ProcessingJobSpec::VideoMosaic { .. } => "mosaic",
        #[cfg(not(feature = "media-processing-cpu"))]
        _ => "unknown",
    }
}

pub(crate) fn job_media_codec(spec: &ProcessingJobSpec) -> (&'static str, String) {
    match spec {
        ProcessingJobSpec::Transcode { video, audio, .. } => match (video, audio) {
            (Some(_), Some(_)) => ("mixed", "mixed".to_string()),
            (Some(v), None) => ("video", format!("{0:?}", v.codec).to_lowercase()),
            (None, Some(a)) => ("audio", format!("{0:?}", a.codec).to_lowercase()),
            (None, None) => ("none", "none".to_string()),
        },
        ProcessingJobSpec::AbrLadder { variants, .. } => {
            if variants.is_empty() {
                ("video", "none".to_string())
            } else if variants.iter().any(|v| v.audio.is_some()) {
                ("mixed", "mixed".to_string())
            } else {
                let mut codecs: HashSet<String> = HashSet::new();
                for v in variants {
                    codecs.insert(format!("{0:?}", v.video.codec).to_lowercase());
                }
                (
                    "video",
                    if codecs.len() == 1 {
                        codecs.into_iter().next().unwrap()
                    } else {
                        "mixed".to_string()
                    },
                )
            }
        }
        ProcessingJobSpec::AudioMix { output, .. } => {
            ("audio", format!("{0:?}", output.codec).to_lowercase())
        }
        ProcessingJobSpec::VideoMosaic { layout, .. } => (
            "video",
            format!(
                "{0:?}",
                layout
                    .video_codec
                    .unwrap_or(cheetah_media_api::processing::VideoCodec::H264)
            )
            .to_lowercase(),
        ),
        ProcessingJobSpec::CaptionExtract { .. } => ("video", "unknown".to_string()),
    }
}

pub(crate) fn job_dimensions(spec: &ProcessingJobSpec) -> String {
    match spec {
        ProcessingJobSpec::Transcode { video, audio, .. } => match (video, audio) {
            (Some(v), None) => format_video_dimensions(v),
            (None, Some(a)) => format_audio_dimensions(a),
            (Some(v), Some(_)) => format!("{}+{}", format_video_dimensions(v), "audio"),
            (None, None) => "none".to_string(),
        },
        ProcessingJobSpec::AbrLadder { variants, .. } => {
            if let Some(v) = variants.first() {
                format_video_dimensions(&v.video)
            } else {
                "none".to_string()
            }
        }
        ProcessingJobSpec::AudioMix { output, .. } => format_audio_dimensions(output),
        ProcessingJobSpec::VideoMosaic { layout, .. } => {
            let w = layout.columns * layout.cell_width;
            let h = layout.rows * layout.cell_height;
            format!("{w}x{h}")
        }
        ProcessingJobSpec::CaptionExtract { .. } => "unknown".to_string(),
    }
}

fn format_video_dimensions(v: &cheetah_media_api::processing::VideoTarget) -> String {
    match (v.width, v.height) {
        (Some(w), Some(h)) => format!("{w}x{h}"),
        _ => "video".to_string(),
    }
}

fn format_audio_dimensions(a: &cheetah_media_api::processing::AudioTarget) -> String {
    match (a.sample_rate, a.channels) {
        (Some(sr), Some(ch)) => format!("{sr}Hzx{ch}ch"),
        (Some(sr), None) => format!("{sr}Hz"),
        (None, Some(ch)) => format!("{ch}ch"),
        (None, None) => "audio".to_string(),
    }
}
