//! Audio mix job worker orchestration.
//!
//! Subscribes to 2-16 source audio streams, forwards frames over a bounded
//! channel to the `AudioMixer` running in a blocking worker thread, and
//! publishes the mixed output stream.

use std::sync::{Arc, Mutex};

use cheetah_codec::{
    frame::{FrameFlags, FrameOrigin},
    track::{MediaKind, TrackInfo, TrackReadiness},
    AVFrame,
};
use cheetah_media_api::{
    ids::StreamKeyBridge,
    processing::{AudioMix, AudioMixInput, AudioTarget, ProcessingJob},
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
use crate::logging::log_job_lifecycle;
use crate::provider::mixer::AudioMixer;

/// Maximum number of audio sources the mixer accepts.
const MAX_SOURCES: usize = 16;
/// Minimum number of audio sources the mixer accepts.
const MIN_SOURCES: usize = 2;
/// Bounded queue between the async feeder and the blocking mixer worker.
const MIX_QUEUE_CAPACITY: usize = 128;

/// Merged stream of source frames indexed by source position.
type SourceStream =
    std::pin::Pin<Box<dyn Stream<Item = (usize, Result<Option<Arc<AVFrame>>, SdkError>)> + Send>>;

/// Commands sent from the async source feeder to the blocking mixer worker.
enum MixInput {
    Frame { source: usize, frame: Arc<AVFrame> },
    EndOfStream { source: usize },
}

/// Build source tracks by waiting for each input stream to announce an audio
/// track. The returned vector is in the same order as `inputs`.
async fn wait_for_source_tracks(
    engine: &EngineContext,
    inputs: &[AudioMixInput],
    cancel: &CancellationToken,
) -> Result<Vec<TrackInfo>, SdkError> {
    use cheetah_codec::MonoTime;

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
                if let Some(audio) = snapshot.tracks.into_iter().find(|t| {
                    t.media_kind == MediaKind::Audio && t.readiness == TrackReadiness::Ready
                }) {
                    found = Some(audio);
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
                "source stream {key} does not have a ready audio track"
            ))
        })?;
        tracks.push(track);
    }
    Ok(tracks)
}

/// Subscribe to every audio source and return the subscriber handles in the
/// same order as `inputs`.
async fn subscribe_to_sources(
    engine: &EngineContext,
    inputs: &[AudioMixInput],
) -> Result<Vec<Box<dyn SubscriberSource>>, SdkError> {
    let mut subscribers = Vec::with_capacity(inputs.len());
    for input in inputs {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(&input.source);
        let key = StreamKey::new(namespace, path);
        let options = SubscriberOptions {
            queue_capacity: 64,
            backpressure: BackpressurePolicy::DropDroppableFirst,
            bootstrap_policy: BootstrapPolicy::default(),
            media_filter: MediaFilter {
                enable_video: false,
                enable_audio: true,
            },
        };
        let subscriber = engine
            .subscriber_api
            .subscribe(key, options)
            .await
            .map_err(|e| SdkError::Internal(format!("audio mix subscribe failed: {e}")))?;
        subscribers.push(subscriber);
    }
    Ok(subscribers)
}

/// Spawn an audio mix worker that subscribes to `inputs` and publishes mixed
/// audio to `target`.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_audio_mix_worker(
    engine: EngineContext,
    config: MediaProcessingModuleConfig,
    inputs: Vec<AudioMixInput>,
    mix: AudioMix,
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
                "audio mix requires at least {MIN_SOURCES} sources, got {}",
                inputs.len()
            )));
        }
        if inputs.len() > MAX_SOURCES {
            return Err(SdkError::InvalidArgument(format!(
                "audio mix supports at most {MAX_SOURCES} sources, got {}",
                inputs.len()
            )));
        }

        let source_tracks = wait_for_source_tracks(&engine, &inputs, &cancel).await?;

        let (sender, receiver) = std::sync::mpsc::sync_channel::<MixInput>(MIX_QUEUE_CAPACITY);

        let worker_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let worker_error_clone = worker_error.clone();

        let inputs_for_worker = inputs.clone();
        let mix_for_worker = mix.clone();
        let job_for_worker = job.clone();

        let handle: Box<dyn JoinHandle> = engine
            .runtime_api
            .spawn_blocking(
                "audio-mix-worker",
                Box::new(move || {
                    if let Err(e) = run_mixer(
                        &config,
                        &inputs_for_worker,
                        &mix_for_worker.output,
                        &source_tracks,
                        receiver,
                        &mut *publisher_sink,
                        &job_for_worker,
                    ) {
                        warn!("audio mix worker failed: {e}");
                        *worker_error_clone.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("{e}"));
                    }
                }),
            )
            .map_err(|e| SdkError::Internal(format!("spawn audio mix worker: {e}")))?;

        // Subscribe to all sources and merge the streams.
        let subscribers = match subscribe_to_sources(&engine, &inputs).await {
            Ok(s) => s,
            Err(e) => {
                drop(sender);
                if let Err(join_err) = handle.wait().await {
                    warn!("audio mix worker joined with error after subscribe failure: {join_err}");
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
                        match sender.try_send(MixInput::Frame { source, frame }) {
                            Ok(()) => {}
                            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                update_progress(&job, |j| j.drops += 1);
                                warn!(
                                    "audio mix input queue full; dropping frame from source {source}"
                                );
                            }
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                warn!("audio mix worker disconnected; stopping feeder");
                                break 'feed;
                            }
                        }
                    }
                    Some((source, Ok(None))) => {
                        // End-of-stream is a control message: retry with a short
                        // non-blocking sleep instead of a blocking send so we do not
                        // park a runtime worker thread.
                        loop {
                            match sender.try_send(MixInput::EndOfStream { source }) {
                                Ok(()) => break,
                                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                    let deadline = cheetah_codec::MonoTime::from_micros(
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
                                        warn!("audio mix cancelled while waiting to deliver EOS");
                                        break 'feed;
                                    }
                                }
                                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                    warn!("audio mix worker disconnected before EOS from source {source}");
                                    break 'feed;
                                }
                            }
                        }
                    }
                    Some((_, Err(e))) => {
                        warn!("audio mix source error: {e}");
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
                "audio mix worker joined with error: {join_err}"
            )));
        }

        if let Some(err) = worker_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            return Err(SdkError::Internal(format!(
                "audio mix worker failed: {err}"
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
                if guard.state == cheetah_media_api::processing::ProcessingJobState::Running {
                    guard.state = cheetah_media_api::processing::ProcessingJobState::Failed;
                    guard.last_error = Some(format!("{e}"));
                }
            }
        }
        let ts = now_ms();
        guard.updated_at = ts;
        guard.finished_at = Some(ts);
        let stage = if guard.state == cheetah_media_api::processing::ProcessingJobState::Failed {
            "failed"
        } else {
            "stopped"
        };
        log_job_lifecycle(&guard, stage, Some(ts.saturating_sub(guard.created_at)));
    }

    let _ = publisher_api.release_publisher(&publisher_lease).await;
    result
}

fn run_mixer(
    config: &MediaProcessingModuleConfig,
    inputs: &[AudioMixInput],
    output: &AudioTarget,
    source_tracks: &[TrackInfo],
    receiver: std::sync::mpsc::Receiver<MixInput>,
    publisher: &mut dyn PublisherSink,
    job: &Option<Arc<Mutex<ProcessingJob>>>,
) -> Result<(), SdkError> {
    let mut mixer = AudioMixer::new(config, inputs, output, source_tracks)
        .map_err(|e| SdkError::Internal(format!("create audio mixer: {e}")))?;

    publisher
        .update_tracks(vec![mixer.output_track.clone()])
        .map_err(|e| SdkError::Internal(format!("update output tracks: {e}")))?;

    let mut frames_out: u64 = 0;
    let mut eos_received: usize = 0;

    loop {
        match receiver.recv() {
            Ok(MixInput::Frame { source, frame }) => {
                if let Err(e) = mixer.submit_source_frame(source, &frame) {
                    return Err(SdkError::Internal(format!("submit source frame: {e}")));
                }
                loop {
                    match mixer.try_mix_frame() {
                        Ok(frames) if !frames.is_empty() => {
                            for mut f in frames {
                                let payload_len = f.payload.len() as u64;
                                f.origin = FrameOrigin::Generated;
                                f.flags.insert(FrameFlags::KEY);
                                match publisher.push_frame(Arc::new(f)) {
                                    Ok(DispatchResult::Accepted) => {
                                        frames_out += 1;
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
                                        return Err(SdkError::Internal(
                                            "audio mix publisher closed".into(),
                                        ));
                                    }
                                    Err(e) => {
                                        return Err(SdkError::Internal(format!(
                                            "publish mixed frame: {e}"
                                        )))
                                    }
                                }
                            }
                        }
                        Ok(_) => break,
                        Err(e) => return Err(SdkError::Internal(format!("mix frame: {e}"))),
                    }
                }
            }
            Ok(MixInput::EndOfStream { source }) => {
                if let Err(e) = mixer.mark_source_eos(source) {
                    return Err(SdkError::Internal(format!("mark source eos: {e}")));
                }
                eos_received += 1;
                if eos_received >= inputs.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let flushed = mixer
        .flush()
        .map_err(|e| SdkError::Internal(format!("flush mixer: {e}")))?;
    for mut f in flushed {
        let payload_len = f.payload.len() as u64;
        f.origin = FrameOrigin::Generated;
        f.flags.insert(FrameFlags::KEY);
        match publisher.push_frame(Arc::new(f)) {
            Ok(DispatchResult::Accepted) => {
                frames_out += 1;
                update_progress(job, |j| {
                    j.frames_out += 1;
                    j.bytes_out += payload_len;
                });
            }
            Ok(DispatchResult::DroppedByPolicy) => update_progress(job, |j| j.drops += 1),
            Ok(DispatchResult::RejectedClosed) => {
                update_progress(job, |j| j.drops += 1);
                return Err(SdkError::Internal("audio mix publisher closed".into()));
            }
            Err(e) => return Err(SdkError::Internal(format!("publish flushed frame: {e}"))),
        }
    }
    info!("audio mix worker finished; {frames_out} frames published");
    Ok(())
}

#[allow(clippy::explicit_auto_deref)]
fn update_progress<F>(job: &Option<Arc<Mutex<ProcessingJob>>>, f: F)
where
    F: FnOnce(&mut ProcessingJob),
{
    if let Some(job) = job.as_ref() {
        let mut guard = job.lock().unwrap_or_else(|e| e.into_inner());
        let created = guard.created_at;
        f(&mut *guard);
        let total = guard.frames_in + guard.frames_out + guard.drops;
        let started = guard.started_at.is_none() && total > 0;
        let first_output = guard.frames_out > 0 && guard.first_output_at.is_none();
        if started || first_output {
            let now = now_ms();
            if started {
                guard.started_at = Some(now);
                log_job_lifecycle(&guard, "started", Some(now.saturating_sub(created)));
            }
            if first_output {
                guard.first_output_at = Some(now);
                log_job_lifecycle(&guard, "first_output", Some(now.saturating_sub(created)));
            }
        }
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
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{frame::FrameFormat, track::TrackId, Timebase};
    use cheetah_media_api::ids::{MediaKey, ProcessingJobId};
    use cheetah_media_api::processing::{AudioCodec, ProcessingJobSpec, ProcessingJobState};

    fn g711a_track() -> TrackInfo {
        let mut track = TrackInfo::new(
            TrackId(0),
            MediaKind::Audio,
            cheetah_codec::track::CodecId::G711A,
            8_000,
        );
        track.sample_rate = Some(8_000);
        track.channels = Some(1);
        track.readiness = TrackReadiness::Ready;
        track
    }

    fn g711a_frame(payload: Vec<u8>, pts: i64) -> AVFrame {
        AVFrame::new(
            TrackId(0),
            MediaKind::Audio,
            cheetah_codec::track::CodecId::G711A,
            FrameFormat::G711Packet,
            pts,
            pts,
            Timebase::new(1, 8_000),
            Bytes::from(payload),
        )
    }

    fn dummy_job() -> Arc<Mutex<ProcessingJob>> {
        let output = AudioTarget {
            codec: AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };
        Arc::new(Mutex::new(ProcessingJob {
            job_id: ProcessingJobId::default(),
            owner: None,
            spec: ProcessingJobSpec::AudioMix {
                inputs: vec![],
                target: MediaKey::with_default_vhost("app", "out", None).unwrap(),
                output,
            },
            state: ProcessingJobState::Running,
            generation: 0,
            profile: "software".to_string(),
            created_at: 0,
            updated_at: 0,
            started_at: None,
            first_output_at: None,
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

    struct RejectingPublisher;

    impl PublisherSink for RejectingPublisher {
        fn update_tracks(&self, _tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            Ok(DispatchResult::RejectedClosed)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    #[test]
    fn run_mixer_stops_when_publisher_closed() {
        let mut config = MediaProcessingModuleConfig::default();
        config.profile = "software".to_string();

        let inputs = vec![
            AudioMixInput {
                source: MediaKey::with_default_vhost("app", "src1", None).unwrap(),
                gain_db: Some(0),
            },
            AudioMixInput {
                source: MediaKey::with_default_vhost("app", "src2", None).unwrap(),
                gain_db: Some(0),
            },
        ];
        let output = AudioTarget {
            codec: AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };

        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        sender
            .send(MixInput::Frame {
                source: 0,
                frame: Arc::new(g711a_frame(vec![0u8; 80], 0)),
            })
            .unwrap();
        sender
            .send(MixInput::Frame {
                source: 1,
                frame: Arc::new(g711a_frame(vec![0u8; 80], 0)),
            })
            .unwrap();
        drop(sender);

        let mut publisher = RejectingPublisher;
        let result = run_mixer(
            &config,
            &inputs,
            &output,
            &[g711a_track(), g711a_track()],
            receiver,
            &mut publisher,
            &None,
        );

        assert!(result.is_err());
        assert!(format!("{result:?}").contains("publisher closed"));
    }

    struct DropAllPublisher;

    impl PublisherSink for DropAllPublisher {
        fn update_tracks(&self, _tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            Ok(DispatchResult::DroppedByPolicy)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    #[test]
    fn run_mixer_counts_dropped_by_policy_and_finishes() {
        let mut config = MediaProcessingModuleConfig::default();
        config.profile = "software".to_string();

        let inputs = vec![
            AudioMixInput {
                source: MediaKey::with_default_vhost("app", "src1", None).unwrap(),
                gain_db: Some(0),
            },
            AudioMixInput {
                source: MediaKey::with_default_vhost("app", "src2", None).unwrap(),
                gain_db: Some(0),
            },
        ];
        let output = AudioTarget {
            codec: AudioCodec::Aac,
            sample_rate: Some(8_000),
            channels: Some(1),
            bit_rate: Some(64_000),
        };

        let (sender, receiver) = std::sync::mpsc::sync_channel(16);
        sender
            .send(MixInput::Frame {
                source: 0,
                frame: Arc::new(g711a_frame(vec![0u8; 80], 0)),
            })
            .unwrap();
        sender
            .send(MixInput::Frame {
                source: 1,
                frame: Arc::new(g711a_frame(vec![0u8; 80], 0)),
            })
            .unwrap();
        drop(sender);

        let job = dummy_job();
        let mut publisher = DropAllPublisher;
        let result = run_mixer(
            &config,
            &inputs,
            &output,
            &[g711a_track(), g711a_track()],
            receiver,
            &mut publisher,
            &Some(job.clone()),
        );

        assert!(
            result.is_ok(),
            "run_mixer should finish when frames are dropped by policy"
        );
        let guard = job.lock().unwrap();
        assert_eq!(guard.frames_out, 0, "no frames should be accepted");
        assert!(guard.drops > 0, "dropped frames should be counted");
    }
}
