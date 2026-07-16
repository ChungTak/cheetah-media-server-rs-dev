//! Proxy session background tasks: connect, bridge frames, retry, cancel.
//!
//! 代理会话后台任务：连接、帧桥接、重试与取消。

use std::pin::Pin;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_media_api::event::{EventHeader, MediaEvent, ProxyStateChanged};
use cheetah_media_api::ids::{MediaKey, ProxyId};
use cheetah_media_api::model::ProxyState;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_sdk::{EngineContext, FfmpegJob, TaskId, TaskKind, TaskOutcome};
use futures::future::{select, Either};
use futures::Future;
#[cfg(any(feature = "rtsp", feature = "http-flv", feature = "rtmp"))]
use futures::FutureExt;
use tracing::{debug, error, warn};
use url::Url;

use crate::config::ProxyModuleConfig;
use crate::registry::ProxyRegistry;

/// Specification of the data-plane work a proxy session should perform.
///
/// 代理会话应执行的数据面工作规格。
#[derive(Debug, Clone)]
pub enum ProxySessionSpec {
    Pull {
        source_url: String,
        destination: MediaKey,
    },
    Push {
        source_media_key: MediaKey,
        destination_url: String,
        protocol: String,
    },
    Ffmpeg {
        source_url: String,
        destination: MediaKey,
        input_options: Vec<String>,
        output_options: Vec<String>,
        job_id: String,
    },
}

/// Spawn a background proxy task and return its cancellation token.
///
/// 派生后台代理任务并返回其取消 token。
pub fn spawn_proxy_task(
    ctx: EngineContext,
    registry: Arc<ProxyRegistry>,
    proxy_id: ProxyId,
    config: ProxyModuleConfig,
    spec: ProxySessionSpec,
) -> Result<CancellationToken, cheetah_sdk::SdkError> {
    let runtime_api = ctx.runtime_api.clone();
    let task_system_api = ctx.task_system_api.clone();
    let task_id = task_system_api.create_task(None, TaskKind::Task, "proxy", "proxy-session")?;
    let cancel = task_system_api.token(task_id)?;
    let cancel_for_task = cancel.clone();

    let fut = Box::pin(proxy_session_loop(
        ctx,
        registry,
        proxy_id,
        config,
        spec,
        cancel_for_task,
        task_id,
    ));
    runtime_api.spawn(fut);
    Ok(cancel)
}

async fn proxy_session_loop(
    ctx: EngineContext,
    registry: Arc<ProxyRegistry>,
    proxy_id: ProxyId,
    config: ProxyModuleConfig,
    spec: ProxySessionSpec,
    cancel: CancellationToken,
    task_id: TaskId,
) {
    debug!(proxy_id = %proxy_id.0, "proxy session started");

    let mut attempt: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            transition_to_stopped(&ctx, &registry, &proxy_id, task_id, None);
            return;
        }

        registry.update_state(&proxy_id, ProxyState::Connecting);
        publish_state(&ctx, &proxy_id, ProxyState::Connecting, None);

        match run_once(&ctx, &registry, &proxy_id, &spec, &cancel, &config).await {
            RunOnceOutcome::Stopped => {
                transition_to_stopped(&ctx, &registry, &proxy_id, task_id, None);
                return;
            }
            RunOnceOutcome::Failed(err) => {
                attempt = attempt.saturating_add(1);
                registry.update_error(&proxy_id, Some(err.clone()));
                registry.bump_retry(&proxy_id);

                if attempt > config.retry_max {
                    warn!(proxy_id = %proxy_id.0, attempt, "proxy exhausted retries: {err}");
                    registry.update_state(&proxy_id, ProxyState::Failed);
                    publish_state(&ctx, &proxy_id, ProxyState::Failed, Some(err.clone()));
                    transition_to_stopped(&ctx, &registry, &proxy_id, task_id, Some(err));
                    return;
                }

                registry.update_state(&proxy_id, ProxyState::Reconnecting);
                publish_state(&ctx, &proxy_id, ProxyState::Reconnecting, Some(err.clone()));

                let delay = retry_delay_ms(&config, attempt);
                debug!(proxy_id = %proxy_id.0, attempt, delay, "proxy retrying after failure: {err}");
                if wait_or_cancel(&ctx.runtime_api, delay, &cancel).await {
                    transition_to_stopped(&ctx, &registry, &proxy_id, task_id, None);
                    return;
                }
            }
        }
    }
}

enum RunOnceOutcome {
    Stopped,
    Failed(String),
}

async fn run_once(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    spec: &ProxySessionSpec,
    cancel: &CancellationToken,
    config: &ProxyModuleConfig,
) -> RunOnceOutcome {
    match spec {
        ProxySessionSpec::Pull {
            source_url,
            destination,
        } => {
            run_pull(
                ctx,
                registry,
                proxy_id,
                source_url,
                destination,
                cancel,
                config,
            )
            .await
        }
        ProxySessionSpec::Push {
            source_media_key,
            destination_url,
            protocol,
        } => {
            run_push(
                ctx,
                registry,
                proxy_id,
                source_media_key,
                destination_url,
                protocol,
                cancel,
                config,
            )
            .await
        }
        ProxySessionSpec::Ffmpeg {
            source_url,
            destination,
            input_options,
            output_options,
            job_id,
        } => {
            run_ffmpeg(
                ctx,
                registry,
                proxy_id,
                source_url,
                destination,
                input_options,
                output_options,
                job_id,
                cancel,
            )
            .await
        }
    }
}

async fn run_pull(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    destination: &MediaKey,
    cancel: &CancellationToken,
    config: &ProxyModuleConfig,
) -> RunOnceOutcome {
    let scheme = match Url::parse(source_url) {
        Ok(u) => u.scheme().to_ascii_lowercase(),
        Err(e) => return RunOnceOutcome::Failed(format!("invalid source url: {e}")),
    };

    let connect_timeout = config.connect_timeout_ms;

    match scheme.as_str() {
        "rtsp" => {
            run_pull_rtsp(
                ctx,
                registry,
                proxy_id,
                source_url,
                destination,
                cancel,
                connect_timeout,
            )
            .await
        }
        "http" | "https" => {
            run_pull_http_flv(
                ctx,
                registry,
                proxy_id,
                source_url,
                destination,
                cancel,
                connect_timeout,
            )
            .await
        }
        other => RunOnceOutcome::Failed(format!(
            "pull proxy scheme '{other}' is not supported by this build (enable rtsp/http-flv features)"
        )),
    }
}

#[cfg(feature = "rtsp")]
async fn run_pull_rtsp(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    destination: &MediaKey,
    cancel: &CancellationToken,
    connect_timeout_ms: u64,
) -> RunOnceOutcome {
    use cheetah_connector::{open_rtsp_pull_to_stream, ConnectorPullOptions};
    use cheetah_media_api::ids::StreamKeyBridge;
    use cheetah_sdk::StreamKey;
    use tracing::info;

    let (ns, path) = StreamKeyBridge::to_namespace_path(destination);
    let target = StreamKey::new(ns, path);
    let options = ConnectorPullOptions {
        cancel: Some(cancel.child_token()),
        ..Default::default()
    };

    let open = open_rtsp_pull_to_stream(
        ctx.runtime_api.clone(),
        ctx.publisher_api.clone(),
        ctx.stream_manager_api.clone(),
        source_url,
        target,
        options,
    );
    let handle = match with_timeout(&ctx.runtime_api, connect_timeout_ms, open, cancel).await {
        TimeoutResult::Ok(Ok(h)) => h,
        TimeoutResult::Ok(Err(e)) => return RunOnceOutcome::Failed(e.to_string()),
        TimeoutResult::TimedOut => {
            return RunOnceOutcome::Failed("rtsp pull connect timeout".into())
        }
        TimeoutResult::Cancelled => return RunOnceOutcome::Stopped,
    };

    registry.update_state(proxy_id, ProxyState::Connected);
    registry.update_error(proxy_id, None);
    publish_state(ctx, proxy_id, ProxyState::Connected, None);
    info!(proxy_id = %proxy_id.0, "rtsp pull proxy connected");

    hold_pull_handle(handle, cancel).await
}

#[cfg(not(feature = "rtsp"))]
async fn run_pull_rtsp(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_url: &str,
    _destination: &MediaKey,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("rtsp pull requires cheetah-proxy-module feature `rtsp`".into())
}

#[cfg(feature = "http-flv")]
async fn run_pull_http_flv(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    destination: &MediaKey,
    cancel: &CancellationToken,
    connect_timeout_ms: u64,
) -> RunOnceOutcome {
    use cheetah_connector::{open_http_flv_pull_with_runtime, ConnectorPullOptions};
    use cheetah_media_api::command::PublishRequest;
    use cheetah_media_api::port::MediaRequestContext;
    use tracing::info;

    let options = ConnectorPullOptions {
        cancel: Some(cancel.child_token()),
        ..Default::default()
    };

    let open = open_http_flv_pull_with_runtime(ctx.runtime_api.clone(), source_url, options);
    let mut pull = match with_timeout(&ctx.runtime_api, connect_timeout_ms, open, cancel).await {
        TimeoutResult::Ok(Ok(h)) => h,
        TimeoutResult::Ok(Err(e)) => return RunOnceOutcome::Failed(e.to_string()),
        TimeoutResult::TimedOut => {
            return RunOnceOutcome::Failed("http-flv pull connect timeout".into())
        }
        TimeoutResult::Cancelled => return RunOnceOutcome::Stopped,
    };

    let media_ctx = MediaRequestContext {
        source_adapter: "proxy".to_string(),
        ..MediaRequestContext::default()
    };
    let publisher = match ctx
        .media_data_plane
        .open_frame_publisher(
            &media_ctx,
            PublishRequest {
                media_key: destination.clone(),
                protocol: "proxy-http-flv".to_string(),
                origin: Some(source_url.to_string()),
                remote_endpoint: None,
                lease_token: None,
                auth_context: Default::default(),
                metadata: Default::default(),
            },
        )
        .await
    {
        Ok(p) => p,
        Err(e) => return RunOnceOutcome::Failed(format!("acquire destination publisher: {e}")),
    };

    registry.update_state(proxy_id, ProxyState::Connected);
    registry.update_error(proxy_id, None);
    publish_state(ctx, proxy_id, ProxyState::Connected, None);
    info!(proxy_id = %proxy_id.0, "http-flv pull proxy connected");

    let mut tracks_announced = false;
    loop {
        if cancel.is_cancelled() {
            let _ = publisher.close().await;
            let _ = pull.close().await;
            return RunOnceOutcome::Stopped;
        }

        match pull.recv().await {
            Ok(Some(frame)) => {
                if !tracks_announced {
                    let tracks = pull.tracks();
                    if !tracks.is_empty() {
                        if let Err(e) = publisher.update_tracks(tracks) {
                            let _ = publisher.close().await;
                            let _ = pull.close().await;
                            return RunOnceOutcome::Failed(format!("update tracks: {e}"));
                        }
                        tracks_announced = true;
                    } else {
                        let track = track_from_frame(&frame);
                        if let Err(e) = publisher.update_tracks(vec![track]) {
                            let _ = publisher.close().await;
                            let _ = pull.close().await;
                            return RunOnceOutcome::Failed(format!("update tracks: {e}"));
                        }
                        tracks_announced = true;
                    }
                }
                if let Err(e) = publisher.push_frame(frame) {
                    let _ = publisher.close().await;
                    let _ = pull.close().await;
                    return RunOnceOutcome::Failed(format!("push frame: {e}"));
                }
            }
            Ok(None) => {
                let _ = publisher.close().await;
                let _ = pull.close().await;
                return RunOnceOutcome::Failed("http-flv pull ended".into());
            }
            Err(e) => {
                let _ = publisher.close().await;
                let _ = pull.close().await;
                return RunOnceOutcome::Failed(e.to_string());
            }
        }
    }
}

#[cfg(not(feature = "http-flv"))]
async fn run_pull_http_flv(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_url: &str,
    _destination: &MediaKey,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("http-flv pull requires cheetah-proxy-module feature `http-flv`".into())
}

async fn run_push(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_media_key: &MediaKey,
    destination_url: &str,
    protocol: &str,
    cancel: &CancellationToken,
    config: &ProxyModuleConfig,
) -> RunOnceOutcome {
    let proto = protocol.to_ascii_lowercase();
    match proto.as_str() {
        "rtmp" => {
            run_push_rtmp(
                ctx,
                registry,
                proxy_id,
                source_media_key,
                destination_url,
                cancel,
                config.connect_timeout_ms,
            )
            .await
        }
        other => RunOnceOutcome::Failed(format!(
            "push proxy protocol '{other}' is not supported by this build (enable rtmp feature)"
        )),
    }
}

#[cfg(feature = "rtmp")]
async fn run_push_rtmp(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_media_key: &MediaKey,
    destination_url: &str,
    cancel: &CancellationToken,
    connect_timeout_ms: u64,
) -> RunOnceOutcome {
    use cheetah_connector::{open_rtmp_push_with_runtime, ConnectorPushOptions};
    use cheetah_media_api::command::SubscribeRequest;
    use cheetah_media_api::ids::MediaSchema;
    use cheetah_media_api::port::MediaRequestContext;
    use tracing::info;

    let media_ctx = MediaRequestContext {
        source_adapter: "proxy".to_string(),
        ..MediaRequestContext::default()
    };
    let mut subscriber = match ctx
        .media_data_plane
        .open_frame_subscriber(
            &media_ctx,
            SubscribeRequest {
                media_key: source_media_key.clone(),
                output_schema: MediaSchema::Rtmp,
                subscriber_kind: "proxy".to_string(),
                start_policy: String::new(),
                auth_context: Default::default(),
            },
        )
        .await
    {
        Ok(s) => s,
        Err(e) => return RunOnceOutcome::Failed(format!("open source subscriber: {e}")),
    };

    let options = ConnectorPushOptions {
        cancel: Some(cancel.child_token()),
        ..Default::default()
    };
    let open = open_rtmp_push_with_runtime(ctx.runtime_api.clone(), destination_url, options);
    let push = match with_timeout(&ctx.runtime_api, connect_timeout_ms, open, cancel).await {
        TimeoutResult::Ok(Ok(h)) => h,
        TimeoutResult::Ok(Err(e)) => {
            let _ = subscriber.close().await;
            return RunOnceOutcome::Failed(e.to_string());
        }
        TimeoutResult::TimedOut => {
            let _ = subscriber.close().await;
            return RunOnceOutcome::Failed("rtmp push connect timeout".into());
        }
        TimeoutResult::Cancelled => {
            let _ = subscriber.close().await;
            return RunOnceOutcome::Stopped;
        }
    };

    if let Err(e) = push.wait_ready().await {
        let _ = push.close();
        let _ = subscriber.close().await;
        return RunOnceOutcome::Failed(format!("rtmp push not ready: {e}"));
    }

    registry.update_state(proxy_id, ProxyState::Connected);
    registry.update_error(proxy_id, None);
    publish_state(ctx, proxy_id, ProxyState::Connected, None);
    info!(proxy_id = %proxy_id.0, "rtmp push proxy connected");

    let mut tracks_announced = false;
    loop {
        if cancel.is_cancelled() {
            let _ = push.close();
            let _ = subscriber.close().await;
            return RunOnceOutcome::Stopped;
        }

        match subscriber.recv().await {
            Ok(Some(frame)) => {
                if !tracks_announced {
                    let tracks = subscriber.tracks();
                    let announce = if tracks.is_empty() {
                        vec![track_from_frame(&frame)]
                    } else {
                        tracks
                    };
                    if let Err(e) = push.update_tracks(announce) {
                        let _ = push.close();
                        let _ = subscriber.close().await;
                        return RunOnceOutcome::Failed(format!("update tracks: {e}"));
                    }
                    tracks_announced = true;
                }
                if let Err(e) = push.push_frame(frame) {
                    let _ = push.close();
                    let _ = subscriber.close().await;
                    return RunOnceOutcome::Failed(format!("push frame: {e}"));
                }
            }
            Ok(None) => {
                let _ = push.close();
                let _ = subscriber.close().await;
                return RunOnceOutcome::Failed("source subscriber ended".into());
            }
            Err(e) => {
                let _ = push.close();
                let _ = subscriber.close().await;
                return RunOnceOutcome::Failed(e.to_string());
            }
        }
    }
}

#[cfg(not(feature = "rtmp"))]
async fn run_push_rtmp(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_media_key: &MediaKey,
    _destination_url: &str,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("rtmp push requires cheetah-proxy-module feature `rtmp`".into())
}

async fn run_ffmpeg(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    destination: &MediaKey,
    input_options: &[String],
    output_options: &[String],
    job_id: &str,
    cancel: &CancellationToken,
) -> RunOnceOutcome {
    if let Err(e) = validate_ffmpeg_options(input_options, output_options) {
        return RunOnceOutcome::Failed(e);
    }

    let command = build_ffmpeg_command(source_url, destination, input_options, output_options);
    let job = FfmpegJob {
        job_id: job_id.to_string(),
        command,
    };
    if let Err(e) = ctx.ffmpeg_api.submit_job(job) {
        return RunOnceOutcome::Failed(format!("submit ffmpeg job: {e}"));
    }

    // LocalFfmpegService is a typed job registry (no child process). Keep the
    // proxy Connected while the job is registered so delete can cancel it.
    registry.update_state(proxy_id, ProxyState::Connected);
    registry.update_error(proxy_id, None);
    publish_state(ctx, proxy_id, ProxyState::Connected, None);
    debug!(proxy_id = %proxy_id.0, job_id, "ffmpeg proxy job registered");

    cancel.cancelled().await;
    let _ = ctx.ffmpeg_api.cancel_job(job_id);
    RunOnceOutcome::Stopped
}

/// Validate FFmpeg option tokens: no shell metacharacters, newlines, or
/// known-dangerous option names.
///
/// 校验 FFmpeg 选项 token：禁止 shell 元字符、换行与危险选项名。
pub fn validate_ffmpeg_options(input: &[String], output: &[String]) -> Result<(), String> {
    for (side, opts) in [("input", input), ("output", output)] {
        for opt in opts {
            if opt.is_empty() {
                return Err(format!("{side} option must not be empty"));
            }
            if opt.chars().any(|c| {
                matches!(
                    c,
                    '\n' | '\r' | ';' | '|' | '&' | '`' | '$' | '(' | ')' | '<' | '>' | '\0'
                )
            }) {
                return Err(format!(
                    "{side} option contains forbidden shell metacharacters"
                ));
            }
            let lower = opt.to_ascii_lowercase();
            if lower == "-filter_complex"
                || lower == "-lavfi"
                || lower.starts_with("filter_complex")
            {
                return Err("filter_complex is not allowed in FFmpeg proxy options".into());
            }
            if lower == "-i" {
                return Err(
                    "explicit -i is not allowed; source URL is controlled by the server".into(),
                );
            }
        }
    }
    Ok(())
}

fn build_ffmpeg_command(
    source_url: &str,
    destination: &MediaKey,
    input_options: &[String],
    output_options: &[String],
) -> String {
    let mut parts = Vec::with_capacity(8 + input_options.len() + output_options.len());
    parts.push("ffmpeg".to_string());
    parts.extend(input_options.iter().cloned());
    parts.push("-i".to_string());
    parts.push(redact_url_credentials(source_url));
    parts.extend(output_options.iter().cloned());
    parts.push(format!(
        "engine://{}/{}/{}",
        destination.vhost.0, destination.app.0, destination.stream.0
    ));
    parts.join(" ")
}

fn redact_url_credentials(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut u) => {
            if !u.username().is_empty() {
                let _ = u.set_username("***");
            }
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        }
        Err(_) => url.to_string(),
    }
}

#[cfg(any(feature = "http-flv", feature = "rtmp"))]
fn track_from_frame(frame: &cheetah_codec::AVFrame) -> cheetah_codec::TrackInfo {
    cheetah_codec::TrackInfo::new(
        frame.track_id,
        frame.media_kind,
        frame.codec,
        frame.timebase.den.max(1),
    )
}

#[cfg(feature = "rtsp")]
async fn hold_pull_handle(
    mut handle: cheetah_connector::PullHandle,
    cancel: &CancellationToken,
) -> RunOnceOutcome {
    loop {
        if cancel.is_cancelled() {
            let _ = handle.close().await;
            return RunOnceOutcome::Stopped;
        }
        match handle.recv().await {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = handle.close().await;
                return RunOnceOutcome::Failed("rtsp pull ended".into());
            }
            Err(e) => {
                let _ = handle.close().await;
                return RunOnceOutcome::Failed(e.to_string());
            }
        }
    }
}

#[cfg(any(feature = "rtsp", feature = "http-flv", feature = "rtmp"))]
enum TimeoutResult<T> {
    Ok(T),
    TimedOut,
    Cancelled,
}

#[cfg(any(feature = "rtsp", feature = "http-flv", feature = "rtmp"))]
async fn with_timeout<T, F>(
    runtime_api: &Arc<dyn RuntimeApi>,
    timeout_ms: u64,
    fut: F,
    cancel: &CancellationToken,
) -> TimeoutResult<T>
where
    F: Future<Output = T> + Send,
    T: Send,
{
    let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + timeout_ms * 1_000);
    let mut timer = runtime_api.sleep_until(deadline);
    let cancel_fut = cancel.cancelled();
    futures::pin_mut!(fut);
    futures::pin_mut!(cancel_fut);
    futures::select! {
        result = fut.fuse() => TimeoutResult::Ok(result),
        _ = timer.wait().fuse() => TimeoutResult::TimedOut,
        _ = cancel_fut.fuse() => TimeoutResult::Cancelled,
    }
}

fn retry_delay_ms(config: &ProxyModuleConfig, attempt: u32) -> u64 {
    let base = config.retry_delay_ms.max(1);
    let exp = attempt.saturating_sub(1).min(16);
    let delay = base.saturating_mul(1u64 << exp);
    delay.min(config.retry_max_delay_ms)
}

/// Returns true if cancelled before the delay elapsed.
async fn wait_or_cancel(
    runtime_api: &Arc<dyn RuntimeApi>,
    delay_ms: u64,
    cancel: &CancellationToken,
) -> bool {
    let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + delay_ms * 1_000);
    let mut timer = runtime_api.sleep_until(deadline);
    match wait_first(timer.wait(), Box::pin(cancel.cancelled())).await {
        WaitOutcome::First => false,
        WaitOutcome::Second => true,
    }
}

enum WaitOutcome {
    First,
    Second,
}

async fn wait_first(
    first: Pin<Box<dyn Future<Output = ()> + Send + '_>>,
    second: Pin<Box<dyn Future<Output = ()> + Send + '_>>,
) -> WaitOutcome {
    match select(first, second).await {
        Either::Left(_) => WaitOutcome::First,
        Either::Right(_) => WaitOutcome::Second,
    }
}

fn transition_to_stopped(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    task_id: TaskId,
    error: Option<String>,
) {
    registry.update_state(proxy_id, ProxyState::Stopped);
    publish_state(ctx, proxy_id, ProxyState::Stopped, error.clone());
    let outcome = if let Some(msg) = error {
        let _ = registry.update_error(proxy_id, Some(msg.clone()));
        TaskOutcome::Failed(msg)
    } else {
        TaskOutcome::Succeeded
    };
    if let Err(e) = ctx.task_system_api.finish(task_id, outcome) {
        error!(task_id = %task_id.0, "failed to finish proxy task: {e}");
    }
    debug!(proxy_id = %proxy_id.0, "proxy session stopped");
}

fn publish_state(
    ctx: &EngineContext,
    proxy_id: &ProxyId,
    state: ProxyState,
    last_error: Option<String>,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let _ = ctx
        .media_event_bus
        .publish(MediaEvent::ProxyStateChanged(ProxyStateChanged {
            header: EventHeader {
                event_id: format!("proxy-{}-{}", proxy_id.0, now),
                occurred_at: now,
                sequence: None,
                media_key: None,
                source: "proxy".to_string(),
                correlation_id: None,
            },
            proxy_id: proxy_id.clone(),
            state,
            last_error,
        }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_rejects_shell_metacharacters() {
        assert!(validate_ffmpeg_options(&["; rm -rf /".into()], &[]).is_err());
        assert!(validate_ffmpeg_options(&[], &["-filter_complex".into()]).is_err());
        assert!(validate_ffmpeg_options(&["-i".into()], &[]).is_err());
        assert!(validate_ffmpeg_options(&["-an".into()], &["-c:v".into(), "copy".into()]).is_ok());
    }

    #[test]
    fn redact_credentials_in_url() {
        let redacted = redact_url_credentials("rtsp://user:secret@cam.example/stream");
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("***"));
    }
}
