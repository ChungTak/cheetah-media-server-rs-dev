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
use cheetah_sdk::{
    EngineContext, FfmpegInput, FfmpegJobSpec, FfmpegJobState, FfmpegOutput, FfmpegResourceLimits,
    TaskId, TaskKind, TaskOutcome,
};
use futures::future::{select, Either};
use futures::Future;
#[cfg(any(feature = "rtsp", feature = "http-flv", feature = "rtmp"))]
use futures::FutureExt;
use tracing::{debug, error, warn};
use url::Url;

use std::net::SocketAddr;

use crate::config::ProxyModuleConfig;
use crate::registry::ProxyRegistry;

/// Specification of the data-plane work a proxy session should perform.
///
/// 代理会话应执行的数据面工作规格。
#[derive(Debug, Clone)]
pub enum ProxySessionSpec {
    Pull {
        source_url: String,
        source_peer: SocketAddr,
        destination: MediaKey,
    },
    Push {
        source_media_key: MediaKey,
        destination_url: String,
        destination_peer: SocketAddr,
        protocol: String,
    },
    Ffmpeg {
        source_url: String,
        source_peer: SocketAddr,
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
            source_peer,
            destination,
        } => {
            run_pull(
                ctx,
                registry,
                proxy_id,
                source_url,
                *source_peer,
                destination,
                cancel,
                config,
            )
            .await
        }
        ProxySessionSpec::Push {
            source_media_key,
            destination_url,
            destination_peer,
            protocol,
        } => {
            run_push(
                ctx,
                registry,
                proxy_id,
                source_media_key,
                destination_url,
                *destination_peer,
                protocol,
                cancel,
                config,
            )
            .await
        }
        ProxySessionSpec::Ffmpeg {
            source_url,
            source_peer,
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
                *source_peer,
                destination,
                input_options,
                output_options,
                job_id,
                cancel,
                config,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_pull(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    source_peer: SocketAddr,
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
                source_peer,
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
                source_peer,
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
#[allow(clippy::too_many_arguments)]
async fn run_pull_rtsp(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    source_peer: SocketAddr,
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
        peer: Some(source_peer),
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
#[allow(clippy::too_many_arguments)]
async fn run_pull_rtsp(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_url: &str,
    _source_peer: SocketAddr,
    _destination: &MediaKey,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("rtsp pull requires cheetah-proxy-module feature `rtsp`".into())
}

#[cfg(feature = "http-flv")]
#[allow(clippy::too_many_arguments)]
async fn run_pull_http_flv(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    source_peer: SocketAddr,
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
        peer: Some(source_peer),
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
#[allow(clippy::too_many_arguments)]
async fn run_pull_http_flv(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_url: &str,
    _source_peer: SocketAddr,
    _destination: &MediaKey,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("http-flv pull requires cheetah-proxy-module feature `http-flv`".into())
}

#[allow(clippy::too_many_arguments)]
async fn run_push(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_media_key: &MediaKey,
    destination_url: &str,
    destination_peer: SocketAddr,
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
                destination_peer,
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
#[allow(clippy::too_many_arguments)]
async fn run_push_rtmp(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_media_key: &MediaKey,
    destination_url: &str,
    destination_peer: SocketAddr,
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
        peer: Some(destination_peer),
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
#[allow(clippy::too_many_arguments)]
async fn run_push_rtmp(
    _ctx: &EngineContext,
    _registry: &ProxyRegistry,
    _proxy_id: &ProxyId,
    _source_media_key: &MediaKey,
    _destination_url: &str,
    _destination_peer: SocketAddr,
    _cancel: &CancellationToken,
    _connect_timeout_ms: u64,
) -> RunOnceOutcome {
    RunOnceOutcome::Failed("rtmp push requires cheetah-proxy-module feature `rtmp`".into())
}

#[allow(clippy::too_many_arguments)]
async fn run_ffmpeg(
    ctx: &EngineContext,
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    source_url: &str,
    source_peer: SocketAddr,
    destination: &MediaKey,
    input_options: &[String],
    output_options: &[String],
    job_id: &str,
    cancel: &CancellationToken,
    config: &ProxyModuleConfig,
) -> RunOnceOutcome {
    if let Err(e) = validate_ffmpeg_options(input_options, output_options) {
        return RunOnceOutcome::Failed(e);
    }

    // FFmpeg performs its own DNS resolution, so rewrite the input URL to the
    // validated peer address to prevent DNS-rebinding SSRF.
    let resolved_source_url = match rewrite_url_to_peer(source_url, source_peer) {
        Ok(url) => url,
        Err(err) => return RunOnceOutcome::Failed(err),
    };
    let redacted_source_url = redact_url_credentials(&resolved_source_url);
    let redacted_original_url = redact_url_credentials(source_url);

    let spec = FfmpegJobSpec {
        profile_id: "default".to_string(),
        input: FfmpegInput::Url {
            url: resolved_source_url.clone(),
        },
        output: FfmpegOutput::Engine {
            media_key: destination.clone(),
        },
        input_options: input_options.to_vec(),
        output_options: output_options.to_vec(),
        resource_limits: FfmpegResourceLimits {
            max_runtime_ms: config.ffmpeg_timeout_ms,
            ..Default::default()
        },
    };

    let handle = match ctx.ffmpeg_api.submit(job_id.to_string(), spec).await {
        Ok(handle) => handle,
        Err(e) => return RunOnceOutcome::Failed(format!("submit ffmpeg job: {e}")),
    };

    registry.update_state(proxy_id, ProxyState::Connected);
    registry.update_error(proxy_id, None);
    publish_state(ctx, proxy_id, ProxyState::Connected, None);
    debug!(proxy_id = %proxy_id.0, job_id, "ffmpeg proxy job submitted");

    let wait_fut = Box::pin(ctx.ffmpeg_api.wait(&handle.job_id));
    let cancel_fut = Box::pin(cancel.cancelled());
    let status = match select(wait_fut, cancel_fut).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => {
            let _ = ctx.ffmpeg_api.cancel(&handle.job_id).await;
            match ctx.ffmpeg_api.wait(&handle.job_id).await {
                Ok(s) => Ok(s),
                Err(e) => {
                    let _ = ctx.ffmpeg_api.remove(&handle.job_id).await;
                    return RunOnceOutcome::Failed(format!(
                        "ffmpeg job cancelled but failed to wait: {e}"
                    ));
                }
            }
        }
    };

    let outcome = match status {
        Ok(status) => match status.state {
            FfmpegJobState::Exited if status.exit_code == Some(0) => RunOnceOutcome::Stopped,
            FfmpegJobState::Cancelled => RunOnceOutcome::Stopped,
            _ => {
                // Strip any embedded source credentials before the summary is persisted
                // in registry errors, logged, or returned to callers.
                let summary = status
                    .exit_summary
                    .replace(&resolved_source_url, &redacted_source_url)
                    .replace(source_url, &redacted_original_url);
                RunOnceOutcome::Failed(summary)
            }
        },
        Err(e) => RunOnceOutcome::Failed(format!("ffmpeg job error: {e}")),
    };

    let _ = ctx.ffmpeg_api.remove(&handle.job_id).await;
    outcome
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

/// Rewrite `source_url` so its host and port match the validated `peer`.
///
/// FFmpeg performs its own DNS resolution, so the command line must carry the
/// already-validated IP address to prevent DNS-rebinding SSRF.
fn rewrite_url_to_peer(source_url: &str, peer: SocketAddr) -> Result<String, String> {
    let mut parsed = Url::parse(source_url).map_err(|err| format!("invalid source url: {err}"))?;
    let host = if peer.ip().is_ipv6() {
        format!("[{ip}]", ip = peer.ip())
    } else {
        peer.ip().to_string()
    };
    parsed
        .set_host(Some(&host))
        .map_err(|err| format!("rewrite source host: {err}"))?;
    parsed
        .set_port(Some(peer.port()))
        .map_err(|_err| "rewrite source port: invalid port".to_string())?;
    Ok(parsed.to_string())
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

    #[test]
    fn rewrite_url_to_peer_preserves_path_and_userinfo() {
        let peer = SocketAddr::from(([127, 0, 0, 1], 1935));
        let rewritten = rewrite_url_to_peer("rtmp://user:pass@cam.example/live/stream", peer)
            .expect("rewrite should succeed");
        assert!(rewritten.contains("127.0.0.1:1935"), "{rewritten}");
        assert!(rewritten.contains("/live/stream"), "{rewritten}");
    }

    #[test]
    fn rewrite_url_to_peer_brackets_ipv6() {
        let peer = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 554));
        let rewritten =
            rewrite_url_to_peer("rtsp://cam.example/stream", peer).expect("rewrite should succeed");
        assert!(rewritten.contains("[::1]:554"), "{rewritten}");
    }
}
