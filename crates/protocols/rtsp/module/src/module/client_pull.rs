use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{RtpPacket, TrackId};
use cheetah_rtsp_core::{RtspMethod, RtspRequestMessage, RtspResponseMessage};
use cheetah_rtsp_driver_tokio::{
    allocate_udp_endpoint, authorization_header_from_response, configure_udp_remote_and_punch,
    spawn_udp_receive_tasks, start_http_tunnel_client, start_tcp_client, RtspClientCommand,
    RtspClientConfig, RtspClientCredentials, RtspClientEvent, RtspClientHandle,
    RtspClientUdpEndpoint, RtspClientUdpRemote, RtspConnectionId,
};
use cheetah_sdk::{
    CancellationToken, EngineContext, JoinHandle as RuntimeJoinHandle, PublisherOptions,
    RuntimeApi, SdkError, StreamKey,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{info, warn};

use crate::config::{RtspHeartbeatMode, RtspModuleConfig, RtspPullJobConfig, RtspPullTransport};
use crate::media::{
    build_rtcp_empty_rr, parse_setup_transport, parse_stream_key_from_uri, RtspSetupTransport,
};
use crate::module::publish::{build_pull_publish_session, ingest_publish_rtp_packet};
use crate::module::session_lifecycle::parse_session_token;
use crate::sdp::parse_announce_sdp;
use crate::session::PublishSession;

pub(super) struct PullJobSupervisorHandle {
    job_name: String,
    join: Box<dyn RuntimeJoinHandle>,
}

const PULL_INGEST_CONNECTION_ID: RtspConnectionId = u64::MAX - 1;
const MAX_PENDING_KEEPALIVE_REQUESTS: usize = 64;

pub(crate) struct PullSetupContext<'a> {
    pub(crate) runtime_api: &'a Arc<dyn RuntimeApi>,
    pub(crate) source_url: &'a str,
    pub(crate) base_url: &'a str,
    pub(crate) peer: SocketAddr,
    pub(crate) transport: PullSelectedTransport,
    pub(crate) cancel: &'a CancellationToken,
    pub(crate) request_timeout: Duration,
    pub(crate) auth: &'a mut PullOutboundAuthState,
    pub(crate) start_cseq: u32,
}

pub(crate) struct PullSetupResult {
    pub(crate) interleaved_rtp_channels: HashMap<u8, TrackId>,
    pub(crate) session_token: String,
    pub(crate) session_timeout_secs: Option<u64>,
    pub(crate) next_cseq: u32,
}

pub(crate) struct PullSetupCompletion {
    pub(crate) setup: PullSetupResult,
    pub(crate) udp_task_handles: Vec<Box<dyn RuntimeJoinHandle>>,
}

struct PendingPullUdpReceiver {
    endpoint: RtspClientUdpEndpoint,
    track_id: TrackId,
    remote: RtspClientUdpRemote,
}

#[derive(Default)]
pub(crate) struct PullOutboundAuthState {
    credentials: Option<RtspClientCredentials>,
    challenge: Option<RtspResponseMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PullAttemptErrorKind {
    Retryable,
    StopJob,
}

#[derive(Debug)]
struct PullAttemptError {
    kind: PullAttemptErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullSelectedTransport {
    TcpInterleaved,
    Udp,
    HttpTunnel,
}

impl PullAttemptError {
    fn retry(message: String) -> Self {
        Self {
            kind: PullAttemptErrorKind::Retryable,
            message,
        }
    }

    fn stop(message: String) -> Self {
        Self {
            kind: PullAttemptErrorKind::StopJob,
            message,
        }
    }
}

pub(super) fn spawn_pull_job_supervisors(
    engine: &EngineContext,
    config: &RtspModuleConfig,
    module_cancel: CancellationToken,
) -> Vec<PullJobSupervisorHandle> {
    let mut handles = Vec::new();
    for job in config.pull_jobs.iter().filter(|job| job.enabled) {
        let engine_ctx = engine.clone();
        let runtime_api = engine.runtime_api.clone();
        let module_config = config.clone();
        let job_clone = job.clone();
        let cancel = module_cancel.child_token();
        let job_name = job.name.clone();
        let join = runtime_api.spawn(Box::pin(async move {
            run_pull_job_supervisor(engine_ctx, module_config, job_clone, cancel).await;
        }));
        handles.push(PullJobSupervisorHandle { job_name, join });
    }
    handles
}

pub(super) async fn wait_pull_job_supervisors(handles: &mut Vec<PullJobSupervisorHandle>) {
    for handle in handles.drain(..) {
        handle.join.abort();
        if let Err(err) = handle.join.wait().await {
            warn!(
                job = %handle.job_name,
                "pull job supervisor exited with join error: {err}"
            );
        }
    }
}

async fn run_pull_job_supervisor(
    engine: EngineContext,
    config: RtspModuleConfig,
    job: RtspPullJobConfig,
    cancel: CancellationToken,
) {
    let retry_backoff = Duration::from_millis(job.retry_backoff_ms.max(1));
    let max_retry_backoff =
        Duration::from_millis(job.max_retry_backoff_ms.max(job.retry_backoff_ms.max(1)));
    let mut current_backoff = retry_backoff;
    let request_timeout = Duration::from_secs(5);
    info!(
        job = %job.name,
        source_url = %job.source_url,
        target_stream_key = %job.target_stream_key,
        "rtsp pull job supervisor started"
    );
    let transports = match supported_pull_transports(&job.transport_preference) {
        Ok(transports) => transports,
        Err(err) => {
            warn!(job = %job.name, "pull job has no supported transport: {err}");
            return;
        }
    };
    let mut transport_index = 0usize;

    loop {
        if cancel.is_cancelled() {
            break;
        }
        let selected_transport = transports[transport_index % transports.len()];
        match run_pull_control_plane_once(
            &engine,
            &config,
            &job,
            &cancel,
            request_timeout,
            selected_transport,
        )
        .await
        {
            Ok(()) => break,
            Err(err) => {
                warn!(
                    job = %job.name,
                    source_url = %job.source_url,
                    transport = ?selected_transport,
                    kind = ?err.kind,
                    "pull control-plane attempt failed: {}",
                    err.message
                );
                if err.kind == PullAttemptErrorKind::StopJob {
                    break;
                }
                transport_index = transport_index.wrapping_add(1);
                if wait_or_cancel(&engine.runtime_api, &cancel, current_backoff).await {
                    break;
                }
                current_backoff = next_retry_backoff(current_backoff, max_retry_backoff);
            }
        }
    }

    info!(job = %job.name, "rtsp pull job supervisor stopped");
}

async fn run_pull_control_plane_once(
    engine: &EngineContext,
    config: &RtspModuleConfig,
    job: &RtspPullJobConfig,
    cancel: &CancellationToken,
    request_timeout: Duration,
    selected_transport: PullSelectedTransport,
) -> Result<(), PullAttemptError> {
    let peer = parse_rtsp_source_peer(&job.source_url).map_err(PullAttemptError::retry)?;
    let target_stream_key =
        parse_target_stream_key(&job.target_stream_key).map_err(PullAttemptError::retry)?;
    let client_cancel = cancel.child_token();
    let mut client = start_pull_client(
        engine.runtime_api.clone(),
        peer,
        &job.source_url,
        &job.name,
        selected_transport,
        client_cancel,
    )?;

    wait_client_connected(&engine.runtime_api, &mut client, cancel, request_timeout)
        .await
        .map_err(PullAttemptError::retry)?;
    let credentials = job.username.as_ref().map(|username| RtspClientCredentials {
        username: username.to_string(),
        password: job.password.as_deref().unwrap_or_default().to_string(),
    });
    let mut outbound_auth = build_pull_outbound_auth_state(credentials);
    let mut cseq = 1_u32;
    let options_response = send_request_with_auth_retry(
        &engine.runtime_api,
        &mut client,
        &mut outbound_auth,
        RtspMethod::Options,
        &job.source_url,
        &mut cseq,
        &[],
        &[],
        cancel,
        request_timeout,
    )
    .await
    .map_err(PullAttemptError::retry)?;
    if options_response.status_code != 200 {
        client.shutdown();
        return Err(PullAttemptError::retry(format!(
            "OPTIONS failed with status {}",
            options_response.status_code
        )));
    }

    let describe_response = send_request_with_auth_retry(
        &engine.runtime_api,
        &mut client,
        &mut outbound_auth,
        RtspMethod::Describe,
        &job.source_url,
        &mut cseq,
        &[("Accept", "application/sdp")],
        &[],
        cancel,
        request_timeout,
    )
    .await
    .map_err(PullAttemptError::retry)?;
    if describe_response.status_code != 200 {
        client.shutdown();
        return Err(PullAttemptError::retry(format!(
            "DESCRIBE failed with status {}",
            describe_response.status_code
        )));
    }
    let describe_body = std::str::from_utf8(describe_response.body.as_ref()).map_err(|_| {
        PullAttemptError::retry("DESCRIBE response body is not valid utf-8".to_string())
    })?;
    // RFC 2326: Content-Base from DESCRIBE response takes priority for resolving control URLs.
    let content_base = describe_response
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("Content-Base"))
        .map(|h| h.value.trim().trim_end_matches('/').to_string());
    let base_url = content_base.as_deref().unwrap_or(&job.source_url);
    let (tracks, control_map) = parse_announce_sdp(describe_body)
        .map_err(|err| PullAttemptError::retry(format!("parse DESCRIBE SDP failed: {err}")))?;
    let track_controls = invert_track_controls(&tracks, &control_map);

    let (lease, sink) = match engine
        .publisher_api
        .acquire_publisher(target_stream_key.clone(), PublisherOptions::default())
        .await
    {
        Ok(value) => value,
        Err(SdkError::Conflict(err)) => {
            return Err(PullAttemptError::stop(format!(
                "target stream already has active publisher, stop pull job: {err}"
            )));
        }
        Err(err) => {
            return Err(PullAttemptError::retry(format!(
                "acquire publisher lease failed: {err}"
            )));
        }
    };
    sink.update_tracks(tracks.clone())
        .map_err(|err| PullAttemptError::retry(format!("update pull tracks failed: {err}")))?;

    let mut publish = build_pull_publish_session(
        config,
        cancel.child_token(),
        lease,
        sink,
        tracks_to_map(&tracks),
    );

    let mut setup_completion = setup_pull_tracks_and_play(
        &mut client,
        &tracks,
        &track_controls,
        PullSetupContext {
            runtime_api: &engine.runtime_api,
            source_url: &job.source_url,
            base_url,
            peer,
            transport: selected_transport,
            cancel,
            request_timeout,
            auth: &mut outbound_auth,
            start_cseq: cseq,
        },
    )
    .await
    .map_err(PullAttemptError::retry)?;

    info!(
        job = %job.name,
        stream = %target_stream_key,
        track_count = tracks.len(),
        setup_tracks = setup_completion.setup.interleaved_rtp_channels.len(),
        keepalive_timeout_secs = setup_completion.setup.session_timeout_secs.unwrap_or(0),
        "pull control-plane prepared tracks and started RTP ingest"
    );

    let session_result = wait_pull_session_end(
        &mut client,
        cancel,
        &mut publish,
        &setup_completion.setup,
        &job.source_url,
        &engine.runtime_api,
        &mut outbound_auth,
        job.heartbeat_mode,
    )
    .await;
    for join in setup_completion.udp_task_handles.drain(..) {
        join.abort();
        let _ = join.wait().await;
    }
    client.shutdown();
    if let Err(err) = publish.sink.close() {
        warn!(job = %job.name, "close pull sink failed: {err}");
        if let Err(release_err) = engine.publisher_api.release_publisher(&publish.lease).await {
            warn!(
                job = %job.name,
                "release pull lease fallback failed after sink close error: {release_err}"
            );
        }
    }

    session_result.map_err(PullAttemptError::retry).map(|_| ())
}

pub(crate) fn supported_pull_transports(
    preferences: &[RtspPullTransport],
) -> Result<Vec<PullSelectedTransport>, String> {
    let mut transports = Vec::new();
    for transport in preferences {
        match transport {
            RtspPullTransport::TcpInterleaved => {
                transports.push(PullSelectedTransport::TcpInterleaved);
            }
            RtspPullTransport::Udp => transports.push(PullSelectedTransport::Udp),
            RtspPullTransport::HttpTunnel => transports.push(PullSelectedTransport::HttpTunnel),
            RtspPullTransport::Multicast => {}
        }
    }
    if transports.is_empty() {
        Err("pull job transport preference contains no supported transport".to_string())
    } else {
        Ok(transports)
    }
}

fn start_pull_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    source_url: &str,
    job_name: &str,
    transport: PullSelectedTransport,
    cancel: CancellationToken,
) -> Result<RtspClientHandle, PullAttemptError> {
    match transport {
        PullSelectedTransport::TcpInterleaved | PullSelectedTransport::Udp => {
            start_tcp_client(runtime_api, peer, RtspClientConfig::default(), cancel).map_err(
                |err| PullAttemptError::retry(format!("start outbound tcp client failed: {err}")),
            )
        }
        PullSelectedTransport::HttpTunnel => start_http_tunnel_client(
            runtime_api,
            peer,
            rtsp_url_path(source_url).map_err(PullAttemptError::retry)?,
            format!("cheetah-pull-{job_name}"),
            RtspClientConfig::default(),
            cancel,
        )
        .map_err(|err| {
            PullAttemptError::retry(format!("start outbound http tunnel client failed: {err}"))
        }),
    }
}

pub(crate) fn rtsp_url_path(url: &str) -> Result<String, String> {
    let rest = url
        .trim()
        .strip_prefix("rtsp://")
        .or_else(|| url.trim().strip_prefix("rtsps://"))
        .ok_or_else(|| "rtsp url must start with rtsp:// or rtsps://".to_string())?;
    let path = rest
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or_default();
    Ok(format!("/{path}"))
}

pub fn parse_rtsp_source_peer(source_url: &str) -> Result<SocketAddr, String> {
    let source = source_url.trim();
    let rest = source
        .strip_prefix("rtsp://")
        .or_else(|| source.strip_prefix("rtsps://"))
        .ok_or_else(|| "source_url must start with rtsp:// or rtsps://".to_string())?;
    let authority = rest
        .split('/')
        .next()
        .ok_or_else(|| "source_url missing authority".to_string())?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if authority.is_empty() {
        return Err("source_url missing host".to_string());
    }
    resolve_rtsp_authority(authority, "source_url")
}

fn resolve_rtsp_authority(authority: &str, field_name: &str) -> Result<SocketAddr, String> {
    let normalized = normalize_rtsp_authority(authority)?;
    normalized
        .to_socket_addrs()
        .map_err(|err| format!("{field_name} authority resolve failed for `{normalized}`: {err}"))?
        .next()
        .ok_or_else(|| format!("{field_name} authority resolved no socket address"))
}

fn normalize_rtsp_authority(authority: &str) -> Result<String, String> {
    let authority = authority.trim();
    if authority.is_empty() {
        return Err("rtsp authority must not be empty".to_string());
    }
    if authority.starts_with('[') && authority.ends_with(']') {
        return Ok(format!("{authority}:554"));
    }
    if authority.bytes().filter(|b| *b == b':').count() == 0 {
        return Ok(format!("{authority}:554"));
    }
    Ok(authority.to_string())
}

fn parse_target_stream_key(target_stream_key: &str) -> Result<StreamKey, String> {
    let trimmed = target_stream_key.trim();
    if trimmed.is_empty() {
        return Err("target_stream_key must not be empty".to_string());
    }
    let pseudo_uri = format!("rtsp://127.0.0.1/{trimmed}");
    parse_stream_key_from_uri(&pseudo_uri)
        .ok_or_else(|| "target_stream_key format is invalid".to_string())
}

fn build_rtsp_request(
    method: &str,
    uri: &str,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) -> RtspRequestMessage {
    let mut all_headers = Vec::with_capacity(headers.len() + 1);
    all_headers.push(cheetah_rtsp_driver_tokio::RtspHeader {
        name: "CSeq".to_string(),
        value: cseq.to_string(),
    });
    all_headers.extend(
        headers
            .iter()
            .map(|(name, value)| cheetah_rtsp_driver_tokio::RtspHeader {
                name: (*name).to_string(),
                value: (*value).to_string(),
            }),
    );
    RtspRequestMessage {
        method: method.to_string(),
        uri: uri.to_string(),
        version: "RTSP/1.0".to_string(),
        headers: all_headers,
        body: Bytes::copy_from_slice(body),
    }
}

pub(crate) fn build_pull_outbound_auth_state(
    credentials: Option<RtspClientCredentials>,
) -> PullOutboundAuthState {
    PullOutboundAuthState {
        credentials,
        challenge: None,
    }
}

fn build_request_authorization(
    auth: &PullOutboundAuthState,
    method: RtspMethod,
    uri: &str,
) -> Option<String> {
    let credentials = auth.credentials.as_ref()?;
    let challenge = auth.challenge.as_ref()?;
    authorization_header_from_response(challenge, method, uri, credentials)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_request_with_auth_retry(
    runtime_api: &Arc<dyn RuntimeApi>,
    client: &mut RtspClientHandle,
    auth: &mut PullOutboundAuthState,
    method: RtspMethod,
    uri: &str,
    cseq: &mut u32,
    headers: &[(&str, &str)],
    body: &[u8],
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<RtspResponseMessage, String> {
    let command_tx = client.command_sender();
    let method_name = method.as_str();
    let request_cseq = *cseq;
    *cseq = cseq.saturating_add(1);
    let mut request_headers = headers.to_vec();
    let request_authorization = build_request_authorization(auth, method.clone(), uri);
    if let Some(value) = request_authorization.as_deref() {
        request_headers.push(("Authorization", value));
    }
    command_tx
        .send(RtspClientCommand::SendRequest(build_rtsp_request(
            method_name,
            uri,
            request_cseq,
            request_headers.as_slice(),
            body,
        )))
        .await
        .map_err(|err| format!("send {method_name} failed: {err}"))?;
    let response =
        wait_response_for_cseq(runtime_api, client, request_cseq, cancel, timeout).await?;
    if response.status_code != 401 {
        return Ok(response);
    }
    let Some(credentials) = auth.credentials.as_ref() else {
        return Ok(response);
    };
    let Some(retry_authorization) =
        authorization_header_from_response(&response, method.clone(), uri, credentials)
    else {
        return Ok(response);
    };
    let retry_cseq = *cseq;
    *cseq = cseq.saturating_add(1);
    auth.challenge = Some(response.clone());
    let mut retry_headers = headers.to_vec();
    retry_headers.push(("Authorization", retry_authorization.as_str()));
    command_tx
        .send(RtspClientCommand::SendRequest(build_rtsp_request(
            method_name,
            uri,
            retry_cseq,
            retry_headers.as_slice(),
            body,
        )))
        .await
        .map_err(|err| format!("retry {method_name} with authorization failed: {err}"))?;
    wait_response_for_cseq(runtime_api, client, retry_cseq, cancel, timeout).await
}

pub(crate) async fn wait_client_connected(
    runtime_api: &Arc<dyn RuntimeApi>,
    client: &mut RtspClientHandle,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = runtime_deadline_after(runtime_api, timeout);
    let mut timeout_timer = runtime_api.sleep_until(deadline);
    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let timeout_fut = timeout_timer.wait().fuse();
        let event_fut = client.recv_event().fuse();
        pin_mut!(cancel_fut, timeout_fut, event_fut);
        select_biased! {
            _ = cancel_fut => return Err("cancelled while waiting outbound client connect".to_string()),
            _ = timeout_fut => return Err("timeout waiting outbound client connect".to_string()),
            maybe_event = event_fut => {
                match maybe_event {
                    Some(RtspClientEvent::Connected { .. }) => return Ok(()),
                    Some(RtspClientEvent::Closed { reason }) => {
                        return Err(format!("outbound client closed before connect: {reason}"));
                    }
                    Some(_) => {}
                    None => return Err("outbound client event channel closed".to_string()),
                }
            }
        }
    }
}

async fn wait_response_for_cseq(
    runtime_api: &Arc<dyn RuntimeApi>,
    client: &mut RtspClientHandle,
    expected_cseq: u32,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<RtspResponseMessage, String> {
    let deadline = runtime_deadline_after(runtime_api, timeout);
    let mut timeout_timer = runtime_api.sleep_until(deadline);
    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let timeout_fut = timeout_timer.wait().fuse();
        let event_fut = client.recv_event().fuse();
        pin_mut!(cancel_fut, timeout_fut, event_fut);
        select_biased! {
            _ = cancel_fut => return Err(format!("cancelled while waiting RTSP response cseq={expected_cseq}")),
            _ = timeout_fut => return Err(format!("timeout waiting RTSP response cseq={expected_cseq}")),
            maybe_event = event_fut => {
                match maybe_event {
                    Some(RtspClientEvent::Response { response }) => {
                        let response_cseq = response
                            .header_value("CSeq")
                            .and_then(|value| value.parse::<u32>().ok());
                        if response_cseq == Some(expected_cseq) {
                            return Ok(response);
                        }
                    }
                    Some(RtspClientEvent::Closed { reason }) => {
                        return Err(format!("outbound client closed while waiting response: {reason}"));
                    }
                    Some(_) => {}
                    None => return Err("outbound client event channel closed".to_string()),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_pull_session_end(
    client: &mut RtspClientHandle,
    cancel: &CancellationToken,
    publish: &mut PublishSession,
    setup: &PullSetupResult,
    source_url: &str,
    runtime_api: &Arc<dyn RuntimeApi>,
    auth: &mut PullOutboundAuthState,
    heartbeat_mode: RtspHeartbeatMode,
) -> Result<(), String> {
    let command_tx = client.command_sender();
    let mut keepalive_cseq = setup.next_cseq;
    let mut pending_keepalive = HashMap::<u32, bool>::new();
    let keepalive_interval = setup
        .session_timeout_secs
        .map(|timeout_secs| Duration::from_secs((timeout_secs / 2).max(1)));
    let mut next_keepalive_due =
        keepalive_interval.map(|interval| runtime_deadline_after(runtime_api, interval));

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let event_fut = client.recv_event().fuse();
        if let Some(keepalive_due) = next_keepalive_due {
            let mut keepalive_timer = runtime_api.sleep_until(keepalive_due);
            let keepalive_fut = keepalive_timer.wait().fuse();
            pin_mut!(cancel_fut, keepalive_fut, event_fut);
            select_biased! {
                _ = cancel_fut => return Ok(()),
                _ = keepalive_fut => {
                    let mut request_headers = vec![("Session", setup.session_token.as_str())];
                    let keepalive_method = match heartbeat_mode {
                        RtspHeartbeatMode::GetParameter => "GET_PARAMETER",
                        RtspHeartbeatMode::Options => "OPTIONS",
                    };
                    let rtsp_method = match heartbeat_mode {
                        RtspHeartbeatMode::GetParameter => RtspMethod::GetParameter,
                        RtspHeartbeatMode::Options => RtspMethod::Options,
                    };
                    let request_authorization =
                        build_request_authorization(auth, rtsp_method, source_url);
                    if let Some(value) = request_authorization.as_deref() {
                        request_headers.push(("Authorization", value));
                    }
                    command_tx
                        .send(RtspClientCommand::SendRequest(build_rtsp_request(
                            keepalive_method,
                            source_url,
                            keepalive_cseq,
                            request_headers.as_slice(),
                            &[],
                        )))
                        .await
                        .map_err(|err| format!("send keepalive {keepalive_method} failed: {err}"))?;
                    insert_pending_keepalive(&mut pending_keepalive, keepalive_cseq, false)?;
                    keepalive_cseq = keepalive_cseq.saturating_add(1);
                    next_keepalive_due =
                        keepalive_interval.map(|interval| runtime_deadline_after(runtime_api, interval));
                }
                maybe_event = event_fut => {
                    match maybe_event {
                        Some(RtspClientEvent::Response { response }) => {
                            let response_cseq = response
                                .header_value("CSeq")
                                .and_then(|value| value.parse::<u32>().ok());
                            let Some(response_cseq) = response_cseq else {
                                continue;
                            };
                            let Some(keepalive_retry) = pending_keepalive.remove(&response_cseq) else {
                                continue;
                            };
                            if response.status_code == 200 {
                                continue;
                            }
                            if response.status_code != 401 {
                                return Err(format!(
                                    "outbound pull keepalive failed with status {}",
                                    response.status_code
                                ));
                            }
                            if keepalive_retry {
                                return Err("outbound pull keepalive authorization retry returned 401".to_string());
                            }
                            let Some(credentials) = auth.credentials.as_ref() else {
                                return Err("outbound pull keepalive challenged with 401 but no credentials configured".to_string());
                            };
                            let Some(retry_authorization) = authorization_header_from_response(
                                &response,
                                RtspMethod::GetParameter,
                                source_url,
                                credentials,
                            ) else {
                                return Err("outbound pull keepalive 401 challenge is not supported".to_string());
                            };
                            auth.challenge = Some(response);
                            let retry_cseq = keepalive_cseq;
                            keepalive_cseq = keepalive_cseq.saturating_add(1);
                            let retry_headers = [
                                ("Session", setup.session_token.as_str()),
                                ("Authorization", retry_authorization.as_str()),
                            ];
                            command_tx
                                .send(RtspClientCommand::SendRequest(build_rtsp_request(
                                    "GET_PARAMETER",
                                    source_url,
                                    retry_cseq,
                                    &retry_headers,
                                    &[],
                                )))
                                .await
                                .map_err(|err| {
                                    format!(
                                        "retry keepalive GET_PARAMETER with authorization failed: {err}"
                                    )
                                })?;
                            insert_pending_keepalive(&mut pending_keepalive, retry_cseq, true)?;
                        }
                        Some(RtspClientEvent::InterleavedFrame { channel, payload }) => {
                            if let Some(track_id) = setup.interleaved_rtp_channels.get(&channel).copied() {
                                if let Some(packet) = RtpPacket::parse(payload.as_ref()) {
                                    ingest_publish_rtp_packet(
                                        PULL_INGEST_CONNECTION_ID,
                                        track_id,
                                        &packet,
                                        publish,
                                        runtime_api,
                                    );
                                }
                            } else if channel % 2 == 1 {
                                if let Ok(rr) = build_rtcp_empty_rr(0x01) {
                                    let _ = command_tx
                                        .send(RtspClientCommand::SendInterleaved {
                                            channel,
                                            payload: rr,
                                        })
                                        .await;
                                }
                            }
                        }
                        Some(RtspClientEvent::UdpRtp { track_id, payload, .. }) => {
                            if let Some(packet) = RtpPacket::parse(payload.as_ref()) {
                                ingest_publish_rtp_packet(
                                    PULL_INGEST_CONNECTION_ID,
                                    TrackId(track_id),
                                    &packet,
                                    publish,
                                    runtime_api,
                                );
                            }
                        }
                        Some(RtspClientEvent::Closed { reason }) => {
                            return Err(format!("outbound pull client closed: {reason}"));
                        }
                        Some(_) => {}
                        None => return Err("outbound client event channel closed".to_string()),
                    }
                }
            }
        } else {
            pin_mut!(cancel_fut, event_fut);
            select_biased! {
                _ = cancel_fut => return Ok(()),
                maybe_event = event_fut => {
                    match maybe_event {
                        Some(RtspClientEvent::Response { response }) => {
                            let response_cseq = response
                                .header_value("CSeq")
                                .and_then(|value| value.parse::<u32>().ok());
                            let Some(response_cseq) = response_cseq else {
                                continue;
                            };
                            let Some(keepalive_retry) = pending_keepalive.remove(&response_cseq) else {
                                continue;
                            };
                            if response.status_code == 200 {
                                continue;
                            }
                            if response.status_code != 401 {
                                return Err(format!(
                                    "outbound pull keepalive failed with status {}",
                                    response.status_code
                                ));
                            }
                            if keepalive_retry {
                                return Err("outbound pull keepalive authorization retry returned 401".to_string());
                            }
                            let Some(credentials) = auth.credentials.as_ref() else {
                                return Err("outbound pull keepalive challenged with 401 but no credentials configured".to_string());
                            };
                            let Some(retry_authorization) = authorization_header_from_response(
                                &response,
                                RtspMethod::GetParameter,
                                source_url,
                                credentials,
                            ) else {
                                return Err("outbound pull keepalive 401 challenge is not supported".to_string());
                            };
                            auth.challenge = Some(response);
                            let retry_cseq = keepalive_cseq;
                            keepalive_cseq = keepalive_cseq.saturating_add(1);
                            let retry_headers = [
                                ("Session", setup.session_token.as_str()),
                                ("Authorization", retry_authorization.as_str()),
                            ];
                            command_tx
                                .send(RtspClientCommand::SendRequest(build_rtsp_request(
                                    "GET_PARAMETER",
                                    source_url,
                                    retry_cseq,
                                    &retry_headers,
                                    &[],
                                )))
                                .await
                                .map_err(|err| {
                                    format!(
                                        "retry keepalive GET_PARAMETER with authorization failed: {err}"
                                    )
                                })?;
                            insert_pending_keepalive(&mut pending_keepalive, retry_cseq, true)?;
                        }
                        Some(RtspClientEvent::InterleavedFrame { channel, payload }) => {
                            if let Some(track_id) = setup.interleaved_rtp_channels.get(&channel).copied() {
                                if let Some(packet) = RtpPacket::parse(payload.as_ref()) {
                                    ingest_publish_rtp_packet(
                                        PULL_INGEST_CONNECTION_ID,
                                        track_id,
                                        &packet,
                                        publish,
                                        runtime_api,
                                    );
                                }
                            } else if channel % 2 == 1 {
                                if let Ok(rr) = build_rtcp_empty_rr(0x01) {
                                    let _ = command_tx
                                        .send(RtspClientCommand::SendInterleaved {
                                            channel,
                                            payload: rr,
                                        })
                                        .await;
                                }
                            }
                        }
                        Some(RtspClientEvent::UdpRtp { track_id, payload, .. }) => {
                            if let Some(packet) = RtpPacket::parse(payload.as_ref()) {
                                ingest_publish_rtp_packet(
                                    PULL_INGEST_CONNECTION_ID,
                                    TrackId(track_id),
                                    &packet,
                                    publish,
                                    runtime_api,
                                );
                            }
                        }
                        Some(RtspClientEvent::Closed { reason }) => {
                            return Err(format!("outbound pull client closed: {reason}"));
                        }
                        Some(_) => {}
                        None => return Err("outbound client event channel closed".to_string()),
                    }
                }
            }
        }
    }
}

pub(crate) fn tracks_to_map(
    tracks: &[cheetah_codec::TrackInfo],
) -> HashMap<TrackId, cheetah_codec::TrackInfo> {
    tracks
        .iter()
        .cloned()
        .map(|track| (track.track_id, track))
        .collect()
}

pub(crate) fn invert_track_controls(
    tracks: &[cheetah_codec::TrackInfo],
    control_map: &HashMap<String, TrackId>,
) -> HashMap<TrackId, String> {
    let mut out = HashMap::new();
    for (control, track_id) in control_map {
        out.entry(*track_id).or_insert_with(|| control.clone());
    }
    for (index, track) in tracks.iter().enumerate() {
        out.entry(track.track_id)
            .or_insert_with(|| format!("trackID={index}"));
    }
    out
}

pub(crate) async fn setup_pull_tracks_and_play(
    client: &mut RtspClientHandle,
    tracks: &[cheetah_codec::TrackInfo],
    track_controls: &HashMap<TrackId, String>,
    ctx: PullSetupContext<'_>,
) -> Result<PullSetupCompletion, String> {
    let mut cseq = ctx.start_cseq;
    let mut session_token: Option<String> = None;
    let mut session_timeout_secs: Option<u64> = None;
    let mut rtp_channels = HashMap::<u8, TrackId>::new();
    let mut udp_task_handles = Vec::<Box<dyn RuntimeJoinHandle>>::new();
    let mut pending_udp_receivers = Vec::<PendingPullUdpReceiver>::new();

    let mut ordered_tracks = tracks.to_vec();
    ordered_tracks.sort_by_key(|track| track.track_id.0);
    for (idx, track) in ordered_tracks.iter().enumerate() {
        let control = track_controls
            .get(&track.track_id)
            .ok_or_else(|| format!("track {} missing control mapping", track.track_id.0))?;
        let track_uri = build_track_control_uri(ctx.base_url, control);
        let rtp_channel = (idx as u8).saturating_mul(2);
        let rtcp_channel = rtp_channel.saturating_add(1);
        let udp_endpoint = if ctx.transport == PullSelectedTransport::Udp {
            let bind_ip = match ctx.peer.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            };
            Some(
                allocate_udp_endpoint(ctx.runtime_api, bind_ip, None)
                    .map_err(|err| format!("allocate pull udp endpoint failed: {err}"))?,
            )
        } else {
            None
        };
        let transport_value = match udp_endpoint.as_ref() {
            Some(endpoint) => format!(
                "RTP/AVP;unicast;client_port={}-{}",
                endpoint.local_rtp.port(),
                endpoint.local_rtcp.port()
            ),
            None => format!("RTP/AVP/TCP;unicast;interleaved={rtp_channel}-{rtcp_channel}"),
        };
        let mut request_headers = vec![("Transport", transport_value.as_str())];
        if let Some(session) = session_token.as_deref() {
            request_headers.push(("Session", session));
        }
        let setup_response = send_request_with_auth_retry(
            ctx.runtime_api,
            client,
            ctx.auth,
            RtspMethod::Setup,
            &track_uri,
            &mut cseq,
            request_headers.as_slice(),
            &[],
            ctx.cancel,
            ctx.request_timeout,
        )
        .await?;
        if setup_response.status_code != 200 {
            return Err(format!(
                "SETUP failed for track {} with status {}",
                track.track_id.0, setup_response.status_code
            ));
        }
        let transport = setup_response.header_value("Transport").ok_or_else(|| {
            format!(
                "SETUP response missing Transport header for track {}",
                track.track_id.0
            )
        })?;
        let parsed_transport = parse_setup_transport(transport).ok_or_else(|| {
            format!(
                "SETUP response Transport header parse failed for track {}: {transport}",
                track.track_id.0
            )
        })?;
        match (ctx.transport, parsed_transport, udp_endpoint) {
            (_, RtspSetupTransport::TcpInterleaved(channels), None) => {
                rtp_channels.insert(channels.rtp_channel, track.track_id);
            }
            (_, RtspSetupTransport::TcpInterleavedAuto, None) => {
                rtp_channels.insert(rtp_channel, track.track_id);
            }
            (PullSelectedTransport::Udp, RtspSetupTransport::UdpUnicast(ports), Some(endpoint)) => {
                let server_rtp = ports.server_rtp_port.ok_or_else(|| {
                    format!(
                        "SETUP response missing server RTP port for track {}",
                        track.track_id.0
                    )
                })?;
                let server_rtcp = ports.server_rtcp_port.ok_or_else(|| {
                    format!(
                        "SETUP response missing server RTCP port for track {}",
                        track.track_id.0
                    )
                })?;
                let remote_ip = ports.source.unwrap_or(ctx.peer.ip());
                let remote_rtp = SocketAddr::new(remote_ip, server_rtp);
                let remote_rtcp = SocketAddr::new(remote_ip, server_rtcp);
                configure_udp_remote_and_punch(&endpoint, remote_rtp, remote_rtcp)
                    .await
                    .map_err(|err| format!("configure pull udp remote failed: {err}"))?;
                pending_udp_receivers.push(PendingPullUdpReceiver {
                    endpoint,
                    track_id: track.track_id,
                    remote: RtspClientUdpRemote {
                        rtp: remote_rtp,
                        rtcp: remote_rtcp,
                    },
                });
            }
            (_, other, _) => {
                return Err(format!(
                    "SETUP response transport mismatch for track {}: {other:?}",
                    track.track_id.0
                ));
            }
        }
        if let Some(session_header) = setup_response.header_value("Session") {
            let token = parse_session_token(session_header).to_string();
            if token.is_empty() {
                return Err("SETUP response Session header is empty".to_string());
            }
            session_token = Some(token);
            session_timeout_secs = parse_session_timeout_secs(session_header);
        }
    }

    if rtp_channels.is_empty() && pending_udp_receivers.is_empty() {
        return Err("SETUP completed without any RTP interleaved channel".to_string());
    }
    let session = session_token
        .as_deref()
        .ok_or_else(|| "SETUP response missing Session header".to_string())?;
    let play_response = send_request_with_auth_retry(
        ctx.runtime_api,
        client,
        ctx.auth,
        RtspMethod::Play,
        ctx.source_url,
        &mut cseq,
        &[("Session", session)],
        &[],
        ctx.cancel,
        ctx.request_timeout,
    )
    .await?;
    if play_response.status_code != 200 {
        return Err(format!(
            "PLAY failed with status {}",
            play_response.status_code
        ));
    }
    for pending in pending_udp_receivers {
        udp_task_handles.extend(spawn_udp_receive_tasks(
            ctx.runtime_api.clone(),
            pending.endpoint,
            pending.track_id.0,
            Some(pending.remote),
            client.event_sender(),
            ctx.cancel.child_token(),
        ));
    }

    Ok(PullSetupCompletion {
        setup: PullSetupResult {
            interleaved_rtp_channels: rtp_channels,
            session_token: session.to_string(),
            session_timeout_secs,
            next_cseq: cseq,
        },
        udp_task_handles,
    })
}

fn build_track_control_uri(source_url: &str, control: &str) -> String {
    let control = control.trim();
    if control.to_ascii_lowercase().starts_with("rtsp://") {
        return control.to_string();
    }

    if control.starts_with('/') {
        let rest = source_url
            .strip_prefix("rtsp://")
            .or_else(|| source_url.strip_prefix("rtsps://"))
            .unwrap_or(source_url)
            .split('/')
            .next()
            .unwrap_or_default();
        let scheme = if source_url.starts_with("rtsps://") {
            "rtsps"
        } else {
            "rtsp"
        };
        return format!("{scheme}://{rest}{control}");
    }

    format!(
        "{}/{}",
        source_url.trim_end_matches('/'),
        control.trim_start_matches('/')
    )
}

fn runtime_deadline_after(
    runtime_api: &Arc<dyn RuntimeApi>,
    duration: Duration,
) -> cheetah_codec::MonoTime {
    let now_micros = runtime_api.now().as_micros();
    let delta = duration.as_micros().min(u128::from(u64::MAX)) as u64;
    cheetah_codec::MonoTime::from_micros(now_micros.saturating_add(delta))
}

fn insert_pending_keepalive(
    pending_keepalive: &mut HashMap<u32, bool>,
    cseq: u32,
    retried: bool,
) -> Result<(), String> {
    if pending_keepalive.len() >= MAX_PENDING_KEEPALIVE_REQUESTS {
        return Err(format!(
            "too many pending keepalive requests: {}",
            pending_keepalive.len()
        ));
    }
    pending_keepalive.insert(cseq, retried);
    Ok(())
}

async fn runtime_sleep(runtime_api: &Arc<dyn RuntimeApi>, duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let mut timer = runtime_api.sleep_until(runtime_deadline_after(runtime_api, duration));
    timer.wait().await;
}

async fn wait_or_cancel(
    runtime_api: &Arc<dyn RuntimeApi>,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let cancel_fut = cancel.cancelled().fuse();
    let sleep_fut = runtime_sleep(runtime_api, duration).fuse();
    pin_mut!(cancel_fut, sleep_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = sleep_fut => false,
    }
}

fn parse_session_timeout_secs(session_header: &str) -> Option<u64> {
    for part in session_header.split(';').skip(1) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("timeout") {
            let secs = value.trim().parse::<u64>().ok()?;
            if secs > 0 {
                return Some(secs);
            }
        }
    }
    None
}

fn next_retry_backoff(current: Duration, max: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > max {
        max
    } else {
        doubled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn parse_session_timeout_secs_extracts_timeout_parameter() {
        assert_eq!(parse_session_timeout_secs("abc123;timeout=60"), Some(60));
        assert_eq!(
            parse_session_timeout_secs("abc123;foo=1;timeout=2;bar=9"),
            Some(2)
        );
        assert_eq!(parse_session_timeout_secs("abc123"), None);
        assert_eq!(parse_session_timeout_secs("abc123;timeout=0"), None);
        assert_eq!(parse_session_timeout_secs("abc123;timeout=bad"), None);
        // Parameters without '=' should be skipped, not abort parsing.
        assert_eq!(
            parse_session_timeout_secs("abc123;noseparator;timeout=45"),
            Some(45)
        );
    }

    #[test]
    fn next_retry_backoff_doubles_and_caps_to_max() {
        let max = Duration::from_millis(500);
        assert_eq!(
            next_retry_backoff(Duration::from_millis(50), max),
            Duration::from_millis(100)
        );
        assert_eq!(
            next_retry_backoff(Duration::from_millis(300), max),
            Duration::from_millis(500)
        );
        assert_eq!(
            next_retry_backoff(Duration::from_millis(500), max),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn pull_transport_preferences_preserve_fallback_order() {
        let transports = supported_pull_transports(&[
            RtspPullTransport::Udp,
            RtspPullTransport::TcpInterleaved,
            RtspPullTransport::HttpTunnel,
            RtspPullTransport::Multicast,
        ])
        .expect("supported transports");

        assert_eq!(
            transports,
            vec![
                PullSelectedTransport::Udp,
                PullSelectedTransport::TcpInterleaved,
                PullSelectedTransport::HttpTunnel
            ]
        );
    }

    #[test]
    fn parse_rtsp_source_peer_accepts_default_port_authorities() {
        assert_eq!(
            parse_rtsp_source_peer("rtsp://10.0.0.5/live/test").expect("ipv4 authority"),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 554)
        );
        assert_eq!(
            parse_rtsp_source_peer("rtsp://[::1]/live/test").expect("ipv6 authority"),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 554)
        );
    }
}
