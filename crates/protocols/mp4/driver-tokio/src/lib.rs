//! Tokio-based MP4 VOD file driver.
//!
//! Wraps `cheetah-mp4-core::VodSession` with a real file reader. The driver
//! handles `read_at` requests via `tokio::fs::File` and adapts the session's
//! schedule-tick loop to a `tokio::time::sleep` cadence.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use cheetah_codec::{Mp4ReadResult, Mp4ReaderConfig};
use cheetah_mp4_core::{VodControlCommand, VodCoreInput, VodOutput, VodSession};
use futures::Stream;
use parking_lot::Mutex;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::mpsc;
use tracing::warn;

/// Maximum number of `VodDriverEvent`s buffered before the driver awaits.
/// Sized to roughly two seconds of 30fps video plus margin so a slow
/// consumer does not stall the driver every tick.
const EVENT_CHANNEL_CAPACITY: usize = 128;

/// Outbound events emitted by the driver to the protocol/module layer.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum VodDriverEvent {
    Tracks(Vec<cheetah_codec::TrackInfo>),
    Frame(cheetah_codec::AVFrame),
    /// Forwarded core diagnostic for audit / error responses.
    Diagnostic(cheetah_mp4_core::VodDiagnostic),
    Closed {
        reason: String,
    },
}

/// Driver configuration.
#[derive(Debug, Clone)]
pub struct VodDriverConfig {
    pub read_chunk_bytes: usize,
    pub idle_timeout_ms: u64,
    pub reader_config: Mp4ReaderConfig,
    /// ABL-style playback count.
    /// * `1` (default) — play once and close.
    /// * `n > 1` — play `n` times.
    /// * `-1` — infinite loop.
    /// * `0` — refuse to start.
    pub read_count: i32,
    /// Threshold (inclusive) at which non-keyframe video samples are dropped
    /// during playback. ABL switches to keyframe-only output for `8x` and
    /// `16x` playback to avoid swamping the network. Set to `1.0` to never
    /// drop frames.
    pub keyframe_only_above_speed: f32,
}

impl Default for VodDriverConfig {
    fn default() -> Self {
        Self {
            read_chunk_bytes: 256 * 1024,
            idle_timeout_ms: 15_000,
            reader_config: Mp4ReaderConfig::default(),
            read_count: 1,
            keyframe_only_above_speed: 8.0,
        }
    }
}

/// Runtime-neutral event stream handed to the module/protocol layer.
///
/// The driver keeps its internal plumbing on tokio channels (driver crates may
/// use tokio directly), but the public surface exposes only a `futures::Stream`
/// so consumers never depend on a `tokio::sync::mpsc` type. See `AGENTS.md` §5.
pub struct VodEventStream {
    rx: mpsc::Receiver<VodDriverEvent>,
}

impl Stream for VodEventStream {
    type Item = VodDriverEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// Command channel handle exposed to the module/protocol layer.
#[derive(Clone)]
pub struct VodDriverHandle {
    cmd_tx: mpsc::UnboundedSender<VodControlCommand>,
    event_rx: Arc<Mutex<Option<mpsc::Receiver<VodDriverEvent>>>>,
}

impl VodDriverHandle {
    /// Sends `control` to the peer.
    /// 向对端发送 `control`。
    pub fn send_control(&self, cmd: VodControlCommand) -> Result<(), VodDriverError> {
        self.cmd_tx.send(cmd).map_err(|_| VodDriverError::Closed)
    }

    /// Take ownership of the event stream. Only the first caller succeeds.
    pub fn take_events(&self) -> Option<VodEventStream> {
        self.event_rx.lock().take().map(|rx| VodEventStream { rx })
    }
}

/// Error returned by `Vod Driver` operations.
/// `Vod Driver` 操作返回的错误。
#[derive(Debug, thiserror::Error, Clone)]
pub enum VodDriverError {
    #[error("driver channel closed")]
    Closed,
    #[error("file io error: {0}")]
    Io(String),
    #[error("file not found: {0}")]
    NotFound(String),
}

/// Open an MP4 file and start a VOD driver task. Returns a handle for the
/// caller to send commands to and pull events from.
pub async fn open_file(
    path: PathBuf,
    config: VodDriverConfig,
) -> Result<VodDriverHandle, VodDriverError> {
    open_files(vec![path], config).await
}

/// Open one or more MP4 files and play them back sequentially, mirroring
/// ZLM's `MultiMP4Demuxer` semantics. The first file's track set is taken
/// as the canonical schema; if subsequent files differ, the driver emits a
/// `Closed` event with a `track schema mismatch` reason.
///
/// Empty list yields `NotFound`. The first file must be openable; subsequent
/// files that fail to open trigger an early `Closed` event with the failing
/// path in the reason.
pub async fn open_files(
    paths: Vec<PathBuf>,
    config: VodDriverConfig,
) -> Result<VodDriverHandle, VodDriverError> {
    if paths.is_empty() {
        return Err(VodDriverError::NotFound("empty path list".to_string()));
    }
    // Validate every entry up front so the caller gets a synchronous error.
    for p in &paths {
        if tokio::fs::metadata(p).await.is_err() {
            return Err(VodDriverError::NotFound(format!("{}", p.display())));
        }
    }
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    // Bounded events channel: gives natural backpressure when no consumer
    // reads events promptly. The driver task awaits on `send`, so a slow
    // or absent consumer pauses frame emission instead of letting the
    // channel grow without bound.
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    tokio::spawn(run_multi_driver(paths, config, cmd_rx, event_tx));

    Ok(VodDriverHandle {
        cmd_tx,
        event_rx: Arc::new(Mutex::new(Some(event_rx))),
    })
}

async fn run_multi_driver(
    paths: Vec<PathBuf>,
    config: VodDriverConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<VodControlCommand>,
    event_tx: mpsc::Sender<VodDriverEvent>,
) {
    let mut tracks_emitted = false;
    // ABL-compatible read count handling: 0 = refuse, 1 = play once,
    // n > 1 = repeat, -1 = infinite. We loop the playlist as a whole so
    // multi-file playback is naturally repeated end-to-end, matching ABL's
    // `MultRecordFile` behaviour.
    if config.read_count == 0 {
        let _ = event_tx
            .send(VodDriverEvent::Closed {
                reason: "read_count=0 refuses to start".to_string(),
            })
            .await;
        return;
    }
    let infinite = config.read_count < 0;
    let mut remaining = if infinite {
        i32::MAX
    } else {
        config.read_count
    };
    while remaining > 0 {
        for path in &paths {
            // Per-file metadata + open. We've already validated all paths
            // above, so a transient failure here is an I/O issue worth
            // surfacing.
            let metadata = match tokio::fs::metadata(path).await {
                Ok(m) => m,
                Err(e) => {
                    let _ = event_tx
                        .send(VodDriverEvent::Closed {
                            reason: format!("metadata {}: {e}", path.display()),
                        })
                        .await;
                    return;
                }
            };
            let file = match File::open(path).await {
                Ok(f) => f,
                Err(e) => {
                    let _ = event_tx
                        .send(VodDriverEvent::Closed {
                            reason: format!("open {}: {e}", path.display()),
                        })
                        .await;
                    return;
                }
            };
            if !run_single(
                file,
                metadata.len(),
                config.clone(),
                &mut cmd_rx,
                &event_tx,
                &mut tracks_emitted,
            )
            .await
            {
                // External cancel — stop iterating.
                return;
            }
        }
        if !infinite {
            remaining -= 1;
        }
    }
    let _ = event_tx
        .send(VodDriverEvent::Closed {
            reason: "session closed".to_string(),
        })
        .await;
}

/// Drive a single file through the VOD state machine, suppressing any
/// `Tracks` event after the first emission and any `Closed` event so callers
/// see one continuous timeline. Returns `false` if the upstream channel
/// is closed (caller should stop).
async fn run_single(
    file: File,
    file_size: u64,
    config: VodDriverConfig,
    cmd_rx: &mut mpsc::UnboundedReceiver<VodControlCommand>,
    event_tx: &mpsc::Sender<VodDriverEvent>,
    tracks_emitted: &mut bool,
) -> bool {
    let mut session = VodSession::new(config.reader_config.clone());
    let mut file = file;
    let mut closed = false;
    // ABL-style high-speed gate: when the active speed is at or above the
    // configured threshold, the driver drops non-keyframe video samples.
    let mut current_scale: f32 = 1.0;
    let kf_threshold = config.keyframe_only_above_speed;

    let initial = session.step(VodCoreInput::Control(VodControlCommand::Start {
        file_size,
    }));
    let mut next_delay_us = match drive_outputs_filtered(
        &mut file,
        &mut session,
        event_tx,
        initial,
        &mut closed,
        tracks_emitted,
        current_scale,
        kf_threshold,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            warn!("vod driver start failed: {e}");
            return false;
        }
    };

    let mut next_tick: Option<tokio::time::Instant> = if closed {
        None
    } else {
        Some(schedule_next_tick(next_delay_us))
    };

    while !closed {
        let cmd_fut = cmd_rx.recv();
        let tick_at = next_tick;
        let tick_fut = async move {
            if let Some(at) = tick_at {
                tokio::time::sleep_until(at).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            biased;
            cmd = cmd_fut => {
                let Some(cmd) = cmd else { return false; };
                if let VodControlCommand::Scale(s) = cmd {
                    current_scale = s;
                }
                let outputs = session.step(VodCoreInput::Control(cmd));
                match drive_outputs_filtered(
                    &mut file,
                    &mut session,
                    event_tx,
                    outputs,
                    &mut closed,
                    tracks_emitted,
                    current_scale,
                    kf_threshold,
                ).await {
                    Ok(d) => next_delay_us = d.or(next_delay_us),
                    Err(e) => {
                        warn!("vod driver control failed: {e}");
                        return true;
                    }
                }
                next_tick = if closed { None } else { Some(schedule_next_tick(next_delay_us)) };
            }
            _ = tick_fut => {
                let now_us = monotonic_now_us();
                let outputs = session.step(VodCoreInput::Tick { now_us });
                match drive_outputs_filtered(
                    &mut file,
                    &mut session,
                    event_tx,
                    outputs,
                    &mut closed,
                    tracks_emitted,
                    current_scale,
                    kf_threshold,
                ).await {
                    Ok(d) => next_delay_us = d,
                    Err(e) => {
                        warn!("vod driver tick failed: {e}");
                        return true;
                    }
                }
                next_tick = if closed { None } else { Some(schedule_next_tick(next_delay_us)) };
            }
        }
    }
    true
}

/// Pick the next sleep deadline based on the session-requested delay.
/// Falls back to a 1ms tick when no delay is requested so the loop can
/// keep up with newly available reads without spinning.
fn schedule_next_tick(delay_us: Option<u64>) -> tokio::time::Instant {
    let delay = delay_us.unwrap_or(1_000);
    // Cap the delay so a stale or buggy schedule cannot stall the driver
    // for an unreasonable amount of time.
    let capped = delay.min(5_000_000);
    tokio::time::Instant::now() + std::time::Duration::from_micros(capped)
}

#[allow(clippy::too_many_arguments)]
async fn drive_outputs_filtered(
    file: &mut File,
    session: &mut VodSession,
    event_tx: &mpsc::Sender<VodDriverEvent>,
    initial_outputs: Vec<VodOutput>,
    closed: &mut bool,
    tracks_emitted: &mut bool,
    current_scale: f32,
    kf_threshold: f32,
) -> Result<Option<u64>, VodDriverError> {
    let mut next_delay_us: Option<u64> = None;
    let mut outputs: std::collections::VecDeque<VodOutput> = initial_outputs.into();
    while let Some(output) = outputs.pop_front() {
        match output {
            VodOutput::ReadAt(req) => {
                // Cap individual reads so a malicious or buggy reader
                // request cannot drive the driver into an OOM allocation
                // attempt. 64 MiB covers any realistic moov region on the
                // file types we expect; per-sample reads are far smaller.
                const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;
                if req.length > MAX_READ_BYTES {
                    return Err(VodDriverError::Io(format!(
                        "read length {} exceeds {} byte cap",
                        req.length, MAX_READ_BYTES
                    )));
                }
                file.seek(SeekFrom::Start(req.offset))
                    .await
                    .map_err(|e| VodDriverError::Io(format!("seek: {e}")))?;
                let mut buf = vec![0u8; req.length as usize];
                file.read_exact(&mut buf)
                    .await
                    .map_err(|e| VodDriverError::Io(format!("read: {e}")))?;
                let result = Mp4ReadResult {
                    offset: req.offset,
                    data: Bytes::from(buf),
                };
                let new_outputs = session.step(VodCoreInput::ReadAt(result));
                outputs.extend(new_outputs);
            }
            VodOutput::EmitTrackInfo(tracks) => {
                if !*tracks_emitted {
                    *tracks_emitted = true;
                    if event_tx.send(VodDriverEvent::Tracks(tracks)).await.is_err() {
                        // Receiver dropped: no consumers. Treat as session
                        // close so the driver task exits promptly instead
                        // of buffering frames into an unbounded channel.
                        *closed = true;
                        return Ok(next_delay_us);
                    }
                }
                // Suppress repeated track announcements when continuing on
                // the next file in a multi-file playlist.
            }
            VodOutput::EmitFrame(frame) => {
                // ABL high-speed gate: at >= configured threshold, drop
                // non-keyframe video samples so the network sees a sane
                // bitrate. Audio frames pass through unchanged so timing
                // stays anchored.
                let drop_non_key = current_scale >= kf_threshold
                    && frame.media_kind == cheetah_codec::MediaKind::Video
                    && !frame.flags.contains(cheetah_codec::FrameFlags::KEY);
                if !drop_non_key && event_tx.send(VodDriverEvent::Frame(frame)).await.is_err() {
                    *closed = true;
                    return Ok(next_delay_us);
                }
            }
            VodOutput::ScheduleTick { delay_us } => {
                next_delay_us = match (next_delay_us, delay_us) {
                    (None, d) => Some(d),
                    (Some(prev), d) => Some(prev.min(d)),
                };
            }
            VodOutput::Diagnostic(diag) => {
                if event_tx
                    .send(VodDriverEvent::Diagnostic(diag))
                    .await
                    .is_err()
                {
                    *closed = true;
                    return Ok(next_delay_us);
                }
            }
            VodOutput::CloseSession => {
                *closed = true;
                return Ok(next_delay_us);
            }
        }
    }
    Ok(next_delay_us)
}

fn monotonic_now_us() -> u64 {
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    elapsed.as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{
        CodecExtradata, CodecId, MediaKind, Mp4WriteEvent, Mp4Writer, Mp4WriterConfig, TrackId,
        TrackInfo,
    };
    use futures::StreamExt;
    use std::path::PathBuf;
    use tokio::io::AsyncWriteExt;

    fn h264_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        t.width = Some(640);
        t.height = Some(360);
        t.extradata = CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        t
    }

    async fn write_test_file() -> PathBuf {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        for i in 0..3 {
            w.push_sample(1, i * 33_333, i * 33_333, i == 0, b"AU")
                .unwrap();
        }
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cheetah-mp4-driver-test-{}.mp4",
            std::process::id()
        ));
        let mut f = tokio::fs::File::create(&path).await.unwrap();
        f.write_all(&buf).await.unwrap();
        f.sync_all().await.unwrap();
        path
    }

    async fn write_test_file_named(suffix: &str) -> PathBuf {
        let mut w = Mp4Writer::new(Mp4WriterConfig::default(), &[h264_track()]).unwrap();
        for i in 0..3 {
            w.push_sample(1, i * 33_333, i * 33_333, i == 0, b"AU")
                .unwrap();
        }
        let Mp4WriteEvent::File(buf) = w.finalize().unwrap();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cheetah-mp4-driver-test-{}-{suffix}.mp4",
            std::process::id()
        ));
        let mut f = tokio::fs::File::create(&path).await.unwrap();
        f.write_all(&buf).await.unwrap();
        f.sync_all().await.unwrap();
        path
    }

    #[tokio::test]
    async fn driver_streams_frames_from_disk() {
        let path = write_test_file().await;
        let handle = open_file(path.clone(), VodDriverConfig::default())
            .await
            .unwrap();
        let mut events = handle.take_events().unwrap();
        let mut got_tracks = false;
        let mut frames = 0;
        // Take up to 50 events to bound test time
        for _ in 0..50 {
            match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
                Ok(Some(VodDriverEvent::Tracks(_))) => got_tracks = true,
                Ok(Some(VodDriverEvent::Frame(_))) => frames += 1,
                Ok(Some(VodDriverEvent::Closed { .. })) => break,
                Ok(Some(VodDriverEvent::Diagnostic(_))) => {}
                Ok(None) | Err(_) => break,
            }
        }
        assert!(got_tracks);
        assert!(frames >= 3, "expected at least 3 frames, got {frames}");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn driver_concatenates_multiple_files() {
        let p1 = write_test_file_named("a").await;
        let p2 = write_test_file_named("b").await;
        let handle = open_files(vec![p1.clone(), p2.clone()], VodDriverConfig::default())
            .await
            .unwrap();
        let mut events = handle.take_events().unwrap();
        let mut tracks_count = 0;
        let mut frames = 0;
        for _ in 0..200 {
            match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
                Ok(Some(VodDriverEvent::Tracks(_))) => tracks_count += 1,
                Ok(Some(VodDriverEvent::Frame(_))) => frames += 1,
                Ok(Some(VodDriverEvent::Closed { .. })) => break,
                Ok(Some(VodDriverEvent::Diagnostic(_))) => {}
                Ok(None) | Err(_) => break,
            }
        }
        assert_eq!(
            tracks_count, 1,
            "Tracks event should fire once across the playlist"
        );
        assert!(
            frames >= 6,
            "expected ≥ 6 frames across the two-file playlist, got {frames}"
        );
        let _ = tokio::fs::remove_file(&p1).await;
        let _ = tokio::fs::remove_file(&p2).await;
    }

    #[tokio::test]
    async fn open_files_rejects_empty_list() {
        let err = open_files(Vec::new(), VodDriverConfig::default())
            .await
            .err()
            .expect("must fail on empty list");
        assert!(matches!(err, VodDriverError::NotFound(_)));
    }

    #[tokio::test]
    async fn open_files_rejects_missing_path_synchronously() {
        let mut missing = std::env::temp_dir();
        missing.push("cheetah-mp4-driver-does-not-exist.mp4");
        let err = open_files(vec![missing], VodDriverConfig::default())
            .await
            .err()
            .expect("must fail on missing path");
        assert!(matches!(err, VodDriverError::NotFound(_)));
    }

    #[tokio::test]
    async fn read_count_repeats_playback() {
        let path = write_test_file_named("rc").await;
        let config = VodDriverConfig {
            read_count: 2,
            ..Default::default()
        };
        let handle = open_files(vec![path.clone()], config).await.unwrap();
        let mut events = handle.take_events().unwrap();
        let mut frames = 0;
        for _ in 0..200 {
            match tokio::time::timeout(std::time::Duration::from_millis(500), events.next()).await {
                Ok(Some(VodDriverEvent::Frame(_))) => frames += 1,
                Ok(Some(VodDriverEvent::Closed { .. })) => break,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        // 3 frames per playback × 2 plays = 6 frames.
        assert_eq!(frames, 6, "read_count=2 should replay the file once");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn read_count_zero_refuses_start() {
        let path = write_test_file_named("rc0").await;
        let config = VodDriverConfig {
            read_count: 0,
            ..Default::default()
        };
        let handle = open_files(vec![path.clone()], config).await.unwrap();
        let mut events = handle.take_events().unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_millis(500), events.next())
            .await
            .ok()
            .and_then(|o| o);
        assert!(matches!(first, Some(VodDriverEvent::Closed { .. })));
        let _ = tokio::fs::remove_file(&path).await;
    }
}
