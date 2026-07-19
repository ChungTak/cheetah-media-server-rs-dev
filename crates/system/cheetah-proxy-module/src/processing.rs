//! Derived transcode stream support for pull proxies.
//!
//! When a pull proxy is requested with a non-passthrough processing policy,
//! the source is first published to a temporary internal stream key. Once
//! the source tracks are discovered, a `ProcessingJobSpec::Transcode` job is
//! started from the temporary ingress to the final destination. The pull
//! handle is held while the job runs; cleanup stops the pull first, drains
//! the processing job, and then releases the temporary ingress publisher.
//!
//! 拉流代理的派生转码流支持。当代理请求了非透传的处理策略时，源流先被发布
//! 到一个临时内部流键；发现源轨道后，从临时入口到最终目标启动转码任务。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_codec::MonoTime;
use cheetah_codec::{MediaKind, TrackInfo};
use cheetah_media_api::ids::{MediaKey, ProcessingJobId, ProxyId, StreamName};
use cheetah_media_api::port::{MediaProcessingApi, MediaRequestContext};
use cheetah_media_api::processing::{
    AudioCodec, AudioTarget, CreateProcessingJob, ProcessingJobSpec, ProcessingJobState,
    ProcessingPolicy, ProcessingPreset, ProcessingTarget, TrackSelection, VideoCodec, VideoTarget,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use tracing::warn;

/// Returns `true` if the policy requires a derived transcode stream.
///
/// 处理策略是否需要派生转码流。
pub(crate) fn requires_processing(policy: &ProcessingPolicy) -> bool {
    !matches!(policy, ProcessingPolicy::Passthrough)
}

/// Build a normalized transcode target for a proxy from the discovered source
/// tracks and the requested processing policy.
///
/// 根据已发现的源轨道和处理策略构建代理的规范化转码目标。
pub(crate) fn build_proxy_transcode_target(
    tracks: &[TrackInfo],
    policy: &ProcessingPolicy,
) -> Option<ProcessingTarget> {
    match policy {
        ProcessingPolicy::Passthrough => None,
        ProcessingPolicy::Transcode { target } => Some(target.clone()),
        ProcessingPolicy::Auto { preset } => {
            let mut target = ProcessingTarget::default();
            for track in tracks {
                match track.media_kind {
                    MediaKind::Video => {
                        let codec = match preset {
                            ProcessingPreset::Quality => VideoCodec::H265,
                            _ => VideoCodec::H264,
                        };
                        target.video = Some(VideoTarget {
                            codec,
                            width: track.width,
                            height: track.height,
                            frame_rate_num: track.fps.map(|r| r.num),
                            frame_rate_den: track.fps.map(|r| r.den),
                            bit_rate: track.bitrate.map(|b| b as u64),
                            gop_size: None,
                            profile: None,
                        });
                    }
                    MediaKind::Audio => {
                        target.audio = Some(AudioTarget {
                            codec: AudioCodec::Aac,
                            sample_rate: track.sample_rate,
                            channels: track.channels,
                            bit_rate: track.bitrate.map(|b| b as u64),
                        });
                    }
                    _ => {}
                }
            }
            if target.video.is_none() && target.audio.is_none() {
                None
            } else {
                Some(target)
            }
        }
    }
}

/// Allocate a temporary ingress `MediaKey` for a pull source.
///
/// The temporary key reuses the destination vhost/app but uses a unique
/// stream name so it does not collide with the final output publisher.
///
/// 为拉流源分配一个临时入口 `MediaKey`，复用目标 vhost/app，但使用唯一流名。
pub(crate) fn temporary_ingress_key(base: &MediaKey, proxy_id: &ProxyId) -> MediaKey {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    MediaKey {
        vhost: base.vhost.clone(),
        app: base.app.clone(),
        stream: StreamName(format!("{}-proxy-{proxy_id}-{now}", base.stream.0)),
        schema: base.schema,
    }
}

/// Start a `Transcode` processing job from `source` to `destination`.
///
/// 从 `source` 到 `destination` 启动 `Transcode` 处理任务。
pub(crate) async fn start_derived_stream(
    ctx: &cheetah_sdk::EngineContext,
    proxy_id: &ProxyId,
    source: &MediaKey,
    destination: &MediaKey,
    target: &ProcessingTarget,
    cancel: &CancellationToken,
) -> Result<ProcessingJobId, String> {
    let processing_api = ctx
        .media_services
        .processing()
        .ok_or_else(|| "media processing provider not available".to_string())?;

    let spec = ProcessingJobSpec::Transcode {
        source: source.clone(),
        target: destination.clone(),
        track_selection: TrackSelection::All,
        audio: target.audio.clone(),
        video: target.video.clone(),
        overlays: Vec::new(),
    };
    let request = CreateProcessingJob {
        idempotency_key: Some(format!("proxy-{proxy_id}")),
        deadline_ms: None,
        spec,
    };
    let media_ctx = MediaRequestContext {
        source_adapter: "proxy".to_string(),
        ..MediaRequestContext::default()
    };
    let job = processing_api
        .create_job(&media_ctx, request)
        .await
        .map_err(|e| format!("create processing job: {e}"))?;

    if let Err(e) = wait_for_job_ready(
        processing_api.as_ref(),
        &media_ctx,
        &job.job_id,
        10_000,
        &ctx.runtime_api,
        cancel,
    )
    .await
    {
        let _ = processing_api.stop_job(&media_ctx, &job.job_id).await;
        let _ = processing_api.delete_job(&media_ctx, &job.job_id).await;
        return Err(e);
    }

    Ok(job.job_id)
}

/// Stop a derived processing job.
///
/// 停止派生处理任务。
pub(crate) async fn stop_derived_stream(
    ctx: &cheetah_sdk::EngineContext,
    job_id: &ProcessingJobId,
) -> Result<(), String> {
    let processing_api = match ctx.media_services.processing() {
        Some(p) => p,
        None => return Ok(()),
    };
    let media_ctx = MediaRequestContext {
        source_adapter: "proxy".to_string(),
        ..MediaRequestContext::default()
    };
    let _ = processing_api.stop_job(&media_ctx, job_id).await;
    let _ = processing_api.delete_job(&media_ctx, job_id).await;
    Ok(())
}

/// Wait until the processing job has started producing media (or announced activity).
///
/// `Running` alone is not sufficient: workers mark Running before the first
/// output frame, so consumers must wait for `started_at` / first output.
async fn wait_for_job_ready(
    processing_api: &dyn MediaProcessingApi,
    ctx: &MediaRequestContext,
    job_id: &ProcessingJobId,
    timeout_ms: u64,
    runtime_api: &Arc<dyn RuntimeApi>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let start = runtime_api.now().as_micros();
    let timeout_us = timeout_ms * 1_000;
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled while waiting for processing job".into());
        }
        match processing_api.get_job(ctx, job_id).await {
            Ok(job) => {
                if matches!(
                    job.state,
                    ProcessingJobState::Failed | ProcessingJobState::Stopped
                ) {
                    return Err(format!(
                        "processing job ended in state {:?} error={:?}",
                        job.state, job.last_error
                    ));
                }
                if job.state == ProcessingJobState::Running
                    && (job.first_output_at.is_some() || job.frames_out > 0)
                {
                    // `started_at` alone means input activity, not encoded output.
                    return Ok(());
                }
            }
            Err(e) => {
                warn!(job_id = %job_id.0, "failed to query processing job: {e}");
            }
        }
        if runtime_api.now().as_micros().saturating_sub(start) >= timeout_us {
            return Err("timed out waiting for processing job first output".into());
        }
        let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + 100_000);
        let mut timer = runtime_api.sleep_until(deadline);
        let _ = timer.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{CodecId, MediaKind, Rational32, TrackId, TrackInfo};
    use cheetah_media_api::ids::{AppName, MediaKey, ProxyId, StreamName, VhostName};

    fn sample_video_track() -> TrackInfo {
        TrackInfo {
            width: Some(1920),
            height: Some(1080),
            fps: Some(Rational32::new(30, 1)),
            bitrate: Some(2_000_000),
            ..TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000)
        }
    }

    fn sample_audio_track() -> TrackInfo {
        TrackInfo {
            sample_rate: Some(48_000),
            channels: Some(2),
            ..TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::Opus, 48_000)
        }
    }

    #[test]
    fn requires_processing_true_for_auto_and_transcode() {
        assert!(!requires_processing(&ProcessingPolicy::Passthrough));
        assert!(requires_processing(&ProcessingPolicy::Auto {
            preset: ProcessingPreset::Balanced
        }));
        assert!(requires_processing(&ProcessingPolicy::Transcode {
            target: ProcessingTarget::default()
        }));
    }

    #[test]
    fn auto_balanced_prefers_h264_aac_from_hevc_opus() {
        let tracks = vec![sample_video_track(), sample_audio_track()];
        let target = build_proxy_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: ProcessingPreset::Balanced,
            },
        )
        .expect("auto target should be built");
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Aac);
        assert_eq!(target.video.as_ref().unwrap().width, Some(1920));
        assert_eq!(target.video.as_ref().unwrap().frame_rate_num, Some(30));
        assert_eq!(target.audio.as_ref().unwrap().sample_rate, Some(48_000));
    }

    #[test]
    fn auto_quality_prefers_h265() {
        let tracks = vec![sample_video_track()];
        let target = build_proxy_transcode_target(
            &tracks,
            &ProcessingPolicy::Auto {
                preset: ProcessingPreset::Quality,
            },
        )
        .expect("auto target should be built");
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H265);
        assert!(target.audio.is_none());
    }

    #[test]
    fn transcode_policy_returns_provided_target() {
        let provided = ProcessingTarget {
            video: Some(VideoTarget {
                codec: VideoCodec::MJPEG,
                width: Some(640),
                height: Some(480),
                frame_rate_num: None,
                frame_rate_den: None,
                bit_rate: None,
                gop_size: None,
                profile: None,
            }),
            audio: None,
        };
        let target = build_proxy_transcode_target(
            &[],
            &ProcessingPolicy::Transcode {
                target: provided.clone(),
            },
        )
        .expect("explicit target returned");
        assert_eq!(target, provided);
    }

    #[test]
    fn temporary_ingress_key_reuses_vhost_app_and_uniquifies_stream() {
        let base = MediaKey {
            vhost: VhostName("__defaultVhost__".to_string()),
            app: AppName("live".to_string()),
            stream: StreamName("cam1".to_string()),
            schema: None,
        };
        let proxy_id = ProxyId("p-123".to_string());
        let key = temporary_ingress_key(&base, &proxy_id);
        assert_eq!(key.vhost, base.vhost);
        assert_eq!(key.app, base.app);
        assert!(key.stream.0.starts_with("cam1-proxy-p-123-"));
        assert_ne!(key.stream.0, base.stream.0);
    }
}
