use std::collections::HashSet;

use cheetah_sdk::{CancellationToken, EngineContext, JoinHandle as RuntimeJoinHandle};
use tracing::{info, warn};

use crate::config::{
    RtspModuleConfig, RtspPullJobConfig, RtspPullTransport, RtspPushJobConfig, RtspPushTransport,
    RtspRelayJobConfig,
};
use crate::module::client_pull::{spawn_pull_job_supervisors, wait_pull_job_supervisors};
use crate::module::client_push::{spawn_push_job_supervisors, wait_push_job_supervisors};

/// `RelayJobSupervisorHandle` data structure.
/// `RelayJobSupervisorHandle` 数据结构.
pub(super) struct RelayJobSupervisorHandle {
    /// `job_name` field of type `String`.
    /// `job_name` 字段，类型为 `String`.
    job_name: String,
    /// `relay_stream_key` field of type `String`.
    /// `relay_stream_key` 字段，类型为 `String`.
    relay_stream_key: String,
    /// `join` field.
    /// `join` 字段.
    join: Box<dyn RuntimeJoinHandle>,
}

/// `spawn_relay_job_supervisors` function.
/// `spawn_relay_job_supervisors` 函数.
pub(super) fn spawn_relay_job_supervisors(
    engine: &EngineContext,
    config: &RtspModuleConfig,
    module_cancel: CancellationToken,
) -> Vec<RelayJobSupervisorHandle> {
    let mut handles = Vec::new();
    for job in config.relay_jobs.iter().filter(|job| job.enabled) {
        let relay_stream_key = relay_stream_key(job);
        let runtime_api = engine.runtime_api.clone();
        let engine_ctx = engine.clone();
        let module_config = config.clone();
        let job_clone = job.clone();
        let cancel = module_cancel.child_token();
        let job_name = job.name.clone();
        let relay_stream_key_for_task = relay_stream_key.clone();
        let join = runtime_api.spawn(Box::pin(async move {
            run_relay_job_supervisor(
                engine_ctx,
                module_config,
                job_clone,
                relay_stream_key_for_task,
                cancel,
            )
            .await;
        }));
        handles.push(RelayJobSupervisorHandle {
            job_name,
            relay_stream_key,
            join,
        });
    }
    handles
}

/// `wait_relay_job_supervisors` function.
/// `wait_relay_job_supervisors` 函数.
pub(super) async fn wait_relay_job_supervisors(handles: &mut Vec<RelayJobSupervisorHandle>) {
    for handle in handles.drain(..) {
        handle.join.abort();
        if let Err(err) = handle.join.wait().await {
            warn!(
                job = %handle.job_name,
                relay_stream_key = %handle.relay_stream_key,
                "relay job supervisor exited with join error: {err}"
            );
        }
    }
}

async fn run_relay_job_supervisor(
    engine: EngineContext,
    config: RtspModuleConfig,
    relay: RtspRelayJobConfig,
    relay_stream_key: String,
    cancel: CancellationToken,
) {
    let pull_job = relay_to_pull_job(&relay, &relay_stream_key);
    let push_job = relay_to_push_job(&relay, &relay_stream_key);

    let mut pull_config = config.clone();
    pull_config.pull_jobs = vec![pull_job];
    let mut push_config = config;
    push_config.push_jobs = vec![push_job];

    info!(
        job = %relay.name,
        relay_stream_key = %relay_stream_key,
        source_url = %relay.source_url,
        target_url = %relay.target_url,
        "rtsp relay job supervisor started"
    );

    let mut pull_handles = spawn_pull_job_supervisors(&engine, &pull_config, cancel.child_token());
    let mut push_handles = spawn_push_job_supervisors(&engine, &push_config, cancel.child_token());

    cancel.cancelled().await;
    wait_pull_job_supervisors(&mut pull_handles).await;
    wait_push_job_supervisors(&mut push_handles).await;

    info!(
        job = %relay.name,
        relay_stream_key = %relay_stream_key,
        "rtsp relay job supervisor stopped"
    );
}

fn relay_to_pull_job(relay: &RtspRelayJobConfig, relay_stream_key: &str) -> RtspPullJobConfig {
    RtspPullJobConfig {
        name: format!("relay::{}::pull", relay.name),
        enabled: true,
        source_url: relay.source_url.clone(),
        target_stream_key: relay_stream_key.to_string(),
        username: None,
        password: None,
        transport_preference: dedup_pull_transports(&relay.transport_preference),
        heartbeat_mode: Default::default(),
        retry_backoff_ms: relay.retry_backoff_ms,
        max_retry_backoff_ms: relay.max_retry_backoff_ms,
    }
}

fn relay_to_push_job(relay: &RtspRelayJobConfig, relay_stream_key: &str) -> RtspPushJobConfig {
    RtspPushJobConfig {
        name: format!("relay::{}::push", relay.name),
        enabled: true,
        source_stream_key: relay_stream_key.to_string(),
        target_url: relay.target_url.clone(),
        username: None,
        password: None,
        transport_preference: relay_push_transports(&relay.transport_preference),
        retry_backoff_ms: relay.retry_backoff_ms,
        max_retry_backoff_ms: relay.max_retry_backoff_ms,
    }
}

fn dedup_pull_transports(transports: &[RtspPullTransport]) -> Vec<RtspPullTransport> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for transport in transports.iter().copied() {
        if seen.insert(transport) {
            out.push(transport);
        }
    }
    if out.is_empty() {
        out.push(RtspPullTransport::TcpInterleaved);
    }
    out
}

fn relay_push_transports(transports: &[RtspPullTransport]) -> Vec<RtspPushTransport> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for transport in transports {
        let mapped = match transport {
            RtspPullTransport::TcpInterleaved => RtspPushTransport::TcpInterleaved,
            RtspPullTransport::Udp => RtspPushTransport::Udp,
            RtspPullTransport::HttpTunnel => RtspPushTransport::HttpTunnel,
            RtspPullTransport::Multicast => continue,
        };
        if seen.insert(mapped) {
            out.push(mapped);
        }
    }
    if out.is_empty() {
        out.push(RtspPushTransport::TcpInterleaved);
    }
    out
}

fn relay_stream_key(relay: &RtspRelayJobConfig) -> String {
    if let Some(local_stream_key) = relay.local_stream_key.as_ref() {
        return local_stream_key.trim().to_string();
    }
    let mut sanitized = String::with_capacity(relay.name.len());
    for ch in relay.name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch);
        } else {
            sanitized.push('-');
        }
    }
    while sanitized.contains("--") {
        sanitized = sanitized.replace("--", "-");
    }
    let sanitized = sanitized.trim_matches('-');
    let suffix = if sanitized.is_empty() {
        "relay".to_string()
    } else {
        sanitized.to_string()
    };
    format!("__relay/{suffix}")
}
