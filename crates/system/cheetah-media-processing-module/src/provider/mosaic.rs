//! Video mosaic job worker orchestration.
//!
//! Subscribes to 2-9 source video streams, forwards frames over a bounded
//! channel to the `VideoMosaicker` running in a blocking worker thread, and
//! publishes the composed output stream.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cheetah_codec::{
    frame::FrameOrigin,
    track::{MediaKind, TrackReadiness},
    AVFrame, CodecExtradata, CodecId, MonoTime, Rational32,
};
use cheetah_media_api::{
    ids::StreamKeyBridge,
    processing::{MosaicLayout, ProcessingJob, VideoMosaicInput},
};
use cheetah_sdk::{
    BackpressurePolicy, BootstrapPolicy, CancellationToken, DispatchResult, EngineContext,
    JoinHandle, MediaFilter, PublishLease, PublisherSink, SdkError, StreamKey, SubscriberOptions,
    SubscriberSource,
};
use futures::{
    future::FutureExt,
    stream::{unfold, SelectAll, Stream, StreamExt},
};
use tracing::{info, warn};

use crate::config::MediaProcessingModuleConfig;
use crate::provider::mosaicker::VideoMosaicker;

const MIN_SOURCES: usize = 2;
const MAX_SOURCES: usize = 9;
const MOSAIC_QUEUE_CAPACITY: usize = 32;

type SourceStream =
    std::pin::Pin<Box<dyn Stream<Item = (usize, Result<Option<Arc<AVFrame>>, SdkError>)> + Send>>;

enum MosaicInput {
    Frame { source: usize, frame: Arc<AVFrame> },
    EndOfStream { source: usize },
}

async fn wait_for_source_tracks(
    engine: &EngineContext,
    inputs: &[VideoMosaicInput],
    cancel: &CancellationToken,
) -> Result<Vec<cheetah_codec::track::TrackInfo>, SdkError> {
    let mut tracks = Vec::with_capacity(inputs.len());
    for input in inputs {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(&input.source);
        let key = StreamKey::new(namespace, path);
        let deadline_us = engine.runtime_api.now().as_micros() + 5_000_000;
        let mut found = None;
        while engine.runtime_api.now().as_micros() < deadline_us {
            if cancel.is_cancelled() {
                return Err(SdkError::Internal(
                    "wait for source tracks cancelled".to_string(),
                ));
            }
            if let Ok(Some(snapshot)) = engine.stream_manager_api.get_stream(&key).await {
                if let Some(video) = snapshot.tracks.into_iter().find(|t| {
                    t.media_kind == MediaKind::Video
                        && t.readiness == TrackReadiness::Ready
                        && matches!(t.codec, CodecId::H264 | CodecId::H265 | CodecId::MJPEG)
                }) {
                    found = Some(video);
                    break;
                }
            }
            let sleep_deadline =
                MonoTime::from_micros(engine.runtime_api.now().as_micros() + 200_000);
            let mut timer = engine.runtime_api.sleep_until(sleep_deadline);
            timer.wait().await;
        }
        let track = found.ok_or_else(|| {
            SdkError::NotFound(format!(
                "source stream {key} does not have a ready H.264/H.265 video track"
            ))
        })?;
        tracks.push(track);
    }
    Ok(tracks)
}

async fn subscribe_to_sources(
    engine: &EngineContext,
    inputs: &[VideoMosaicInput],
) -> Result<Vec<Box<dyn SubscriberSource>>, SdkError> {
    let mut subscribers = Vec::with_capacity(inputs.len());
    for input in inputs {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(&input.source);
        let key = StreamKey::new(namespace, path);
        let options = SubscriberOptions {
            queue_capacity: 64,
            backpressure: BackpressurePolicy::DropDroppableFirst,
            bootstrap_policy: BootstrapPolicy {
                mode: cheetah_sdk::BootstrapMode::LiveTail,
                max_bootstrap_age_ms: Some(1_500),
                max_bootstrap_frames: 32,
                wait_for_next_random_access_point: true,
            },
            media_filter: MediaFilter {
                enable_video: true,
                enable_audio: false,
            },
        };
        let subscriber = engine
            .subscriber_api
            .subscribe(key, options)
            .await
            .map_err(|e| SdkError::Internal(format!("video mosaic subscribe failed: {e}")))?;
        subscribers.push(subscriber);
    }
    Ok(subscribers)
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_video_mosaic_worker(
    engine: EngineContext,
    config: MediaProcessingModuleConfig,
    inputs: Vec<VideoMosaicInput>,
    layout: MosaicLayout,
    publisher_lease: PublishLease,
    mut publisher_sink: Box<dyn PublisherSink>,
    cancel: CancellationToken,
    job: Option<Arc<Mutex<ProcessingJob>>>,
) -> Result<(), SdkError> {
    let publisher_api = engine.publisher_api.clone();
    let job_for_state = job.clone();

    let result = async move {
        if inputs.len() < MIN_SOURCES {
            return Err(SdkError::InvalidArgument(format!(
                "video mosaic requires at least {MIN_SOURCES} sources, got {}",
                inputs.len()
            )));
        }
        if inputs.len() > MAX_SOURCES {
            return Err(SdkError::InvalidArgument(format!(
                "video mosaic supports at most {MAX_SOURCES} sources, got {}",
                inputs.len()
            )));
        }

        let source_tracks = wait_for_source_tracks(&engine, &inputs, &cancel).await?;

        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<MosaicInput>(MOSAIC_QUEUE_CAPACITY);

        let worker_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let worker_error_clone = worker_error.clone();

        let config_for_worker = config.clone();
        let inputs_for_worker = inputs.clone();
        let layout_for_worker = layout.clone();
        let source_tracks_for_worker = source_tracks.clone();
        let job_for_worker = job.clone();

        let handle: Box<dyn JoinHandle> = engine
            .runtime_api
            .spawn_blocking(
                "video-mosaic-worker",
                Box::new(move || {
                    if let Err(e) = run_mosaicker(
                        &config_for_worker,
                        &inputs_for_worker,
                        &layout_for_worker,
                        &source_tracks_for_worker,
                        receiver,
                        &mut *publisher_sink,
                        &job_for_worker,
                    ) {
                        warn!("video mosaic worker failed: {e}");
                        *worker_error_clone.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("{e}"));
                    }
                }),
            )
            .map_err(|e| SdkError::Internal(format!("spawn video mosaic worker: {e}")))?;

        let subscribers = match subscribe_to_sources(&engine, &inputs).await {
            Ok(s) => s,
            Err(e) => {
                drop(sender);
                if let Err(join_err) = handle.wait().await {
                    warn!("video mosaic worker joined with error after subscribe failure: {join_err}");
                }
                return Err(e);
            }
        };

        let mut combined: SelectAll<SourceStream> = SelectAll::new();
        for (i, subscriber) in subscribers.into_iter().enumerate() {
            let stream = unfold(Some(subscriber), move |sub| async move {
                let mut sub = sub?;
                match sub.recv().await {
                    Ok(None) => Some(((i, Ok(None)), None)),
                    other => Some(((i, other), Some(sub))),
                }
            });
            combined.push(Box::pin(stream));
        }

        async {
            'feed: loop {
                let cancel_fut = cancel.cancelled().fuse();
                let next_fut = combined.next().fuse();
                futures::pin_mut!(cancel_fut, next_fut);
                let selected = futures::select_biased! {
                    _ = cancel_fut => None,
                    item = next_fut => item,
                };
                match selected {
                    Some((source, Ok(Some(frame)))) => {
                        update_progress(&job, |j| {
                            j.frames_in += 1;
                            j.bytes_in += frame.payload.len() as u64;
                        });
                        match sender.try_send(MosaicInput::Frame { source, frame }) {
                            Ok(()) => {}
                            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                update_progress(&job, |j| j.drops += 1);
                                warn!(source, "video mosaic input queue full; dropping frame");
                            }
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                warn!("video mosaic worker disconnected; stopping feeder");
                                break 'feed;
                            }
                        }
                    }
                    Some((source, Ok(None))) => loop {
                        match sender.try_send(MosaicInput::EndOfStream { source }) {
                            Ok(()) => break,
                            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                let deadline = MonoTime::from_micros(
                                    engine.runtime_api.now().as_micros() + 1_000,
                                );
                                let mut sleep = engine.runtime_api.sleep_until(deadline);
                                let eos_cancel_fut = cancel.cancelled().fuse();
                                let eos_sleep_fut = sleep.wait().fuse();
                                futures::pin_mut!(eos_cancel_fut, eos_sleep_fut);
                                let eos_cancelled = futures::select_biased! {
                                    _ = eos_cancel_fut => true,
                                    _ = eos_sleep_fut => false,
                                };
                                if eos_cancelled {
                                    warn!("video mosaic cancelled while waiting to deliver EOS");
                                    break 'feed;
                                }
                            }
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                warn!(
                                    "video mosaic worker disconnected before EOS from source {source}"
                                );
                                break 'feed;
                            }
                        }
                    },
                    Some((_, Err(e))) => {
                        warn!("video mosaic source error: {e}");
                        break 'feed;
                    }
                    None => break 'feed,
                }
            }
            Ok::<(), SdkError>(())
        }
        .await?;

        drop(sender);
        if let Err(join_err) = handle.wait().await {
            return Err(SdkError::Internal(format!(
                "video mosaic worker joined with error: {join_err}"
            )));
        }

        if let Some(err) = worker_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            return Err(SdkError::Internal(format!(
                "video mosaic worker failed: {err}"
            )));
        }

        Ok::<(), SdkError>(())
    }
    .await;

    if let Some(job) = job_for_state.as_ref() {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        match &result {
            Ok(()) => {
                if guard.state == cheetah_media_api::processing::ProcessingJobState::Running {
                    guard.state = cheetah_media_api::processing::ProcessingJobState::Stopped;
                }
            }
            Err(e) => {
                guard.state = cheetah_media_api::processing::ProcessingJobState::Failed;
                guard.last_error = Some(format!("{e}"));
            }
        }
        let ts = now_ms();
        guard.updated_at = ts;
        guard.finished_at = Some(ts);
    }

    let _ = publisher_api.release_publisher(&publisher_lease).await;
    result
}

fn run_mosaicker(
    config: &MediaProcessingModuleConfig,
    inputs: &[VideoMosaicInput],
    layout: &MosaicLayout,
    source_tracks: &[cheetah_codec::track::TrackInfo],
    receiver: std::sync::mpsc::Receiver<MosaicInput>,
    publisher: &mut dyn PublisherSink,
    job: &Option<Arc<Mutex<ProcessingJob>>>,
) -> Result<(), SdkError> {
    let mut mosaicker = VideoMosaicker::new(config, inputs, layout, source_tracks)
        .map_err(|e| SdkError::Internal(format!("create video mosaicker: {e}")))?;

    let initial_track = mosaicker.output_track().clone();
    if let Err(e) = publisher.update_tracks(vec![initial_track.clone()]) {
        return Err(SdkError::Internal(format!("update output tracks: {e}")));
    }
    let mut last_announced_extradata = Some(initial_track.extradata.clone());

    let output_fps = mosaicker
        .output_track()
        .fps
        .unwrap_or(Rational32::new(30, 1));
    let interval = if output_fps.num > 0 {
        Duration::from_secs_f64(output_fps.den as f64 / output_fps.num as f64)
    } else {
        Duration::from_millis(33)
    };
    let mut next_tick = Instant::now() + interval;

    loop {
        let timeout = next_tick.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(timeout) {
            Ok(MosaicInput::Frame { source, frame }) => {
                mosaicker
                    .submit_source_frame(source, &frame)
                    .map_err(|e| SdkError::Internal(format!("submit source frame: {e}")))?;
                if Instant::now() < next_tick {
                    continue;
                }
            }
            Ok(MosaicInput::EndOfStream { source }) => {
                mosaicker
                    .mark_source_eos(source)
                    .map_err(|e| SdkError::Internal(format!("mark source eos: {e}")))?;
                if mosaicker.all_sources_eos() {
                    break;
                }
                if Instant::now() < next_tick {
                    continue;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let frames = mosaicker
                    .tick()
                    .map_err(|e| SdkError::Internal(format!("mosaic tick: {e}")))?;
                publish_frames(
                    frames,
                    mosaicker.output_track(),
                    publisher,
                    job,
                    &mut last_announced_extradata,
                )?;
                break;
            }
        }

        let frames = mosaicker
            .tick()
            .map_err(|e| SdkError::Internal(format!("mosaic tick: {e}")))?;
        publish_frames(
            frames,
            mosaicker.output_track(),
            publisher,
            job,
            &mut last_announced_extradata,
        )?;

        next_tick = Instant::now() + interval;
    }

    let flushed = mosaicker
        .flush()
        .map_err(|e| SdkError::Internal(format!("flush mosaicker: {e}")))?;
    publish_frames(
        flushed,
        mosaicker.output_track(),
        publisher,
        job,
        &mut last_announced_extradata,
    )?;

    info!("video mosaic worker finished");
    Ok(())
}

fn publish_frames(
    frames: Vec<AVFrame>,
    output_track: &cheetah_codec::track::TrackInfo,
    publisher: &mut dyn PublisherSink,
    job: &Option<Arc<Mutex<ProcessingJob>>>,
    last_announced_extradata: &mut Option<CodecExtradata>,
) -> Result<(), SdkError> {
    if frames.is_empty() {
        return Ok(());
    }
    if last_announced_extradata.as_ref() != Some(&output_track.extradata) {
        if let Err(e) = publisher.update_tracks(vec![output_track.clone()]) {
            return Err(SdkError::Internal(format!(
                "update mosaic output tracks: {e}"
            )));
        }
        *last_announced_extradata = Some(output_track.extradata.clone());
    }
    for mut f in frames {
        let payload_len = f.payload.len() as u64;
        f.origin = FrameOrigin::Generated;
        let _ = f.set_duration(1);
        match publisher.push_frame(Arc::new(f)) {
            Ok(DispatchResult::Accepted) => {
                update_progress(job, |j| {
                    j.frames_out += 1;
                    j.bytes_out += payload_len;
                });
            }
            Ok(DispatchResult::DroppedByPolicy) => {
                update_progress(job, |j| j.drops += 1);
            }
            Ok(DispatchResult::RejectedClosed) => {
                update_progress(job, |j| j.drops += 1);
                return Err(SdkError::Internal("mosaic publisher closed".into()));
            }
            Err(e) => return Err(SdkError::Internal(format!("publish mosaic frame: {e}"))),
        }
    }
    Ok(())
}

#[allow(clippy::explicit_auto_deref)]
fn update_progress<F>(job: &Option<Arc<Mutex<ProcessingJob>>>, f: F)
where
    F: FnOnce(&mut ProcessingJob),
{
    if let Some(job) = job.as_ref() {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut *guard);
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;
    use cheetah_codec::{track::TrackId, track::TrackInfo};
    use cheetah_media_api::ids::MediaKey;
    use cheetah_media_api::processing::{MosaicCell, ProcessingJobSpec, ProcessingJobState};

    fn two_source_tracks() -> Vec<TrackInfo> {
        let mut tracks = vec![
            TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 30),
            TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 30),
        ];
        for track in &mut tracks {
            track.readiness = TrackReadiness::Ready;
            track.width = Some(160);
            track.height = Some(120);
            track.fps = Some(Rational32::new(30, 1));
        }
        tracks
    }

    fn two_source_inputs() -> Vec<VideoMosaicInput> {
        vec![
            VideoMosaicInput {
                source: MediaKey::new("_", "app", "s1", None).unwrap(),
                cell: MosaicCell {
                    column: 0,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
            VideoMosaicInput {
                source: MediaKey::new("_", "app", "s2", None).unwrap(),
                cell: MosaicCell {
                    column: 1,
                    row: 0,
                    z_order: 0,
                },
                audio_gain_db: None,
                fit: None,
                label: None,
            },
        ]
    }

    fn fast_layout() -> MosaicLayout {
        MosaicLayout {
            columns: 2,
            rows: 1,
            cell_width: 80,
            cell_height: 60,
            background: None,
            frame_rate_num: Some(1000),
            frame_rate_den: Some(1),
            bit_rate: None,
            gop_size: None,
            video_codec: None,
            fit: None,
        }
    }

    fn dummy_job() -> Arc<Mutex<ProcessingJob>> {
        Arc::new(Mutex::new(ProcessingJob {
            job_id: cheetah_media_api::processing::ProcessingJobId::default(),
            spec: ProcessingJobSpec::VideoMosaic {
                inputs: vec![],
                target: MediaKey::new("_", "app", "out", None).unwrap(),
                layout: fast_layout(),
                audio_mix: None,
                overlays: vec![],
            },
            state: ProcessingJobState::Running,
            generation: 0,
            profile: "software".to_string(),
            created_at: 0,
            updated_at: 0,
            started_at: None,
            finished_at: None,
            input_keys: vec![],
            output_keys: vec![],
            ref_count: 1,
            restart_count: 0,
            frames_in: 0,
            frames_out: 0,
            bytes_in: 0,
            bytes_out: 0,
            drops: 0,
            pending: 0,
            flushes: 0,
            resets: 0,
            last_error: None,
        }))
    }

    struct MockPublisher {
        result: DispatchResult,
        tracks: Arc<Mutex<Vec<cheetah_codec::track::TrackInfo>>>,
    }

    impl MockPublisher {
        fn new(result: DispatchResult) -> Self {
            Self {
                result,
                tracks: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl PublisherSink for MockPublisher {
        fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            *self.tracks.lock().unwrap() = tracks;
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            Ok(self.result)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    fn run_with_publisher(
        result: DispatchResult,
    ) -> Result<(Arc<Mutex<ProcessingJob>>, String), SdkError> {
        let config = MediaProcessingModuleConfig {
            profile: "software".to_string(),
            ..Default::default()
        };
        let layout = fast_layout();
        let inputs = two_source_inputs();
        let tracks = two_source_tracks();
        let job = dummy_job();
        let (sender, receiver) = std::sync::mpsc::sync_channel::<MosaicInput>(8);
        let mut publisher = MockPublisher::new(result);

        let err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let err_clone = err.clone();
        let job_for_worker = job.clone();

        thread::scope(|s| {
            s.spawn(|| {
                if let Err(e) = run_mosaicker(
                    &config,
                    &inputs,
                    &layout,
                    &tracks,
                    receiver,
                    &mut publisher,
                    &Some(job_for_worker),
                ) {
                    *err_clone.lock().unwrap() = Some(format!("{e}"));
                }
            });
            thread::sleep(Duration::from_millis(5));
            drop(sender);
        });

        let msg = err.lock().unwrap().clone().unwrap_or_default();
        if msg.is_empty() {
            Ok((job, msg))
        } else {
            Err(SdkError::Internal(msg))
        }
    }

    #[test]
    #[cfg(feature = "media-processing-cpu")]
    fn run_mosaicker_stops_when_publisher_closed() {
        let result = run_with_publisher(DispatchResult::RejectedClosed);
        assert!(
            result.is_err(),
            "run_mosaicker should fail when publisher is closed"
        );
    }

    #[test]
    #[cfg(feature = "media-processing-cpu")]
    fn run_mosaicker_counts_dropped_by_policy() {
        let (job, _) = run_with_publisher(DispatchResult::DroppedByPolicy).unwrap();
        let guard = job.lock().unwrap();
        assert_eq!(guard.frames_out, 0, "no frames should be accepted");
        assert!(guard.drops > 0, "dropped frames should be counted");
    }
}
