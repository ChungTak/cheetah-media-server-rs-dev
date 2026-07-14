use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cheetah_media_api::model::{
    FfmpegJobSpec, FfmpegResourceLimits, OutputPolicy, TranscodePolicy,
};
use cheetah_runtime_api::{oneshot_channel, OneShotReceiver, OneShotSender};
use cheetah_sdk::{CancellationToken, FfmpegApi, FfmpegJob, FfmpegJobOutcome, SdkError};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

const STDERR_PREVIEW_BYTES: usize = 1024;
const ALLOWED_SOURCE_SCHEMES: &[&str] = &["http", "https", "rtmp", "rtmps", "rtsp", "rtsps", "srt"];

/// In-memory ffmpeg job registry that spawns and monitors external FFmpeg processes.
///
/// 内存 ffmpeg 任务注册表，负责生成并监控外部 FFmpeg 进程。
pub struct EngineFfmpegService {
    jobs: Arc<DashMap<String, FfmpegProcess>>,
    binary_path: Option<String>,
}

impl Default for EngineFfmpegService {
    fn default() -> Self {
        Self {
            jobs: Arc::new(DashMap::new()),
            binary_path: None,
        }
    }
}

struct FfmpegProcess {
    job: FfmpegJob,
    cancel: CancellationToken,
    outcome: Arc<Mutex<Option<FfmpegJobOutcome>>>,
    signal_tx: Mutex<Option<OneShotSender>>,
    signal_rx: Mutex<Option<OneShotReceiver>>,
}

impl EngineFfmpegService {
    pub fn with_binary_path(binary_path: Option<String>) -> Self {
        Self {
            jobs: Arc::new(DashMap::new()),
            binary_path,
        }
    }

    fn resolve_binary(&self) -> Result<String, SdkError> {
        if let Some(path) = &self.binary_path {
            return Ok(path.clone());
        }
        Err(SdkError::Unavailable(
            "ffmpeg binary path not configured".to_string(),
        ))
    }

    fn validate_spec(spec: &FfmpegJobSpec) -> Result<(), SdkError> {
        if spec.source_url.is_empty() {
            return Err(SdkError::InvalidArgument(
                "ffmpeg source URL is empty".to_string(),
            ));
        }
        if !spec.source_url.contains("://") {
            return Err(SdkError::InvalidArgument(
                "ffmpeg source URL is missing scheme".to_string(),
            ));
        }
        if let Some(scheme) = spec.source_url.split("://").next() {
            let scheme = scheme.to_lowercase();
            if !ALLOWED_SOURCE_SCHEMES.contains(&scheme.as_str()) {
                return Err(SdkError::InvalidArgument(format!(
                    "ffmpeg source URL scheme '{scheme}' is not allowed"
                )));
            }
        }
        if spec.source_url.starts_with('-') {
            return Err(SdkError::InvalidArgument(
                "ffmpeg source URL looks like an option".to_string(),
            ));
        }
        if spec.timeout_ms == 0 {
            return Err(SdkError::InvalidArgument(
                "ffmpeg timeout must be > 0".to_string(),
            ));
        }
        if !spec.enable_audio && !spec.enable_video {
            return Err(SdkError::InvalidArgument(
                "at least one of audio or video must be enabled".to_string(),
            ));
        }
        if spec.output_policy != OutputPolicy::None {
            return Err(SdkError::InvalidArgument(
                "non-null output_policy requires MediaUrlResolverApi (S4-T6)".to_string(),
            ));
        }
        Ok(())
    }

    fn build_args(spec: &FfmpegJobSpec) -> Vec<String> {
        let mut args = vec!["-y".to_string()];

        args.push("-i".to_string());
        args.push(spec.source_url.clone());

        let TranscodePolicy {
            disable_video,
            disable_audio,
            out_width,
            out_height,
            g711_to_aac,
            h264_decode_encode,
        } = &spec.transcode_policy;

        let video_enabled = spec.enable_video && !disable_video;
        let audio_enabled = spec.enable_audio && !disable_audio;

        if !video_enabled {
            args.push("-vn".to_string());
        } else {
            if *h264_decode_encode {
                args.push("-vcodec".to_string());
                args.push("libx264".to_string());
            }
            if out_width.is_some() || out_height.is_some() {
                let w = out_width.map_or_else(|| "-1".to_string(), |v| v.to_string());
                let h = out_height.map_or_else(|| "-1".to_string(), |v| v.to_string());
                args.push("-vf".to_string());
                args.push(format!("scale={w}:{h}"));
            }
        }
        if !audio_enabled {
            args.push("-an".to_string());
        } else if *g711_to_aac {
            args.push("-acodec".to_string());
            args.push("aac".to_string());
        }

        if spec.output_policy == OutputPolicy::None {
            args.push("-f".to_string());
            args.push("null".to_string());
            args.push("-".to_string());
        }

        args
    }
}

#[async_trait]
impl FfmpegApi for EngineFfmpegService {
    async fn submit_job(&self, job: FfmpegJob) -> Result<(), SdkError> {
        Self::validate_spec(&job.spec)?;

        if self.jobs.contains_key(&job.job_id) {
            return Err(SdkError::AlreadyExists(format!(
                "ffmpeg job {}",
                job.job_id
            )));
        }

        let binary = self.resolve_binary()?;
        let args = Self::build_args(&job.spec);

        let mut cmd = Command::new(&binary);
        cmd.args(&args).stdout(Stdio::null()).stderr(Stdio::piped());

        #[cfg(unix)]
        apply_resource_limits(&mut cmd, &job.spec.resource_limits, job.spec.timeout_ms);

        let mut child = cmd
            .spawn()
            .map_err(|e| SdkError::Unavailable(format!("failed to spawn ffmpeg process: {e}")))?;

        let cancel = CancellationToken::new();
        let outcome = Arc::new(Mutex::new(None));
        let (tx, rx) = oneshot_channel();

        let process = FfmpegProcess {
            job: job.clone(),
            cancel: cancel.clone(),
            outcome: outcome.clone(),
            signal_tx: Mutex::new(Some(tx)),
            signal_rx: Mutex::new(Some(rx)),
        };

        let cancel_for_task = cancel.clone();
        let entry = self.jobs.entry(job.job_id.clone());
        match entry {
            Entry::Occupied(_) => {
                let _ = child.kill().await;
                return Err(SdkError::AlreadyExists(format!(
                    "ffmpeg job {}",
                    job.job_id
                )));
            }
            Entry::Vacant(v) => {
                v.insert(process);
            }
        }

        let job_id = job.job_id.clone();
        let jobs = self.jobs.clone();
        tokio::spawn(async move {
            let result = run_child(child, cancel_for_task, job.spec.timeout_ms).await;
            if let Some(proc) = jobs.get(&job_id) {
                *proc.value().outcome.lock().unwrap() = Some(result.clone());
                if let Some(tx) = proc.value().signal_tx.lock().unwrap().take() {
                    let _ = tx.send();
                }
            }
        });

        Ok(())
    }

    async fn cancel_job(&self, job_id: &str) -> Result<(), SdkError> {
        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))?;
        entry.value().cancel.cancel();
        Ok(())
    }

    async fn wait_job(&self, job_id: &str) -> Result<FfmpegJobOutcome, SdkError> {
        let rx = {
            let entry = self
                .jobs
                .get(job_id)
                .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))?;
            if let Some(outcome) = entry.value().outcome.lock().unwrap().clone() {
                return Ok(outcome);
            }
            let maybe_rx = entry.value().signal_rx.lock().unwrap().take();
            maybe_rx.ok_or_else(|| SdkError::Conflict("ffmpeg job already awaited".to_string()))?
        };

        let mut rx = rx;
        let _ = rx.recv().await;

        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))?;
        let maybe_outcome = entry.value().outcome.lock().unwrap().clone();
        maybe_outcome.ok_or_else(|| SdkError::Internal("ffmpeg job outcome missing".to_string()))
    }

    fn list_jobs(&self) -> Vec<FfmpegJob> {
        let mut out: Vec<_> = self
            .jobs
            .iter()
            .map(|entry| entry.value().job.clone())
            .collect();
        out.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        out
    }
}

async fn run_child(
    mut child: Child,
    cancel: CancellationToken,
    timeout_ms: u64,
) -> FfmpegJobOutcome {
    let duration = Duration::from_millis(timeout_ms.max(1));

    tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            FfmpegJobOutcome::Cancelled
        }
        res = tokio::time::timeout(duration, child.wait()) => {
            match res {
                Ok(Ok(status)) => {
                    let code = status.code().unwrap_or(-1);
                    let stderr = read_stderr_preview(&mut child).await;
                    if code == 0 {
                        FfmpegJobOutcome::Succeeded
                    } else {
                        FfmpegJobOutcome::Failed(format!("ffmpeg exited with code {code}: {stderr}"))
                    }
                }
                Ok(Err(e)) => FfmpegJobOutcome::Failed(format!("failed to wait for ffmpeg: {e}")),
                Err(_) => {
                    let _ = child.kill().await;
                    FfmpegJobOutcome::Timeout
                }
            }
        }
    }
}

async fn read_stderr_preview(child: &mut Child) -> String {
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };

    let mut buf = [0u8; STDERR_PREVIEW_BYTES];
    let read_fut = stderr.read(&mut buf);
    match tokio::time::timeout(Duration::from_secs(1), read_fut).await {
        Ok(Ok(n)) if n > 0 => String::from_utf8_lossy(&buf[..n]).into_owned(),
        _ => String::new(),
    }
}

#[cfg(unix)]
fn apply_resource_limits(cmd: &mut Command, limits: &FfmpegResourceLimits, timeout_ms: u64) {
    let memory = limits.max_memory_bytes;
    let cpu_percent = limits.max_cpu_percent;
    if memory.is_none() && cpu_percent.is_none() {
        return;
    }
    unsafe {
        cmd.pre_exec(move || set_rlimits(memory, cpu_percent, timeout_ms));
    }
}

#[cfg(unix)]
fn set_rlimits(
    memory: Option<u64>,
    cpu_percent: Option<u32>,
    timeout_ms: u64,
) -> std::io::Result<()> {
    if let Some(bytes) = memory {
        let limit = libc::rlimit {
            rlim_cur: bytes,
            rlim_max: bytes,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    if let Some(percent) = cpu_percent {
        let timeout_s = timeout_ms / 1000;
        let cpu_s = (timeout_s * percent as u64) / 100;
        let cpu_s = cpu_s.max(1);
        let limit = libc::rlimit {
            rlim_cur: cpu_s,
            rlim_max: cpu_s,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use cheetah_media_api::ids::MediaKey;
    use cheetah_media_api::model::{
        FfmpegJobSpec, FfmpegResourceLimits, OutputPolicy, TranscodePolicy,
    };

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn fake_ffmpeg_bin(script: &str) -> String {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("cheetah-fake-ffmpeg-{n}"));
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn fake_spec(source_url: &str, timeout_ms: u64) -> FfmpegJobSpec {
        FfmpegJobSpec {
            source_url: source_url.to_string(),
            destination: MediaKey::with_default_vhost("live", "stream", None).unwrap(),
            transcode_policy: TranscodePolicy::default(),
            output_policy: OutputPolicy::None,
            timeout_ms,
            resource_limits: FfmpegResourceLimits::default(),
            enable_audio: true,
            enable_video: true,
        }
    }

    fn fake_job(id: &str, source_url: &str, timeout_ms: u64) -> FfmpegJob {
        FfmpegJob {
            job_id: id.to_string(),
            proxy_id: id.to_string(),
            spec: fake_spec(source_url, timeout_ms),
        }
    }

    #[tokio::test]
    async fn submit_and_wait_success() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nexit 0\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j1", "http://example/source", 5000))
            .await
            .unwrap();
        let outcome = svc.wait_job("j1").await.unwrap();
        assert_eq!(outcome, FfmpegJobOutcome::Succeeded);
    }

    #[tokio::test]
    async fn submit_and_wait_failed_reads_stderr() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\necho 'bad input' >&2\nexit 1\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j2", "http://example/source", 5000))
            .await
            .unwrap();
        let outcome = svc.wait_job("j2").await.unwrap();
        match outcome {
            FfmpegJobOutcome::Failed(m) => {
                assert!(m.contains("code 1"));
                assert!(m.contains("bad input"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_and_wait_timeout() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nsleep 100\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j3", "http://example/source", 50))
            .await
            .unwrap();
        let outcome = svc.wait_job("j3").await.unwrap();
        assert_eq!(outcome, FfmpegJobOutcome::Timeout);
    }

    #[tokio::test]
    async fn cancel_job_maps_to_cancelled() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nsleep 100\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j4", "http://example/source", 5000))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        svc.cancel_job("j4").await.unwrap();
        let outcome = svc.wait_job("j4").await.unwrap();
        assert_eq!(outcome, FfmpegJobOutcome::Cancelled);
    }

    #[tokio::test]
    async fn wait_job_is_idempotent() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nexit 0\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j5", "http://example/source", 5000))
            .await
            .unwrap();
        let first = svc.wait_job("j5").await.unwrap();
        let second = svc.wait_job("j5").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first, FfmpegJobOutcome::Succeeded);
    }

    #[tokio::test]
    async fn list_jobs_includes_submitted_job() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nexit 0\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j6", "http://example/source", 5000))
            .await
            .unwrap();
        svc.wait_job("j6").await.unwrap();
        let jobs = svc.list_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "j6");
    }

    #[tokio::test]
    async fn rejects_missing_scheme() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let mut spec = fake_spec("not-a-url", 5000);
        spec.output_policy = OutputPolicy::None;
        let err = svc
            .submit_job(FfmpegJob {
                job_id: "j7".to_string(),
                proxy_id: "j7".to_string(),
                spec,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_option_injection_source_url() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let err = svc
            .submit_job(fake_job("j8", "-i http://example", 5000))
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_both_audio_and_video_disabled() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let mut spec = fake_spec("http://example", 5000);
        spec.enable_audio = false;
        spec.enable_video = false;
        let err = svc
            .submit_job(FfmpegJob {
                job_id: "j9".to_string(),
                proxy_id: "j9".to_string(),
                spec,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_non_null_output_policy() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let mut spec = fake_spec("http://example", 5000);
        spec.output_policy = OutputPolicy::Mp4;
        let err = svc
            .submit_job(FfmpegJob {
                job_id: "j10".to_string(),
                proxy_id: "j10".to_string(),
                spec,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn duplicate_job_id_is_rejected_and_original_untouched() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nexit 0\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        svc.submit_job(fake_job("j11", "http://example", 5000))
            .await
            .unwrap();
        let err = svc
            .submit_job(fake_job("j11", "http://example", 5000))
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::AlreadyExists(_)));
        let outcome = svc.wait_job("j11").await.unwrap();
        assert_eq!(outcome, FfmpegJobOutcome::Succeeded);
    }

    #[tokio::test]
    async fn missing_binary_path_returns_unavailable() {
        let svc = EngineFfmpegService::with_binary_path(None);
        let err = svc
            .submit_job(fake_job("j12", "http://example", 5000))
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::Unavailable(_)));
    }

    #[tokio::test]
    async fn rejects_file_scheme() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let err = svc
            .submit_job(fake_job("j13", "file:///etc/passwd", 5000))
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_data_scheme() {
        let svc = EngineFfmpegService::with_binary_path(Some("/bin/true".to_string()));
        let err = svc
            .submit_job(fake_job("j14", "data://text/plain,foo", 5000))
            .await
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidArgument(_)));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn memory_resource_limit_is_enforced() {
        let bin = fake_ffmpeg_bin("#!/bin/sh\nexit 0\n");
        let svc = EngineFfmpegService::with_binary_path(Some(bin));
        let mut spec = fake_spec("http://example", 1000);
        spec.resource_limits.max_memory_bytes = Some(1);
        svc.submit_job(FfmpegJob {
            job_id: "j15".to_string(),
            proxy_id: "j15".to_string(),
            spec,
        })
        .await
        .unwrap();
        let outcome = svc.wait_job("j15").await.unwrap();
        assert!(
            !matches!(outcome, FfmpegJobOutcome::Succeeded),
            "expected job to fail under memory limit, got {outcome:?}"
        );
    }

    #[test]
    fn build_args_skips_video_codec_when_video_disabled() {
        let mut spec = fake_spec("http://example", 5000);
        spec.enable_video = false;
        spec.transcode_policy.h264_decode_encode = true;
        spec.transcode_policy.out_width = Some(1280);
        let args = EngineFfmpegService::build_args(&spec);
        assert!(args.contains(&"-vn".to_string()));
        assert!(!args.contains(&"-vcodec".to_string()));
        assert!(!args.contains(&"-vf".to_string()));
    }

    #[test]
    fn build_args_skips_audio_codec_when_audio_disabled() {
        let mut spec = fake_spec("http://example", 5000);
        spec.enable_audio = false;
        spec.transcode_policy.g711_to_aac = true;
        let args = EngineFfmpegService::build_args(&spec);
        assert!(args.contains(&"-an".to_string()));
        assert!(!args.contains(&"-acodec".to_string()));
    }
}
