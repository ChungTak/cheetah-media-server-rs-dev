//! Single-stream transcoding job worker.
//!
//! Subscribes to a source stream, transcodes selected video/audio tracks to the
//! requested targets, and publishes derived frames on a single target stream.
//!
//! Only compiled when `media-processing-cpu` is enabled so that both the video
//! and audio transcode sessions are available.

use std::sync::{Arc, Mutex};

use cheetah_codec::{
    track::{CodecId, MediaKind, TrackInfo},
    AVFrame,
};
use cheetah_media_api::{
    error::Result as MediaResult,
    processing::{AudioCodec, AudioTarget, ProcessingJob, TrackSelection, VideoCodec, VideoTarget},
    MediaError,
};
use cheetah_sdk::{
    CancellationToken, EngineContext, MediaFilter, PublishLease, PublisherSink, SdkError,
    StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::warn;

use crate::config::MediaProcessingModuleConfig;
use crate::provider::audio::{AudioTranscodeSession, AudioTranscodeSpec};
use crate::provider::video::{VideoTranscodeSession, VideoTranscodeSpec};

/// Convert a processing `VideoCodec` to the engine `CodecId`.
fn video_codec_to_codec_id(codec: VideoCodec) -> Option<CodecId> {
    match codec {
        VideoCodec::H264 => Some(CodecId::H264),
        VideoCodec::H265 => Some(CodecId::H265),
        VideoCodec::MJPEG => Some(CodecId::MJPEG),
    }
}

/// Convert a processing `AudioCodec` to the engine `CodecId`.
fn audio_codec_to_codec_id(codec: AudioCodec) -> Option<CodecId> {
    match codec {
        AudioCodec::G711A => Some(CodecId::G711A),
        AudioCodec::G711U => Some(CodecId::G711U),
        AudioCodec::Aac => Some(CodecId::AAC),
        AudioCodec::Opus => Some(CodecId::Opus),
        _ => None,
    }
}

fn video_target_to_spec(target: &VideoTarget) -> MediaResult<VideoTranscodeSpec> {
    let codec = video_codec_to_codec_id(target.codec).ok_or_else(|| {
        MediaError::unsupported(format!(
            "unsupported video target codec: {codec:?}",
            codec = target.codec
        ))
    })?;
    let mut spec = VideoTranscodeSpec::new(codec);
    if let (Some(w), Some(h)) = (target.width, target.height) {
        spec = spec.with_dimensions(w, h);
    }
    if let (Some(n), Some(d)) = (target.frame_rate_num, target.frame_rate_den) {
        spec = spec.with_frame_rate(n, d);
    }
    if let Some(b) = target.bit_rate {
        spec = spec.with_bitrate(b as u32);
    }
    Ok(spec)
}

fn audio_target_to_spec(target: &AudioTarget) -> MediaResult<AudioTranscodeSpec> {
    let codec = audio_codec_to_codec_id(target.codec).ok_or_else(|| {
        MediaError::unsupported(format!(
            "unsupported audio target codec: {codec:?}",
            codec = target.codec
        ))
    })?;
    let sample_rate = target.sample_rate.unwrap_or(48_000).clamp(8_000, 192_000);
    let channels = target.channels.unwrap_or(2).clamp(1, 2);
    let bitrate = target
        .bit_rate
        .map(|b| b as u32)
        .unwrap_or_else(|| default_audio_bitrate(codec));
    Ok(AudioTranscodeSpec {
        codec,
        sample_rate,
        channels,
        bitrate,
    })
}

fn default_audio_bitrate(codec: CodecId) -> u32 {
    match codec {
        CodecId::G711A | CodecId::G711U => 64_000,
        CodecId::Opus => 64_000,
        CodecId::AAC => 128_000,
        _ => 128_000,
    }
}

/// Input sent from the async feeder to the blocking transcode worker.
enum TranscodeInput {
    Video(Arc<AVFrame>),
    Audio(Arc<AVFrame>),
}

/// Synchronous worker that owns the transcode sessions and publisher sink.
struct TranscodeWorker {
    video_session: Option<VideoTranscodeSession>,
    audio_session: Option<AudioTranscodeSession>,
    publisher: Box<dyn PublisherSink>,
    job: Option<Arc<Mutex<ProcessingJob>>>,
}

impl TranscodeWorker {
    fn new(
        config: &MediaProcessingModuleConfig,
        source_video: Option<&TrackInfo>,
        source_audio: Option<&TrackInfo>,
        video_target: Option<&VideoTarget>,
        audio_target: Option<&AudioTarget>,
        publisher: Box<dyn PublisherSink>,
        job: Option<Arc<Mutex<ProcessingJob>>>,
    ) -> MediaResult<Self> {
        let mut tracks = Vec::new();
        let video_session = if let Some(target) = video_target {
            let source = source_video.ok_or_else(|| {
                MediaError::invalid_argument(
                    "transcode requested video but source has no video track",
                )
            })?;
            let spec = video_target_to_spec(target)?;
            let session = VideoTranscodeSession::new(source, &spec, config)?;
            tracks.push(session.output_track().clone());
            Some(session)
        } else {
            None
        };

        let audio_session = if let Some(target) = audio_target {
            let source = source_audio.ok_or_else(|| {
                MediaError::invalid_argument(
                    "transcode requested audio but source has no audio track",
                )
            })?;
            let spec = audio_target_to_spec(target)?;
            let session = AudioTranscodeSession::new(source, &spec, config)?;
            tracks.push(session.output_track().clone());
            Some(session)
        } else {
            None
        };

        publisher
            .update_tracks(tracks)
            .map_err(|e| MediaError::internal(format!("update publisher tracks failed: {e}")))?;

        Ok(Self {
            video_session,
            audio_session,
            publisher,
            job,
        })
    }

    fn process(&mut self, input: TranscodeInput) -> Result<(), SdkError> {
        match input {
            TranscodeInput::Video(frame) => self.process_video(&frame),
            TranscodeInput::Audio(frame) => self.process_audio(&frame),
        }
    }

    fn process_video(&mut self, frame: &AVFrame) -> Result<(), SdkError> {
        update_progress(&self.job, |job| {
            job.frames_in += 1;
            job.bytes_in += frame.payload.len() as u64;
        });

        // Flush the previous GOP on each keyframe so the encoder emits output and
        // the session is reset for the next group of pictures.
        if frame.flags.contains(cheetah_codec::frame::FrameFlags::KEY) {
            if let Some(session) = self.video_session.as_mut() {
                let flushed = session
                    .flush()
                    .map_err(|e| SdkError::Internal(format!("video GOP flush failed: {e}")))?;
                for f in flushed {
                    self.push_frame(f)?;
                }
            }
        }

        let session = self
            .video_session
            .as_mut()
            .ok_or_else(|| SdkError::InvalidArgument("no video target configured".into()))?;

        let output = session
            .submit(frame)
            .map_err(|e| SdkError::Internal(format!("video transcode failed: {e}")))?;

        for frame in output {
            self.push_frame(frame)?;
        }
        Ok(())
    }

    fn process_audio(&mut self, frame: &AVFrame) -> Result<(), SdkError> {
        update_progress(&self.job, |job| {
            job.frames_in += 1;
            job.bytes_in += frame.payload.len() as u64;
        });

        let session = self
            .audio_session
            .as_mut()
            .ok_or_else(|| SdkError::InvalidArgument("no audio target configured".into()))?;

        let output = session
            .submit(frame)
            .map_err(|e| SdkError::Internal(format!("audio transcode failed: {e}")))?;

        for frame in output {
            self.push_frame(frame)?;
        }
        Ok(())
    }

    fn push_frame(&mut self, mut frame: AVFrame) -> Result<(), SdkError> {
        frame.origin = cheetah_codec::frame::FrameOrigin::Generated;
        let bytes_out = frame.payload.len() as u64;
        match self.publisher.push_frame(Arc::new(frame)) {
            Ok(_) => {
                update_progress(&self.job, |job| {
                    job.frames_out += 1;
                    job.bytes_out += bytes_out;
                });
                Ok(())
            }
            Err(e) => {
                update_progress(&self.job, |job| job.drops += 1);
                Err(e)
            }
        }
    }

    fn flush_and_close(mut self) -> Result<(), SdkError> {
        if let Some(session) = self.video_session.as_mut() {
            for frame in session
                .flush()
                .map_err(|e| SdkError::Internal(format!("video flush failed: {e}")))?
            {
                let _ = self.push_frame(frame);
            }
        }
        if let Some(session) = self.audio_session.as_mut() {
            for frame in session
                .flush()
                .map_err(|e| SdkError::Internal(format!("audio flush failed: {e}")))?
            {
                let _ = self.push_frame(frame);
            }
        }
        self.publisher.close()
    }
}

fn update_progress<F>(job: &Option<Arc<Mutex<ProcessingJob>>>, f: F)
where
    F: FnOnce(&mut ProcessingJob),
{
    if let Some(job) = job.as_ref() {
        if let Ok(mut guard) = job.lock() {
            f(&mut guard);
        }
    }
}

/// Spawn a transcode job worker and return an async handle to its feeder loop.
///
/// The feeder loop reads frames from the subscriber, sends them to a blocking
/// transcode worker, and signals cancellation by dropping the channel.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_transcode_worker(
    engine: EngineContext,
    config: MediaProcessingModuleConfig,
    source: StreamKey,
    _target: StreamKey,
    track_selection: TrackSelection,
    video_target: Option<VideoTarget>,
    audio_target: Option<AudioTarget>,
    publisher_lease: PublishLease,
    publisher: Box<dyn PublisherSink>,
    cancel: CancellationToken,
    job: Option<Arc<Mutex<ProcessingJob>>>,
) -> Result<(), SdkError> {
    let (source_video, source_audio) = wait_for_source_tracks(
        &engine,
        &source,
        &track_selection,
        video_target.is_some(),
        audio_target.is_some(),
    )
    .await?;

    let worker = TranscodeWorker::new(
        &config,
        source_video.as_ref(),
        source_audio.as_ref(),
        video_target.as_ref(),
        audio_target.as_ref(),
        publisher,
        job.clone(),
    )
    .map_err(|e| SdkError::Internal(format!("create transcode worker: {e}")))?;

    let (tx, rx) = std::sync::mpsc::sync_channel::<TranscodeInput>(64);

    let handle = engine
        .runtime_api
        .spawn_blocking(
            "transcode-worker",
            Box::new(move || {
                let mut worker = worker;
                while let Ok(input) = rx.recv() {
                    if let Err(e) = worker.process(input) {
                        warn!("transcode worker dropped frame: {e}");
                    }
                }
                if let Err(e) = worker.flush_and_close() {
                    warn!("transcode worker flush/close failed: {e}");
                }
            }),
        )
        .map_err(|e| SdkError::Internal(format!("spawn transcode worker: {e}")))?;

    let media_filter = MediaFilter {
        enable_video: video_target.is_some(),
        enable_audio: audio_target.is_some(),
    };
    let subscriber_options = SubscriberOptions {
        queue_capacity: 256,
        backpressure: cheetah_sdk::BackpressurePolicy::DropDroppableFirst,
        bootstrap_policy: cheetah_sdk::BootstrapPolicy::default(),
        media_filter,
    };
    let mut subscriber = engine
        .subscriber_api
        .subscribe(source, subscriber_options)
        .await
        .map_err(|e| SdkError::Internal(format!("subscribe failed: {e}")))?;

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);

        let frame = select_biased! {
            _ = cancel_fut => break,
            frame = recv_fut => frame,
        };

        match frame {
            Ok(Some(frame)) => {
                let input = if frame.media_kind == MediaKind::Video {
                    TranscodeInput::Video(frame)
                } else {
                    TranscodeInput::Audio(frame)
                };
                if tx.try_send(input).is_err() {
                    warn!("transcode input queue full; dropping frame");
                    update_progress(&job, |job| job.drops += 1);
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("transcode subscriber error: {e}");
                break;
            }
        }
    }

    drop(tx);
    let _ = handle.wait().await;
    let _ = engine
        .publisher_api
        .release_publisher(&publisher_lease)
        .await;
    Ok(())
}

async fn wait_for_source_tracks(
    engine: &EngineContext,
    source: &StreamKey,
    track_selection: &TrackSelection,
    need_video: bool,
    need_audio: bool,
) -> Result<(Option<TrackInfo>, Option<TrackInfo>), SdkError> {
    let deadline = engine.runtime_api.now().as_micros() + 5_000_000;
    while engine.runtime_api.now().as_micros() < deadline {
        if let Ok(Some(snapshot)) = engine.stream_manager_api.get_stream(source).await {
            let mut video = None;
            let mut audio = None;
            for track in &snapshot.tracks {
                if track.readiness != cheetah_codec::track::TrackReadiness::Ready {
                    continue;
                }
                match track.media_kind {
                    MediaKind::Video
                        if video.is_none()
                            && (*track_selection == TrackSelection::All
                                || *track_selection == TrackSelection::VideoOnly
                                || need_video) =>
                    {
                        video = Some(track.clone());
                    }
                    MediaKind::Audio
                        if audio.is_none()
                            && (*track_selection == TrackSelection::All
                                || *track_selection == TrackSelection::AudioOnly
                                || need_audio) =>
                    {
                        audio = Some(track.clone());
                    }
                    _ => {}
                }
            }
            if (!need_video || video.is_some()) && (!need_audio || audio.is_some()) {
                return Ok((video, audio));
            }
        }
        let sleep_deadline =
            cheetah_codec::MonoTime::from_micros(engine.runtime_api.now().as_micros() + 200_000);
        let mut timer = engine.runtime_api.sleep_until(sleep_deadline);
        timer.wait().await;
    }

    Err(SdkError::NotFound(format!(
        "source stream {source} does not have required ready tracks"
    )))
}
