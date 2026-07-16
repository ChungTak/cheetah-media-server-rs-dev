use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_sdk::{
    FfmpegApi, FfmpegInput, FfmpegJobHandle, FfmpegJobSpec, FfmpegJobState, FfmpegJobStatus,
    FfmpegOutput, SdkError,
};
use dashmap::DashMap;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, watch, Semaphore};

/// A configured FFmpeg profile.
///
/// Profiles are controlled by the engine configuration; callers cannot supply an
/// arbitrary executable path through the public `FfmpegApi`.
#[derive(Debug, Clone)]
pub struct FfmpegProfile {
    pub executable: PathBuf,
}

impl Default for FfmpegProfile {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("ffmpeg"),
        }
    }
}

#[derive(Clone)]
struct JobEntry {
    status: Arc<watch::Sender<FfmpegJobStatus>>,
    cancel: Arc<watch::Sender<bool>>,
    // Keep a receiver alive so a cancellation sent before the worker has
    // subscribed is not lost and the sender is never closed while queued.
    #[allow(dead_code)]
    cancel_rx: watch::Receiver<bool>,
}

/// In-process FFmpeg executor.
///
/// Spawns real `ffmpeg` child processes without a shell, enforces runtime limits,
/// and tracks job lifecycle through `FfmpegApi`.
#[derive(Clone)]
pub struct LocalFfmpegService {
    jobs: DashMap<String, JobEntry>,
    profiles: HashMap<String, FfmpegProfile>,
    semaphore: Arc<Semaphore>,
}

impl Default for LocalFfmpegService {
    fn default() -> Self {
        Self::with_executable("ffmpeg")
    }
}

impl LocalFfmpegService {
    /// Create a service that uses `ffmpeg` from `$PATH` with a default profile.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a service with a custom default profile executable.
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            FfmpegProfile {
                executable: executable.into(),
            },
        );
        Self {
            jobs: DashMap::new(),
            profiles,
            semaphore: Arc::new(Semaphore::new(8)),
        }
    }

    /// Set the maximum number of concurrently running FFmpeg jobs.
    pub fn with_max_concurrent_jobs(mut self, n: usize) -> Self {
        self.semaphore = Arc::new(Semaphore::new(n.max(1)));
        self
    }

    /// Register an additional profile.
    pub fn with_profile(mut self, id: impl Into<String>, profile: FfmpegProfile) -> Self {
        self.profiles.insert(id.into(), profile);
        self
    }

    fn current_status(&self, job_id: &str) -> Option<FfmpegJobStatus> {
        self.jobs
            .get(job_id)
            .map(|entry| entry.value().status.borrow().clone())
    }
}

#[async_trait]
impl FfmpegApi for LocalFfmpegService {
    async fn submit(
        &self,
        job_id: String,
        spec: FfmpegJobSpec,
    ) -> Result<FfmpegJobHandle, SdkError> {
        // Allow re-submission of the same job id only once the previous run has
        // reached a terminal state. This supports proxy retries and delete/recreate
        // flows that reuse the id derived from the proxy id.
        if let Some(entry) = self.jobs.get(&job_id) {
            if entry.value().status.borrow().state.is_terminal() {
                drop(entry);
                self.jobs.remove(&job_id);
            } else {
                return Err(SdkError::AlreadyExists(format!("ffmpeg job {job_id}")));
            }
        }

        let profile = self
            .profiles
            .get(&spec.profile_id)
            .cloned()
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg profile {}", spec.profile_id)))?;

        let created_at = now_ms();
        let status = FfmpegJobStatus {
            job_id: job_id.clone(),
            state: FfmpegJobState::Pending,
            created_at,
            started_at: None,
            finished_at: None,
            exit_code: None,
            exit_summary: String::new(),
            pid: None,
        };

        let (status_tx, _status_rx) = watch::channel(status.clone());
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let status = Arc::new(status_tx);
        let cancel = Arc::new(cancel_tx);

        let handle_status = status.borrow().clone();
        let task_status = status.clone();
        let task_cancel = cancel.clone();

        let entry = JobEntry {
            status,
            cancel,
            cancel_rx,
        };
        self.jobs.insert(job_id.clone(), entry);

        let semaphore = self.semaphore.clone();
        let max_stderr_lines = spec.resource_limits.max_stderr_lines;
        let max_runtime_ms = spec.resource_limits.max_runtime_ms;

        tokio::spawn(run_ffmpeg_job(
            job_id.clone(),
            spec,
            profile.executable,
            semaphore,
            task_status,
            task_cancel,
            max_stderr_lines,
            max_runtime_ms,
        ));

        Ok(FfmpegJobHandle {
            job_id,
            status: handle_status,
        })
    }

    async fn get(&self, job_id: &str) -> Result<FfmpegJobStatus, SdkError> {
        self.current_status(job_id)
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))
    }

    async fn list(&self) -> Vec<FfmpegJobStatus> {
        self.jobs
            .iter()
            .map(|entry| entry.value().status.borrow().clone())
            .collect()
    }

    async fn wait(&self, job_id: &str) -> Result<FfmpegJobStatus, SdkError> {
        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))?;
        let mut rx = entry.value().status.subscribe();
        drop(entry);

        loop {
            {
                let status = rx.borrow_and_update().clone();
                if status.state.is_terminal() {
                    return Ok(status);
                }
            }
            rx.changed()
                .await
                .map_err(|_| SdkError::Internal("ffmpeg job status channel closed".into()))?;
        }
    }

    async fn cancel(&self, job_id: &str) -> Result<(), SdkError> {
        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))?;
        if entry.value().status.borrow().state.is_terminal() {
            return Ok(());
        }
        let _ = entry.value().cancel.send(true);
        Ok(())
    }

    async fn remove(&self, job_id: &str) -> Result<(), SdkError> {
        self.jobs
            .remove(job_id)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_ffmpeg_job(
    _job_id: String,
    spec: FfmpegJobSpec,
    executable: PathBuf,
    semaphore: Arc<Semaphore>,
    status: Arc<watch::Sender<FfmpegJobStatus>>,
    cancel: Arc<watch::Sender<bool>>,
    max_stderr_lines: usize,
    max_runtime_ms: u64,
) {
    // Update to Pending (the initial state is already Pending, but keep the channel fresh).
    let _ = status.send_if_modified(|s| {
        if s.state != FfmpegJobState::Pending {
            s.state = FfmpegJobState::Pending;
            true
        } else {
            false
        }
    });

    // Race the semaphore against an early cancellation so a queued job can be
    // cancelled promptly instead of waiting for a free slot.
    let mut cancel_rx = cancel.subscribe();
    if *cancel_rx.borrow_and_update() {
        finish_job(
            &status,
            FfmpegJobState::Cancelled,
            None,
            "cancelled".to_string(),
        );
        return;
    }

    let _permit = tokio::select! {
        _ = cancel_rx.changed() => {
            finish_job(&status, FfmpegJobState::Cancelled, None, "cancelled".to_string());
            return;
        }
        result = semaphore.acquire() => match result {
            Ok(permit) => permit,
            Err(_) => {
                finish_job(
                    &status,
                    FfmpegJobState::Failed,
                    None,
                    "semaphore closed".to_string(),
                );
                return;
            }
        },
    };

    // Re-subscribe after acquiring the permit; the old `changed()` future was cancelled.
    let mut cancel_rx = cancel.subscribe();
    if *cancel_rx.borrow_and_update() {
        finish_job(
            &status,
            FfmpegJobState::Cancelled,
            None,
            "cancelled".to_string(),
        );
        return;
    }

    let input_url = match spec.input {
        FfmpegInput::Url { url } => url,
    };

    let output_url = match spec.output {
        FfmpegOutput::Url { url } => url,
        FfmpegOutput::Engine { media_key } => format!(
            "engine://{}/{}/{}",
            media_key.vhost.0, media_key.app.0, media_key.stream.0
        ),
    };

    let mut cmd = Command::new(&executable);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd.args(&spec.input_options);
    cmd.arg("-i").arg(&input_url);
    cmd.args(&spec.output_options);
    cmd.arg(&output_url);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let summary = format!("failed to spawn ffmpeg: {err}");
            finish_job(&status, FfmpegJobState::Failed, None, summary);
            return;
        }
    };

    let started_at = now_ms();
    let pid = child.id();
    status.send_modify(|s| {
        s.state = FfmpegJobState::Running;
        s.started_at = Some(started_at);
        s.pid = pid;
    });

    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            finish_job(
                &status,
                FfmpegJobState::Failed,
                None,
                "failed to capture stderr".to_string(),
            );
            return;
        }
    };

    let (stderr_tx, stderr_rx) = oneshot::channel::<Vec<String>>();
    tokio::spawn(collect_stderr(stderr, max_stderr_lines, stderr_tx));

    let mut cancel_rx = cancel.subscribe();
    let initial_cancelled = *cancel_rx.borrow_and_update();

    let timeout = Duration::from_millis(max_runtime_ms);

    let (exit_code, cancelled) = if initial_cancelled {
        let _ = child.kill().await;
        let _ = child.wait().await;
        (None, true)
    } else {
        tokio::select! {
            _ = cancel_rx.changed() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                (None, true)
            }
            result = tokio::time::timeout(timeout, child.wait()) => {
                match result {
                    Ok(Ok(exit)) => (exit.code(), false),
                    Ok(Err(err)) => {
                        finish_job(&status, FfmpegJobState::Failed, None, format!("wait error: {err}"));
                        let _ = wait_stderr(stderr_rx).await;
                        status.send_modify(|s| s.finished_at = Some(now_ms()));
                        return;
                    }
                    Err(_) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        (None, false)
                    }
                }
            }
        }
    };

    // Reap the child in case timeout/cancel left it in an intermediate state.
    let exit_code = if exit_code.is_none() && !cancelled {
        // Timeout path already killed and waited; code is intentionally None.
        None
    } else {
        exit_code
    };

    let stderr_lines = wait_stderr(stderr_rx).await;
    let summary = build_exit_summary(exit_code, cancelled, &stderr_lines);
    let state = if cancelled {
        FfmpegJobState::Cancelled
    } else if exit_code == Some(0) {
        FfmpegJobState::Exited
    } else {
        FfmpegJobState::Failed
    };

    finish_job(&status, state, exit_code, summary);
}

async fn wait_stderr(rx: oneshot::Receiver<Vec<String>>) -> Vec<String> {
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(lines)) => lines,
        _ => {
            // Stderr reader did not finish in time; fall back to an empty buffer.
            Vec::new()
        }
    }
}

async fn collect_stderr<R>(reader: R, max_lines: usize, tx: oneshot::Sender<Vec<String>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader).lines();
    let mut ring = VecDeque::with_capacity(max_lines);
    while let Ok(Some(line)) = reader.next_line().await {
        if max_lines == 0 {
            continue;
        }
        if ring.len() >= max_lines {
            ring.pop_front();
        }
        ring.push_back(line);
    }
    let lines: Vec<String> = ring.into_iter().collect();
    let _ = tx.send(lines);
}

fn build_exit_summary(exit_code: Option<i32>, cancelled: bool, stderr_lines: &[String]) -> String {
    let mut parts = Vec::new();
    if cancelled {
        parts.push("cancelled".to_string());
    } else if exit_code.is_none() {
        parts.push("timeout".to_string());
    } else {
        parts.push(format!("exit_code={}", exit_code.unwrap()));
    }
    for line in stderr_lines.iter().rev().take(20) {
        parts.push(line.clone());
    }
    parts.join(" | ")
}

fn finish_job(
    status: &watch::Sender<FfmpegJobStatus>,
    state: FfmpegJobState,
    exit_code: Option<i32>,
    summary: String,
) {
    status.send_modify(|s| {
        s.state = state;
        s.exit_code = exit_code;
        s.exit_summary = summary;
        s.finished_at = Some(now_ms());
    });
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    use cheetah_sdk::FfmpegResourceLimits;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    fn script(content: &[u8]) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "cheetah_ffmpeg_test_{}_{}.sh",
            now_ms(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let mut file = std::fs::File::create(&path).expect("create temp script");
        file.write_all(content).expect("write temp script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata().unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[tokio::test]
    async fn ffmpeg_job_exits_successfully() {
        let path = script(b"#!/bin/sh\nexit 0\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let handle = service
            .submit(
                "job-ok".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Exited);
        assert_eq!(status.exit_code, Some(0));
    }

    #[tokio::test]
    async fn ffmpeg_job_exits_nonzero() {
        let path = script(b"#!/bin/sh\necho 'something broke' >&2\nexit 1\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let handle = service
            .submit(
                "job-fail".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Failed);
        assert_eq!(status.exit_code, Some(1));
        assert!(status.exit_summary.contains("something broke"));
    }

    #[tokio::test]
    async fn ffmpeg_job_times_out() {
        let path = script(b"#!/bin/sh\nwhile true; do sleep 1; done\n");
        let service = LocalFfmpegService::with_executable(path.clone()).with_max_concurrent_jobs(1);
        let handle = service
            .submit(
                "job-timeout".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 100,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Failed);
        assert!(status.exit_summary.contains("timeout"));
    }

    #[tokio::test]
    async fn ffmpeg_job_can_be_cancelled() {
        let path = script(b"#!/bin/sh\nwhile true; do sleep 1; done\n");
        let service = LocalFfmpegService::with_executable(path.clone()).with_max_concurrent_jobs(1);
        let handle = service
            .submit(
                "job-cancel".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 60_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Give the process a moment to start before cancelling.
        tokio::time::sleep(Duration::from_millis(50)).await;
        service.cancel(&handle.job_id).await.unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Cancelled);
    }

    #[tokio::test]
    async fn missing_executable_fails_to_spawn() {
        let service = LocalFfmpegService::with_executable("/no/such/ffmpeg/binary");
        let handle = service
            .submit(
                "job-missing".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Failed);
        assert!(status.exit_summary.contains("failed to spawn"));
    }

    #[tokio::test]
    async fn stderr_ring_buffer_truncates() {
        let path =
            script(b"#!/bin/sh\nfor i in $(seq 1 100); do echo \"line $i\" >&2; done\nexit 1\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let handle = service
            .submit(
                "job-stderr".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_stderr_lines: 10,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let status = service.wait(&handle.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Failed);
        assert!(
            status.exit_summary.contains("line 100"),
            "summary: {}",
            status.exit_summary
        );
        assert!(
            !status.exit_summary.contains("line 1 |"),
            "summary: {}",
            status.exit_summary
        );
        assert!(
            !status.exit_summary.contains("line 90"),
            "summary: {}",
            status.exit_summary
        );
    }

    #[tokio::test]
    async fn concurrency_limit_is_enforced() {
        let path = script(b"#!/bin/sh\nsleep 2\n");
        let service = LocalFfmpegService::with_executable(path.clone()).with_max_concurrent_jobs(1);

        let h1 = service
            .submit(
                "job-c1".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 5_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Allow the first job to acquire the only permit.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let h2 = service
            .submit(
                "job-c2".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 5_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let s1 = service.get(&h1.job_id).await.unwrap();
        let s2 = service.get(&h2.job_id).await.unwrap();
        assert_eq!(s1.state, FfmpegJobState::Running);
        assert_eq!(s2.state, FfmpegJobState::Pending);

        // Clean up by cancelling both.
        let _ = service.cancel(&h1.job_id).await;
        let _ = service.cancel(&h2.job_id).await;
    }

    #[tokio::test]
    async fn submit_rejects_duplicate_job_id() {
        let path = script(b"#!/bin/sh\nwhile true; do sleep 1; done\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let spec = FfmpegJobSpec {
            input: FfmpegInput::Url { url: "in".into() },
            output: FfmpegOutput::Url { url: "out".into() },
            resource_limits: FfmpegResourceLimits {
                max_runtime_ms: 5_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let _ = service.submit("dup".into(), spec.clone()).await.unwrap();
        let err = service.submit("dup".into(), spec).await.unwrap_err();
        assert!(matches!(err, SdkError::AlreadyExists(_)));
        let _ = service.cancel("dup").await;
    }

    #[tokio::test]
    async fn resubmit_after_terminal_reuses_job_id() {
        let path = script(b"#!/bin/sh\nexit 0\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let spec = FfmpegJobSpec {
            input: FfmpegInput::Url { url: "in".into() },
            output: FfmpegOutput::Url { url: "out".into() },
            ..Default::default()
        };
        let h1 = service.submit("reused".into(), spec.clone()).await.unwrap();
        let s1 = service.wait(&h1.job_id).await.unwrap();
        assert_eq!(s1.state, FfmpegJobState::Exited);

        let h2 = service.submit("reused".into(), spec).await.unwrap();
        let s2 = service.wait(&h2.job_id).await.unwrap();
        assert_eq!(s2.state, FfmpegJobState::Exited);
        assert!(s2.created_at >= s1.created_at);
    }

    #[tokio::test]
    async fn remove_releases_job_id_and_status() {
        let path = script(b"#!/bin/sh\nexit 0\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let spec = FfmpegJobSpec {
            input: FfmpegInput::Url { url: "in".into() },
            output: FfmpegOutput::Url { url: "out".into() },
            ..Default::default()
        };
        let handle = service.submit("gone".into(), spec.clone()).await.unwrap();
        let _ = service.wait(&handle.job_id).await.unwrap();
        service.remove(&handle.job_id).await.unwrap();

        let err = service.get(&handle.job_id).await.unwrap_err();
        assert!(matches!(err, SdkError::NotFound(_)));

        // The same id can be reused after removal.
        let handle2 = service.submit("gone".into(), spec).await.unwrap();
        let s2 = service.wait(&handle2.job_id).await.unwrap();
        assert_eq!(s2.state, FfmpegJobState::Exited);
    }

    #[tokio::test]
    async fn pending_job_cancels_without_waiting_for_slot() {
        let slow = script(b"#!/bin/sh\nsleep 5\n");
        let service = LocalFfmpegService::with_executable(slow).with_max_concurrent_jobs(1);

        let _ = service
            .submit(
                "first".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 10_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Give the first job time to acquire the only permit.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let pending = service
            .submit(
                "pending-cancel".into(),
                FfmpegJobSpec {
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    resource_limits: FfmpegResourceLimits {
                        max_runtime_ms: 10_000,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let start = Instant::now();
        service.cancel(&pending.job_id).await.unwrap();
        let status = service.wait(&pending.job_id).await.unwrap();
        assert_eq!(status.state, FfmpegJobState::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn unknown_profile_is_rejected() {
        let path = script(b"#!/bin/sh\nexit 0\n");
        let service = LocalFfmpegService::with_executable(path.clone());
        let err = service
            .submit(
                "job-bad-profile".into(),
                FfmpegJobSpec {
                    profile_id: "unknown".into(),
                    input: FfmpegInput::Url { url: "in".into() },
                    output: FfmpegOutput::Url { url: "out".into() },
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::NotFound(_)));
    }
}
