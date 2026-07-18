//! RTMP/FLV processing helpers: request AAC/H.264 derived streams.
//!
//! `ensure_derived_push_source` decides whether a push job needs a transcoded
//! stream before connecting to the remote RTMP server, and creates a
//! `ProcessingJobSpec::Transcode` job when needed.

use cheetah_codec::{CodecId, MediaKind, TrackInfo};
use cheetah_sdk::media_api::ids::{MediaKey, StreamKeyBridge};
use cheetah_sdk::media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::processing::{
    AudioCodec, AudioTarget, CreateProcessingJob, ProcessingJobSpec, ProcessingJobState,
    ProcessingPolicy, ProcessingTarget, TrackSelection, VideoCodec, VideoTarget,
};
use cheetah_sdk::{CancellationToken, EngineContext, MediaProcessingApi, SdkError, StreamKey};

/// Derived-stream job handle returned to the push supervisor.
pub struct DerivedPushSource {
    /// The stream key to subscribe to (source or derived).
    pub stream_key: StreamKey,
    /// If a processing job was created, its id so the supervisor can stop it.
    pub processing_job_id: Option<cheetah_sdk::ProcessingJobId>,
}

/// Resolve the actual source stream key for an RTMP push job.
///
/// - `ProcessingPolicy::Passthrough` returns the original `source`.
/// - `ProcessingPolicy::Auto` creates a transcode job only when the source
///   tracks are not already H.264/AAC (or when `TrackSelection` restricts).
/// - `ProcessingPolicy::Transcode` always creates a job with the explicit target.
///
/// The returned `DerivedPushSource` carries the optional job id so the caller can
/// stop the job when the push job stops.
pub async fn ensure_derived_push_source(
    engine: &EngineContext,
    job_name: &str,
    source: StreamKey,
    policy: &ProcessingPolicy,
    track_selection: TrackSelection,
    cancel: &CancellationToken,
) -> Result<DerivedPushSource, SdkError> {
    if matches!(policy, ProcessingPolicy::Passthrough) {
        return Ok(DerivedPushSource {
            stream_key: source,
            processing_job_id: None,
        });
    }

    let processing_api = engine
        .media_services
        .processing()
        .ok_or_else(|| SdkError::Internal("media processing provider is not registered".into()))?;

    let snapshot = engine
        .stream_manager_api
        .get_stream(&source)
        .await
        .map_err(|e| SdkError::Internal(format!("failed to query source stream: {e}")))?;
    let tracks = snapshot.map(|s| s.tracks).unwrap_or_default();

    let Some(target) = build_rtmp_transcode_target(&tracks, policy, track_selection) else {
        return Ok(DerivedPushSource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let source_key = stream_key_to_media_key(&source)?;
    let derived_stream_name = format!(
        "{}_rtmp_{}",
        sanitize_stream_name(&source.path),
        sanitize_stream_name(job_name)
    );
    let target_key = MediaKey::new(
        source_key.vhost.0.clone(),
        source_key.app.0.clone(),
        &derived_stream_name,
        None,
    )
    .map_err(|e| SdkError::InvalidArgument(format!("invalid derived media key: {e}")))?;
    let derived_stream_key = StreamKey::new(source.namespace.clone(), derived_stream_name);

    let ctx = MediaRequestContext::default();
    let spec = ProcessingJobSpec::Transcode {
        source: source_key,
        target: target_key,
        track_selection,
        audio: target.audio,
        video: target.video,
        overlays: Vec::new(),
    };
    let job = processing_api
        .create_job(
            &ctx,
            CreateProcessingJob {
                idempotency_key: Some(format!("rtmp_push_{}_{}", source, job_name)),
                deadline_ms: None,
                spec,
            },
        )
        .await
        .map_err(|e| SdkError::Internal(format!("create transcode job for rtmp push: {e}")))?;

    // Wait briefly for the transcode job to be running.
    wait_for_running_job(
        processing_api.as_ref(),
        &ctx,
        &job.job_id,
        cancel,
        engine.runtime_api.clone(),
    )
    .await?;

    Ok(DerivedPushSource {
        stream_key: derived_stream_key,
        processing_job_id: Some(job.job_id),
    })
}

/// Stop a derived processing job when the push job exits.
pub async fn stop_derived_push_job(engine: &EngineContext, job_id: cheetah_sdk::ProcessingJobId) {
    if let Some(processing_api) = engine.media_services.processing() {
        let ctx = MediaRequestContext::default();
        let _ = processing_api.stop_job(&ctx, &job_id).await;
    }
}

/// Build the transcode target for RTMP output, returning `None` when no
/// transcode is required.
fn build_rtmp_transcode_target(
    tracks: &[TrackInfo],
    policy: &ProcessingPolicy,
    track_selection: TrackSelection,
) -> Option<ProcessingTarget> {
    match policy {
        ProcessingPolicy::Passthrough => None,
        ProcessingPolicy::Transcode { target } => {
            Some(apply_track_selection(target, track_selection))
        }
        ProcessingPolicy::Auto { .. } => build_auto_target(tracks, track_selection),
    }
}

/// For `Auto`, emit H.264/AAC targets when the source is not already in the
/// desired codec. If any track needs transcoding, include targets for all selected
/// tracks so the transcode worker does not drop the already-conformant track.
fn build_auto_target(
    tracks: &[TrackInfo],
    track_selection: TrackSelection,
) -> Option<ProcessingTarget> {
    let include_video = track_selection != TrackSelection::AudioOnly;
    let include_audio = track_selection != TrackSelection::VideoOnly;

    let source_video = tracks.iter().find(|t| t.media_kind == MediaKind::Video);
    let source_audio = tracks.iter().find(|t| t.media_kind == MediaKind::Audio);

    let needs_video_transcode = include_video
        && source_video.is_some()
        && source_video.is_none_or(|t| t.codec != CodecId::H264);
    let needs_audio_transcode = include_audio
        && source_audio.is_some()
        && source_audio.is_none_or(|t| t.codec != CodecId::AAC);

    if !needs_video_transcode && !needs_audio_transcode {
        return None;
    }

    let video = if include_video && source_video.is_some() {
        Some(VideoTarget {
            codec: VideoCodec::H264,
            width: source_video.and_then(|t| t.width),
            height: source_video.and_then(|t| t.height),
            frame_rate_num: source_video.and_then(|t| t.fps.map(|r| r.num)),
            frame_rate_den: source_video.and_then(|t| t.fps.map(|r| r.den)),
            bit_rate: None,
            gop_size: None,
            profile: None,
        })
    } else {
        None
    };

    let audio = if include_audio && source_audio.is_some() {
        Some(AudioTarget {
            codec: AudioCodec::Aac,
            sample_rate: source_audio.and_then(|t| t.sample_rate),
            channels: source_audio.and_then(|t| t.channels),
            bit_rate: None,
        })
    } else {
        None
    };

    Some(ProcessingTarget { video, audio })
}

/// Apply `TrackSelection` to an explicit `ProcessingTarget`.
fn apply_track_selection(
    target: &ProcessingTarget,
    track_selection: TrackSelection,
) -> ProcessingTarget {
    match track_selection {
        TrackSelection::All => target.clone(),
        TrackSelection::AudioOnly => ProcessingTarget {
            video: None,
            audio: target.audio.clone(),
        },
        TrackSelection::VideoOnly => ProcessingTarget {
            video: target.video.clone(),
            audio: None,
        },
    }
}

fn stream_key_to_media_key(stream_key: &StreamKey) -> Result<MediaKey, SdkError> {
    StreamKeyBridge::from_namespace_path(&stream_key.namespace, &stream_key.path)
        .map_err(|e| SdkError::InvalidArgument(format!("invalid stream key: {e}")))
}

fn sanitize_stream_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Poll the processing job until it reaches `Running` or a short timeout passes.
async fn wait_for_running_job(
    processing_api: &dyn MediaProcessingApi,
    ctx: &MediaRequestContext,
    job_id: &cheetah_sdk::ProcessingJobId,
    cancel: &CancellationToken,
    runtime_api: std::sync::Arc<dyn cheetah_sdk::RuntimeApi>,
) -> Result<(), SdkError> {
    let deadline = runtime_api.now().as_micros() + 5_000_000;
    while runtime_api.now().as_micros() < deadline && !cancel.is_cancelled() {
        match processing_api.get_job(ctx, job_id).await {
            Ok(job) if job.state == ProcessingJobState::Running => return Ok(()),
            Ok(job) if job.state == ProcessingJobState::Failed => {
                return Err(SdkError::Internal(format!(
                    "transcode job {job_id} failed: {}",
                    job.last_error.unwrap_or_default()
                )));
            }
            Ok(_) => {}
            Err(_) => break,
        }
        let sleep_deadline =
            cheetah_codec::MonoTime::from_micros(runtime_api.now().as_micros() + 200_000);
        runtime_api.sleep_until(sleep_deadline).wait().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_track(codec: CodecId) -> TrackInfo {
        TrackInfo::new(cheetah_codec::TrackId(0), MediaKind::Video, codec, 90_000)
    }

    fn audio_track(codec: CodecId) -> TrackInfo {
        TrackInfo::new(cheetah_codec::TrackId(1), MediaKind::Audio, codec, 48_000)
    }

    #[test]
    fn passthrough_never_transcodes() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::AAC)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Passthrough,
            TrackSelection::All,
        );
        assert!(target.is_none());
    }

    #[test]
    fn auto_skips_transcode_for_h264_aac() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::AAC)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: cheetah_sdk::ProcessingPreset::Balanced,
            },
            TrackSelection::All,
        );
        assert!(target.is_none());
    }

    #[test]
    fn auto_requests_h264_aac_for_unsupported_codecs() {
        let tracks = vec![video_track(CodecId::H265), audio_track(CodecId::Opus)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: cheetah_sdk::ProcessingPreset::Balanced,
            },
            TrackSelection::All,
        )
        .unwrap();
        assert!(target.video.is_some());
        assert!(target.audio.is_some());
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Aac);
    }

    #[test]
    fn track_selection_limits_auto_target() {
        let tracks = vec![video_track(CodecId::H265), audio_track(CodecId::Opus)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: cheetah_sdk::ProcessingPreset::Balanced,
            },
            TrackSelection::AudioOnly,
        )
        .unwrap();
        assert!(target.video.is_none());
        assert!(target.audio.is_some());
    }

    #[test]
    fn auto_keeps_h264_when_audio_needs_transcode() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::Opus)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: cheetah_sdk::ProcessingPreset::Balanced,
            },
            TrackSelection::All,
        )
        .unwrap();
        assert!(target.video.is_some());
        assert!(target.audio.is_some());
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Aac);
    }

    #[test]
    fn auto_keeps_aac_when_video_needs_transcode() {
        let tracks = vec![video_track(CodecId::H265), audio_track(CodecId::AAC)];
        let target = build_rtmp_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: cheetah_sdk::ProcessingPreset::Balanced,
            },
            TrackSelection::All,
        )
        .unwrap();
        assert!(target.video.is_some());
        assert!(target.audio.is_some());
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Aac);
    }
}
