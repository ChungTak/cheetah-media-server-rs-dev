//! WebRTC processing helpers: derive Opus/H.264 play streams.
//!
//! `ensure_derived_play_source` decides whether a WHEP/WHEP-style play session
//! needs a transcoded stream before subscribing, and creates a
//! `ProcessingJobSpec::Transcode` job when needed.

use cheetah_codec::{CodecId, MediaKind, MonoTime, TrackInfo};
use cheetah_sdk::media_api::ids::{MediaKey, StreamKeyBridge};
use cheetah_sdk::media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::processing::{
    AudioCodec, AudioTarget, CreateProcessingJob, ProcessingJobSpec, ProcessingJobState,
    ProcessingTarget, VideoCodec, VideoTarget,
};
use cheetah_sdk::{CancellationToken, EngineContext, MediaProcessingApi, SdkError, StreamKey};

use crate::codec_policy::AudioOutputStrategy;
use crate::config::CodecProfileWire;

/// Derived-stream job handle returned to the WebRTC play path.
pub struct DerivedPlaySource {
    /// The stream key to subscribe to (source or derived).
    pub stream_key: StreamKey,
    /// If a processing job was created, its id so the supervisor can stop it.
    pub processing_job_id: Option<cheetah_sdk::ProcessingJobId>,
}

/// Resolve the actual source stream key for a WebRTC play session.
///
/// - `AudioOutputStrategy::Passthrough` and `CodecProfileWire::Passthrough`
///   return the original `source` unless other tracks need transcoding.
/// - `AudioOutputStrategy::Auto` creates a transcode job only when the source
///   tracks are not already compatible; if processing is unavailable it falls
///   back to the source stream and the play loop drops incompatible tracks.
/// - `AudioOutputStrategy::TranscodeToOpus` always requests Opus audio output
///   and fails if transcoding is required but unavailable.
///
/// The returned `DerivedPlaySource` carries the optional job id so the caller
/// can stop the job when the play session stops.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_derived_play_source(
    engine: &EngineContext,
    job_name: &str,
    source: StreamKey,
    audio_strategy: AudioOutputStrategy,
    codec_profile: CodecProfileWire,
    prefer_video_codec: &str,
    prefer_audio_codec: &str,
    cancel: &CancellationToken,
) -> Result<DerivedPlaySource, SdkError> {
    let snapshot = engine
        .stream_manager_api
        .get_stream(&source)
        .await
        .map_err(|e| SdkError::Internal(format!("failed to query source stream: {e}")))?;
    let tracks = snapshot.map(|s| s.tracks).unwrap_or_default();

    let target = match build_webrtc_transcode_target(
        &tracks,
        audio_strategy,
        codec_profile,
        prefer_video_codec,
        prefer_audio_codec,
    ) {
        Ok(t) => t,
        Err(err) => return Err(err),
    };

    let Some(target) = target else {
        return Ok(DerivedPlaySource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let Some(processing_api) = engine.media_services.processing() else {
        if matches!(audio_strategy, AudioOutputStrategy::TranscodeToOpus) {
            return Err(SdkError::Unavailable(
                "media processing provider is not registered but transcoding to Opus is required"
                    .into(),
            ));
        }
        // Auto: fall back to the source stream and let the play loop drop
        // incompatible tracks.
        return Ok(DerivedPlaySource {
            stream_key: source,
            processing_job_id: None,
        });
    };

    let source_key = stream_key_to_media_key(&source)?;
    let derived_stream_name = format!(
        "{}_webrtc_{}",
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

    if let Err(err) = wait_for_publisher_release(engine, &derived_stream_key, cancel).await {
        return Err(SdkError::Internal(format!(
            "transcode target still has a publisher: {err}"
        )));
    }

    let ctx = MediaRequestContext::default();
    let spec = ProcessingJobSpec::Transcode {
        source: source_key,
        target: target_key,
        track_selection: cheetah_sdk::media_api::processing::TrackSelection::All,
        audio: target.audio,
        video: target.video,
        overlays: Vec::new(),
    };
    let idempotency_key = format!("webrtc_play_{source}_{job_name}");
    let job = processing_api
        .create_job(
            &ctx,
            CreateProcessingJob {
                idempotency_key: Some(idempotency_key),
                deadline_ms: None,
                spec,
            },
        )
        .await
        .map_err(|e| SdkError::Internal(format!("create transcode job for webrtc play: {e}")))?;

    if let Err(err) = wait_for_running_job(
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

/// Delete a derived processing job when the play session exits.
pub async fn stop_derived_play_job(engine: &EngineContext, job_id: cheetah_sdk::ProcessingJobId) {
    let Some(processing_api) = engine.media_services.processing() else {
        return;
    };
    let ctx = MediaRequestContext::default();
    let _ = processing_api.delete_job(&ctx, &job_id).await;
}

/// Wait until the target stream has no active publisher or no longer exists.
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
/// or disappeared from the provider.
/// Build the transcode target for WebRTC play output.
///
/// Returns `None` when no transcode is required. For `AudioOutputStrategy::Auto`
/// with unsupported source codecs, `None` is returned so the caller falls back
/// to passthrough and the play loop drops incompatible frames. For explicit
/// `TranscodeToOpus`, unsupported combinations return an error.
fn build_webrtc_transcode_target(
    tracks: &[TrackInfo],
    audio_strategy: AudioOutputStrategy,
    codec_profile: CodecProfileWire,
    prefer_video_codec: &str,
    prefer_audio_codec: &str,
) -> Result<Option<ProcessingTarget>, SdkError> {
    let source_video = tracks.iter().find(|t| t.media_kind == MediaKind::Video);
    let source_audio = tracks.iter().find(|t| t.media_kind == MediaKind::Audio);

    let mut audio_target = resolve_audio_target(
        source_audio,
        audio_strategy,
        codec_profile,
        prefer_audio_codec,
    )?;
    let mut video_target = resolve_video_target(source_video, codec_profile, prefer_video_codec)?;

    let needs_audio_transcode = audio_target.is_some();
    let needs_video_transcode = video_target.is_some();

    if !needs_audio_transcode && !needs_video_transcode {
        return Ok(None);
    }

    // If any track is being transcoded, make sure the other selected track is
    // also emitted by the transcode job. This prevents the worker from dropping
    // the already-conformant track because its target is `None`.
    //
    // If the conformant track cannot be copied by the processing backend and
    // the strategy is Auto, fall back to source playback so the play loop can
    // drop only the incompatible track instead of failing the whole session.
    let can_fall_back = audio_strategy == AudioOutputStrategy::Auto
        && codec_profile != CodecProfileWire::Passthrough;

    if needs_video_transcode && audio_target.is_none() {
        if let Some(source_audio) = source_audio {
            match audio_passthrough_target(source_audio) {
                Ok(t) => audio_target = t,
                Err(_) if can_fall_back => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }
    if needs_audio_transcode && video_target.is_none() {
        if let Some(source_video) = source_video {
            match video_passthrough_target(source_video) {
                Ok(t) => video_target = t,
                Err(_) if can_fall_back => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    // If after filling passthrough targets we lost both, no transcode is needed.
    if audio_target.is_none() && video_target.is_none() {
        return Ok(None);
    }

    Ok(Some(ProcessingTarget {
        video: video_target,
        audio: audio_target,
    }))
}

fn resolve_audio_target(
    source_audio: Option<&TrackInfo>,
    audio_strategy: AudioOutputStrategy,
    codec_profile: CodecProfileWire,
    prefer_audio_codec: &str,
) -> Result<Option<AudioTarget>, SdkError> {
    let Some(source) = source_audio else {
        return Ok(None);
    };

    let client_supports_g711 = {
        let pref = prefer_audio_codec.to_ascii_lowercase();
        pref == "any" || pref.contains("g711") || pref.contains("pcma") || pref.contains("pcmu")
    };

    match audio_strategy {
        AudioOutputStrategy::Passthrough => Ok(None),
        AudioOutputStrategy::TranscodeToOpus => {
            if source.codec == CodecId::Opus {
                Ok(None)
            } else {
                Ok(Some(AudioTarget {
                    codec: AudioCodec::Opus,
                    sample_rate: Some(48_000),
                    channels: Some(2),
                    bit_rate: None,
                }))
            }
        }
        AudioOutputStrategy::Auto => match source.codec {
            CodecId::Opus => Ok(None),
            CodecId::G711A | CodecId::G711U if client_supports_g711 => Ok(None),
            CodecId::G711A | CodecId::G711U | CodecId::AAC | CodecId::MP3 => {
                Ok(Some(AudioTarget {
                    codec: AudioCodec::Opus,
                    sample_rate: Some(48_000),
                    channels: Some(2),
                    bit_rate: None,
                }))
            }
            other => {
                if codec_profile == CodecProfileWire::Passthrough {
                    Err(SdkError::Unavailable(format!(
                        "audio codec {other:?} cannot be played with passthrough profile and no transcoding support"
                    )))
                } else {
                    // Auto: fallback to source; the play loop will drop unknown audio.
                    Ok(None)
                }
            }
        },
    }
}

fn resolve_video_target(
    source_video: Option<&TrackInfo>,
    codec_profile: CodecProfileWire,
    prefer_video_codec: &str,
) -> Result<Option<VideoTarget>, SdkError> {
    let Some(source) = source_video else {
        return Ok(None);
    };

    if codec_profile == CodecProfileWire::Passthrough
        || is_video_codec_playable(source.codec, codec_profile)
    {
        return Ok(None);
    }

    let target_codec = determine_video_target_codec(codec_profile, prefer_video_codec)?;

    if Some(target_codec) == codec_id_to_video_codec(source.codec) {
        return Ok(None);
    }

    if !is_video_codec_decodable(source.codec) {
        return Err(SdkError::Unavailable(format!(
            "video codec {:?} is not supported by the processing backend and cannot be transcoded to {:?}",
            source.codec, target_codec
        )));
    }

    Ok(Some(VideoTarget {
        codec: target_codec,
        width: source.width,
        height: source.height,
        frame_rate_num: source.fps.map(|r| r.num),
        frame_rate_den: source.fps.map(|r| r.den),
        bit_rate: None,
        gop_size: None,
        profile: None,
    }))
}

fn is_video_codec_playable(codec: CodecId, profile: CodecProfileWire) -> bool {
    match profile {
        CodecProfileWire::Browser => matches!(
            codec,
            CodecId::H264 | CodecId::VP8 | CodecId::VP9 | CodecId::AV1
        ),
        CodecProfileWire::Device => matches!(
            codec,
            CodecId::H264 | CodecId::H265 | CodecId::VP8 | CodecId::VP9 | CodecId::AV1
        ),
        CodecProfileWire::Passthrough => true,
    }
}

fn determine_video_target_codec(
    codec_profile: CodecProfileWire,
    prefer_video_codec: &str,
) -> Result<VideoCodec, SdkError> {
    use crate::codec_policy::WebRtcVideoCodecPreference;

    let pref = WebRtcVideoCodecPreference::from_str_lossy(prefer_video_codec);
    if !pref.is_allowed(codec_profile) {
        return Err(SdkError::InvalidArgument(format!(
            "preferVideoCodec={prefer_video_codec} is not allowed under {codec_profile:?} profile"
        )));
    }

    match codec_profile {
        CodecProfileWire::Browser => match pref {
            WebRtcVideoCodecPreference::H264 | WebRtcVideoCodecPreference::Any => {
                Ok(VideoCodec::H264)
            }
            WebRtcVideoCodecPreference::H265 => Err(SdkError::InvalidArgument(
                "H265 is not allowed under Browser profile".into(),
            )),
            _ => Ok(VideoCodec::H264),
        },
        CodecProfileWire::Device | CodecProfileWire::Passthrough => match pref {
            WebRtcVideoCodecPreference::H265 => Ok(VideoCodec::H265),
            _ => Ok(VideoCodec::H264),
        },
    }
}

fn is_video_codec_decodable(codec: CodecId) -> bool {
    matches!(codec, CodecId::H264 | CodecId::H265 | CodecId::MJPEG)
}

fn codec_id_to_video_codec(codec: CodecId) -> Option<VideoCodec> {
    match codec {
        CodecId::H264 => Some(VideoCodec::H264),
        CodecId::H265 => Some(VideoCodec::H265),
        CodecId::MJPEG => Some(VideoCodec::MJPEG),
        _ => None,
    }
}

fn audio_passthrough_target(source: &TrackInfo) -> Result<Option<AudioTarget>, SdkError> {
    let codec = match source.codec {
        CodecId::G711A => AudioCodec::G711A,
        CodecId::G711U => AudioCodec::G711U,
        CodecId::AAC => AudioCodec::Aac,
        CodecId::Opus => AudioCodec::Opus,
        CodecId::MP3 => AudioCodec::Mp3,
        other => {
            return Err(SdkError::Unavailable(format!(
                "audio codec {other:?} cannot be preserved by the processing backend"
            )))
        }
    };
    Ok(Some(AudioTarget {
        codec,
        sample_rate: source.sample_rate,
        channels: source.channels,
        bit_rate: None,
    }))
}

fn video_passthrough_target(source: &TrackInfo) -> Result<Option<VideoTarget>, SdkError> {
    let codec = match source.codec {
        CodecId::H264 => VideoCodec::H264,
        CodecId::H265 => VideoCodec::H265,
        CodecId::MJPEG => VideoCodec::MJPEG,
        other => {
            return Err(SdkError::Unavailable(format!(
                "video codec {other:?} cannot be preserved by the processing backend"
            )))
        }
    };
    Ok(Some(VideoTarget {
        codec,
        width: source.width,
        height: source.height,
        frame_rate_num: source.fps.map(|r| r.num),
        frame_rate_den: source.fps.map(|r| r.den),
        bit_rate: None,
        gop_size: None,
        profile: None,
    }))
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
        let sleep_deadline = MonoTime::from_micros(runtime_api.now().as_micros() + 200_000);
        runtime_api.sleep_until(sleep_deadline).wait().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{MediaKind, TrackId};

    fn video_track(codec: CodecId) -> TrackInfo {
        TrackInfo::new(TrackId(0), MediaKind::Video, codec, 90_000)
    }

    fn audio_track(codec: CodecId) -> TrackInfo {
        TrackInfo::new(TrackId(1), MediaKind::Audio, codec, 48_000)
    }

    #[test]
    fn passthrough_never_transcodes() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::Opus)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Passthrough,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap();
        assert!(target.is_none());
    }

    #[test]
    fn auto_skips_transcode_for_h264_opus() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::Opus)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Auto,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap();
        assert!(target.is_none());
    }

    #[test]
    fn auto_requests_h264_opus_for_unsupported_codecs() {
        let tracks = vec![video_track(CodecId::H265), audio_track(CodecId::AAC)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Auto,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap()
        .unwrap();
        assert!(target.video.is_some());
        assert!(target.audio.is_some());
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Opus);
    }

    #[test]
    fn transcode_to_opus_keeps_h264_video() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::AAC)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::TranscodeToOpus,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap()
        .unwrap();
        assert!(target.video.is_some());
        assert!(target.audio.is_some());
        assert_eq!(target.video.as_ref().unwrap().codec, VideoCodec::H264);
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Opus);
    }

    #[test]
    fn auto_g711_passthrough_when_client_supports() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::G711A)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Auto,
            CodecProfileWire::Browser,
            "h264",
            "g711a",
        )
        .unwrap();
        assert!(target.is_none());
    }

    #[test]
    fn auto_g711_to_opus_when_client_prefers_opus() {
        let tracks = vec![video_track(CodecId::H264), audio_track(CodecId::G711A)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Auto,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap()
        .unwrap();
        assert!(target.audio.is_some());
        assert_eq!(target.audio.as_ref().unwrap().codec, AudioCodec::Opus);
    }

    #[test]
    fn auto_falls_back_when_conformant_track_cannot_be_passed_through() {
        // Browser-playable VP8 video + AAC audio cannot be combined in one
        // transcode job because the processing backend cannot copy VP8.
        // Auto should fall back to source playback rather than fail.
        let tracks = vec![video_track(CodecId::VP8), audio_track(CodecId::AAC)];
        let target = build_webrtc_transcode_target(
            &tracks,
            AudioOutputStrategy::Auto,
            CodecProfileWire::Browser,
            "h264",
            "opus",
        )
        .unwrap();
        assert!(target.is_none());
    }
}
