//! ABR ladder job worker.
//!
//! Subscribes to a single source stream once, then fans each incoming A/V frame
//! out to 1-4 independent transcoding variants. Each variant owns its own
//! encoder, publisher and blocking worker; the feeder task is responsible for
//! counting source frames exactly once and for routing frames to every variant
//! queue.
//!
//! Only compiled when `media-processing-cpu` is enabled so the per-variant
//! `TranscodeWorker` is available.

use std::sync::{Arc, Mutex};

use cheetah_codec::{frame::FrameFlags, track::MediaKind};
use cheetah_media_api::{
    ids::StreamKeyBridge,
    processing::{AbrVariant, ProcessingJob, TrackSelection},
};
use cheetah_sdk::{
    BackpressurePolicy, BootstrapPolicy, CancellationToken, EngineContext, JoinHandle, MediaFilter,
    PublishLease, PublisherSink, SdkError, StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::warn;

use crate::config::MediaProcessingModuleConfig;
use crate::provider::transcode::{
    finish_job, transcode_queue, update_progress, wait_for_source_tracks, TranscodeInput,
    TranscodeQueueSender, TranscodeWorker,
};

struct VariantContext {
    target: StreamKey,
    sender: TranscodeQueueSender,
    worker_error: Arc<Mutex<Option<String>>>,
    has_audio: bool,
}

/// Spawn an ABR ladder worker that publishes 1-4 derived renditions of `source`.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_abr_ladder_worker(
    engine: EngineContext,
    config: MediaProcessingModuleConfig,
    source: StreamKey,
    variants: Vec<AbrVariant>,
    publishers: Vec<(PublishLease, Box<dyn PublisherSink>)>,
    cancel: CancellationToken,
    job: Option<Arc<Mutex<ProcessingJob>>>,
) -> Result<(), SdkError> {
    if variants.is_empty() {
        return Err(SdkError::InvalidArgument(
            "ABR ladder requires at least one variant".into(),
        ));
    }
    if variants.len() > 4 {
        return Err(SdkError::InvalidArgument(
            "ABR ladder supports at most four variants".into(),
        ));
    }

    let finish_job_ref = job.clone();
    let publisher_api = engine.publisher_api.clone();

    // Split publishers so leases can be released even if sinks are consumed by
    // worker creation and the body exits early.
    let (leases, mut sinks): (Vec<PublishLease>, Vec<Box<dyn PublisherSink>>) =
        publishers.into_iter().unzip();
    let mut handles: Vec<Box<dyn JoinHandle>> = Vec::with_capacity(variants.len());
    let mut contexts: Vec<VariantContext> = Vec::with_capacity(variants.len());

    let result = async {
        let need_audio = variants.iter().any(|v| v.audio.is_some());
        let (source_video, source_audio) = wait_for_source_tracks(
            &engine,
            &source,
            &TrackSelection::All,
            true,
            need_audio,
            &cancel,
        )
        .await?;

        for variant in &variants {
            let (namespace, path) = StreamKeyBridge::to_namespace_path(&variant.target);
            let target = StreamKey::new(namespace, path);
            let publisher = sinks.remove(0);

            let worker = TranscodeWorker::new(
                &config,
                source_video.as_ref(),
                source_audio.as_ref(),
                Some(&variant.video),
                variant.audio.as_ref(),
                Arc::from(publisher),
                job.clone(),
                false,
            )
            .map_err(|e| SdkError::Internal(format!("create abr variant worker: {e}")))?;

            let (sender, receiver) = transcode_queue(64);
            let worker_error = Arc::new(Mutex::new(None));
            let worker_error_clone = worker_error.clone();

            let handle = engine
                .runtime_api
                .spawn_blocking(
                    "abr-variant-worker",
                    Box::new(move || {
                        let mut worker = worker;
                        let mut process_error: Option<String> = None;
                        while let Some(input) = receiver.recv() {
                            if let Err(e) = worker.process(input) {
                                warn!("abr variant worker stopping: {e}");
                                process_error = Some(format!("{e}"));
                                break;
                            }
                        }
                        if let Err(e) = worker.flush_and_close() {
                            warn!("abr variant worker flush/close failed: {e}");
                            process_error.get_or_insert_with(|| format!("{e}"));
                        }
                        if let Some(err) = process_error {
                            *worker_error_clone.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(err);
                        }
                    }),
                )
                .map_err(|e| SdkError::Internal(format!("spawn abr variant worker: {e}")))?;

            handles.push(handle);
            contexts.push(VariantContext {
                target,
                sender,
                worker_error,
                has_audio: variant.audio.is_some(),
            });
        }

        let media_filter = MediaFilter {
            enable_video: true,
            enable_audio: variants.iter().any(|v| v.audio.is_some()),
        };
        let subscriber_options = SubscriberOptions {
            queue_capacity: 256,
            backpressure: BackpressurePolicy::DropDroppableFirst,
            bootstrap_policy: BootstrapPolicy::default(),
            media_filter,
        };
        let mut subscriber = engine
            .subscriber_api
            .subscribe(source, subscriber_options)
            .await
            .map_err(|e| SdkError::Internal(format!("abr subscribe failed: {e}")))?;

        let mut subscriber_error: Option<SdkError> = None;
        'feed: loop {
            let cancel_fut = cancel.cancelled().fuse();
            let recv_fut = subscriber.recv().fuse();
            pin_mut!(cancel_fut, recv_fut);

            let frame = select_biased! {
                _ = cancel_fut => break,
                frame = recv_fut => frame,
            };

            match frame {
                Ok(Some(frame)) => {
                    let bytes_in = frame.payload.len() as u64;
                    let is_key = frame.media_kind == MediaKind::Video
                        && frame.flags.contains(FrameFlags::KEY);

                    let input = match frame.media_kind {
                        MediaKind::Video => TranscodeInput::Video(frame),
                        MediaKind::Audio => TranscodeInput::Audio(frame),
                        _ => continue,
                    };

                    update_progress(&job, |j| {
                        j.frames_in += 1;
                        j.bytes_in += bytes_in;
                    });

                    for ctx in &contexts {
                        if matches!(input, TranscodeInput::Audio(_)) && !ctx.has_audio {
                            continue;
                        }

                        if ctx
                            .worker_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .is_some()
                        {
                            subscriber_error = Some(SdkError::Internal(format!(
                                "abr variant for {} failed",
                                ctx.target
                            )));
                            break 'feed;
                        }

                        let queue_input = match &input {
                            TranscodeInput::Video(f) => TranscodeInput::Video(Arc::clone(f)),
                            TranscodeInput::Audio(f) => TranscodeInput::Audio(Arc::clone(f)),
                        };

                        match ctx.sender.try_send(queue_input) {
                            Ok(evicted) => {
                                if evicted > 0 {
                                    update_progress(&job, |j| j.drops += evicted as u64);
                                }
                            }
                            Err(_) => {
                                if is_key {
                                    warn!(
                                        "abr variant queue full; dropping keyframe for {}",
                                        ctx.target
                                    );
                                }
                                update_progress(&job, |j| j.drops += 1);
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("abr subscriber error: {e}");
                    subscriber_error = Some(SdkError::Internal(format!("subscriber error: {e}")));
                    break;
                }
            }
        }

        if let Some(err) = subscriber_error {
            return Err(err);
        }
        Ok(())
    }
    .await;

    // Cleanup runs regardless of whether the body returned early or succeeded.
    // Drop senders first so blocking workers wake up, then join them, then
    // release every publisher lease.
    // Join every started variant worker after dropping its sender so it can
    // flush and exit. Collect the first worker error for reporting.
    let mut worker_error: Option<String> = None;
    for (ctx, handle) in contexts.into_iter().zip(handles.into_iter()) {
        let VariantContext {
            target,
            sender,
            worker_error: we,
            has_audio: _,
        } = ctx;
        drop(sender);
        let join_result = handle.wait().await;
        if let Some(err) = we.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if worker_error.is_none() {
                worker_error = Some(format!("{target}: {err}"));
            }
        };
        if let Err(e) = join_result {
            worker_error.get_or_insert_with(|| format!("{target}: worker joined with error: {e}"));
        }
    }

    // Release every publisher lease, including those for variants that never
    // started, so a failed ladder never permanently reserves rendition streams.
    for lease in leases {
        let _ = publisher_api.release_publisher(&lease).await;
    }

    let final_result = match (result, worker_error) {
        (Err(e), _) => Err(e),
        (Ok(_), Some(err)) => Err(SdkError::Internal(err)),
        (Ok(_), None) => Ok(()),
    };

    finish_job(&finish_job_ref, final_result.as_ref().err());
    final_result
}
