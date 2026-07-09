use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{AVFrame, TrackId, TrackInfo};
use cheetah_rtsp_core::{RtspMethod, RtspRequestMessage, RtspResponseMessage};
use cheetah_rtsp_driver_tokio::{
    allocate_udp_endpoint, authorization_header_from_response, client::RtspClientCommandSender,
    configure_udp_remote_and_punch, start_http_tunnel_client, start_tcp_client, RtspClientCommand,
    RtspClientConfig, RtspClientCredentials, RtspClientEvent, RtspClientHandle,
};
use cheetah_sdk::{
    AsyncUdpSocket, BootstrapPolicy, CancellationToken, EngineContext,
    JoinHandle as RuntimeJoinHandle, RuntimeApi, StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{info, warn};

use crate::config::{RtspModuleConfig, RtspPushJobConfig, RtspPushTransport};
use crate::media::{
    build_rtcp_sender_report, packetize_frame_to_rtp_with_timestamp, parse_setup_transport,
    RtspSetupTransport,
};
use crate::module::session_guard::{default_payload_type, runtime_unix_time_micros};
use crate::module::session_lifecycle::parse_session_token;
use crate::sdp::build_describe_sdp;

const MAX_PENDING_KEEPALIVE_REQUESTS: usize = 64;

pub(super) struct PushJobSupervisorHandle {
    job_name: String,
    join: Box<dyn RuntimeJoinHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushAttemptErrorKind {
    Retryable,
    StopJob,
}

#[derive(Debug)]
struct PushAttemptError {
    kind: PushAttemptErrorKind,
    message: String,
}

#[derive(Debug, Clone)]
struct PushTrackState {
    track_id: TrackId,
    payload_type: u8,
    seq: u16,
    ssrc: u32,
    transport: PushTrackTransport,
    packets_sent: u32,
    octets_sent: u32,
    last_rtp_timestamp: u32,
    sr_sent: bool,
}

#[derive(Clone)]
enum PushTrackTransport {
    TcpInterleaved {
        rtp_channel: u8,
        rtcp_channel: u8,
    },
    Udp {
        rtp_socket: Arc<dyn AsyncUdpSocket>,
        rtcp_socket: Arc<dyn AsyncUdpSocket>,
        target_rtp: SocketAddr,
        target_rtcp: SocketAddr,
    },
}

impl std::fmt::Debug for PushTrackTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TcpInterleaved {
                rtp_channel,
                rtcp_channel,
            } => f
                .debug_struct("TcpInterleaved")
                .field("rtp_channel", rtp_channel)
                .field("rtcp_channel", rtcp_channel)
                .finish(),
            Self::Udp {
                target_rtp,
                target_rtcp,
                ..
            } => f
                .debug_struct("Udp")
                .field("target_rtp", target_rtp)
                .field("target_rtcp", target_rtcp)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushSelectedTransport {
    TcpInterleaved,
    Udp,
    HttpTunnel,
}

struct PushSessionContext<'a> {
    engine: &'a EngineContext,
    runtime_api: &'a Arc<dyn RuntimeApi>,
    config: &'a RtspModuleConfig,
    source_stream_key: &'a StreamKey,
    source_tracks: &'a [TrackInfo],
    push_tracks: &'a mut [PushTrackState],
    session_token: &'a str,
    session_timeout_secs: Option<u64>,
    keepalive_cseq_start: u32,
    target_url: &'a str,
    auth: &'a mut PushOutboundAuthState,
    cancel: &'a CancellationToken,
}

struct PushSetupResult {
    push_tracks: Vec<PushTrackState>,
    session_token: String,
    session_timeout_secs: Option<u64>,
    next_cseq: u32,
}

#[derive(Default)]
struct PushOutboundAuthState {
    credentials: Option<RtspClientCredentials>,
    challenge: Option<RtspResponseMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrackShape {
    track_id: TrackId,
    media_kind: cheetah_codec::MediaKind,
    codec: cheetah_codec::CodecId,
    clock_rate: u32,
    payload_type: Option<u8>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
    aac_rtp_packetization: cheetah_codec::AacRtpPacketization,
    aac_latm_config_in_band: bool,
}

impl PushAttemptError {
    fn retry(message: String) -> Self {
        Self {
            kind: PushAttemptErrorKind::Retryable,
            message,
        }
    }

    fn stop(message: String) -> Self {
        Self {
            kind: PushAttemptErrorKind::StopJob,
            message,
        }
    }
}

pub(super) fn spawn_push_job_supervisors(
    engine: &EngineContext,
    config: &RtspModuleConfig,
    module_cancel: CancellationToken,
) -> Vec<PushJobSupervisorHandle> {
    let mut handles = Vec::new();
    for job in config.push_jobs.iter().filter(|job| job.enabled) {
        let engine_ctx = engine.clone();
        let runtime_api = engine.runtime_api.clone();
        let module_config = config.clone();
        let job_clone = job.clone();
        let cancel = module_cancel.child_token();
        let job_name = job.name.clone();
        let join = runtime_api.spawn(Box::pin(async move {
            run_push_job_supervisor(engine_ctx, module_config, job_clone, cancel).await;
        }));
        handles.push(PushJobSupervisorHandle { job_name, join });
    }
    handles
}

pub(super) async fn wait_push_job_supervisors(handles: &mut Vec<PushJobSupervisorHandle>) {
    for handle in handles.drain(..) {
        handle.join.abort();
        if let Err(err) = handle.join.wait().await {
            warn!(
                job = %handle.job_name,
                "push job supervisor exited with join error: {err}"
            );
        }
    }
}

async fn run_push_job_supervisor(
    engine: EngineContext,
    config: RtspModuleConfig,
    job: RtspPushJobConfig,
    cancel: CancellationToken,
) {
    let retry_backoff = Duration::from_millis(job.retry_backoff_ms.max(1));
    let max_retry_backoff =
        Duration::from_millis(job.max_retry_backoff_ms.max(job.retry_backoff_ms.max(1)));
    let mut current_backoff = retry_backoff;
    let request_timeout = Duration::from_secs(5);
    info!(
        job = %job.name,
        source_stream_key = %job.source_stream_key,
        target_url = %job.target_url,
        "rtsp push job supervisor started"
    );
    let transports = match supported_push_transports(&job.transport_preference) {
        Ok(transports) => transports,
        Err(err) => {
            warn!(job = %job.name, "push job has no supported transport: {err}");
            return;
        }
    };
    let mut transport_index = 0usize;

    loop {
        if cancel.is_cancelled() {
            break;
        }
        let selected_transport = transports[transport_index % transports.len()];
        match run_push_announce_once(
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
                    target_url = %job.target_url,
                    transport = ?selected_transport,
                    kind = ?err.kind,
                    "push control-plane attempt failed: {}",
                    err.message
                );
                if err.kind == PushAttemptErrorKind::StopJob {
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

    info!(job = %job.name, "rtsp push job supervisor stopped");
}

async fn run_push_announce_once(
    engine: &EngineContext,
    config: &RtspModuleConfig,
    job: &RtspPushJobConfig,
    cancel: &CancellationToken,
    request_timeout: Duration,
    selected_transport: PushSelectedTransport,
) -> Result<(), PushAttemptError> {
    let target_peer = parse_rtsp_peer(&job.target_url).map_err(PushAttemptError::retry)?;
    let source_stream_key =
        parse_source_stream_key(&job.source_stream_key).map_err(PushAttemptError::retry)?;
    let source_tracks = wait_source_tracks(engine, &source_stream_key, cancel)
        .await
        .map_err(PushAttemptError::retry)?;
    if source_tracks.is_empty() {
        return Err(PushAttemptError::stop(
            "source stream has no tracks for ANNOUNCE".to_string(),
        ));
    }

    let mut subscriber = engine
        .subscriber_api
        .subscribe(
            source_stream_key.clone(),
            SubscriberOptions {
                queue_capacity: config.subscriber_queue_capacity,
                backpressure: config.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::none(),
                ..Default::default()
            },
        )
        .await
        .map_err(|err| PushAttemptError::retry(format!("subscribe source stream failed: {err}")))?;

    let client_cancel = cancel.child_token();
    let mut client = start_push_client(
        engine.runtime_api.clone(),
        target_peer,
        &job.target_url,
        &job.name,
        selected_transport,
        client_cancel,
    )?;

    wait_client_connected(&engine.runtime_api, &mut client, cancel, request_timeout)
        .await
        .map_err(PushAttemptError::retry)?;
    let mut outbound_auth = build_push_outbound_auth_state(job);
    let mut cseq = 1_u32;

    let options_response = send_request_with_auth_retry(
        &engine.runtime_api,
        &mut client,
        &mut outbound_auth,
        RtspMethod::Options,
        &job.target_url,
        &mut cseq,
        &[],
        &[],
        cancel,
        request_timeout,
    )
    .await
    .map_err(PushAttemptError::retry)?;
    if options_response.status_code != 200 {
        client.shutdown();
        let _ = subscriber.close().await;
        return Err(PushAttemptError::retry(format!(
            "OPTIONS failed with status {}",
            options_response.status_code
        )));
    }

    let (announce_sdp, control_map) = build_describe_sdp(&job.target_url, &source_tracks);
    let announce_response = send_request_with_auth_retry(
        &engine.runtime_api,
        &mut client,
        &mut outbound_auth,
        RtspMethod::Announce,
        &job.target_url,
        &mut cseq,
        &[("Content-Type", "application/sdp")],
        announce_sdp.as_bytes(),
        cancel,
        request_timeout,
    )
    .await
    .map_err(PushAttemptError::retry)?;
    if announce_response.status_code != 200 {
        client.shutdown();
        let _ = subscriber.close().await;
        return Err(PushAttemptError::retry(format!(
            "ANNOUNCE failed with status {}",
            announce_response.status_code
        )));
    }
    let track_controls = invert_track_controls(&source_tracks, &control_map);
    let mut push_setup = setup_push_tracks_and_record(
        &engine.runtime_api,
        &mut client,
        job,
        &source_tracks,
        &track_controls,
        &mut outbound_auth,
        target_peer,
        selected_transport,
        cseq,
        cancel,
        request_timeout,
    )
    .await
    .map_err(PushAttemptError::retry)?;

    info!(
        job = %job.name,
        stream_key = %source_stream_key,
        track_count = source_tracks.len(),
        keepalive_timeout_secs = push_setup.session_timeout_secs.unwrap_or(0),
        "push job source subscribed and ANNOUNCE/SETUP/RECORD completed"
    );

    let mut session_ctx = PushSessionContext {
        engine,
        runtime_api: &engine.runtime_api,
        config,
        source_stream_key: &source_stream_key,
        source_tracks: &source_tracks,
        push_tracks: &mut push_setup.push_tracks,
        session_token: push_setup.session_token.as_str(),
        session_timeout_secs: push_setup.session_timeout_secs,
        keepalive_cseq_start: push_setup.next_cseq,
        target_url: &job.target_url,
        auth: &mut outbound_auth,
        cancel,
    };
    let session_result =
        wait_push_session_end(&mut client, &mut subscriber, &mut session_ctx).await;
    client.shutdown();
    let _ = subscriber.close().await;
    session_result.map_err(PushAttemptError::retry)
}

async fn wait_push_session_end(
    client: &mut RtspClientHandle,
    subscriber: &mut Box<dyn cheetah_sdk::SubscriberSource>,
    ctx: &mut PushSessionContext<'_>,
) -> Result<(), String> {
    let track_map = ctx
        .source_tracks
        .iter()
        .cloned()
        .map(|track| (track.track_id, track))
        .collect::<std::collections::HashMap<TrackId, TrackInfo>>();
    if track_map.is_empty() {
        return Err("push track map is empty".to_string());
    }

    let mut push_track_map = ctx
        .push_tracks
        .iter()
        .cloned()
        .map(|state| (state.track_id, state))
        .collect::<std::collections::HashMap<TrackId, PushTrackState>>();
    let command_tx = client.command_sender();
    let expected_track_shape = track_shapes(ctx.source_tracks);
    let track_poll_interval = Duration::from_millis(500);
    let mut next_track_poll_due = runtime_deadline_after(ctx.runtime_api, track_poll_interval);
    let keepalive_interval = ctx
        .session_timeout_secs
        .map(|timeout_secs| Duration::from_secs((timeout_secs / 2).max(1)));
    let mut next_keepalive_due_micros = keepalive_interval.map(|interval| {
        ctx.runtime_api
            .now()
            .as_micros()
            .saturating_add(interval.as_micros().min(u128::from(u64::MAX)) as u64)
    });
    let mut keepalive_cseq = ctx.keepalive_cseq_start;
    let mut pending_keepalive = std::collections::HashMap::<u32, bool>::new();

    loop {
        if let (Some(interval), Some(keepalive_due_micros)) =
            (keepalive_interval, next_keepalive_due_micros)
        {
            let now_micros = ctx.runtime_api.now().as_micros();
            if now_micros >= keepalive_due_micros {
                let mut request_headers = vec![("Session", ctx.session_token)];
                let request_authorization =
                    build_request_authorization(ctx.auth, RtspMethod::GetParameter, ctx.target_url);
                if let Some(value) = request_authorization.as_deref() {
                    request_headers.push(("Authorization", value));
                }
                command_tx
                    .send(RtspClientCommand::SendRequest(build_rtsp_request(
                        "GET_PARAMETER",
                        ctx.target_url,
                        keepalive_cseq,
                        request_headers.as_slice(),
                        &[],
                    )))
                    .await
                    .map_err(|err| format!("send push keepalive GET_PARAMETER failed: {err}"))?;
                insert_pending_keepalive(&mut pending_keepalive, keepalive_cseq, false)?;
                keepalive_cseq = keepalive_cseq.saturating_add(1);
                next_keepalive_due_micros = Some(
                    now_micros
                        .saturating_add(interval.as_micros().min(u128::from(u64::MAX)) as u64),
                );
            }
        }

        let cancel_fut = ctx.cancel.cancelled().fuse();
        let event_fut = client.recv_event().fuse();
        let frame_fut = subscriber.recv().fuse();
        let mut track_poll_timer = ctx.runtime_api.sleep_until(next_track_poll_due);
        let track_poll_fut = track_poll_timer.wait().fuse();
        pin_mut!(cancel_fut, event_fut, frame_fut, track_poll_fut);
        select_biased! {
            _ = cancel_fut => return Ok(()),
            _ = track_poll_fut => {
                let latest_tracks = match ctx
                    .engine
                    .stream_manager_api
                    .get_stream(ctx.source_stream_key)
                    .await
                {
                    Ok(Some(snapshot)) if !snapshot.tracks.is_empty() => snapshot.tracks,
                    Ok(Some(_)) => {
                        return Err("source stream tracks became empty, rebuild push session".to_string());
                    }
                    Ok(None) => {
                        return Err("source stream disappeared, rebuild push session".to_string());
                    }
                    Err(err) => {
                        return Err(format!("query source stream snapshot failed: {err}"));
                    }
                };
                if track_shapes(&latest_tracks) != expected_track_shape {
                    return Err("source stream tracks changed, rebuild push session".to_string());
                }
                next_track_poll_due = runtime_deadline_after(ctx.runtime_api, track_poll_interval);
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
                                "outbound push keepalive failed with status {}",
                                response.status_code
                            ));
                        }
                        if keepalive_retry {
                            return Err("outbound push keepalive authorization retry returned 401".to_string());
                        }
                        let Some(credentials) = ctx.auth.credentials.as_ref() else {
                            return Err("outbound push keepalive challenged with 401 but no credentials configured".to_string());
                        };
                        let Some(retry_authorization) = authorization_header_from_response(
                            &response,
                            RtspMethod::GetParameter,
                            ctx.target_url,
                            credentials,
                        ) else {
                            return Err("outbound push keepalive 401 challenge is not supported".to_string());
                        };
                        ctx.auth.challenge = Some(response);
                        let retry_cseq = keepalive_cseq;
                        keepalive_cseq = keepalive_cseq.saturating_add(1);
                        let retry_headers = [
                            ("Session", ctx.session_token),
                            ("Authorization", retry_authorization.as_str()),
                        ];
                        command_tx
                            .send(RtspClientCommand::SendRequest(build_rtsp_request(
                                "GET_PARAMETER",
                                ctx.target_url,
                                retry_cseq,
                                &retry_headers,
                                &[],
                            )))
                            .await
                            .map_err(|err| {
                                format!(
                                    "retry push keepalive GET_PARAMETER with authorization failed: {err}"
                                )
                            })?;
                        insert_pending_keepalive(&mut pending_keepalive, retry_cseq, true)?;
                    }
                    Some(RtspClientEvent::Closed { reason }) => {
                        return Err(format!("outbound push client closed: {reason}"));
                    }
                    Some(_) => {}
                    None => return Err("outbound push client event channel closed".to_string()),
                }
            }
            recv_result = frame_fut => {
                match recv_result {
                    Ok(Some(frame)) => {
                        let Some(track) = track_map.get(&frame.track_id) else {
                            continue;
                        };
                        let Some(state) = push_track_map.get_mut(&frame.track_id) else {
                            continue;
                        };
                        send_push_frame(
                            ctx.runtime_api,
                            ctx.config,
                            &command_tx,
                            frame.as_ref(),
                            track,
                            state,
                        ).await?;
                    }
                    Ok(None) => return Err("source subscriber ended".to_string()),
                    Err(err) => return Err(format!("source subscriber receive failed: {err}")),
                }
            }
        }
    }
}

fn track_shapes(tracks: &[TrackInfo]) -> Vec<TrackShape> {
    let mut out = tracks
        .iter()
        .map(|track| TrackShape {
            track_id: track.track_id,
            media_kind: track.media_kind,
            codec: track.codec,
            clock_rate: track.clock_rate,
            payload_type: track.payload_type,
            sample_rate: track.sample_rate,
            channels: track.channels,
            aac_rtp_packetization: track.aac_rtp_packetization,
            aac_latm_config_in_band: track.aac_latm_config_in_band,
        })
        .collect::<Vec<_>>();
    out.sort_by_key(|shape| shape.track_id.0);
    out
}

async fn send_push_frame(
    runtime_api: &Arc<dyn RuntimeApi>,
    config: &RtspModuleConfig,
    command_tx: &RtspClientCommandSender,
    frame: &AVFrame,
    track: &TrackInfo,
    state: &mut PushTrackState,
) -> Result<(), String> {
    let (primary_ts, fallback_ts) =
        cheetah_codec::select_egress_timestamps(track.media_kind, frame.pts, frame.dts);
    let rtp_timestamp = cheetah_codec::media_ts_to_rtp_ticks(
        primary_ts,
        fallback_ts,
        frame.timebase,
        track.clock_rate,
    );
    let packets = packetize_frame_to_rtp_with_timestamp(
        frame,
        track,
        state.payload_type,
        &mut state.seq,
        state.ssrc,
        config.rtp_mtu,
        rtp_timestamp,
    );
    if packets.is_empty() {
        return Ok(());
    }
    for packet in packets {
        state.packets_sent = state.packets_sent.wrapping_add(1);
        state.octets_sent = state
            .octets_sent
            .wrapping_add(packet.payload.len().min(u32::MAX as usize) as u32);
        state.last_rtp_timestamp = packet.header.timestamp;
        let payload = packet.encode();
        match &state.transport {
            PushTrackTransport::TcpInterleaved { rtp_channel, .. } => {
                command_tx
                    .send(RtspClientCommand::SendInterleaved {
                        channel: *rtp_channel,
                        payload,
                    })
                    .await
                    .map_err(|err| format!("send push interleaved RTP failed: {err}"))?;
            }
            PushTrackTransport::Udp {
                rtp_socket,
                target_rtp,
                ..
            } => {
                rtp_socket
                    .send_to(payload.as_ref(), *target_rtp)
                    .await
                    .map_err(|err| format!("send push UDP RTP failed: {err}"))?;
            }
        }
    }

    if !state.sr_sent {
        let sr = build_rtcp_sender_report(
            state.ssrc,
            state.last_rtp_timestamp,
            state.packets_sent,
            state.octets_sent,
            runtime_unix_time_micros(runtime_api),
        )
        .map_err(|err| format!("build push RTCP SR failed: {err}"))?;
        match &state.transport {
            PushTrackTransport::TcpInterleaved { rtcp_channel, .. } => {
                command_tx
                    .send(RtspClientCommand::SendInterleaved {
                        channel: *rtcp_channel,
                        payload: sr,
                    })
                    .await
                    .map_err(|err| format!("send push interleaved RTCP SR failed: {err}"))?;
            }
            PushTrackTransport::Udp {
                rtcp_socket,
                target_rtcp,
                ..
            } => {
                rtcp_socket
                    .send_to(sr.as_ref(), *target_rtcp)
                    .await
                    .map_err(|err| format!("send push UDP RTCP SR failed: {err}"))?;
            }
        }
        state.sr_sent = true;
    }
    Ok(())
}

fn invert_track_controls(
    tracks: &[TrackInfo],
    control_map: &std::collections::HashMap<String, TrackId>,
) -> std::collections::HashMap<TrackId, String> {
    let mut out = std::collections::HashMap::new();
    for (control, track_id) in control_map {
        out.entry(*track_id).or_insert_with(|| control.clone());
    }
    for (index, track) in tracks.iter().enumerate() {
        out.entry(track.track_id)
            .or_insert_with(|| format!("trackID={index}"));
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn setup_push_tracks_and_record(
    runtime_api: &Arc<dyn RuntimeApi>,
    client: &mut RtspClientHandle,
    job: &RtspPushJobConfig,
    tracks: &[TrackInfo],
    track_controls: &std::collections::HashMap<TrackId, String>,
    auth: &mut PushOutboundAuthState,
    target_peer: SocketAddr,
    selected_transport: PushSelectedTransport,
    start_cseq: u32,
    cancel: &CancellationToken,
    request_timeout: Duration,
) -> Result<PushSetupResult, String> {
    let mut cseq = start_cseq;
    let mut session_token: Option<String> = None;
    let mut session_timeout_secs: Option<u64> = None;
    let mut setup_tracks = Vec::new();

    let mut ordered_tracks = tracks.to_vec();
    ordered_tracks.sort_by_key(|track| track.track_id.0);
    for (index, track) in ordered_tracks.iter().enumerate() {
        let control = track_controls
            .get(&track.track_id)
            .ok_or_else(|| format!("track {} missing control mapping", track.track_id.0))?;
        let track_uri = build_track_control_uri(&job.target_url, control);
        let rtp_channel = (index as u8).saturating_mul(2);
        let rtcp_channel = rtp_channel.saturating_add(1);
        let udp_endpoint = if selected_transport == PushSelectedTransport::Udp {
            let bind_ip = match target_peer.ip() {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            };
            Some(
                allocate_udp_endpoint(runtime_api, bind_ip, None)
                    .map_err(|err| format!("allocate push udp endpoint failed: {err}"))?,
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
            runtime_api,
            client,
            auth,
            RtspMethod::Setup,
            &track_uri,
            &mut cseq,
            request_headers.as_slice(),
            &[],
            cancel,
            request_timeout,
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
                "SETUP response Transport parse failed for track {}: {transport}",
                track.track_id.0
            )
        })?;
        let push_transport = match (selected_transport, parsed_transport, udp_endpoint) {
            (_, RtspSetupTransport::TcpInterleaved(channels), None) => {
                PushTrackTransport::TcpInterleaved {
                    rtp_channel: channels.rtp_channel,
                    rtcp_channel: channels.rtcp_channel,
                }
            }
            (_, RtspSetupTransport::TcpInterleavedAuto, None) => {
                PushTrackTransport::TcpInterleaved {
                    rtp_channel,
                    rtcp_channel,
                }
            }
            (PushSelectedTransport::Udp, RtspSetupTransport::UdpUnicast(ports), Some(endpoint)) => {
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
                let target_rtp = SocketAddr::new(target_peer.ip(), server_rtp);
                let target_rtcp = SocketAddr::new(target_peer.ip(), server_rtcp);
                configure_udp_remote_and_punch(&endpoint, target_rtp, target_rtcp)
                    .await
                    .map_err(|err| format!("configure push udp remote failed: {err}"))?;
                PushTrackTransport::Udp {
                    rtp_socket: endpoint.rtp_socket,
                    rtcp_socket: endpoint.rtcp_socket,
                    target_rtp,
                    target_rtcp,
                }
            }
            (_, other, _) => {
                return Err(format!(
                    "SETUP response transport mismatch for track {}: {other:?}",
                    track.track_id.0
                ));
            }
        };
        if let Some(session_header) = setup_response.header_value("Session") {
            let token = parse_session_token(session_header).to_string();
            if token.is_empty() {
                return Err("SETUP response Session header is empty".to_string());
            }
            session_token = Some(token);
            session_timeout_secs = parse_session_timeout_secs(session_header);
        }
        setup_tracks.push(PushTrackState {
            track_id: track.track_id,
            payload_type: track
                .payload_type
                .unwrap_or_else(|| default_payload_type(track.codec)),
            seq: 1,
            ssrc: 0x4455_0000u32.wrapping_add(track.track_id.0),
            transport: push_transport,
            packets_sent: 0,
            octets_sent: 0,
            last_rtp_timestamp: 0,
            sr_sent: false,
        });
    }

    let session = session_token
        .as_deref()
        .ok_or_else(|| "SETUP response missing Session header".to_string())?;
    let record_response = send_request_with_auth_retry(
        runtime_api,
        client,
        auth,
        RtspMethod::Record,
        &job.target_url,
        &mut cseq,
        &[("Session", session)],
        &[],
        cancel,
        request_timeout,
    )
    .await?;
    if record_response.status_code != 200 {
        return Err(format!(
            "RECORD failed with status {}",
            record_response.status_code
        ));
    }
    Ok(PushSetupResult {
        push_tracks: setup_tracks,
        session_token: session.to_string(),
        session_timeout_secs,
        next_cseq: cseq,
    })
}

fn build_track_control_uri(base_url: &str, control: &str) -> String {
    let control = control.trim();
    if control.to_ascii_lowercase().starts_with("rtsp://") {
        return control.to_string();
    }
    if control.starts_with('/') {
        let rest = base_url
            .strip_prefix("rtsp://")
            .or_else(|| base_url.strip_prefix("rtsps://"))
            .unwrap_or(base_url)
            .split('/')
            .next()
            .unwrap_or_default();
        let scheme = if base_url.starts_with("rtsps://") {
            "rtsps"
        } else {
            "rtsp"
        };
        return format!("{scheme}://{rest}{control}");
    }
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        control.trim_start_matches('/')
    )
}

async fn wait_source_tracks(
    engine: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
) -> Result<Vec<cheetah_codec::TrackInfo>, String> {
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled while waiting source stream".to_string());
        }
        match engine.stream_manager_api.get_stream(stream_key).await {
            Ok(Some(snapshot)) if !snapshot.tracks.is_empty() => return Ok(snapshot.tracks),
            Ok(_) => {
                if wait_or_cancel(&engine.runtime_api, cancel, Duration::from_millis(100)).await {
                    return Err("cancelled while waiting source stream".to_string());
                }
            }
            Err(err) => return Err(format!("query source stream snapshot failed: {err}")),
        }
    }
}

fn parse_rtsp_peer(target_url: &str) -> Result<SocketAddr, String> {
    let target = target_url.trim();
    let rest = target
        .strip_prefix("rtsp://")
        .or_else(|| target.strip_prefix("rtsps://"))
        .ok_or_else(|| "target_url must start with rtsp:// or rtsps://".to_string())?;
    let authority = rest
        .split('/')
        .next()
        .ok_or_else(|| "target_url missing authority".to_string())?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if authority.is_empty() {
        return Err("target_url missing host".to_string());
    }
    resolve_rtsp_authority(authority, "target_url")
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

fn supported_push_transports(
    preferences: &[RtspPushTransport],
) -> Result<Vec<PushSelectedTransport>, String> {
    let mut transports = Vec::new();
    for transport in preferences {
        match transport {
            RtspPushTransport::TcpInterleaved => {
                transports.push(PushSelectedTransport::TcpInterleaved);
            }
            RtspPushTransport::Udp => transports.push(PushSelectedTransport::Udp),
            RtspPushTransport::HttpTunnel => transports.push(PushSelectedTransport::HttpTunnel),
        }
    }
    if transports.is_empty() {
        Err("push job transport preference contains no supported transport".to_string())
    } else {
        Ok(transports)
    }
}

fn start_push_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    target_url: &str,
    job_name: &str,
    transport: PushSelectedTransport,
    cancel: CancellationToken,
) -> Result<RtspClientHandle, PushAttemptError> {
    match transport {
        PushSelectedTransport::TcpInterleaved | PushSelectedTransport::Udp => {
            start_tcp_client(runtime_api, peer, RtspClientConfig::default(), cancel).map_err(
                |err| PushAttemptError::retry(format!("start outbound tcp client failed: {err}")),
            )
        }
        PushSelectedTransport::HttpTunnel => start_http_tunnel_client(
            runtime_api,
            peer,
            rtsp_url_path(target_url).map_err(PushAttemptError::retry)?,
            format!("cheetah-push-{job_name}"),
            RtspClientConfig::default(),
            cancel,
        )
        .map_err(|err| {
            PushAttemptError::retry(format!("start outbound http tunnel client failed: {err}"))
        }),
    }
}

fn rtsp_url_path(url: &str) -> Result<String, String> {
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

fn parse_source_stream_key(source_stream_key: &str) -> Result<StreamKey, String> {
    let trimmed = source_stream_key.trim();
    if trimmed.is_empty() {
        return Err("source_stream_key must not be empty".to_string());
    }
    let pseudo_uri = format!("rtsp://127.0.0.1/{trimmed}");
    crate::media::parse_stream_key_from_uri(&pseudo_uri)
        .ok_or_else(|| "source_stream_key format is invalid".to_string())
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

fn build_push_outbound_auth_state(job: &RtspPushJobConfig) -> PushOutboundAuthState {
    let credentials = job.username.as_ref().map(|username| RtspClientCredentials {
        username: username.to_string(),
        password: job.password.as_deref().unwrap_or_default().to_string(),
    });
    PushOutboundAuthState {
        credentials,
        challenge: None,
    }
}

fn build_request_authorization(
    auth: &PushOutboundAuthState,
    method: RtspMethod,
    uri: &str,
) -> Option<String> {
    let credentials = auth.credentials.as_ref()?;
    let challenge = auth.challenge.as_ref()?;
    authorization_header_from_response(challenge, method, uri, credentials)
}

#[allow(clippy::too_many_arguments)]
async fn send_request_with_auth_retry(
    runtime_api: &Arc<dyn RuntimeApi>,
    client: &mut RtspClientHandle,
    auth: &mut PushOutboundAuthState,
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

async fn wait_client_connected(
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

fn insert_pending_keepalive(
    pending_keepalive: &mut std::collections::HashMap<u32, bool>,
    cseq: u32,
    retried: bool,
) -> Result<(), String> {
    if pending_keepalive.len() >= MAX_PENDING_KEEPALIVE_REQUESTS {
        return Err(format!(
            "too many pending push keepalive requests: {}",
            pending_keepalive.len()
        ));
    }
    pending_keepalive.insert(cseq, retried);
    Ok(())
}

fn parse_session_timeout_secs(session_header: &str) -> Option<u64> {
    let timeout = session_header.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("timeout") {
            return None;
        }
        value.trim().parse::<u64>().ok()
    })?;
    if timeout == 0 {
        None
    } else {
        Some(timeout)
    }
}

fn runtime_deadline_after(
    runtime_api: &Arc<dyn RuntimeApi>,
    duration: Duration,
) -> cheetah_codec::MonoTime {
    let now_micros = runtime_api.now().as_micros();
    let delta = duration.as_micros().min(u128::from(u64::MAX)) as u64;
    cheetah_codec::MonoTime::from_micros(now_micros.saturating_add(delta))
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
    fn parse_rtsp_peer_accepts_default_port_authorities() {
        assert_eq!(
            parse_rtsp_peer("rtsp://10.0.0.6/live/target").expect("ipv4 authority"),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 6)), 554)
        );
        assert_eq!(
            parse_rtsp_peer("rtsp://[::1]/live/target").expect("ipv6 authority"),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 554)
        );
    }

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
    }

    #[test]
    fn push_transport_preferences_preserve_fallback_order() {
        let transports = supported_push_transports(&[
            RtspPushTransport::Udp,
            RtspPushTransport::TcpInterleaved,
            RtspPushTransport::HttpTunnel,
        ])
        .expect("supported transports");

        assert_eq!(
            transports,
            vec![
                PushSelectedTransport::Udp,
                PushSelectedTransport::TcpInterleaved,
                PushSelectedTransport::HttpTunnel
            ]
        );
    }
}
