//! Single-stream transcoding job worker.
//!
//! Subscribes to a source stream, transcodes selected video/audio tracks to the
//! requested targets, and publishes derived frames on a single target stream.
//!
//! Only compiled when `media-processing-cpu` is enabled so that both the video
//! and audio transcode sessions are available.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_codec::{
    frame::FrameFlags,
    track::{CodecId, MediaKind, TrackInfo},
    AVFrame,
};
use cheetah_media_api::{
    error::Result as MediaResult,
    processing::{
        AudioCodec, AudioTarget, ProcessingJob, ProcessingJobState, TrackSelection, VideoCodec,
        VideoTarget,
    },
    MediaError,
};
use cheetah_sdk::{
    CancellationToken, DispatchResult, EngineContext, MediaFilter, PublishLease, PublisherSink,
    SdkError, StreamKey, SubscriberOptions,
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
        AudioCodec::Mp3 => Some(CodecId::MP3),
        AudioCodec::Pcm => None,
    }
}

pub(crate) fn video_target_to_spec(target: &VideoTarget) -> MediaResult<VideoTranscodeSpec> {
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

pub(crate) fn audio_target_to_spec(target: &AudioTarget) -> MediaResult<AudioTranscodeSpec> {
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

pub(crate) fn default_audio_bitrate(codec: CodecId) -> u32 {
    match codec {
        CodecId::G711A | CodecId::G711U => 64_000,
        CodecId::Opus => 64_000,
        CodecId::AAC => 128_000,
        _ => 128_000,
    }
}

/// Input sent from the async feeder to the blocking transcode worker.
pub(crate) enum TranscodeInput {
    Video(Arc<AVFrame>),
    Audio(Arc<AVFrame>),
}

impl TranscodeInput {
    pub(crate) fn is_keyframe(&self) -> bool {
        match self {
            TranscodeInput::Video(frame) => frame.flags.contains(FrameFlags::KEY),
            TranscodeInput::Audio(_) => false,
        }
    }
}

/// Bounded queue between the async feeder and the blocking worker.
///
/// Non-keyframe drops are returned to the caller so they can be counted. When a
/// keyframe arrives and the queue is full, droppable frames at the front are
/// evicted; if the queue is still full, it is cleared and the keyframe is kept,
/// so the decoder can always resynchronize on a fresh access point.
pub(crate) struct QueueState {
    items: VecDeque<TranscodeInput>,
    closed: bool,
}

pub(crate) struct TranscodeQueueSender {
    inner: Arc<Mutex<QueueState>>,
    condvar: Arc<Condvar>,
    cap: usize,
}

pub(crate) struct TranscodeQueueReceiver {
    inner: Arc<Mutex<QueueState>>,
    condvar: Arc<Condvar>,
}

pub(crate) fn transcode_queue(cap: usize) -> (TranscodeQueueSender, TranscodeQueueReceiver) {
    let inner = Arc::new(Mutex::new(QueueState {
        items: VecDeque::new(),
        closed: false,
    }));
    let condvar = Arc::new(Condvar::new());
    let sender = TranscodeQueueSender {
        inner: inner.clone(),
        condvar: condvar.clone(),
        cap,
    };
    let receiver = TranscodeQueueReceiver { inner, condvar };
    (sender, receiver)
}

impl TranscodeQueueSender {
    /// Try to enqueue an input. Returns `Ok(evicted)` on success where `evicted`
    /// is the number of previously queued droppable frames that were dropped to
    /// make room for a keyframe, and `Err(input)` if the input itself was dropped.
    pub(crate) fn try_send(&self, input: TranscodeInput) -> Result<usize, TranscodeInput> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed {
            return Err(input);
        }

        if guard.items.len() < self.cap {
            guard.items.push_back(input);
            self.condvar.notify_one();
            return Ok(0);
        }

        if !input.is_keyframe() {
            return Err(input);
        }

        // Evict droppable frames from the front to make room for the keyframe.
        let mut evicted = 0;
        while guard.items.len() >= self.cap
            && guard
                .items
                .front()
                .is_some_and(|front| !front.is_keyframe())
        {
            guard.items.pop_front();
            evicted += 1;
        }

        if guard.items.len() >= self.cap {
            // The queue is full of keyframes; start fresh with the newest one.
            evicted += guard.items.len();
            guard.items.clear();
        }

        guard.items.push_back(input);
        self.condvar.notify_one();
        Ok(evicted)
    }
}

impl Drop for TranscodeQueueSender {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        self.condvar.notify_all();
    }
}

impl TranscodeQueueReceiver {
    pub(crate) fn recv(&self) -> Option<TranscodeInput> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(item) = guard.items.pop_front() {
                return Some(item);
            }
            if guard.closed {
                return None;
            }
            guard = self.condvar.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Synchronous worker that owns the transcode sessions and publisher sink.
pub(crate) struct TranscodeWorker {
    video_session: Option<VideoTranscodeSession>,
    audio_session: Option<AudioTranscodeSession>,
    publisher: Arc<dyn PublisherSink>,
    last_announced_tracks: Vec<TrackInfo>,
    count_input: bool,
    job: Option<Arc<Mutex<ProcessingJob>>>,
}

impl TranscodeWorker {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: &MediaProcessingModuleConfig,
        source_video: Option<&TrackInfo>,
        source_audio: Option<&TrackInfo>,
        video_target: Option<&VideoTarget>,
        audio_target: Option<&AudioTarget>,
        publisher: Arc<dyn PublisherSink>,
        job: Option<Arc<Mutex<ProcessingJob>>>,
        count_input: bool,
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
            .update_tracks(tracks.clone())
            .map_err(|e| MediaError::internal(format!("update publisher tracks failed: {e}")))?;

        Ok(Self {
            video_session,
            audio_session,
            publisher,
            last_announced_tracks: tracks,
            count_input,
            job,
        })
    }

    pub(crate) fn current_output_tracks(&self) -> Vec<TrackInfo> {
        let mut tracks = Vec::new();
        if let Some(session) = self.video_session.as_ref() {
            tracks.push(session.output_track().clone());
        }
        if let Some(session) = self.audio_session.as_ref() {
            tracks.push(session.output_track().clone());
        }
        tracks
    }

    pub(crate) fn announce_tracks(&mut self) -> Result<(), SdkError> {
        let tracks = self.current_output_tracks();
        if tracks != self.last_announced_tracks {
            self.publisher
                .update_tracks(tracks.clone())
                .map_err(|e| SdkError::Internal(format!("update publisher tracks failed: {e}")))?;
            self.last_announced_tracks = tracks;
        }
        Ok(())
    }

    pub(crate) fn process(&mut self, input: TranscodeInput) -> Result<(), SdkError> {
        match input {
            TranscodeInput::Video(frame) => self.process_video(&frame),
            TranscodeInput::Audio(frame) => self.process_audio(&frame),
        }
    }

    pub(crate) fn process_video(&mut self, frame: &AVFrame) -> Result<(), SdkError> {
        if self.count_input {
            update_progress(&self.job, |job| {
                job.frames_in += 1;
                job.bytes_in += frame.payload.len() as u64;
            });
        }

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
        self.announce_tracks()?;
        Ok(())
    }

    pub(crate) fn process_audio(&mut self, frame: &AVFrame) -> Result<(), SdkError> {
        if self.count_input {
            update_progress(&self.job, |job| {
                job.frames_in += 1;
                job.bytes_in += frame.payload.len() as u64;
            });
        }

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
        self.announce_tracks()?;
        Ok(())
    }

    pub(crate) fn push_frame(&mut self, mut frame: AVFrame) -> Result<(), SdkError> {
        frame.origin = cheetah_codec::frame::FrameOrigin::Generated;
        let bytes_out = frame.payload.len() as u64;
        match self.publisher.push_frame(Arc::new(frame)) {
            Ok(DispatchResult::Accepted) => {
                update_progress(&self.job, |job| {
                    job.frames_out += 1;
                    job.bytes_out += bytes_out;
                });
                Ok(())
            }
            Ok(DispatchResult::DroppedByPolicy) => {
                update_progress(&self.job, |job| job.drops += 1);
                Ok(())
            }
            Ok(DispatchResult::RejectedClosed) => {
                update_progress(&self.job, |job| job.drops += 1);
                Err(SdkError::Internal("publisher closed".into()))
            }
            Err(e) => {
                update_progress(&self.job, |job| job.drops += 1);
                Err(e)
            }
        }
    }

    pub(crate) fn flush_and_close(mut self) -> Result<(), SdkError> {
        if let Some(session) = self.video_session.as_mut() {
            for frame in session
                .flush()
                .map_err(|e| SdkError::Internal(format!("video flush failed: {e}")))?
            {
                self.push_frame(frame)?;
            }
        }
        if let Some(session) = self.audio_session.as_mut() {
            for frame in session
                .flush()
                .map_err(|e| SdkError::Internal(format!("audio flush failed: {e}")))?
            {
                self.push_frame(frame)?;
            }
        }
        self.announce_tracks()?;
        self.publisher.close()
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn update_progress<F>(job: &Option<Arc<Mutex<ProcessingJob>>>, f: F)
where
    F: FnOnce(&mut ProcessingJob),
{
    if let Some(job) = job.as_ref() {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
        guard.updated_at = now_ms();
    }
}

pub(crate) fn finish_job(job: &Option<Arc<Mutex<ProcessingJob>>>, last_error: Option<&SdkError>) {
    if let Some(job) = job.as_ref() {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        let finished_at = now_ms();
        guard.finished_at = Some(finished_at);
        guard.last_error = last_error.map(|e| e.to_string());
        guard.updated_at = finished_at;
        if guard.state == ProcessingJobState::Running {
            guard.state = if last_error.is_some() {
                ProcessingJobState::Failed
            } else {
                ProcessingJobState::Stopped
            };
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
    // Run the worker body and then transition the shared job to a terminal
    // state regardless of whether it completed or failed early.
    let publisher_api = engine.publisher_api.clone();
    let release_lease = publisher_lease.clone();
    let finish_job_ref = job.clone();

    let result = async move {
        let (source_video, source_audio) = wait_for_source_tracks(
            &engine,
            &source,
            &track_selection,
            video_target.is_some(),
            audio_target.is_some(),
            &cancel,
        )
        .await?;

        let publisher: Arc<dyn PublisherSink> = Arc::from(publisher);
        let worker_publisher = Arc::clone(&publisher);
        let worker = TranscodeWorker::new(
            &config,
            source_video.as_ref(),
            source_audio.as_ref(),
            video_target.as_ref(),
            audio_target.as_ref(),
            worker_publisher,
            job.clone(),
            true,
        )
        .map_err(|e| SdkError::Internal(format!("create transcode worker: {e}")))?;

        let (tx, rx) = transcode_queue(64);
        let worker_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let worker_error_clone = worker_error.clone();

        let handle = engine
            .runtime_api
            .spawn_blocking(
                "transcode-worker",
                Box::new(move || {
                    let mut worker = worker;
                    let mut process_error: Option<String> = None;
                    while let Some(input) = rx.recv() {
                        if let Err(e) = worker.process(input) {
                            warn!("transcode worker stopping: {e}");
                            process_error = Some(format!("{e}"));
                            break;
                        }
                    }
                    if let Err(e) = worker.flush_and_close() {
                        warn!("transcode worker flush/close failed: {e}");
                        process_error.get_or_insert_with(|| format!("{e}"));
                    }
                    if let Some(err) = process_error {
                        *worker_error_clone
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(err);
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
        let source_key_for_request = source.clone();
        let mut subscriber = engine
            .subscriber_api
            .subscribe(source, subscriber_options)
            .await
            .map_err(|e| SdkError::Internal(format!("subscribe failed: {e}")))?;

        let mut subscriber_error: Option<SdkError> = None;
        loop {
            if publisher.take_keyframe_requests() > 0 {
                if let Err(err) = engine
                    .stream_manager_api
                    .request_keyframe(&source_key_for_request)
                    .await
                {
                    warn!("transcode keyframe request to source {source_key_for_request} failed: {err}");
                }
            }

            let cancel_fut = cancel.cancelled().fuse();
            let recv_fut = subscriber.recv().fuse();
            pin_mut!(cancel_fut, recv_fut);

            let frame = select_biased! {
                _ = cancel_fut => break,
                frame = recv_fut => frame,
            };

            match frame {
                Ok(Some(frame)) => {
                    let input = match frame.media_kind {
                        MediaKind::Video => TranscodeInput::Video(frame),
                        MediaKind::Audio => TranscodeInput::Audio(frame),
                        // Data/subtitle frames cannot be transcoded; skip them
                        // instead of misrouting them to the audio session.
                        _ => continue,
                    };
                    match tx.try_send(input) {
                        Ok(evicted) => {
                            if evicted > 0 {
                                update_progress(&job, |job| job.drops += evicted as u64);
                            }
                        }
                        Err(_) => {
                            warn!("transcode input queue full; dropping frame");
                            update_progress(&job, |job| job.drops += 1);
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("transcode subscriber error: {e}");
                    subscriber_error = Some(SdkError::Internal(format!("subscriber error: {e}")));
                    break;
                }
            }
        }

        drop(tx);
        let join_result = handle.wait().await;
        if let Some(err) = worker_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            return Err(SdkError::Internal(err));
        }
        if let Err(e) = join_result {
            return Err(SdkError::Internal(format!(
                "transcode worker joined with error: {e}"
            )));
        }
        if let Some(err) = subscriber_error {
            return Err(err);
        }
        Ok(())
    }
    .await;

    finish_job(&finish_job_ref, result.as_ref().err());
    let _ = publisher_api.release_publisher(&release_lease).await;
    result
}

pub(crate) async fn wait_for_source_tracks(
    engine: &EngineContext,
    source: &StreamKey,
    track_selection: &TrackSelection,
    need_video: bool,
    need_audio: bool,
    cancel: &CancellationToken,
) -> Result<(Option<TrackInfo>, Option<TrackInfo>), SdkError> {
    let deadline = engine.runtime_api.now().as_micros() + 5_000_000;
    while engine.runtime_api.now().as_micros() < deadline {
        if cancel.is_cancelled() {
            return Err(SdkError::Internal(
                "wait for source tracks cancelled".to_string(),
            ));
        }
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
