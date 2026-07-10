//! Real `TaskExecutor` implementation that subscribes to engine streams
//! and drives a per-format `RecordContainerWriter` to disk.
//!
//! V1 ships MP4 only (single-file finalize-on-stop). FLV / HLS / PS writers
//! are wired to the same dispatch loop and can be enabled as their
//! container writers stabilise.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::record::{
    make_default_writer, mp4 as mp4_record, RecordContainerWriter, RecordDiagnostic, RecordFormat,
    RecordWriteEvent,
};
use cheetah_codec::TrackInfo;
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, EngineContext, JoinHandle, StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

use crate::config::RecordModuleConfig;
use crate::metadata::{RecordFileMetadata, RecordFormatStr, RecordTaskState, RecordTrackSummary};
use crate::registry::RecordRegistry;
use crate::task::{RecordTask, TaskExecutor, TaskExecutorError};

/// Live-recording executor.
///
/// Spawns one async task per `RecordTask` via `RuntimeApi::spawn`. Each task
/// subscribes to the engine source stream, drives a `RecordContainerWriter`,
/// and finalizes the output on cancel/EOS.
pub struct RecordExecutor {
    /// `engine` field of type `EngineContext`.
    /// `engine` 字段，类型为 `EngineContext`.
    engine: EngineContext,
    /// `config` field of type `RecordModuleConfig`.
    /// `config` 字段，类型为 `RecordModuleConfig`.
    config: RecordModuleConfig,
    /// `registry` field.
    /// `registry` 字段.
    registry: Arc<RecordRegistry>,
    /// `handles` field.
    /// `处理` 字段.
    handles: Mutex<HashMap<String, TaskHandle>>,
}

struct TaskHandle {
    cancel: CancellationToken,
    join: Option<Box<dyn JoinHandle>>,
}

impl RecordExecutor {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(
        engine: EngineContext,
        config: RecordModuleConfig,
        registry: Arc<RecordRegistry>,
    ) -> Self {
        Self {
            engine,
            config,
            registry,
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Cancel all running tasks. Used during module stop.
    pub async fn shutdown(&self) {
        let snapshot: Vec<TaskHandle> = {
            let mut map = self.handles.lock();
            map.drain().map(|(_, v)| v).collect()
        };
        for handle in snapshot {
            handle.cancel.cancel();
            if let Some(join) = handle.join {
                let _ = join.wait().await;
            }
        }
    }
}

#[async_trait]
impl TaskExecutor for RecordExecutor {
    async fn spawn(&self, task: RecordTask) -> Result<(), TaskExecutorError> {
        let task_id = task.task_id.clone();

        // Hold the executor lock for the entire spawn so the duplicate
        // check, the `runtime_api.spawn` call, and the handle insert happen
        // atomically. `RuntimeApi::spawn` is synchronous (returns a join
        // handle without awaiting) so this is safe.
        let mut handles = self.handles.lock();
        if handles.contains_key(&task_id) {
            return Err(TaskExecutorError::SpawnFailed(format!(
                "task already running: {task_id}"
            )));
        }

        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let engine = self.engine.clone();
        let config = self.config.clone();
        let registry = self.registry.clone();

        let join = self.engine.runtime_api.spawn(Box::pin(async move {
            run_record_task(task, engine, config, registry, task_cancel).await;
        }));

        handles.insert(
            task_id,
            TaskHandle {
                cancel,
                join: Some(join),
            },
        );
        Ok(())
    }

    async fn stop(&self, task_id: &str) -> Result<(), TaskExecutorError> {
        let handle = {
            let mut handles = self.handles.lock();
            handles.remove(task_id)
        };
        let Some(mut handle) = handle else {
            return Err(TaskExecutorError::NotFound(task_id.to_string()));
        };
        handle.cancel.cancel();
        if let Some(join) = handle.join.take() {
            let _ = join.wait().await;
        }
        Ok(())
    }
}

async fn run_record_task(
    task: RecordTask,
    engine: EngineContext,
    config: RecordModuleConfig,
    registry: Arc<RecordRegistry>,
    cancel: CancellationToken,
) {
    let task_id = task.task_id.clone();
    let format = task.template.format;

    // Resolve stream key from source_stream_key. SMS/cheetah convention is
    // `app/stream`; we accept either form for safety.
    let stream_key = parse_stream_key(&task.template.source_stream_key, &task.template);

    // Stamp the start time once and reuse it for the on-disk path, the file
    // id, and the registry metadata. Calling `wall_clock_ms()` twice when
    // building the path is unsafe across a midnight tick.
    let start_ms = wall_clock_ms();

    // Build the output file path now so that an early failure surfaces
    // before we open a subscriber.
    let path = build_output_path(&config, &task, format, start_ms);
    if let Some(parent) = path.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            warn!(%task_id, ?path, %err, "record: failed to create output dir");
            mark_failed(&registry, &task_id);
            return;
        }
    }

    // Wait for the source stream to appear (publisher may not be ready yet).
    let snapshot = match wait_for_stream(&engine, &stream_key, &cancel).await {
        Some(s) => s,
        None => {
            warn!(%task_id, %stream_key, "record: cancelled before source stream became available");
            mark_failed(&registry, &task_id);
            return;
        }
    };

    let mut subscriber = match engine
        .subscriber_api
        .subscribe(stream_key.clone(), subscriber_options(&config))
        .await
    {
        Ok(s) => s,
        Err(err) => {
            warn!(%task_id, %stream_key, %err, "record: subscribe failed");
            mark_failed(&registry, &task_id);
            return;
        }
    };

    let mut writer: Box<dyn RecordContainerWriter> = match format {
        RecordFormat::Mp4 => Box::new(mp4_record::Mp4FileWriter::new(
            mp4_record::Mp4FileWriterConfig {
                faststart: false,
                drop_below_bytes: 0,
            },
        )),
        other => make_default_writer(other),
    };
    if let Err(err) = writer.update_tracks(&snapshot.tracks) {
        warn!(%task_id, ?format, %err, "record: writer rejected initial tracks");
        let _ = subscriber.close().await;
        mark_failed(&registry, &task_id);
        return;
    }

    let mut frames_written: u64 = 0;
    let mut current_tracks = snapshot.tracks.clone();
    let mut last_track_check_frames = 0usize;
    let mut intermediate_events: Vec<RecordWriteEvent> = Vec::new();

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        let next = select_biased! {
            _ = cancel_fut => break,
            recv = recv_fut => recv,
        };

        match next {
            Ok(Some(frame)) => {
                // Periodically check for track-info updates so writers see
                // late-arriving config (e.g., AAC ASC after first audio frame).
                last_track_check_frames += 1;
                if frame.is_key_frame() || last_track_check_frames >= 60 {
                    last_track_check_frames = 0;
                    if let Ok(Some(latest)) =
                        engine.stream_manager_api.get_stream(&stream_key).await
                    {
                        if latest.tracks != current_tracks
                            && writer.update_tracks(&latest.tracks).is_ok()
                        {
                            current_tracks = latest.tracks;
                        }
                    }
                }

                match writer.push_frame(frame.as_ref()) {
                    Ok(events) => {
                        if !events.is_empty() {
                            for ev in events {
                                stage_event(ev, &mut intermediate_events, &task_id);
                            }
                        }
                        frames_written += 1;
                    }
                    Err(err) => {
                        warn!(%task_id, %err, "record: writer push_frame failed");
                        break;
                    }
                }
            }
            Ok(None) => break,
            Err(err) => {
                warn!(%task_id, %err, "record: subscriber recv failed");
                break;
            }
        }
    }

    // Finalize and flush whatever the writer produced.
    let mut pending_events = intermediate_events;
    match writer.finalize() {
        Ok(events) => pending_events.extend(events),
        Err(err) => {
            warn!(%task_id, %err, "record: writer finalize failed");
        }
    }
    let _ = subscriber.close().await;

    let mut bytes_written: u64 = 0;
    let mut dropped_tiny = false;
    let mut file_opened = false;
    let mut had_io_error = false;

    for ev in pending_events {
        match ev {
            RecordWriteEvent::Bytes(buf) => {
                if !file_opened {
                    match File::create(&path).await {
                        Ok(mut f) => match f.write_all(&buf).await {
                            Ok(()) => {
                                if let Err(err) = f.flush().await {
                                    warn!(%task_id, ?path, %err, "record: flush failed");
                                }
                                bytes_written += buf.len() as u64;
                                file_opened = true;
                            }
                            Err(err) => {
                                warn!(%task_id, ?path, %err, "record: write_all failed");
                                let _ = tokio::fs::remove_file(&path).await;
                                had_io_error = true;
                            }
                        },
                        Err(err) => {
                            warn!(%task_id, ?path, %err, "record: File::create failed");
                            had_io_error = true;
                        }
                    }
                } else {
                    match tokio::fs::OpenOptions::new().append(true).open(&path).await {
                        Ok(mut f) => {
                            if let Err(err) = f.write_all(&buf).await {
                                warn!(%task_id, ?path, %err, "record: append failed");
                                had_io_error = true;
                            } else {
                                bytes_written += buf.len() as u64;
                            }
                        }
                        Err(err) => {
                            warn!(%task_id, ?path, %err, "record: append open failed");
                            had_io_error = true;
                        }
                    }
                }
            }
            RecordWriteEvent::Segment { .. }
            | RecordWriteEvent::InitSegment { .. }
            | RecordWriteEvent::Playlist { .. } => {
                // Segmented formats (HLS) are not finalized in V1 of the
                // record-to-disk path; the writer still emits diagnostics
                // we surface below.
                debug!(%task_id, "record: segmented event ignored in V1");
            }
            RecordWriteEvent::Diagnostic(diag) => {
                if let RecordDiagnostic::DropTinyFile { .. } = diag {
                    dropped_tiny = true;
                }
                debug!(%task_id, ?diag, "record: writer diagnostic");
            }
        }
    }

    if dropped_tiny {
        let _ = tokio::fs::remove_file(&path).await;
        info!(%task_id, "record: file dropped below size threshold");
        mark_stopped(&registry, &task_id);
        return;
    }
    if !file_opened || bytes_written == 0 {
        // Either nothing usable was produced or an I/O error fired before
        // any bytes landed on disk. Surface as Failed so callers can tell
        // the difference from a graceful empty-stop.
        let _ = tokio::fs::remove_file(&path).await;
        if had_io_error {
            warn!(%task_id, %frames_written, "record: task failed due to I/O error");
            mark_failed(&registry, &task_id);
        } else {
            info!(%task_id, %frames_written, "record: no bytes produced");
            mark_stopped(&registry, &task_id);
        }
        return;
    }

    let end_ms = wall_clock_ms();
    let file_meta = RecordFileMetadata {
        file_id: format!("{}-{}", task_id, start_ms),
        task_id: task_id.clone(),
        format: RecordFormatStr::from(format),
        path: path.to_string_lossy().to_string(),
        duration_ms: end_ms.saturating_sub(start_ms) as u64,
        size_bytes: bytes_written,
        start_time_ms: start_ms,
        end_time_ms: end_ms,
        track_summary: current_tracks.iter().map(track_summary_from_info).collect(),
    };
    if let Err(err) = registry.insert_file(file_meta) {
        warn!(%task_id, %err, "record: insert_file failed");
    }
    info!(%task_id, ?path, %bytes_written, %frames_written, "record: task finished");
    mark_stopped(&registry, &task_id);
}

fn stage_event(ev: RecordWriteEvent, staged: &mut Vec<RecordWriteEvent>, task_id: &str) {
    match ev {
        // Bytes / segment events produced mid-stream are flushed alongside
        // any final-state events after the subscribe loop exits, so the disk
        // I/O stays in one place. For MP4 the writer only emits the final
        // file in `finalize()`, so this is exercised by HLS and similar
        // segmented formats.
        RecordWriteEvent::Bytes(_)
        | RecordWriteEvent::Segment { .. }
        | RecordWriteEvent::InitSegment { .. }
        | RecordWriteEvent::Playlist { .. } => staged.push(ev),
        RecordWriteEvent::Diagnostic(diag) => {
            debug!(%task_id, ?diag, "record: writer diagnostic (mid)");
        }
    }
}

fn subscriber_options(config: &RecordModuleConfig) -> SubscriberOptions {
    // Recording is a tail subscriber: take the full GOP from the bootstrap
    // window so the file always begins on a keyframe, and keep some
    // headroom on top so live frames do not immediately collide with the
    // bootstrap allocation.
    let bootstrap = config.queue_capacity.max(64);
    let queue_capacity = bootstrap.saturating_add(bootstrap / 2).max(bootstrap + 64);
    SubscriberOptions {
        queue_capacity,
        bootstrap_policy: BootstrapPolicy::full_gop(bootstrap, None),
        ..Default::default()
    }
}

fn parse_stream_key(source: &str, template: &crate::task::RecordTaskTemplate) -> StreamKey {
    if let Some((ns, path)) = source.split_once('/') {
        StreamKey::new(ns, path)
    } else {
        StreamKey::new(template.app.as_str(), template.stream.as_str())
    }
}

async fn wait_for_stream(
    engine: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
) -> Option<cheetah_sdk::StreamSnapshot> {
    use cheetah_codec::MonoTime;
    let runtime = engine.runtime_api.clone();
    let mut backoff_ms: u64 = 50;
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        if let Ok(Some(snapshot)) = engine.stream_manager_api.get_stream(stream_key).await {
            if !snapshot.tracks.is_empty() {
                return Some(snapshot);
            }
        }
        let now = runtime.now();
        let deadline = MonoTime::from_micros(now.as_micros().saturating_add(backoff_ms * 1_000));
        let mut timer = runtime.sleep_until(deadline);
        let cancel_fut = cancel.cancelled().fuse();
        let timer_fut = timer.wait().fuse();
        pin_mut!(cancel_fut, timer_fut);
        select_biased! {
            _ = cancel_fut => return None,
            _ = timer_fut => {}
        }
        backoff_ms = (backoff_ms * 2).min(500);
    }
}

fn build_output_path(
    config: &RecordModuleConfig,
    task: &RecordTask,
    format: RecordFormat,
    timestamp_ms: i64,
) -> PathBuf {
    let mut path = PathBuf::from(&config.root_path);
    let date = format_ymd(timestamp_ms);
    path.push(&task.template.app);
    path.push(&task.template.stream);
    path.push(date);
    let name = format!(
        "{}-{}.{}",
        sanitize_segment(&task.task_id),
        timestamp_ms,
        format.extension()
    );
    path.push(name);
    path
}

fn sanitize_segment(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '/' | '\\' | '.' => '_',
            _ => c,
        })
        .collect()
}

fn wall_clock_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn format_ymd(ms: i64) -> String {
    let secs = (ms / 1_000).max(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Inverse of Howard Hinnant's `days_from_civil` (mirrors the helper used
/// in `zlm_compat`).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn track_summary_from_info(t: &TrackInfo) -> RecordTrackSummary {
    RecordTrackSummary {
        kind: format!("{:?}", t.media_kind).to_lowercase(),
        codec: format!("{:?}", t.codec).to_lowercase(),
    }
}

fn mark_failed(registry: &RecordRegistry, task_id: &str) {
    let _ = registry.update_task_state(task_id, RecordTaskState::Failed);
}

fn mark_stopped(registry: &RecordRegistry, task_id: &str) {
    let _ = registry.update_task_state(task_id, RecordTaskState::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_round_trips_against_known_dates() {
        // 2026-05-23 is days since epoch = 20596
        let (y, m, d) = civil_from_days(20596);
        assert_eq!((y, m, d), (2026, 5, 23));
    }

    #[test]
    fn parse_stream_key_splits_namespace() {
        let tpl = crate::task::RecordTaskTemplate {
            format: RecordFormat::Mp4,
            app: "live".into(),
            stream: "abc".into(),
            source_stream_key: "live/abc".into(),
            duration_limit_ms: 0,
            segment_duration_ms: 0,
            segment_count_limit: 0,
        };
        let key = parse_stream_key("live/abc", &tpl);
        assert_eq!(key.namespace, "live");
        assert_eq!(key.path, "abc");
    }

    #[test]
    fn parse_stream_key_falls_back_to_template_when_no_slash() {
        let tpl = crate::task::RecordTaskTemplate {
            format: RecordFormat::Mp4,
            app: "live".into(),
            stream: "abc".into(),
            source_stream_key: "abc".into(),
            duration_limit_ms: 0,
            segment_duration_ms: 0,
            segment_count_limit: 0,
        };
        let key = parse_stream_key("abc", &tpl);
        assert_eq!(key.namespace, "live");
        assert_eq!(key.path, "abc");
    }

    #[test]
    fn sanitize_segment_replaces_path_separators() {
        assert_eq!(sanitize_segment("a/b/c"), "a_b_c");
        assert_eq!(sanitize_segment("a.mp4"), "a_mp4");
    }

    #[test]
    fn build_output_path_uses_supplied_timestamp_for_both_dir_and_filename() {
        // Reusing the same `start_ms` for the YYYY-MM-DD dir and for the
        // filename ensures the two cannot disagree across a midnight tick.
        let cfg = RecordModuleConfig {
            root_path: "/tmp/rec".into(),
            ..RecordModuleConfig::default()
        };
        let task = RecordTask {
            task_id: "rec-live-stream/x.mp4".into(),
            template: crate::task::RecordTaskTemplate {
                format: RecordFormat::Mp4,
                app: "live".into(),
                stream: "stream".into(),
                source_stream_key: "live/stream".into(),
                duration_limit_ms: 0,
                segment_duration_ms: 0,
                segment_count_limit: 0,
            },
        };
        let path = build_output_path(&cfg, &task, RecordFormat::Mp4, 1_779_500_000_000);
        let s = path.to_string_lossy();
        // Date directory is derived from the supplied ms.
        assert!(s.contains("/2026-05-23/"), "missing date dir: {s}");
        // Filename embeds the same ms.
        assert!(s.ends_with("-1779500000000.mp4"), "filename mismatch: {s}");
        // Sanitized task id (no '/' or '.') appears in the filename.
        assert!(
            s.contains("rec-live-stream_x_mp4"),
            "sanitize mismatch: {s}"
        );
    }

    #[test]
    fn subscriber_options_keeps_headroom_above_bootstrap_window() {
        let mut cfg = RecordModuleConfig::default();
        cfg.queue_capacity = 256;
        let opts = subscriber_options(&cfg);
        // Bootstrap window respects `queue_capacity`.
        assert_eq!(opts.bootstrap_policy.max_bootstrap_frames, 256);
        // Queue holds the bootstrap GOP plus headroom for live frames.
        assert!(
            opts.queue_capacity > opts.bootstrap_policy.max_bootstrap_frames,
            "subscriber queue must exceed bootstrap window, got q={} bs={}",
            opts.queue_capacity,
            opts.bootstrap_policy.max_bootstrap_frames
        );
    }
}
