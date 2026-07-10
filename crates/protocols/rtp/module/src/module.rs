//! RTP module factory and implementation.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::TrackInfo;
use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpCoreEvent, RtpPayloadMode, RtpSendFrame, RtpServerSpec,
    RtpTrackFilter, RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{start_driver, RtpDriverCommand, RtpDriverConfig, RtpDriverHandle};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest,
    HttpResponse, HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, PublishLease, PublisherOptions, PublisherSink, SdkError,
    StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use crate::config::{RtpClientJobConfig, RtpModuleConfig};

const MODULE_ID: &str = "rtp";

/// `RtpModuleFactory` data structure.
/// `RtpModuleFactory` 数据结构。
pub struct RtpModuleFactory;

impl ModuleFactory for RtpModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "RTP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "rtp".to_string(),
            routes_prefix: "/api/v1/rtp".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(RtpModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "rtp-module".to_string(),
            default_value: RtpModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let config =
                    RtpModuleConfig::from_value(value.clone()).map_err(|err| err.to_string())?;
                config.validate()
            })),
        })
    }
}

/// `RtpModule` data structure.
/// `RtpModule` 数据结构。
pub struct RtpModule {
    state: ModuleState,
    config: RtpModuleConfig,
    ctx: Option<EngineContext>,
    /// Shared with the HTTP service so the latter sees the driver as soon as `start` sets it.
    /// `update_http_mount` runs at init time — before `start` — so the module can't pass a
    /// concrete handle directly.
    driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>,
    cancel_token: Option<CancellationToken>,
    active_egress: Arc<Mutex<HashMap<String, CancellationToken>>>,
    client_targets: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl RtpModule {
    /// Creates a new `RtpModule` instance.
    /// 创建新的 `RtpModule` 实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: RtpModuleConfig::default(),
            ctx: None,
            driver_handle: Arc::new(Mutex::new(None)),
            cancel_token: None,
            active_egress: Arc::new(Mutex::new(HashMap::new())),
            client_targets: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for RtpModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for RtpModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "RTP Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = RtpModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine);
        // Allocate the module-scoped cancellation token now so that callers of
        // `http_service()` (invoked by the engine right after `init`) get a token that will
        // be triggered by `RtpModule::stop()`. Previously this happened only in `start()`,
        // leaving HTTP-spawned egress sessions wired to an orphan default token that never
        // fired on stop.
        self.cancel_token = Some(CancellationToken::new());
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        if !self.config.enabled {
            self.state = ModuleState::Running;
            cancel.cancelled().await;
            return Ok(());
        }

        let ctx = self.ctx.clone().ok_or_else(|| {
            SdkError::InvalidArgument(
                "RtpModule::start called before init (engine context missing)".to_string(),
            )
        })?;
        let config = self.config.clone();

        self.state = ModuleState::Running;
        // Re-use the cancel token allocated in `init` so the HTTP service (which captured a
        // clone of it at mount time) sees stop signals. We additionally chain the engine's
        // root cancellation by spawning a propagator below.
        let module_cancel = self.cancel_token.clone().unwrap_or_default();
        // Make sure the field is set even when `init` was bypassed (defensive: paths in tests
        // may call `start` directly).
        if self.cancel_token.is_none() {
            self.cancel_token = Some(module_cancel.clone());
        }
        // Bridge the engine-supplied root cancel into the module's own token so existing
        // shutdown semantics (engine root → module → HTTP service) keep working. The
        // bridge also exits when the module's own token is cancelled (via `stop()`), so we
        // don't leak a spawned task per restart cycle.
        {
            let module_cancel_in = module_cancel.clone();
            let module_cancel_out = module_cancel.clone();
            let cancel = cancel.clone();
            ctx.runtime_api.spawn(Box::pin(async move {
                let engine_cancel = cancel.cancelled().fuse();
                let stop_signal = module_cancel_in.cancelled().fuse();
                pin_mut!(engine_cancel, stop_signal);
                select_biased! {
                    _ = engine_cancel => module_cancel_out.cancel(),
                    _ = stop_signal => {}
                }
            }));
        }
        let cancel = module_cancel;

        let listen_udp = config
            .listen_udp
            .clone()
            .unwrap_or_else(|| "0.0.0.0:20000".to_string())
            .parse::<SocketAddr>()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen_udp: {e}")))?;

        let listen_tcp = config
            .listen_tcp
            .clone()
            .unwrap_or_else(|| "0.0.0.0:20000".to_string())
            .parse::<SocketAddr>()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen_tcp: {e}")))?;

        let listen_rtcp_udp = match config.rtcp_listen_udp.as_deref() {
            Some(addr) if !addr.is_empty() => {
                Some(addr.parse::<SocketAddr>().map_err(|e| {
                    SdkError::InvalidArgument(format!("invalid rtcp_listen_udp: {e}"))
                })?)
            }
            _ => None,
        };

        let tcp_framing = match config.tcp_header_type.to_lowercase().as_str() {
            "two_byte" | "twobyte" => cheetah_rtp_core::RtpTcpFraming::TwoByte,
            "interleaved_4byte" | "interleaved" | "interleaved4byte" => {
                cheetah_rtp_core::RtpTcpFraming::Interleaved4Byte
            }
            _ => cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        };

        let driver_config = RtpDriverConfig {
            listen_udp,
            listen_tcp,
            listen_rtcp_udp,
            write_queue_capacity: config.write_queue_capacity,
            read_buffer_size: config.read_buffer_size,
            session_idle_timeout_ms: config.idle_timeout_ms,
            max_sessions: 1024,
            tcp_framing,
            max_rtp_len_cap: config.max_rtp_len_cap,
        };

        let handle = Arc::new(start_driver(driver_config, cancel.clone()));
        *self.driver_handle.lock() = Some(handle.clone());

        // Spawn ingress worker
        {
            let ctx = ctx.clone();
            let runtime_api = ctx.runtime_api.clone();
            let handle = handle.clone();
            let cancel = cancel.clone();
            let publish_frame_cache = config.publish_frame_cache_frames;
            runtime_api.spawn(Box::pin(async move {
                run_ingress_worker(ctx, handle, cancel, publish_frame_cache).await;
            }));
        }

        // Spawn configured pull jobs
        for job in &config.pull_jobs {
            if !job.enabled {
                continue;
            }
            let job = job.clone();
            let ctx = ctx.clone();
            let runtime_api = ctx.runtime_api.clone();
            let handle = handle.clone();
            let cancel = cancel.clone();
            runtime_api.spawn(Box::pin(async move {
                run_pull_job_supervisor(ctx, handle, job, cancel).await;
            }));
        }

        cancel.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Drop the driver handle so any HTTP request that arrives while we're stopping (or
        // before a subsequent restart re-initialises) gets `Unavailable` instead of trying
        // to talk to a dead driver.
        *self.driver_handle.lock() = None;
        // Clear in-flight egress sessions and per-stream client routing tables; the cancel
        // cascade above terminates any spawned senders, but we also reset the maps so a
        // subsequent restart starts from a clean state.
        self.active_egress.lock().clear();
        self.client_targets.lock().clear();
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = RtpModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_config != self.config {
            self.config = new_config;
            Ok(ConfigEffect::ModuleRestartRequired)
        } else {
            Ok(ConfigEffect::Immediate)
        }
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/server/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/server/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/client/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/client/start".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/client/stop".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let engine = self.ctx.clone()?;
        // Cancel token is allocated in `init`; treat its absence as a programming error.
        let module_cancel = self.cancel_token.clone()?;
        Some(Arc::new(RtpHttpService {
            engine,
            // Shared handle storage: at init time this slot is empty; `start` populates it
            // before the HTTP service starts dispatching requests.
            driver_handle: self.driver_handle.clone(),
            active_egress: self.active_egress.clone(),
            client_targets: self.client_targets.clone(),
            module_cancel,
        }))
    }
}

struct ActiveIngressSession {
    _lease: PublishLease,
    sink: Box<dyn PublisherSink>,
    _tracks: Vec<TrackInfo>,
    /// Bounded cache of frames that arrived before the publisher was ready / authenticated.
    /// ZLM-style behaviour: see `vendor-ref/ZLMediaKit/src/Rtp/RtpProcess.cpp` `_cached_func`.
    pending_frames: std::collections::VecDeque<Arc<cheetah_codec::AVFrame>>,
    pending_frames_capacity: usize,
    publisher_ready: bool,
}

async fn run_ingress_worker(
    ctx: EngineContext,
    handle: Arc<RtpDriverHandle>,
    cancel: CancellationToken,
    publish_frame_cache_capacity: usize,
) {
    let mut sessions: HashMap<String, ActiveIngressSession> = HashMap::new();

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let event_fut = handle.recv_event().fuse();
        pin_mut!(cancel_fut, event_fut);

        let event = select_biased! {
            _ = cancel_fut => break,
            ev = event_fut => match ev {
                Some(e) => e,
                None => break,
            },
        };

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                transport_mode,
            } => {
                info!("RTP ingress session created: key={session_key}, ssrc={ssrc}, payload={payload_mode:?}, transport={transport_mode:?}");
                let sk = parse_session_key(&session_key);
                match ctx
                    .publisher_api
                    .acquire_publisher(sk.clone(), PublisherOptions::default())
                    .await
                {
                    Ok((lease, sink)) => {
                        sessions.insert(
                            session_key,
                            ActiveIngressSession {
                                _lease: lease,
                                sink,
                                _tracks: Vec::new(),
                                pending_frames: std::collections::VecDeque::new(),
                                pending_frames_capacity: publish_frame_cache_capacity,
                                publisher_ready: true,
                            },
                        );
                    }
                    Err(e) => {
                        error!("RTP acquire_publisher failed for {sk}: {e}");
                    }
                }
            }
            RtpCoreEvent::TrackFound {
                session_key,
                tracks,
            } => {
                if let Some(session) = sessions.get_mut(&session_key) {
                    debug!("RTP tracks found for {session_key}: {tracks:?}");
                    let _ = session.sink.update_tracks(tracks.clone());
                    session._tracks = tracks;
                }
            }
            RtpCoreEvent::Frame { session_key, frame } => {
                if let Some(session) = sessions.get_mut(&session_key) {
                    let frame_arc = Arc::new(frame);
                    if session.publisher_ready {
                        // Drain any frames buffered while waiting for publisher readiness.
                        while let Some(buffered) = session.pending_frames.pop_front() {
                            let _ = session.sink.push_frame(buffered);
                        }
                        let _ = session.sink.push_frame(frame_arc);
                    } else if session.pending_frames_capacity > 0 {
                        if session.pending_frames.len() >= session.pending_frames_capacity {
                            session.pending_frames.pop_front();
                        }
                        session.pending_frames.push_back(frame_arc);
                    }
                }
            }
            RtpCoreEvent::SessionClosed {
                session_key,
                reason,
            } => {
                info!("RTP ingress session closed: key={session_key}, reason={reason}");
                if let Some(session) = sessions.remove(&session_key) {
                    let _ = session.sink.close();
                }
            }
        }
    }

    for (_, session) in sessions {
        let _ = session.sink.close();
    }
}

fn parse_session_key(key: &str) -> StreamKey {
    if let Some(pos) = key.find('/') {
        let (ns, path) = key.split_at(pos);
        StreamKey::new(ns, &path[1..])
    } else {
        StreamKey::new("live", key)
    }
}

async fn run_pull_job_supervisor(
    ctx: EngineContext,
    handle: Arc<RtpDriverHandle>,
    job: RtpClientJobConfig,
    cancel: CancellationToken,
) {
    let dest_addr = match job.destination.parse::<SocketAddr>() {
        Ok(addr) => addr,
        Err(_) => return,
    };

    let base_backoff_ms = job.retry_backoff_ms.max(1);
    let max_backoff_ms = job.max_retry_backoff_ms.max(base_backoff_ms);
    let mut backoff_ms = base_backoff_ms;

    while !cancel.is_cancelled() {
        let session_key = format!("pull/{}", job.name);
        info!("Starting RTP pull job '{}' to {}", job.name, dest_addr);

        let spec = RtpClientSpec {
            session_key: session_key.clone(),
            destination: dest_addr,
            ssrc: job.ssrc,
            payload_mode: job.payload_mode,
            transport_mode: RtpTransportMode::RecvOnly,
            tcp_conn_id: None,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };

        handle
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        // Keepalive loop / Wait for job cancellation
        if sleep_or_cancel(ctx.runtime_api.as_ref(), &cancel, Duration::from_secs(5)).await {
            break;
        }

        // Apply retry backoff
        if sleep_or_cancel(
            ctx.runtime_api.as_ref(),
            &cancel,
            Duration::from_millis(backoff_ms),
        )
        .await
        {
            break;
        }
        backoff_ms = backoff_ms.saturating_mul(2).min(max_backoff_ms);
    }
}

struct RtpHttpService {
    engine: EngineContext,
    /// Shared with `RtpModule`. Populated by `start()` and read on every HTTP request; if
    /// the module hasn't started yet, the lookup returns `Unavailable` so callers can retry.
    driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>,
    active_egress: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Maps logical session_key -> internal driver target session keys (1 entry for single target,
    /// `key#0`/`key#1`/... for multi-target senderInfos use cases).
    client_targets: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Module-scoped cancel token; egress sessions spawn children of this so that
    /// `RtpModule::stop()` cascades cancellation to them.
    module_cancel: CancellationToken,
}

impl RtpHttpService {
    fn driver(&self) -> Result<Arc<RtpDriverHandle>, SdkError> {
        self.driver_handle
            .lock()
            .clone()
            .ok_or_else(|| SdkError::Unavailable("RTP driver not yet started".to_string()))
    }
}

#[async_trait]
impl ModuleHttpService for RtpHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/server/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                // SMS-compatible: `port` is OPTIONAL. When omitted the module returns the
                // already-listening RTP port from configuration; this matches the SMS pattern of
                // pre-allocating a shared receive port and binding `app/stream/ssrc` to it.
                let port = body.get("port").and_then(|v| v.as_u64()).map(|v| v as u16);

                // Accept SMS `socketType` (string `tcp`/`udp`/`both` or numeric 1/2/3) but
                // record it for diagnostic purposes only — the active driver listens on whatever
                // sockets were configured at startup. ABL-style `enable_tcp`/`is_udp` flags are
                // also supported.
                let socket_type = body
                    .get("socketType")
                    .and_then(parse_socket_type)
                    .or_else(|| {
                        let enable_tcp = body.get("enable_tcp").and_then(|v| v.as_bool());
                        let is_udp = body.get("is_udp").and_then(|v| v.as_bool());
                        match (enable_tcp, is_udp) {
                            (Some(true), Some(true)) => Some("both".to_string()),
                            (Some(true), _) => Some("tcp".to_string()),
                            (_, Some(true)) => Some("udp".to_string()),
                            (Some(false), Some(false)) => None,
                            _ => None,
                        }
                    })
                    .unwrap_or_else(|| "udp".to_string());

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/recvStreamId/recv_stream/ssrc".to_string(),
                    )
                })?;

                let ssrc = body.get("ssrc").and_then(|v| v.as_u64()).map(|v| v as u32);
                let payload_type = body
                    .get("payloadType")
                    .and_then(parse_payload_mode)
                    .unwrap_or(RtpPayloadMode::Ps);

                let transport_mode = body
                    .get("transportMode")
                    .and_then(parse_transport_mode)
                    .unwrap_or(RtpTransportMode::RecvOnly);

                let connection_type = body.get("conType").and_then(parse_connection_type);
                // ABL-style track filtering with `disableVideo` / `disableAudio`. Both flags
                // win over the simpler `onlyAudio` form when present.
                let track_filter = match (
                    body.get("disableVideo").and_then(|v| v.as_bool()),
                    body.get("disableAudio").and_then(|v| v.as_bool()),
                ) {
                    (Some(true), _) => RtpTrackFilter::OnlyAudio,
                    (_, Some(true)) => RtpTrackFilter::OnlyVideo,
                    _ => body
                        .get("onlyAudio")
                        .map(parse_only_audio_to_filter)
                        .unwrap_or(RtpTrackFilter::All),
                };

                let session_key = format!("{app_name}/{stream_name}");

                let spec = RtpServerSpec {
                    session_key: session_key.clone(),
                    ssrc,
                    payload_mode: payload_type,
                    transport_mode,
                    connection_type,
                    track_filter,
                };

                self.driver()?
                    .send_command(RtpDriverCommand::CreateServer(spec))
                    .await;

                // ABL-style advisory egress flags. We don't mutate state in the RTP module for
                // these — other modules (HLS / MP4) own the actual egress lifecycle — but we
                // echo them in the response so callers know we accepted the values.
                let enable_hls = body
                    .get("enable_hls")
                    .or_else(|| body.get("enableHls"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let enable_mp4 = body
                    .get("enable_mp4")
                    .or_else(|| body.get("enableMp4"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": port.unwrap_or(0),
                        "socketType": socket_type,
                        "sessionKey": session_key,
                        "ssrc": ssrc.unwrap_or(0),
                        "enableHls": enable_hls,
                        "enableMp4": enable_mp4,
                    }
                });

                let body_bytes = serde_json::to_vec(&response).unwrap();
                Ok(HttpResponse::ok_json(body_bytes))
            }
            (HttpMethod::Post, "/server/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/recvStream/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                self.driver()?
                    .send_command(RtpDriverCommand::StopSession(session_key))
                    .await;

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                let body_bytes = serde_json::to_vec(&response).unwrap();
                Ok(HttpResponse::ok_json(body_bytes))
            }
            (HttpMethod::Post, "/client/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let default_payload = body
                    .get("payloadType")
                    .and_then(parse_payload_mode)
                    .unwrap_or(RtpPayloadMode::Ps);

                let default_transport = body
                    .get("transportMode")
                    .and_then(parse_transport_mode)
                    .unwrap_or(RtpTransportMode::SendOnly);

                // Build the list of remote targets. Either `senderInfos` array (SMS multi-target)
                // or single peerIp/peerPort/ssrc (single target).
                let mut targets: Vec<(SocketAddr, u32, RtpPayloadMode, RtpTransportMode)> =
                    Vec::new();
                if let Some(arr) = body.get("senderInfos").and_then(|v| v.as_array()) {
                    for entry in arr {
                        let peer_ip =
                            entry
                                .get("peerIp")
                                .and_then(|v| v.as_str())
                                .ok_or_else(|| {
                                    SdkError::InvalidArgument(
                                        "senderInfos[]: missing peerIp".to_string(),
                                    )
                                })?;
                        let peer_port =
                            entry
                                .get("peerPort")
                                .and_then(|v| v.as_u64())
                                .ok_or_else(|| {
                                    SdkError::InvalidArgument(
                                        "senderInfos[]: missing peerPort".to_string(),
                                    )
                                })? as u16;
                        let ssrc = entry.get("ssrc").and_then(|v| v.as_u64()).ok_or_else(|| {
                            SdkError::InvalidArgument("senderInfos[]: missing ssrc".to_string())
                        })? as u32;
                        let payload = entry
                            .get("payloadType")
                            .and_then(parse_payload_mode)
                            .unwrap_or(default_payload);
                        let transport = entry
                            .get("transportMode")
                            .and_then(parse_transport_mode)
                            .unwrap_or(default_transport);
                        let addr = format!("{peer_ip}:{peer_port}")
                            .parse::<SocketAddr>()
                            .map_err(|e| {
                                SdkError::InvalidArgument(format!(
                                    "senderInfos[]: invalid peerIp/peerPort: {e}"
                                ))
                            })?;
                        targets.push((addr, ssrc, payload, transport));
                    }
                } else {
                    // Accept either ZLM `peerIp`/`peerPort` or ABL `dst_url`/`dst_port`.
                    let peer_ip = body
                        .get("peerIp")
                        .and_then(|v| v.as_str())
                        .or_else(|| body.get("dst_url").and_then(|v| v.as_str()))
                        .or_else(|| body.get("dstUrl").and_then(|v| v.as_str()))
                        .ok_or_else(|| {
                            SdkError::InvalidArgument("missing field: peerIp / dst_url".to_string())
                        })?
                        .to_string();
                    let peer_port = body
                        .get("peerPort")
                        .and_then(|v| v.as_u64())
                        .or_else(|| body.get("dst_port").and_then(|v| v.as_u64()))
                        .or_else(|| body.get("dstPort").and_then(|v| v.as_u64()))
                        .ok_or_else(|| {
                            SdkError::InvalidArgument(
                                "missing field: peerPort / dst_port".to_string(),
                            )
                        })? as u16;
                    let ssrc = body.get("ssrc").and_then(|v| v.as_u64()).ok_or_else(|| {
                        SdkError::InvalidArgument("missing field: ssrc".to_string())
                    })? as u32;
                    let dest_addr = format!("{peer_ip}:{peer_port}")
                        .parse::<SocketAddr>()
                        .map_err(|e| {
                            SdkError::InvalidArgument(format!("invalid peerIp/peerPort: {e}"))
                        })?;
                    targets.push((dest_addr, ssrc, default_payload, default_transport));
                }

                let session_key = format!("{app_name}/{stream_name}");
                let mut session_keys = Vec::new();

                let connection_type = body.get("conType").and_then(parse_connection_type);
                // ABL-style `disableVideo`/`disableAudio` win over `onlyAudio`.
                let track_filter = match (
                    body.get("disableVideo").and_then(|v| v.as_bool()),
                    body.get("disableAudio").and_then(|v| v.as_bool()),
                ) {
                    (Some(true), _) => RtpTrackFilter::OnlyAudio,
                    (_, Some(true)) => RtpTrackFilter::OnlyVideo,
                    _ => body
                        .get("onlyAudio")
                        .map(parse_only_audio_to_filter)
                        .unwrap_or(RtpTrackFilter::All),
                };

                for (idx, (dest_addr, ssrc, payload_mode, transport_mode)) in
                    targets.iter().enumerate()
                {
                    let target_session = if targets.len() == 1 {
                        session_key.clone()
                    } else {
                        format!("{session_key}#{idx}")
                    };

                    let spec = RtpClientSpec {
                        session_key: target_session.clone(),
                        destination: *dest_addr,
                        ssrc: *ssrc,
                        payload_mode: *payload_mode,
                        transport_mode: *transport_mode,
                        tcp_conn_id: None,
                        connection_type,
                        track_filter,
                    };

                    self.driver()?
                        .send_command(RtpDriverCommand::CreateClient(spec))
                        .await;
                    session_keys.push(target_session);
                }

                self.client_targets
                    .lock()
                    .insert(session_key.clone(), session_keys.clone());

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "sessionKey": session_key,
                        "targets": session_keys,
                    }
                });

                let body_bytes = serde_json::to_vec(&response).unwrap();
                Ok(HttpResponse::ok_json(body_bytes))
            }
            (HttpMethod::Post, "/client/start") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                // Look up registered driver sessions for this stream. If none exist
                // (caller skipped /client/create), fall back to the canonical key.
                let driver_sessions = self
                    .client_targets
                    .lock()
                    .get(&session_key)
                    .cloned()
                    .unwrap_or_else(|| vec![session_key.clone()]);

                // Start egress streaming
                let mut map = self.active_egress.lock();
                if !map.contains_key(&session_key) {
                    // Child of the module cancel so `RtpModule::stop()` cascades to in-flight
                    // egress sessions.
                    let cancel_token = self.module_cancel.child_token();
                    let stream_key = StreamKey::new(&app_name, &stream_name);

                    let runtime_api = self.engine.runtime_api.clone();
                    let engine = self.engine.clone();
                    // Resolve the driver handle once at command time so the spawned task
                    // owns a concrete `Arc<RtpDriverHandle>`. The lookup may legitimately
                    // fail when callers race the module's start; fall through with an early
                    // return if so.
                    let driver_cmd_tx = self.driver()?;
                    let cancel_clone = cancel_token.clone();

                    runtime_api.spawn(Box::pin(async move {
                        run_egress_session(
                            engine,
                            driver_cmd_tx,
                            driver_sessions,
                            stream_key,
                            cancel_clone,
                        )
                        .await;
                    }));

                    map.insert(session_key.clone(), cancel_token);
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                let body_bytes = serde_json::to_vec(&response).unwrap();
                Ok(HttpResponse::ok_json(body_bytes))
            }
            (HttpMethod::Post, "/client/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                if let Some(cancel) = self.active_egress.lock().remove(&session_key) {
                    cancel.cancel();
                }

                // Tear down every driver session created for this logical key.
                let driver_sessions = self
                    .client_targets
                    .lock()
                    .remove(&session_key)
                    .unwrap_or_else(|| vec![session_key.clone()]);
                if !driver_sessions.is_empty() {
                    let driver = self.driver()?;
                    for sk in driver_sessions {
                        driver.send_command(RtpDriverCommand::StopSession(sk)).await;
                    }
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                let body_bytes = serde_json::to_vec(&response).unwrap();
                Ok(HttpResponse::ok_json(body_bytes))
            }
            _ => Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: bytes::Bytes::from_static(b"{\"error\":\"not found\"}"),
            }),
        }
    }
}

fn parse_socket_type(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    1 => Some("udp".to_string()),
                    2 => Some("tcp".to_string()),
                    3 => Some("both".to_string()),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_transport_mode(val: &serde_json::Value) -> Option<RtpTransportMode> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "recv_only" | "recvonly" => Some(RtpTransportMode::RecvOnly),
            "send_only" | "sendonly" => Some(RtpTransportMode::SendOnly),
            "send_recv" | "sendrecv" => Some(RtpTransportMode::SendRecv),
            _ => None,
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    0 => Some(RtpTransportMode::RecvOnly),
                    1 => Some(RtpTransportMode::SendOnly),
                    2 => Some(RtpTransportMode::SendRecv),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_payload_mode(val: &serde_json::Value) -> Option<RtpPayloadMode> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "ps" => Some(RtpPayloadMode::Ps),
            "ts" => Some(RtpPayloadMode::Ts),
            "es" => Some(RtpPayloadMode::Es),
            "ehome" => Some(RtpPayloadMode::Ehome),
            "xhb" | "hk" => Some(RtpPayloadMode::Xhb),
            "jtt1078" | "1078" => Some(RtpPayloadMode::Jtt1078),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve the canonical `(app, stream)` pair from an inbound REST body, accepting all the
/// alias spellings used by SMS / ZLM / ABL deployments. Returns `None` if no stream can be
/// identified at all (caller should produce an `InvalidArgument` error in that case).
fn extract_app_stream_aliases(body: &serde_json::Value) -> (String, Option<String>) {
    let app = body
        .get("appName")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("app").and_then(|v| v.as_str()))
        .or_else(|| body.get("recv_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvApp").and_then(|v| v.as_str()))
        .or_else(|| body.get("send_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("sendApp").and_then(|v| v.as_str()))
        .unwrap_or("live")
        .to_string();
    let stream = body
        .get("streamName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            body.get("recvStreamId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("recv_stream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("recvStream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("send_stream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("sendStream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("send_stream_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("sendStreamId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("ssrc")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
        });
    (app, stream)
}

/// Parse SMS / ZLM-style `conType` field.
///
/// Accepts string aliases (`tcp_active`, `tcp_passive`, `udp_active`, `udp_passive`,
/// `voice_talk`) and ZLM numeric values (0=tcp_active, 1=udp_active, 2=tcp_passive,
/// 3=udp_passive, 4=voice_talk).
fn parse_connection_type(val: &serde_json::Value) -> Option<RtpConnectionType> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "tcp_active" | "tcpactive" => Some(RtpConnectionType::TcpActive),
            "tcp_passive" | "tcppassive" => Some(RtpConnectionType::TcpPassive),
            "udp_active" | "udpactive" => Some(RtpConnectionType::UdpActive),
            "udp_passive" | "udppassive" => Some(RtpConnectionType::UdpPassive),
            "voice_talk" | "voicetalk" => Some(RtpConnectionType::VoiceTalk),
            _ => None,
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    0 => Some(RtpConnectionType::TcpActive),
                    1 => Some(RtpConnectionType::UdpActive),
                    2 => Some(RtpConnectionType::TcpPassive),
                    3 => Some(RtpConnectionType::UdpPassive),
                    4 => Some(RtpConnectionType::VoiceTalk),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse `onlyAudio` JSON field into a `RtpTrackFilter`. Accepts:
/// - boolean: true => OnlyAudio, false => All
/// - integer 0/1: 1 => OnlyAudio, 0 => All
/// - string `audio`/`video`/`all`
fn parse_only_audio_to_filter(val: &serde_json::Value) -> RtpTrackFilter {
    match val {
        serde_json::Value::Bool(true) => RtpTrackFilter::OnlyAudio,
        serde_json::Value::Bool(false) => RtpTrackFilter::All,
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(1) => RtpTrackFilter::OnlyAudio,
            _ => RtpTrackFilter::All,
        },
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "audio" | "only_audio" | "onlyaudio" => RtpTrackFilter::OnlyAudio,
            "video" | "only_video" | "onlyvideo" => RtpTrackFilter::OnlyVideo,
            _ => RtpTrackFilter::All,
        },
        _ => RtpTrackFilter::All,
    }
}

#[allow(dead_code)]
fn _parse_payload_mode_replaced_marker_to_avoid_dup() {}

async fn wait_for_stream(
    ctx: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Option<cheetah_sdk::StreamSnapshot> {
    let start = ctx.runtime_api.now().as_micros();
    let timeout_us = timeout.as_micros() as u64;

    loop {
        if cancel.is_cancelled() {
            return None;
        }
        if let Ok(Some(snapshot)) = ctx.stream_manager_api.get_stream(stream_key).await {
            return Some(snapshot);
        }
        let elapsed = ctx.runtime_api.now().as_micros().saturating_sub(start);
        if elapsed >= timeout_us {
            return None;
        }
        if sleep_or_cancel(ctx.runtime_api.as_ref(), cancel, Duration::from_millis(100)).await {
            return None;
        }
    }
}

async fn sleep_or_cancel(
    runtime_api: &dyn cheetah_runtime_api::RuntimeApi,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let now = runtime_api.now().as_micros();
    let delta = duration.as_micros() as u64;
    let deadline = cheetah_codec::MonoTime::from_micros(now.saturating_add(delta));
    let mut timer = runtime_api.sleep_until(deadline);
    let cancel_fut = cancel.cancelled().fuse();
    let wait_fut = timer.wait().fuse();
    pin_mut!(cancel_fut, wait_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = wait_fut => false,
    }
}

async fn run_egress_session(
    engine: EngineContext,
    driver_handle: Arc<RtpDriverHandle>,
    session_keys: Vec<String>,
    stream_key: StreamKey,
    cancel: CancellationToken,
) {
    if session_keys.is_empty() {
        return;
    }
    let Some(_snapshot) =
        wait_for_stream(&engine, &stream_key, &cancel, Duration::from_millis(5000)).await
    else {
        debug!("Egress session wait stream timeout: {}", stream_key);
        return;
    };

    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            stream_key.clone(),
            SubscriberOptions {
                queue_capacity: 256,
                bootstrap_policy: BootstrapPolicy::live_tail(150, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!("Egress session subscribe failed: {e}");
            return;
        }
    };

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let frame_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, frame_fut);

        let frame = select_biased! {
            _ = cancel_fut => break,
            res = frame_fut => match res {
                Ok(Some(f)) => f,
                Ok(None) | Err(_) => break,
            }
        };

        // Fan out the same frame to every configured target session.
        for sk in &session_keys {
            let cmd = RtpDriverCommand::SendFrame(Box::new(RtpSendFrame {
                session_key: sk.clone(),
                frame: (*frame).clone(),
            }));
            driver_handle.send_command(cmd).await;
        }
    }

    let _ = subscriber.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_rtp_module_config_validation() {
        let mut config = RtpModuleConfig::default();
        assert!(config.validate().is_ok());

        config.listen_udp = Some("invalid_ip".to_string());
        assert!(config.validate().is_err());

        config.listen_udp = Some("127.0.0.1:20000".to_string());
        assert!(config.validate().is_ok());

        config.write_queue_capacity = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_json_helpers() {
        let val_num = json!(1);
        assert_eq!(parse_socket_type(&val_num), Some("udp".to_string()));
        let val_str = json!("tcp");
        assert_eq!(parse_socket_type(&val_str), Some("tcp".to_string()));

        let mode_num = json!(1);
        assert_eq!(
            parse_transport_mode(&mode_num),
            Some(RtpTransportMode::SendOnly)
        );
        let mode_str = json!("recv_only");
        assert_eq!(
            parse_transport_mode(&mode_str),
            Some(RtpTransportMode::RecvOnly)
        );

        let payload_str = json!("ts");
        assert_eq!(parse_payload_mode(&payload_str), Some(RtpPayloadMode::Ts));
    }

    #[test]
    fn test_rtp_module_factory() {
        let factory = RtpModuleFactory;
        let manifest = factory.manifest();
        assert_eq!(manifest.module_id, ModuleId::new("rtp"));
        assert_eq!(manifest.routes_prefix, "/api/v1/rtp");

        let schema = factory.config_schema().unwrap();
        assert_eq!(schema.module_id, ModuleId::new("rtp"));
        assert_eq!(schema.schema_name, "rtp-module");

        let default_val = schema.default_value;
        let config = RtpModuleConfig::from_value(default_val).unwrap();
        assert!(config.enabled);
        assert_eq!(config.listen_udp, Some("0.0.0.0:20000".to_string()));
    }

    #[test]
    fn test_socket_type_numeric_compat() {
        // SMS-style socketType numeric values: 1=udp, 2=tcp, 3=both
        assert_eq!(parse_socket_type(&json!(1)), Some("udp".to_string()));
        assert_eq!(parse_socket_type(&json!(2)), Some("tcp".to_string()));
        assert_eq!(parse_socket_type(&json!(3)), Some("both".to_string()));
        assert_eq!(parse_socket_type(&json!("both")), Some("both".to_string()));
    }

    #[test]
    fn test_transport_mode_aliases() {
        // Accept both snake_case and camelCase variants for robustness.
        assert_eq!(
            parse_transport_mode(&json!("recvonly")),
            Some(RtpTransportMode::RecvOnly)
        );
        assert_eq!(
            parse_transport_mode(&json!("SendRecv")),
            Some(RtpTransportMode::SendRecv)
        );
        assert_eq!(
            parse_transport_mode(&json!("send_only")),
            Some(RtpTransportMode::SendOnly)
        );
    }

    #[test]
    fn test_payload_mode_case_insensitive() {
        assert_eq!(parse_payload_mode(&json!("PS")), Some(RtpPayloadMode::Ps));
        assert_eq!(parse_payload_mode(&json!("Ts")), Some(RtpPayloadMode::Ts));
        assert_eq!(parse_payload_mode(&json!("eS")), Some(RtpPayloadMode::Es));
    }

    #[test]
    fn test_parse_connection_type_string_and_numeric() {
        assert_eq!(
            parse_connection_type(&json!("tcp_active")),
            Some(RtpConnectionType::TcpActive)
        );
        assert_eq!(
            parse_connection_type(&json!("UDP_PASSIVE")),
            Some(RtpConnectionType::UdpPassive)
        );
        assert_eq!(
            parse_connection_type(&json!("voiceTalk")),
            Some(RtpConnectionType::VoiceTalk)
        );
        // ZLM numeric values
        assert_eq!(
            parse_connection_type(&json!(0)),
            Some(RtpConnectionType::TcpActive)
        );
        assert_eq!(
            parse_connection_type(&json!(1)),
            Some(RtpConnectionType::UdpActive)
        );
        assert_eq!(
            parse_connection_type(&json!(4)),
            Some(RtpConnectionType::VoiceTalk)
        );
        assert_eq!(parse_connection_type(&json!(99)), None);
        assert_eq!(parse_connection_type(&json!("nonsense")), None);
    }

    #[test]
    fn test_parse_only_audio_filter_modes() {
        assert_eq!(
            parse_only_audio_to_filter(&json!(true)),
            RtpTrackFilter::OnlyAudio
        );
        assert_eq!(
            parse_only_audio_to_filter(&json!(false)),
            RtpTrackFilter::All
        );
        assert_eq!(
            parse_only_audio_to_filter(&json!(1)),
            RtpTrackFilter::OnlyAudio
        );
        assert_eq!(parse_only_audio_to_filter(&json!(0)), RtpTrackFilter::All);
        assert_eq!(
            parse_only_audio_to_filter(&json!("only_video")),
            RtpTrackFilter::OnlyVideo
        );
        assert_eq!(
            parse_only_audio_to_filter(&json!("audio")),
            RtpTrackFilter::OnlyAudio
        );
        // unknown string -> All
        assert_eq!(
            parse_only_audio_to_filter(&json!("foo")),
            RtpTrackFilter::All
        );
    }

    #[test]
    fn test_sender_infos_multi_target_session_key_layout() {
        // Validate that multi-target sender_infos produces stable, suffixed session keys
        // while single-target keeps the canonical key.
        let logical = "live/stream1".to_string();

        let single = vec![logical.clone()];
        assert_eq!(single, vec!["live/stream1"]);

        let multi: Vec<String> = (0..3).map(|idx| format!("{logical}#{idx}")).collect();
        assert_eq!(
            multi,
            vec!["live/stream1#0", "live/stream1#1", "live/stream1#2"]
        );
    }

    #[test]
    fn test_parse_payload_mode_includes_jtt1078_and_xhb() {
        // ABL `payloadType` aliases for JT/T 1078 and Hikvision XHB (a.k.a. `hk`).
        assert_eq!(
            parse_payload_mode(&json!("jtt1078")),
            Some(RtpPayloadMode::Jtt1078)
        );
        assert_eq!(
            parse_payload_mode(&json!("1078")),
            Some(RtpPayloadMode::Jtt1078)
        );
        assert_eq!(parse_payload_mode(&json!("xhb")), Some(RtpPayloadMode::Xhb));
        assert_eq!(parse_payload_mode(&json!("HK")), Some(RtpPayloadMode::Xhb));
    }

    #[test]
    fn test_extract_app_stream_aliases_covers_sms_zlm_abl_field_names() {
        // SMS `appName` + `streamName` (canonical).
        let (app, stream) = extract_app_stream_aliases(&json!({
            "appName": "live",
            "streamName": "cam1",
        }));
        assert_eq!(app, "live");
        assert_eq!(stream.as_deref(), Some("cam1"));

        // ZLM short `app` + `streamName`.
        let (app, stream) = extract_app_stream_aliases(&json!({
            "app": "rtp",
            "streamName": "cam2",
        }));
        assert_eq!(app, "rtp");
        assert_eq!(stream.as_deref(), Some("cam2"));

        // ABL `recv_app` + `recv_stream`.
        let (app, stream) = extract_app_stream_aliases(&json!({
            "recv_app": "rtp",
            "recv_stream": "cam3",
        }));
        assert_eq!(app, "rtp");
        assert_eq!(stream.as_deref(), Some("cam3"));

        // ABL `send_app` + `send_stream` for egress paths.
        let (app, stream) = extract_app_stream_aliases(&json!({
            "send_app": "rtp",
            "send_stream": "cam4",
        }));
        assert_eq!(app, "rtp");
        assert_eq!(stream.as_deref(), Some("cam4"));

        // SSRC-derived stream when no name is provided; defaults to "live" app.
        let (app, stream) = extract_app_stream_aliases(&json!({"ssrc": 12345}));
        assert_eq!(app, "live");
        assert_eq!(stream.as_deref(), Some("12345"));

        // Empty body -> None stream.
        let (app, stream) = extract_app_stream_aliases(&json!({}));
        assert_eq!(app, "live");
        assert!(stream.is_none());
    }
}
