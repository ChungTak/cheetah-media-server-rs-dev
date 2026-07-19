//! Current resource usage for `MediaProcessingProvider`.
//!
//! Used by `apply_config` to decide whether a lowered bound can be applied
//! immediately or requires a module restart.

use cheetah_media_api::processing::{
    AbrVariant, AudioTarget, MosaicLayout, Overlay, OverlayKind, ProcessingJob, ProcessingJobSpec,
    ProcessingJobState, VideoTarget,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct ProcessingUsageSnapshot {
    pub active_jobs: u32,
    pub max_inputs: u32,
    pub max_overlays: u32,
    pub max_image_width: u32,
    pub max_image_height: u32,
    pub max_video_pixel_rate: u64,
    pub max_overlay_text_length: u32,
}

pub(crate) fn usage_from_job(job: &ProcessingJob, usage: &mut ProcessingUsageSnapshot) {
    if matches!(
        job.state,
        ProcessingJobState::Stopped | ProcessingJobState::Failed
    ) {
        return;
    }
    usage.active_jobs += 1;
    apply_spec(&job.spec, usage);
}

fn apply_spec(spec: &ProcessingJobSpec, usage: &mut ProcessingUsageSnapshot) {
    match spec {
        ProcessingJobSpec::Transcode {
            audio,
            video,
            overlays,
            ..
        } => {
            usage.max_inputs = usage.max_inputs.max(1);
            usage.max_overlays = usage.max_overlays.max(overlays.len() as u32);
            for overlay in overlays {
                apply_overlay(overlay, usage);
            }
            if let Some(video) = video {
                apply_video_target(video, usage);
            }
            if let Some(audio) = audio {
                apply_audio_target(audio, usage);
            }
        }
        ProcessingJobSpec::AbrLadder { variants, .. } => {
            usage.max_inputs = usage.max_inputs.max(1);
            for variant in variants {
                apply_abr_variant(variant, usage);
            }
        }
        ProcessingJobSpec::AudioMix { inputs, output, .. } => {
            usage.max_inputs = usage.max_inputs.max(inputs.len() as u32);
            apply_audio_target(output, usage);
        }
        ProcessingJobSpec::VideoMosaic {
            inputs,
            layout,
            audio_mix,
            overlays,
            ..
        } => {
            usage.max_inputs = usage.max_inputs.max(inputs.len() as u32);
            usage.max_overlays = usage.max_overlays.max(overlays.len() as u32);
            apply_mosaic_layout(layout, usage);
            for overlay in overlays {
                apply_overlay(overlay, usage);
            }
            if let Some(mix) = audio_mix {
                apply_audio_target(&mix.output, usage);
            }
        }
        ProcessingJobSpec::CaptionExtract { .. } => {
            usage.max_inputs = usage.max_inputs.max(1);
        }
    }
}

fn apply_video_target(video: &VideoTarget, usage: &mut ProcessingUsageSnapshot) {
    if let (Some(w), Some(h)) = (video.width, video.height) {
        usage.max_image_width = usage.max_image_width.max(w);
        usage.max_image_height = usage.max_image_height.max(h);
        let fps = fps_from(&video.frame_rate_num, &video.frame_rate_den).unwrap_or(30.0);
        let pixel_rate = (w as u64)
            .saturating_mul(h as u64)
            .saturating_mul(fps.max(0.0) as u64);
        usage.max_video_pixel_rate = usage.max_video_pixel_rate.max(pixel_rate);
    }
}

fn apply_mosaic_layout(layout: &MosaicLayout, usage: &mut ProcessingUsageSnapshot) {
    let w = layout.columns.saturating_mul(layout.cell_width);
    let h = layout.rows.saturating_mul(layout.cell_height);
    usage.max_image_width = usage.max_image_width.max(w);
    usage.max_image_height = usage.max_image_height.max(h);
    let fps = fps_from(&layout.frame_rate_num, &layout.frame_rate_den).unwrap_or(30.0);
    let pixel_rate = (w as u64)
        .saturating_mul(h as u64)
        .saturating_mul(fps.max(0.0) as u64);
    usage.max_video_pixel_rate = usage.max_video_pixel_rate.max(pixel_rate);
}

fn apply_abr_variant(variant: &AbrVariant, usage: &mut ProcessingUsageSnapshot) {
    apply_video_target(&variant.video, usage);
    if let Some(audio) = &variant.audio {
        apply_audio_target(audio, usage);
    }
}

fn apply_audio_target(_audio: &AudioTarget, _usage: &mut ProcessingUsageSnapshot) {
    // Encoded frame size for audio is not tracked at the job spec level.
}

fn apply_overlay(overlay: &Overlay, usage: &mut ProcessingUsageSnapshot) {
    if let OverlayKind::Text { text, .. } = &overlay.kind {
        let chars = text.chars().count() as u32;
        usage.max_overlay_text_length = usage.max_overlay_text_length.max(chars);
    }
}

fn fps_from(num: &Option<u32>, den: &Option<u32>) -> Option<f64> {
    match (num, den) {
        (Some(n), Some(d)) if *d != 0 => Some(*n as f64 / *d as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::{FileHandle, MediaKey, ProcessingJobId};
    use cheetah_media_api::processing::{
        AudioCodec, AudioMixInput, AudioTarget, MosaicCell, MosaicLayout, VideoCodec, VideoTarget,
    };

    fn job(spec: ProcessingJobSpec) -> ProcessingJob {
        ProcessingJob {
            job_id: ProcessingJobId("test".to_string()),
            owner: None,
            spec,
            state: ProcessingJobState::Running,
            generation: 1,
            profile: "software".to_string(),
            created_at: 0,
            updated_at: 0,
            started_at: None,
            first_output_at: None,
            finished_at: None,
            input_keys: Vec::new(),
            output_keys: Vec::new(),
            ref_count: 0,
            restart_count: 0,
            frames_in: 0,
            frames_out: 0,
            bytes_in: 0,
            bytes_out: 0,
            pending: 0,
            drops: 0,
            flushes: 0,
            resets: 0,
            last_error: None,
        }
    }

    fn key(name: &str) -> MediaKey {
        MediaKey::with_default_vhost("live", name, None).unwrap()
    }

    #[test]
    fn audio_mix_counts_inputs() {
        let mut usage = ProcessingUsageSnapshot::default();
        usage_from_job(
            &job(ProcessingJobSpec::AudioMix {
                inputs: vec![
                    AudioMixInput {
                        source: key("a"),
                        gain_db: None,
                    },
                    AudioMixInput {
                        source: key("b"),
                        gain_db: None,
                    },
                ],
                target: key("out"),
                output: AudioTarget {
                    codec: AudioCodec::Aac,
                    sample_rate: None,
                    channels: None,
                    bit_rate: None,
                },
            }),
            &mut usage,
        );
        assert_eq!(usage.active_jobs, 1);
        assert_eq!(usage.max_inputs, 2);
    }

    #[test]
    fn video_mosaic_computes_pixel_rate() {
        let mut usage = ProcessingUsageSnapshot::default();
        usage_from_job(
            &job(ProcessingJobSpec::VideoMosaic {
                inputs: vec![
                    cheetah_media_api::processing::VideoMosaicInput {
                        source: key("a"),
                        cell: MosaicCell {
                            column: 0,
                            row: 0,
                            z_order: 0,
                        },
                        audio_gain_db: None,
                        fit: None,
                        label: None,
                    },
                    cheetah_media_api::processing::VideoMosaicInput {
                        source: key("b"),
                        cell: MosaicCell {
                            column: 1,
                            row: 0,
                            z_order: 0,
                        },
                        audio_gain_db: None,
                        fit: None,
                        label: None,
                    },
                ],
                target: key("out"),
                layout: MosaicLayout {
                    columns: 2,
                    rows: 1,
                    cell_width: 640,
                    cell_height: 360,
                    background: None,
                    frame_rate_num: Some(30),
                    frame_rate_den: Some(1),
                    bit_rate: None,
                    gop_size: None,
                    video_codec: Some(VideoCodec::H264),
                    fit: None,
                },
                audio_mix: None,
                overlays: Vec::new(),
            }),
            &mut usage,
        );
        assert_eq!(usage.active_jobs, 1);
        assert_eq!(usage.max_inputs, 2);
        assert_eq!(usage.max_image_width, 1280);
        assert_eq!(usage.max_image_height, 360);
        assert_eq!(usage.max_video_pixel_rate, 1280 * 360 * 30);
    }

    #[test]
    fn transcode_with_text_overlay_counts_chars() {
        let mut usage = ProcessingUsageSnapshot::default();
        usage_from_job(
            &job(ProcessingJobSpec::Transcode {
                source: key("in"),
                target: key("out"),
                track_selection: cheetah_media_api::processing::TrackSelection::All,
                audio: None,
                video: Some(VideoTarget {
                    codec: VideoCodec::H264,
                    width: Some(1920),
                    height: Some(1080),
                    frame_rate_num: Some(60),
                    frame_rate_den: Some(1),
                    bit_rate: None,
                    gop_size: None,
                    profile: None,
                }),
                overlays: vec![Overlay {
                    kind: OverlayKind::Text {
                        text: "hello 世界".to_string(),
                        font_handle: FileHandle("font".to_string()),
                    },
                    position: cheetah_media_api::processing::OverlayPosition { x: 0, y: 0 },
                    size: None,
                    opacity: None,
                }],
            }),
            &mut usage,
        );
        assert_eq!(usage.max_inputs, 1);
        assert_eq!(usage.max_overlays, 1);
        assert_eq!(usage.max_image_width, 1920);
        assert_eq!(usage.max_image_height, 1080);
        assert_eq!(usage.max_video_pixel_rate, 1920 * 1080 * 60);
        assert_eq!(usage.max_overlay_text_length, 8); // 'hello 世界' is 8 chars
    }
}
