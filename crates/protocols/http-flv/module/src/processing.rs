//! HTTP-FLV derived play source helpers.
//!
//! Mirrors RTMP Auto play semantics: request a shared H.264/AAC transcode job when
//! the source is not already FLV-playable. Stable stream names and job sharing live
//! in the media-processing provider so RTMP and HTTP-FLV consumers reuse one worker.

use cheetah_codec::{CodecId, MediaKind, MonoTime, TrackInfo};
use cheetah_sdk::media_api::ids::{MediaKey, StreamKeyBridge};
use cheetah_sdk::media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::processing::{
    AudioCodec, AudioTarget, CreateProcessingJob, ProcessingJobSpec, ProcessingJobState,
    TrackSelection, VideoCodec, VideoTarget,
};
use cheetah_sdk::{CancellationToken, EngineContext, MediaProcessingApi, SdkError, StreamKey};

/// Derived play source for an HTTP-FLV session.
pub struct DerivedPlaySource {
    pub stream_key: StreamKey,
    pub processing_job_id: Option<cheetah_sdk::ProcessingJobId>,
}

/// Resolve H.264/AAC derived play stream using Auto policy.
pub async fn ensure_derived_play_source(
    engine: &EngineContext,
    source: StreamKey,
    cancel: &CancellationToken,
) -> Result<DerivedPlaySource, SdkError> {
    let snapshot = engine
        .stream_manager_api
        .get_stream(&source)
        .await
        .map_err(|e| SdkError::Internal(format!("failed to query source stream: {e}")))?;
    let tracks = snapshot.map(|s| s.tracks).unwrap_or_default();

    let Some(target) = build_auto_target(&tracks) else {
        return Ok(DerivedPlaySource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let Some(processing_api) = engine.media_services.processing() else {
        return Ok(DerivedPlaySource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let source_key = stream_key_to_media_key(&source)?;
    let derived_stream_name = stable_derived_name(&source, &target);
    let target_key = MediaKey::new(
        source_key.vhost.0.clone(),
        source_key.app.0.clone(),
        &derived_stream_name,
        None,
    )
    .map_err(|e| SdkError::InvalidArgument(format!("invalid derived media key: {e}")))?;
    let derived_stream_key = StreamKey::new(source.namespace.clone(), derived_stream_name);

    let ctx = MediaRequestContext {
        source_adapter: "http-flv".to_string(),
        ..MediaRequestContext::default()
    };
    let spec = ProcessingJobSpec::Transcode {
        source: source_key,
        target: target_key,
        track_selection: TrackSelection::All,
        audio: target.audio,
        video: target.video,
        overlays: Vec::new(),
    };
    let job = processing_api
        .create_job(
            &ctx,
            CreateProcessingJob {
                idempotency_key: Some(format!("http_flv_play_{source}")),
                deadline_ms: None,
                spec,
            },
        )
        .await
        .map_err(|e| SdkError::Internal(format!("create transcode job for http-flv play: {e}")))?;

    let derived_stream_key = job
        .output_keys
        .first()
        .map(|k| {
            let (namespace, path) = StreamKeyBridge::to_namespace_path(k);
            StreamKey::new(namespace, path)
        })
        .unwrap_or(derived_stream_key);

    if let Err(err) = wait_for_job_ready(
        processing_api.as_ref(),
        &ctx,
        &job.job_id,
        cancel,
        engine.runtime_api.clone(),
    )
    .await
    {
        let _ = processing_api.delete_job(&ctx, &job.job_id).await;
        return Err(err);
    }

    Ok(DerivedPlaySource {
        stream_key: derived_stream_key,
        processing_job_id: Some(job.job_id),
    })
}

pub async fn stop_derived_job(engine: &EngineContext, job_id: cheetah_sdk::ProcessingJobId) {
    if let Some(processing_api) = engine.media_services.processing() {
        let ctx = MediaRequestContext::default();
        let _ = processing_api.delete_job(&ctx, &job_id).await;
    }
}

fn build_auto_target(
    tracks: &[TrackInfo],
) -> Option<cheetah_sdk::media_api::processing::ProcessingTarget> {
    let source_video = tracks.iter().find(|t| t.media_kind == MediaKind::Video);
    let source_audio = tracks.iter().find(|t| t.media_kind == MediaKind::Audio);

    let needs_video = source_video.is_some_and(|t| t.codec != CodecId::H264);
    let needs_audio = source_audio.is_some_and(|t| t.codec != CodecId::AAC);
    if !needs_video && !needs_audio {
        return None;
    }

    let video = source_video.map(|t| VideoTarget {
        codec: VideoCodec::H264,
        width: t.width,
        height: t.height,
        frame_rate_num: t.fps.map(|r| r.num),
        frame_rate_den: t.fps.map(|r| r.den),
        bit_rate: None,
        gop_size: None,
        profile: None,
    });
    let audio = source_audio.map(|t| AudioTarget {
        codec: AudioCodec::Aac,
        sample_rate: t.sample_rate,
        channels: t.channels,
        bit_rate: None,
    });
    Some(cheetah_sdk::media_api::processing::ProcessingTarget { video, audio })
}

fn stable_derived_name(
    source: &StreamKey,
    target: &cheetah_sdk::media_api::processing::ProcessingTarget,
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
    // Keep the same prefix pattern as RTMP so shareable fingerprint + stream names align.
    format!(
        "{}_rtmp_{}_{}_all",
        sanitize_stream_name(&source.path),
        video,
        audio
    )
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

fn stream_key_to_media_key(stream_key: &StreamKey) -> Result<MediaKey, SdkError> {
    StreamKeyBridge::from_namespace_path(&stream_key.namespace, &stream_key.path)
        .map_err(|e| SdkError::InvalidArgument(format!("invalid stream key: {e}")))
}

async fn wait_for_job_ready(
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
                    "transcode job {job_id} stopped before ready"
                )));
            }
            Ok(job)
                if job.state == ProcessingJobState::Running
                    && (job.first_output_at.is_some() || job.frames_out > 0) =>
            {
                // Do not treat `started_at` (first input/drop) as media-ready.
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                return Err(SdkError::Internal(format!(
                    "failed to poll transcode job {job_id}: {e}"
                )));
            }
        }
        let sleep_deadline = MonoTime::from_micros(runtime_api.now().as_micros() + 200_000);
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
