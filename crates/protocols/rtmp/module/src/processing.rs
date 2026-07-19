//! RTMP/FLV processing helpers: request AAC/H.264 derived streams.
//!
//! `ensure_derived_push_source` decides whether a push job needs a transcoded
//! stream before connecting to the remote RTMP server, and creates a
//! `ProcessingJobSpec::Transcode` job when needed.

use cheetah_codec::{CodecId, MediaKind, MonoTime, TrackId, TrackInfo};
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

    let Some(processing_api) = engine.media_services.processing() else {
        // Auto: degrade to passthrough when processing is unavailable.
        // Explicit Transcode must fail closed.
        if matches!(policy, ProcessingPolicy::Transcode { .. }) {
            return Err(SdkError::Unavailable(
                "media processing provider is not registered but Transcode policy requires it"
                    .into(),
            ));
        }
        return Ok(DerivedPushSource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let source_key = stream_key_to_media_key(&source)?;
    // Stable name so concurrent push/play consumers share one derived job.
    let derived_stream_name = stable_rtmp_derived_name(&source, &target, track_selection);
    let target_key = MediaKey::new(
        source_key.vhost.0.clone(),
        source_key.app.0.clone(),
        &derived_stream_name,
        None,
    )
    .map_err(|e| SdkError::InvalidArgument(format!("invalid derived media key: {e}")))?;
    let derived_stream_key = StreamKey::new(source.namespace.clone(), derived_stream_name);

    // Give any previous transcode job on this derived key time to release its
    // publisher lease before we try to acquire a new one.
    if let Err(err) = wait_for_publisher_release(engine, &derived_stream_key, cancel).await {
        return Err(SdkError::Internal(format!(
            "transcode target still has a publisher: {err}"
        )));
    }

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
                // Stable idempotency key for Auto/Transcode so retries attach.
                idempotency_key: Some(format!(
                    "rtmp_derived_{}_{}",
                    source,
                    sanitize_stream_name(job_name)
                )),
                deadline_ms: None,
                spec,
            },
        )
        .await
        .map_err(|e| SdkError::Internal(format!("create transcode job for rtmp push: {e}")))?;

    // Prefer the job's registered output key (shared attach may return an older target).
    let derived_stream_key = job
        .output_keys
        .first()
        .map(|k| {
            let (namespace, path) = StreamKeyBridge::to_namespace_path(k);
            StreamKey::new(namespace, path)
        })
        .unwrap_or(derived_stream_key);

    // Wait until the job has produced its first output (tracks ready), not just Running.
    if let Err(err) = wait_for_job_first_output(
        processing_api.as_ref(),
        &ctx,
        &job.job_id,
        cancel,
        engine.runtime_api.clone(),
    )
    .await
    {
        // The job is never handed to the caller, so delete it here to avoid leaks.
        let _ = processing_api.delete_job(&ctx, &job.job_id).await;
        return Err(err);
    }

    Ok(DerivedPushSource {
        stream_key: derived_stream_key,
        processing_job_id: Some(job.job_id),
    })
}

/// Resolve a derived H.264/AAC play source for RTMP/HTTP-FLV consumers.
///
/// Uses `ProcessingPolicy::Auto` semantics: create a shared transcode job only when
/// the source has tracks that are not already RTMP-playable as H.264/AAC.
pub async fn ensure_derived_play_source(
    engine: &EngineContext,
    source: StreamKey,
    cancel: &CancellationToken,
) -> Result<DerivedPushSource, SdkError> {
    ensure_derived_push_source(
        engine,
        "play",
        source,
        &ProcessingPolicy::Auto {
            preset: cheetah_sdk::media_api::processing::ProcessingPreset::Conservative,
        },
        TrackSelection::All,
        cancel,
    )
    .await
}

/// Delete a derived processing job when the push job exits.
///
/// `delete_job` removes the entry from the provider's registry, unlike
/// `stop_job`, which leaves stopped jobs in memory.
pub async fn stop_derived_push_job(engine: &EngineContext, job_id: cheetah_sdk::ProcessingJobId) {
    if let Some(processing_api) = engine.media_services.processing() {
        let ctx = MediaRequestContext::default();
        let _ = processing_api.delete_job(&ctx, &job_id).await;
    }
}

/// Wait until the target stream has no active publisher or no longer exists.
/// This avoids a publisher-lease Conflict when recreating a transcode job on the
/// same derived key after the previous job was stopped.
pub async fn wait_for_publisher_release(
    engine: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
) -> Result<(), SdkError> {
    let timeout_us = engine.runtime_api.now().as_micros() + 5_000_000;
    while engine.runtime_api.now().as_micros() < timeout_us && !cancel.is_cancelled() {
        match engine.stream_manager_api.get_stream(stream_key).await {
            Ok(None) => return Ok(()),
            Ok(Some(snapshot)) if !snapshot.publisher_active => return Ok(()),
            Ok(Some(_)) => {}
            Err(_) => return Ok(()),
        }
        let sleep_deadline = MonoTime::from_micros(engine.runtime_api.now().as_micros() + 100_000);
        engine.runtime_api.sleep_until(sleep_deadline).wait().await;
    }
    if cancel.is_cancelled() {
        Err(SdkError::Internal("cancelled".into()))
    } else {
        Err(SdkError::Internal(
            "timeout waiting for derived stream publisher release".into(),
        ))
    }
}

/// Returns `false` when a derived processing job has reached a terminal state
/// or disappeared from the provider, indicating the derived stream is dead.
pub async fn is_derived_job_alive(
    engine: &EngineContext,
    job_id: &cheetah_sdk::ProcessingJobId,
) -> bool {
    let Some(processing_api) = engine.media_services.processing() else {
        return false;
    };
    let ctx = MediaRequestContext::default();
    match processing_api.get_job(&ctx, job_id).await {
        Ok(job) => !matches!(
            job.state,
            ProcessingJobState::Stopped | ProcessingJobState::Failed
        ),
        Err(_) => false,
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

/// Stable signature of the codec kinds present on a stream, used to detect
/// source track changes for `ProcessingPolicy::Auto` without re-resolving
/// on every reconnect.
pub(crate) fn tracks_codec_signature(tracks: &[TrackInfo]) -> Vec<(TrackId, MediaKind, CodecId)> {
    let mut signature: Vec<_> = tracks
        .iter()
        .map(|t| (t.track_id, t.media_kind, t.codec))
        .collect();
    signature.sort_by(|a, b| a.0.cmp(&b.0));
    signature
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

/// Stable derived stream name for RTMP/HTTP-FLV so concurrent consumers share one job.
fn stable_rtmp_derived_name(
    source: &StreamKey,
    target: &ProcessingTarget,
    track_selection: TrackSelection,
) -> String {
    let video = target
        .video
        .as_ref()
        .map(|v| format!("{:?}", v.codec).to_ascii_lowercase())
        .unwrap_or_else(|| "na".into());
    let audio = target
        .audio
        .as_ref()
        .map(|a| format!("{:?}", a.codec).to_ascii_lowercase())
        .unwrap_or_else(|| "na".into());
    let sel = format!("{track_selection:?}").to_ascii_lowercase();
    format!(
        "{}_rtmp_{}_{}_{}",
        sanitize_stream_name(&source.path),
        video,
        audio,
        sel
    )
}

/// Poll the processing job until it has produced first output, or fail on timeout.
async fn wait_for_job_first_output(
    processing_api: &dyn MediaProcessingApi,
    ctx: &MediaRequestContext,
    job_id: &cheetah_sdk::ProcessingJobId,
    cancel: &CancellationToken,
    runtime_api: std::sync::Arc<dyn cheetah_sdk::RuntimeApi>,
) -> Result<(), SdkError> {
    let deadline = runtime_api.now().as_micros() + 10_000_000;
    while runtime_api.now().as_micros() < deadline && !cancel.is_cancelled() {
        match processing_api.get_job(ctx, job_id).await {
            Ok(job) if job.state == ProcessingJobState::Failed => {
                return Err(SdkError::Internal(format!(
                    "transcode job {job_id} failed: {}",
                    job.last_error.unwrap_or_default()
                )));
            }
            Ok(job) if job.state == ProcessingJobState::Stopped => {
                return Err(SdkError::Internal(format!(
                    "transcode job {job_id} stopped before first output"
                )));
            }
            Ok(job)
                if job.state == ProcessingJobState::Running
                    && (job.first_output_at.is_some() || job.frames_out > 0) =>
            {
                // Only first encoded output means consumers can attach safely.
                // `started_at` fires on first input/drop and is not media-ready.
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                return Err(SdkError::Internal(format!(
                    "failed to poll transcode job {job_id}: {e}"
                )));
            }
        }
        let sleep_deadline =
            cheetah_codec::MonoTime::from_micros(runtime_api.now().as_micros() + 200_000);
        runtime_api.sleep_until(sleep_deadline).wait().await;
    }
    if cancel.is_cancelled() {
        return Err(SdkError::Internal(
            "cancelled waiting for transcode job".into(),
        ));
    }
    Err(SdkError::Internal(format!(
        "timeout waiting for first output from transcode job {job_id}"
    )))
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
