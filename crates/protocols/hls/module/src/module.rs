use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_hls_core::{HlsContainer, PlaylistBuilder, StreamKeyParts};
use cheetah_hls_driver_tokio::{
    start_server, HlsCommandSender, HlsConnectionId, HlsCoreEvent, HlsDriverCommand,
    HlsDriverConfig, HlsDriverEvent, HlsServerHandle,
};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability,
    ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, OneShotReceiver, RuntimeApi, SdkError,
    ServiceDescriptor, StreamKey, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt, StreamExt};
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::config::HlsModuleConfig;
use crate::muxer::{MuxerOutput, StreamMuxer, StreamMuxerConfig};

const MODULE_ID: &str = "hls";

/// A pending blocking playlist request waiting for a specific MSN/Part to be produced.
#[derive(Debug)]
#[allow(dead_code)]
struct PendingPlaylistRequest {
    connection_id: HlsConnectionId,
    target_msn: u64,
    target_part: Option<u64>,
    session_id: Option<u64>,
    /// Track lane for demuxed per-track requests (None = legacy/muxed).
    lane: Option<cheetah_hls_core::TrackLane>,
    legacy: bool,
    rewind: bool,
    accept_gzip: bool,
    include_stream_key: bool,
    created_at_us: u64,
}

/// A pending part request waiting for a specific part to be produced.
#[derive(Debug)]
struct PendingPartRequest {
    connection_id: HlsConnectionId,
    target_part_seq: u64,
    /// Track lane for demuxed per-track part requests (None = legacy/video).
    lane: Option<cheetah_hls_core::TrackLane>,
    created_at_us: u64,
}

/// Shared pending requests for a single stream.
#[derive(Default)]
struct StreamPendingRequests {
    playlists: Vec<PendingPlaylistRequest>,
    parts: Vec<PendingPartRequest>,
}

/// Map of stream_key → pending requests.
type PendingMap = Arc<Mutex<HashMap<String, StreamPendingRequests>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartRequestDecision {
    Ready,
    Pending,
    NotFound,
}

pub struct HlsModuleFactory;

impl ModuleFactory for HlsModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "HLS Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "hls".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![ModuleCapability::Subscribe],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(HlsModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "hls-module".to_string(),
            default_value: HlsModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                HlsModuleConfig::from_value(value.clone())
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })),
        })
    }
}

struct HlsModule {
    info: ModuleInfo,
    state: ModuleState,
    engine: Option<EngineContext>,
    config: HlsModuleConfig,
    runtime_cancel: Option<CancellationToken>,
    runtime_loops: Vec<OneShotReceiver>,
}

impl HlsModule {
    fn new() -> Self {
        Self {
            info: ModuleInfo {
                module_id: ModuleId::new(MODULE_ID),
                display_name: "HLS Module".to_string(),
                state: ModuleState::Created,
            },
            state: ModuleState::Created,
            engine: None,
            config: HlsModuleConfig::default(),
            runtime_cancel: None,
            runtime_loops: Vec::new(),
        }
    }
}

#[async_trait]
impl Module for HlsModule {
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = HlsModuleConfig::from_value(ctx.initial_config)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        let Some(engine) = self.engine.clone() else {
            return Err(SdkError::Unavailable(
                "hls module is not initialized".to_string(),
            ));
        };

        if !self.config.enabled {
            self.runtime_cancel = Some(cancel);
            self.state = ModuleState::Running;
            return Ok(());
        }

        let listen: SocketAddr = self
            .config
            .listen
            .parse()
            .map_err(|err| SdkError::InvalidArgument(format!("invalid hls.listen: {err}")))?;

        let server_cancel = cancel.child_token();
        let driver_config = hls_driver_config(&self.config);
        let driver = if let Some(tls_cfg) = &self.config.tls {
            cheetah_hls_driver_tokio::start_tls_server(
                engine.runtime_api.clone(),
                listen,
                driver_config,
                cheetah_hls_driver_tokio::HlsTlsConfig {
                    cert_path: tls_cfg.cert_path.clone(),
                    key_path: tls_cfg.key_path.clone(),
                },
                server_cancel.clone(),
            )
        } else {
            start_server(
                engine.runtime_api.clone(),
                listen,
                driver_config,
                server_cancel.clone(),
            )
        }
        .map_err(|err| SdkError::Internal(format!("start hls driver failed: {err}")))?;

        if let Err(err) = engine.service_registry.register(ServiceDescriptor {
            name: MODULE_ID.to_string(),
            endpoint: format!("hls://{}", self.config.listen),
            metadata: Default::default(),
        }) {
            driver.shutdown();
            let _ = driver.wait().await;
            return Err(SdkError::Internal(format!(
                "register hls service failed: {err}"
            )));
        }

        let event_task = spawn_runtime_task(
            engine.runtime_api.clone(),
            run_server_loop(
                engine.clone(),
                self.config.clone(),
                driver,
                server_cancel.clone(),
            ),
        );

        let mut runtime_loops = vec![event_task];
        // Spawn pull jobs
        for job in self.config.pull_jobs.iter().filter(|j| j.enabled).cloned() {
            let job_cancel = server_cancel.child_token();
            runtime_loops.push(spawn_runtime_task(
                engine.runtime_api.clone(),
                crate::pull::run_hls_pull_job(engine.runtime_api.clone(), job, job_cancel),
            ));
        }

        self.runtime_cancel = Some(server_cancel);
        self.runtime_loops = runtime_loops;
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.runtime_cancel.take() {
            cancel.cancel();
        }
        for mut join in self.runtime_loops.drain(..) {
            let _ = join.recv().await;
        }
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.service_registry.unregister(MODULE_ID);
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = HlsModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if next == self.config {
            return Ok(ConfigEffect::Immediate);
        }
        self.config = next;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}

fn spawn_runtime_task<F>(runtime_api: Arc<dyn RuntimeApi>, fut: F) -> OneShotReceiver
where
    F: Future<Output = ()> + Send + 'static,
{
    let (done_tx, done_rx) = runtime_api.oneshot();
    let wrapped = async move {
        fut.await;
        let _ = done_tx.send();
    };
    let _ = runtime_api.spawn(Box::pin(wrapped));
    done_rx
}

fn hls_driver_config(config: &HlsModuleConfig) -> HlsDriverConfig {
    HlsDriverConfig {
        set_session_cookie: !config.origin_mode,
        ..HlsDriverConfig::default()
    }
}

/// Session UID generator.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn new_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Per-stream muxer state managed by the server loop.
type MuxerMap = Arc<Mutex<HashMap<String, Arc<Mutex<StreamMuxer>>>>>;

/// Per-session tracking state.
struct SessionState {
    last_request_us: u64,
    bytes_sent: u64,
}

/// Tracks player sessions: stream_key → {session_id → state}
type SessionMap = Arc<Mutex<HashMap<String, HashMap<u64, SessionState>>>>;

fn refresh_session_activity(
    sessions: &SessionMap,
    stream_key: &str,
    session_id: Option<u64>,
    now_us: u64,
) {
    if let Some(uid) = session_id {
        sessions
            .lock()
            .entry(stream_key.to_string())
            .or_default()
            .entry(uid)
            .and_modify(|s| s.last_request_us = now_us)
            .or_insert(SessionState {
                last_request_us: now_us,
                bytes_sent: 0,
            });
    }
}

async fn run_server_loop(
    engine: EngineContext,
    config: HlsModuleConfig,
    mut driver: HlsServerHandle,
    cancel: CancellationToken,
) {
    let muxers: MuxerMap = Arc::new(Mutex::new(HashMap::new()));
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let cmd_tx = driver.command_sender();

    // Channel for subscriber tasks to notify "new content available for stream X"
    let (content_notify_tx, mut content_notify_rx) = futures::channel::mpsc::channel::<String>(256);

    // Spawn periodic session cleanup task
    let cleanup_cancel = cancel.child_token();
    let cleanup_sessions = sessions.clone();
    let cleanup_muxers = muxers.clone();
    let cleanup_runtime = engine.runtime_api.clone();
    let cleanup_event_bus = engine.event_bus.clone();
    let timeout_us = config.session_timeout_secs * 1_000_000;
    let interval_us = (config.session_timeout_secs * 1_000_000).max(2_000_000) / 2;
    let hls_demand = config.hls_demand;
    let _ = engine.runtime_api.spawn(Box::pin({
        let cancel = cleanup_cancel.clone();
        async move {
            loop {
                let now = cleanup_runtime.now();
                let deadline = cheetah_codec::MonoTime::from_micros(now.as_micros() + interval_us);
                let mut timer = cleanup_runtime.sleep_until(deadline);
                futures::select_biased! {
                    _ = cancel.cancelled().fuse() => break,
                    _ = timer.wait().fuse() => {}
                }
                cleanup_expired_sessions(
                    &cleanup_sessions,
                    &cleanup_muxers,
                    &cleanup_runtime,
                    timeout_us,
                    hls_demand,
                    &cleanup_event_bus,
                );
            }
        }
    }));

    let blocking_timeout_us = config.blocking_timeout_ms * 1000;

    loop {
        // Drain any queued content notifications first
        while let Ok(stream_key) = content_notify_rx.try_recv() {
            release_pending_requests(
                &muxers,
                &pending,
                &cmd_tx,
                &stream_key,
                blocking_timeout_us,
                config.cache_control.chunklist_with_directives_max_age,
                config.cache_control.partial_segment_max_age,
            )
            .await;
        }

        let timeout_deadline =
            pending_timeout_deadline(engine.runtime_api.as_ref(), &pending, blocking_timeout_us);
        let mut pending_timeout = engine.runtime_api.sleep_until(timeout_deadline);

        let cancelled = cancel.cancelled().fuse();
        let event = driver.recv_event().fuse();
        let notify = content_notify_rx.next().fuse();
        let timeout = pending_timeout.wait().fuse();
        pin_mut!(cancelled, event, notify, timeout);

        select_biased! {
            _ = cancelled => break,
            _ = timeout => {
                release_all_pending_requests(
                    &muxers,
                    &pending,
                    &cmd_tx,
                    blocking_timeout_us,
                    config.cache_control.chunklist_with_directives_max_age,
                    config.cache_control.partial_segment_max_age,
                )
                .await;
            }
            maybe_key = notify => {
                if let Some(stream_key) = maybe_key {
                    release_pending_requests(
                        &muxers,
                        &pending,
                        &cmd_tx,
                        &stream_key,
                        blocking_timeout_us,
                        config.cache_control.chunklist_with_directives_max_age,
                        config.cache_control.partial_segment_max_age,
                    )
                    .await;
                }
            }
            maybe_event = event => {
                let Some(ev) = maybe_event else { break };
                match ev {
                    HlsDriverEvent::ConnectionOpened { .. } => {}
                    HlsDriverEvent::ConnectionClosed { connection_id, .. } => {
                        // Remove pending requests for this connection
                        let mut pmap = pending.lock();
                        for stream_pending in pmap.values_mut() {
                            stream_pending.playlists.retain(|r| r.connection_id != connection_id);
                            stream_pending.parts.retain(|r| r.connection_id != connection_id);
                        }
                    }
                    HlsDriverEvent::Core { connection_id, event } => {
                        handle_core_event(
                            &engine,
                            &config,
                            &muxers,
                            &sessions,
                            &pending,
                            &cmd_tx,
                            connection_id,
                            event,
                            &content_notify_tx,
                        ).await;
                    }
                }
            }
        }
    }
}

/// Remove sessions that haven't made a request within the timeout.
/// When hls_demand is true, disable muxers for streams with no active sessions.
fn cleanup_expired_sessions(
    sessions: &SessionMap,
    muxers: &MuxerMap,
    runtime: &Arc<dyn RuntimeApi>,
    timeout_us: u64,
    hls_demand: bool,
    event_bus: &Arc<dyn cheetah_sdk::EventBus>,
) {
    let now_us = runtime.now().as_micros();
    let mut sess_map = sessions.lock();
    let mut empty_streams = Vec::new();

    for (stream_key, player_map) in sess_map.iter_mut() {
        player_map.retain(|_uid, state| now_us.saturating_sub(state.last_request_us) < timeout_us);
        if player_map.is_empty() {
            empty_streams.push(stream_key.clone());
        }
    }

    for key in &empty_streams {
        sess_map.remove(key);
    }

    // Emit on_stream_none_reader for streams that lost all viewers
    for key in &empty_streams {
        event_bus.publish(cheetah_sdk::SystemEvent::System(
            cheetah_sdk::SystemLifecycleEvent {
                component: "hls".to_string(),
                phase: "on_none_reader".to_string(),
                message: Some(format!("{{\"stream\":\"{key}\"}}")),
            },
        ));
    }

    // Disable muxers for streams with no viewers (on-demand mode)
    if hls_demand {
        let muxer_map = muxers.lock();
        for key in &empty_streams {
            if let Some(muxer) = muxer_map.get(key) {
                muxer.lock().enabled = false;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_core_event(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    sessions: &SessionMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    connection_id: HlsConnectionId,
    event: HlsCoreEvent,
    content_notify_tx: &futures::channel::mpsc::Sender<String>,
) {
    match event {
        HlsCoreEvent::MasterPlaylistRequested {
            stream_key,
            headers,
            ..
        } => {
            debug!(
                "hls master playlist requested: stream_key={}/{} conn={}",
                stream_key.namespace, stream_key.stream_path, connection_id
            );
            // CDN auth check: if cdn_secret configured and Authorization header present but invalid
            if !config.cdn_secret.is_empty()
                && headers.authorization.is_some()
                && !is_cdn_authorized(&headers.authorization, &config.cdn_secret)
            {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 401,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Unauthorized"),
                        headers: cors_headers(),
                    })
                    .await;
                return;
            }

            let session_id = if config.origin_mode {
                0
            } else {
                new_session_id()
            };
            let key = stream_key_string(&stream_key);

            // Enforce max_sessions_per_stream: evict oldest if limit exceeded
            if !config.origin_mode && config.max_sessions_per_stream > 0 {
                let mut map = sessions.lock();
                let stream_sessions = map.entry(key.clone()).or_default();
                while stream_sessions.len() >= config.max_sessions_per_stream {
                    // Evict oldest session
                    if let Some((&oldest_id, _)) = stream_sessions
                        .iter()
                        .min_by_key(|(_, s)| s.last_request_us)
                    {
                        stream_sessions.remove(&oldest_id);
                    } else {
                        break;
                    }
                }
            }

            ensure_muxer(
                engine,
                config,
                muxers,
                pending,
                cmd_tx,
                &stream_key,
                content_notify_tx,
            );
            if config.hls_demand {
                if let Some(m) = muxers.lock().get(&key) {
                    m.lock().enabled = true;
                }
            }
            wait_for_demuxed_master_muxer(engine, config, muxers, &key).await;

            let content = {
                let map = muxers.lock();
                let muxer = map.get(&key).map(|m| m.lock());
                build_master_playlist_content(
                    &stream_key,
                    muxer.as_deref(),
                    session_id,
                    config.stream_key_validation,
                )
            };
            let send_result = cmd_tx
                .send(HlsDriverCommand::SendResponse {
                    connection_id,
                    status: 200,
                    content_type: "application/vnd.apple.mpegurl",
                    body: bytes::Bytes::from(content),
                    headers: cors_headers_with_max_age(
                        config.cache_control.master_playlist_max_age,
                    ),
                })
                .await;
            debug!(
                "hls master playlist response sent to driver: conn={} ok={}",
                connection_id,
                send_result.is_ok()
            );

            // Track session and ensure muxer exists
            if !config.origin_mode {
                sessions.lock().entry(key.clone()).or_default().insert(
                    session_id,
                    SessionState {
                        last_request_us: engine.runtime_api.now().as_micros(),
                        bytes_sent: 0,
                    },
                );
            }

            // Emit on_play event
            engine.event_bus.publish(cheetah_sdk::SystemEvent::System(
                cheetah_sdk::SystemLifecycleEvent {
                    component: "hls".to_string(),
                    phase: "on_play".to_string(),
                    message: Some(format!(
                        "{{\"stream\":\"{key}\",\"session_id\":{session_id}}}"
                    )),
                },
            ));
        }
        HlsCoreEvent::MediaPlaylistRequested {
            stream_key,
            session_id,
            legacy,
            rewind,
            headers,
            ..
        } => {
            let is_cdn = is_cdn_authorized(&headers.authorization, &config.cdn_secret);

            // Update session activity
            if let Some(uid) = session_id {
                let key = stream_key_string(&stream_key);
                sessions
                    .lock()
                    .entry(key)
                    .or_default()
                    .entry(uid)
                    .and_modify(|s| s.last_request_us = engine.runtime_api.now().as_micros())
                    .or_insert(SessionState {
                        last_request_us: engine.runtime_api.now().as_micros(),
                        bytes_sent: 0,
                    });
            }

            // Ensure muxer exists (player may request media playlist directly)
            ensure_muxer(
                engine,
                config,
                muxers,
                pending,
                cmd_tx,
                &stream_key,
                content_notify_tx,
            );

            let key = stream_key_string(&stream_key);

            // Wait briefly for first segment so we don't return an empty playlist
            // (ffplay/some players treat empty playlist as EOF). Cap at 5s to limit
            // event loop blocking. After ready, never wait.
            let muxer_arc = { muxers.lock().get(&key).cloned() };
            if let Some(muxer_ref) = &muxer_arc {
                if !muxer_ref.lock().is_ready() {
                    for _ in 0..10 {
                        if muxer_ref.lock().is_ready() {
                            break;
                        }
                        let deadline = cheetah_codec::MonoTime::from_micros(
                            engine.runtime_api.now().as_micros() + 500_000,
                        );
                        engine.runtime_api.sleep_until(deadline).wait().await;
                    }
                }
            }

            let (playlist, cached_gzip) = {
                let map = muxers.lock();
                map.get(&key)
                    .map(|m| {
                        let mux = m.lock();
                        if mux.is_ready() {
                            if rewind {
                                (
                                    Some(mux.playlist_rewind_with_token(
                                        session_id,
                                        config.stream_key_validation,
                                    )),
                                    None,
                                )
                            } else if !config.stream_key_validation
                                && !legacy
                                && session_id.is_none()
                                && headers.accept_gzip
                            {
                                if let Some(gz) = mux.cached_playlist_gzip() {
                                    let plain = mux.playlist(None);
                                    (Some(plain), Some(gz))
                                } else {
                                    (
                                        Some(mux.playlist_with_options_and_token(
                                            session_id, legacy, false,
                                        )),
                                        None,
                                    )
                                }
                            } else {
                                (
                                    Some(mux.playlist_with_options_and_token(
                                        session_id,
                                        legacy,
                                        config.stream_key_validation,
                                    )),
                                    None,
                                )
                            }
                        } else {
                            // Still not ready after wait: return 404 so client retries
                            (None, None)
                        }
                    })
                    .unwrap_or((None, None))
            };
            match playlist {
                Some(content) => {
                    let mut resp_headers = if is_cdn {
                        cors_headers_cdn()
                    } else {
                        cors_headers_with_max_age(config.cache_control.chunklist_max_age)
                    };
                    let (body, gzipped) = if let Some(gz) = cached_gzip {
                        (gz, true)
                    } else {
                        playlist_response_body(&content, headers.accept_gzip)
                    };
                    if gzipped {
                        push_gzip_response_headers(&mut resp_headers);
                    }
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "application/vnd.apple.mpegurl",
                            body,
                            headers: resp_headers,
                        })
                        .await;
                }
                _ => {
                    // Not ready — return a valid empty playlist so hls.js retries
                    // gracefully instead of treating 404 as a fatal error.
                    let container = parse_container(&config.container);
                    let version = match container {
                        HlsContainer::Fmp4 => 7,
                        HlsContainer::Ts => 3,
                    };
                    let empty = format!(
                        "#EXTM3U\n#EXT-X-VERSION:{version}\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n"
                    );
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "application/vnd.apple.mpegurl",
                            body: bytes::Bytes::from(empty),
                            headers: cors_headers_no_cache(),
                        })
                        .await;
                }
            }
        }
        HlsCoreEvent::SegmentRequested {
            stream_key,
            segment_name,
            session_id,
            key_token,
            ..
        } => {
            // Update session activity
            if let Some(uid) = session_id {
                let key = stream_key_string(&stream_key);
                let now_us = engine.runtime_api.now().as_micros();
                sessions
                    .lock()
                    .entry(key)
                    .or_default()
                    .entry(uid)
                    .and_modify(|s| s.last_request_us = now_us)
                    .or_insert(SessionState {
                        last_request_us: now_us,
                        bytes_sent: 0,
                    });
            }

            let key = stream_key_string(&stream_key);

            // Stream key validation
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }

            let (segment_data, container) = {
                let map = muxers.lock();
                match map.get(&key) {
                    Some(m) => {
                        let mux = m.lock();
                        (mux.get_segment(&segment_name), mux.container())
                    }
                    None => (None, HlsContainer::Ts),
                }
            };

            // Disk fallback: if not in memory ring, try reading from disk
            let segment_data = match segment_data {
                Some(data) => Some(data),
                None if config.file_output.enabled
                    && (config.file_output.storage_mode == "disk"
                        || config.file_output.storage_mode == "hybrid") =>
                {
                    let file_writer = cheetah_hls_driver_tokio::HlsFileWriter::new(
                        std::path::PathBuf::from(&config.file_output.output_dir),
                        config.file_output.max_disk_segments,
                    );
                    let ext = match container {
                        HlsContainer::Fmp4 => ".m4s",
                        HlsContainer::Ts => ".ts",
                    };
                    let filename = format!(
                        "{}/{}/{}{ext}",
                        stream_key.namespace, stream_key.stream_path, segment_name
                    );
                    file_writer.read_file(&filename).await.ok()
                }
                None => None,
            };

            let content_type = match container {
                HlsContainer::Fmp4 => "video/mp4",
                HlsContainer::Ts => "video/mp2t",
            };
            match segment_data {
                Some(ref data) => {
                    // Track bytes sent
                    if let Some(uid) = session_id {
                        let skey = stream_key_string(&stream_key);
                        if let Some(players) = sessions.lock().get_mut(&skey) {
                            if let Some(state) = players.get_mut(&uid) {
                                state.bytes_sent += data.len() as u64;
                            }
                        }
                    }
                    let headers = segment_response_headers(
                        &segment_name,
                        config.cache_control.segment_max_age,
                    );
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type,
                            body: data.clone(),
                            headers,
                        })
                        .await;
                }
                None => {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Segment Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                }
            }
        }
        HlsCoreEvent::InitSegmentRequested {
            stream_key,
            session_id,
            key_token,
            ..
        } => {
            if let Some(uid) = session_id {
                let key = stream_key_string(&stream_key);
                let now_us = engine.runtime_api.now().as_micros();
                sessions
                    .lock()
                    .entry(key)
                    .or_default()
                    .entry(uid)
                    .and_modify(|s| s.last_request_us = now_us)
                    .or_insert(SessionState {
                        last_request_us: now_us,
                        bytes_sent: 0,
                    });
            }
            let key = stream_key_string(&stream_key);
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }
            let init_data = {
                let map = muxers.lock();
                map.get(&key).and_then(|m| m.lock().init_segment())
            };
            match init_data {
                Some(data) => {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "video/mp4",
                            body: data,
                            headers: cors_headers_with_max_age(
                                config.cache_control.partial_segment_max_age,
                            ),
                        })
                        .await;
                }
                None => {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Init Segment Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                }
            }
        }
        HlsCoreEvent::BlockingPlaylistRequested {
            stream_key,
            session_id,
            blocking,
            legacy,
            rewind,
            headers,
            ..
        } => {
            // Ensure muxer exists (client may jump to blocking request directly)
            ensure_muxer(
                engine,
                config,
                muxers,
                pending,
                cmd_tx,
                &stream_key,
                content_notify_tx,
            );

            let key = stream_key_string(&stream_key);

            // Check if blocking condition is already satisfied
            let satisfied = {
                let map = muxers.lock();
                map.get(&key)
                    .map(|m| m.lock().is_blocking_satisfied(blocking.msn, blocking.part))
                    .unwrap_or(true)
            };

            if satisfied {
                // Already satisfied — return playlist immediately
                let playlist = {
                    let map = muxers.lock();
                    map.get(&key).map(|m| {
                        let mux = m.lock();
                        if rewind {
                            mux.playlist_rewind_with_token(session_id, config.stream_key_validation)
                        } else {
                            mux.playlist_with_options_and_token(
                                session_id,
                                legacy,
                                config.stream_key_validation,
                            )
                        }
                    })
                };
                match playlist {
                    Some(content) => {
                        let mut resp_headers = cors_headers_with_max_age(
                            config.cache_control.chunklist_with_directives_max_age,
                        );
                        let (body, gzipped) = playlist_response_body(&content, headers.accept_gzip);
                        if gzipped {
                            push_gzip_response_headers(&mut resp_headers);
                        }
                        let _ = cmd_tx
                            .send(HlsDriverCommand::SendResponse {
                                connection_id,
                                status: 200,
                                content_type: "application/vnd.apple.mpegurl",
                                body,
                                headers: resp_headers,
                            })
                            .await;
                    }
                    None => {
                        let _ = cmd_tx
                            .send(HlsDriverCommand::SendResponse {
                                connection_id,
                                status: 404,
                                content_type: "text/plain",
                                body: bytes::Bytes::from_static(b"Stream Not Found"),
                                headers: cors_headers(),
                            })
                            .await;
                    }
                }
            } else {
                // Not satisfied — check pending limit and either add or degrade
                let over_limit = {
                    let mut pmap = pending.lock();
                    let stream_pending = pmap.entry(key).or_default();
                    let total = stream_pending.playlists.len() + stream_pending.parts.len();
                    if total >= config.max_pending_requests {
                        true
                    } else {
                        stream_pending.playlists.push(PendingPlaylistRequest {
                            connection_id,
                            target_msn: blocking.msn,
                            target_part: blocking.part,
                            session_id,
                            lane: None,
                            legacy,
                            rewind,
                            accept_gzip: headers.accept_gzip,
                            include_stream_key: config.stream_key_validation,
                            created_at_us: current_time_us(),
                        });
                        false
                    }
                };

                if over_limit {
                    // Over limit: return current playlist immediately (degrade)
                    let playlist = {
                        let map = muxers.lock();
                        map.get(&stream_key_string(&stream_key)).map(|m| {
                            let mux = m.lock();
                            if rewind {
                                mux.playlist_rewind_with_token(
                                    session_id,
                                    config.stream_key_validation,
                                )
                            } else {
                                mux.playlist_with_options_and_token(
                                    session_id,
                                    legacy,
                                    config.stream_key_validation,
                                )
                            }
                        })
                    };
                    let (body, gzipped) = playlist
                        .as_deref()
                        .map(|content| playlist_response_body(content, headers.accept_gzip))
                        .unwrap_or_else(|| (bytes::Bytes::new(), false));
                    let mut resp_headers = cors_headers_no_cache();
                    if gzipped {
                        push_gzip_response_headers(&mut resp_headers);
                    }
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "application/vnd.apple.mpegurl",
                            body,
                            headers: resp_headers,
                        })
                        .await;
                }
            }
        }
        HlsCoreEvent::PartRequested {
            stream_key,
            part_name,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);

            // Stream key validation
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }

            // Parse part sequence from name (e.g., "part_5" → 5)
            let part_seq = part_name
                .strip_prefix("part_")
                .and_then(|s| s.parse::<u64>().ok());

            let Some(seq) = part_seq else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Invalid Part Name"),
                        headers: cors_headers(),
                    })
                    .await;
                return;
            };

            // Try to get the part data
            let part_state = {
                let map = muxers.lock();
                match map.get(&key) {
                    Some(m) => {
                        let mux = m.lock();
                        Some((mux.get_part(seq), mux.next_part_seq(), mux.is_ll_hls()))
                    }
                    None => None,
                }
            };

            match classify_part_request(
                part_state
                    .as_ref()
                    .map(|(data, next_seq, is_ll)| (data.clone(), *next_seq, *is_ll)),
                seq,
            ) {
                PartRequestDecision::Ready => {
                    let data = part_state
                        .and_then(|(data, _, _)| data)
                        .expect("ready part has data");
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "video/mp4",
                            body: data,
                            headers: cors_headers(),
                        })
                        .await;
                }
                PartRequestDecision::Pending => {
                    // Part not yet produced but is the next expected — hold request
                    let over_limit = {
                        let mut pmap = pending.lock();
                        let stream_pending = pmap.entry(key).or_default();
                        let total = stream_pending.playlists.len() + stream_pending.parts.len();
                        if total >= config.max_pending_requests {
                            true
                        } else {
                            stream_pending.parts.push(PendingPartRequest {
                                connection_id,
                                target_part_seq: seq,
                                lane: None,
                                created_at_us: current_time_us(),
                            });
                            false
                        }
                    };
                    if over_limit {
                        let _ = cmd_tx
                            .send(HlsDriverCommand::SendResponse {
                                connection_id,
                                status: 404,
                                content_type: "text/plain",
                                body: bytes::Bytes::from_static(b"Part Not Found"),
                                headers: cors_headers(),
                            })
                            .await;
                    }
                }
                PartRequestDecision::NotFound => {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Part Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                }
            }
        }
        HlsCoreEvent::TrackInitSegmentRequested {
            stream_key,
            lane,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }
            let data = {
                let map = muxers.lock();
                map.get(&key)
                    .and_then(|m| m.lock().track_init_segment(lane))
            };
            if let Some(data) = data {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 200,
                        content_type: "video/mp4",
                        body: data,
                        headers: cors_headers_with_max_age(config.cache_control.segment_max_age),
                    })
                    .await;
            } else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Not Found"),
                        headers: cors_headers(),
                    })
                    .await;
            }
        }
        HlsCoreEvent::TrackPartRequested {
            stream_key,
            lane,
            part_name,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }
            // Parse part seq: "video_part_5" -> 5, "audio_part_3" -> 3
            let part_seq = part_name
                .rsplit('_')
                .next()
                .and_then(|s| s.parse::<u64>().ok());
            let Some(seq) = part_seq else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Invalid Part Name"),
                        headers: cors_headers(),
                    })
                    .await;
                return;
            };
            let (data, next_seq) = {
                let map = muxers.lock();
                match map.get(&key) {
                    Some(m) => {
                        let mux = m.lock();
                        (mux.track_part(lane, seq), mux.track_next_part_seq(lane))
                    }
                    None => (None, 0),
                }
            };
            if let Some(data) = data {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 200,
                        content_type: "video/mp4",
                        body: data,
                        headers: cors_headers_with_max_age(
                            config.cache_control.partial_segment_max_age,
                        ),
                    })
                    .await;
            } else if seq == next_seq {
                // Part not yet produced but is the next expected — hold request
                let over_limit = {
                    let mut pmap = pending.lock();
                    let stream_pending = pmap.entry(key).or_default();
                    let total = stream_pending.playlists.len() + stream_pending.parts.len();
                    if total >= config.max_pending_requests {
                        true
                    } else {
                        stream_pending.parts.push(PendingPartRequest {
                            connection_id,
                            target_part_seq: seq,
                            lane: Some(lane),
                            created_at_us: current_time_us(),
                        });
                        false
                    }
                };
                if over_limit {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Part Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                }
            } else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Not Found"),
                        headers: cors_headers(),
                    })
                    .await;
            }
        }
        HlsCoreEvent::TrackSegmentRequested {
            stream_key,
            lane,
            segment_name,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);
            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }
            let data = {
                let map = muxers.lock();
                map.get(&key)
                    .and_then(|m| m.lock().track_segment(lane, &segment_name))
            };
            if let Some(data) = data {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 200,
                        content_type: "video/mp4",
                        body: data,
                        headers: cors_headers_with_max_age(config.cache_control.segment_max_age),
                    })
                    .await;
            } else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Not Found"),
                        headers: cors_headers(),
                    })
                    .await;
            }
        }
        HlsCoreEvent::TrackMediaPlaylistRequested {
            stream_key,
            lane,
            session_id,
            blocking,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);
            refresh_session_activity(
                sessions,
                &key,
                session_id,
                engine.runtime_api.now().as_micros(),
            );
            ensure_muxer(
                engine,
                config,
                muxers,
                pending,
                cmd_tx,
                &stream_key,
                content_notify_tx,
            );
            if config.hls_demand {
                if let Some(m) = muxers.lock().get(&key) {
                    m.lock().enabled = true;
                }
            }
            wait_for_demuxed_master_muxer(engine, config, muxers, &key).await;

            if config.stream_key_validation {
                let valid = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| {
                            let expected = m.lock().stream_key().to_owned();
                            key_token.as_deref() == Some(&expected)
                        })
                        .unwrap_or(false)
                };
                if !valid {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 404,
                            content_type: "text/plain",
                            body: bytes::Bytes::from_static(b"Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                    return;
                }
            }
            let include_stream_key = config.stream_key_validation;

            // Blocking hold: if _HLS_msn/_HLS_part present and not yet satisfied, queue
            if let Some(ref bp) = blocking {
                let satisfied = {
                    let map = muxers.lock();
                    map.get(&key)
                        .map(|m| m.lock().is_track_blocking_satisfied(lane, bp.msn, bp.part))
                        .unwrap_or(true)
                };
                if !satisfied {
                    let over_limit = {
                        let mut pmap = pending.lock();
                        let stream_pending = pmap.entry(key.clone()).or_default();
                        let total = stream_pending.playlists.len() + stream_pending.parts.len();
                        if total >= config.max_pending_requests {
                            true
                        } else {
                            stream_pending.playlists.push(PendingPlaylistRequest {
                                connection_id,
                                target_msn: bp.msn,
                                target_part: bp.part,
                                session_id,
                                lane: Some(lane),
                                legacy: false,
                                rewind: false,
                                accept_gzip: false,
                                include_stream_key,
                                created_at_us: current_time_us(),
                            });
                            false
                        }
                    };
                    if !over_limit {
                        return; // held — will be released by content_notify
                    }
                    // Over limit: fall through to return current playlist
                }
            }

            let playlist = {
                let map = muxers.lock();
                map.get(&key).and_then(|m| {
                    m.lock()
                        .track_playlist(lane, session_id, include_stream_key)
                })
            };
            if let Some(content) = playlist {
                let max_age = if blocking.is_some() {
                    config.cache_control.chunklist_with_directives_max_age
                } else {
                    config.cache_control.chunklist_max_age
                };
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 200,
                        content_type: "application/vnd.apple.mpegurl",
                        body: bytes::Bytes::from(content),
                        headers: cors_headers_with_max_age(max_age),
                    })
                    .await;
            } else {
                let _ = cmd_tx
                    .send(HlsDriverCommand::SendResponse {
                        connection_id,
                        status: 404,
                        content_type: "text/plain",
                        body: bytes::Bytes::from_static(b"Not Found"),
                        headers: cors_headers(),
                    })
                    .await;
            }
        }
    }
}

/// Ensure a muxer + subscriber task exists for the given stream.
fn ensure_muxer(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    stream_key: &StreamKeyParts,
    content_notify_tx: &futures::channel::mpsc::Sender<String>,
) {
    let key = stream_key_string(stream_key);
    let mut map = muxers.lock();
    if map.contains_key(&key) {
        return;
    }

    let effective_container = parse_container(&config.container);
    let ll_hls_enabled =
        config.ll_hls_enabled && effective_container == cheetah_hls_core::HlsContainer::Fmp4;
    let muxer = Arc::new(Mutex::new(StreamMuxer::new(StreamMuxerConfig {
        segment_duration_ms: config.segment_duration_ms,
        segment_count: config.segment_count,
        ready_threshold: config.ready_threshold,
        force_segment_after_ms: config.force_segment_after_ms,
        fast_register: config.fast_register,
        container: effective_container,
        ll_hls_enabled,
        part_target_ms: config.part_target_ms,
        max_completed_segments: config.segment_count + 2,
        ll_hls_packaging_mode: cheetah_hls_core::LlHlsPackagingMode::parse(
            &config.ll_hls_packaging_mode,
        ),
        origin_mode: config.origin_mode,
        stream_name: key.clone(),
    })));
    map.insert(key.clone(), muxer.clone());

    // Spawn subscriber task
    let engine2 = engine.clone();
    let sdk_stream_key = StreamKey::new(&stream_key.namespace, &stream_key.stream_path);
    let muxers_ref = muxers.clone();
    let pending_ref = pending.clone();
    let cmd_tx_ref = cmd_tx.clone();
    let file_output = config.file_output.clone();
    let blocking_timeout_us = config.blocking_timeout_ms * 1000;
    let directives_max_age = config.cache_control.chunklist_with_directives_max_age;
    let partial_segment_max_age = config.cache_control.partial_segment_max_age;
    let concluded_retention_secs = config.concluded_retention_secs;
    let ns = stream_key.namespace.clone();
    let sp = stream_key.stream_path.clone();
    let notify_tx = content_notify_tx.clone();

    let runtime_api = engine.runtime_api.clone();
    let _ = runtime_api.spawn(Box::pin(async move {
        run_subscriber(
            engine2,
            sdk_stream_key,
            muxer,
            key,
            muxers_ref,
            pending_ref,
            cmd_tx_ref,
            blocking_timeout_us,
            directives_max_age,
            partial_segment_max_age,
            file_output,
            concluded_retention_secs,
            ns,
            sp,
            notify_tx,
        )
        .await;
    }));
}

#[allow(clippy::too_many_arguments)]
async fn run_subscriber(
    engine: EngineContext,
    stream_key: StreamKey,
    muxer: Arc<Mutex<StreamMuxer>>,
    muxer_key: String,
    muxers: MuxerMap,
    pending: PendingMap,
    cmd_tx: HlsCommandSender,
    blocking_timeout_us: u64,
    directives_max_age: i32,
    partial_segment_max_age: i32,
    file_output: crate::config::HlsFileOutputConfig,
    concluded_retention_secs: u64,
    app: String,
    stream: String,
    mut content_notify_tx: futures::channel::mpsc::Sender<String>,
) {
    use cheetah_hls_driver_tokio::HlsFileWriter;
    use std::path::PathBuf;

    let disk_enabled = file_output.enabled
        && (file_output.storage_mode == "disk" || file_output.storage_mode == "hybrid");

    let mut file_writer = if disk_enabled {
        let w = HlsFileWriter::new(
            PathBuf::from(&file_output.output_dir),
            file_output.max_disk_segments,
        );
        if let Err(e) = w.init().await {
            warn!("HLS file writer init failed for {muxer_key}: {e}");
        }
        Some(w)
    } else {
        None
    };

    // Subscribe to the stream. The HLS muxer is created on the first playlist
    // request, which can race ahead of the publisher (e.g. CDN origins polling
    // the stream eagerly). Rather than aborting on the first "not found" we
    // back off and retry for a short window so a slow publisher (HEVC enhanced
    // RTMP, large keyframes, network jitter) can register the stream.
    let mut subscriber = {
        const SUBSCRIBE_RETRY_TOTAL: usize = 30;
        const SUBSCRIBE_RETRY_INTERVAL_MS: u64 = 200;
        let mut last_err: Option<SdkError> = None;
        let mut sub = None;
        for attempt in 0..SUBSCRIBE_RETRY_TOTAL {
            match engine
                .subscriber_api
                .subscribe(
                    stream_key.clone(),
                    SubscriberOptions {
                        queue_capacity: 256,
                        bootstrap_policy: BootstrapPolicy::full_gop(150, Some(5000)),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(s) => {
                    sub = Some(s);
                    break;
                }
                Err(SdkError::NotFound(msg)) => {
                    last_err = Some(SdkError::NotFound(msg));
                    if attempt + 1 == SUBSCRIBE_RETRY_TOTAL {
                        break;
                    }
                    let deadline = cheetah_codec::MonoTime::from_micros(
                        engine.runtime_api.now().as_micros() + SUBSCRIBE_RETRY_INTERVAL_MS * 1000,
                    );
                    engine.runtime_api.sleep_until(deadline).wait().await;
                    continue;
                }
                Err(other) => {
                    last_err = Some(other);
                    break;
                }
            }
        }
        match sub {
            Some(s) => s,
            None => {
                let err = last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                warn!("HLS subscribe failed for {muxer_key}: {err}");
                muxers.lock().remove(&muxer_key);
                return;
            }
        }
    };

    // Initialize muxer with track info
    if let Ok(Some(snapshot)) = engine.stream_manager_api.get_stream(&stream_key).await {
        if !snapshot.tracks.is_empty() {
            muxer.lock().set_tracks(&snapshot.tracks);
        }
    }

    // Write init segment to disk if fMP4 mode
    if let Some(writer) = &file_writer {
        let init_data = muxer.lock().init_segment();
        if let Some(data) = init_data {
            let _ = writer.write_init_segment(&app, &stream, &data).await;
        }
    }

    // Receive frames and feed to muxer
    let mut audio_track_refreshed = false;
    loop {
        match subscriber.recv().await {
            Ok(Some(frame)) => {
                // First audio frame may arrive before the AAC AudioSpecificConfig was
                // published to the stream snapshot. In that case re-pull the snapshot
                // and feed the muxer again so that ADTS wrapping uses the right
                // sampling-rate / channel layout.
                if !audio_track_refreshed && frame.media_kind == cheetah_codec::MediaKind::Audio {
                    audio_track_refreshed = true;
                    if muxer.lock().needs_aac_config_refresh() {
                        if let Ok(Some(snapshot)) =
                            engine.stream_manager_api.get_stream(&stream_key).await
                        {
                            if !snapshot.tracks.is_empty() {
                                muxer.lock().set_tracks(&snapshot.tracks);
                            }
                        }
                    }
                }

                let outputs = muxer.lock().push_frame(&frame);
                let has_new_segment = outputs
                    .iter()
                    .any(|o| matches!(o, MuxerOutput::SegmentReady { .. }));
                let has_new_part = outputs
                    .iter()
                    .any(|o| matches!(o, MuxerOutput::PartReady(_)));

                // Notify main loop that new content is available for pending release
                if has_new_part || has_new_segment {
                    let _ = content_notify_tx.try_send(muxer_key.clone());
                }

                if has_new_segment {
                    // Extract segment info once under a single lock
                    let (seg_info, playlist, container) = {
                        let mux = muxer.lock();
                        let seg = mux.latest_segment();
                        let pl = mux.playlist(None);
                        let c = mux.container();
                        (seg, pl, c)
                    };

                    // Emit on_segment event
                    if let Some((ref seg_name, ref seg_data)) = seg_info {
                        engine.event_bus.publish(cheetah_sdk::SystemEvent::System(
                            cheetah_sdk::SystemLifecycleEvent {
                                component: "hls".to_string(),
                                phase: "on_segment".to_string(),
                                message: Some(format!(
                                    "{{\"stream\":\"{app}/{stream}\",\"segment\":\"{seg_name}\",\"size\":{size}}}",
                                    size = seg_data.len()
                                )),
                            },
                        ));
                    }

                    if let Some(writer) = &mut file_writer {
                        if let Some((name, data)) = seg_info {
                            let ext = match container {
                                cheetah_hls_core::HlsContainer::Fmp4 => ".m4s",
                                cheetah_hls_core::HlsContainer::Ts => ".ts",
                            };
                            let filename = format!("{name}{ext}");
                            let _ = writer.write_segment(&app, &stream, &filename, &data).await;
                        }
                        let _ = writer.write_playlist(&app, &stream, &playlist).await;
                    }
                }
            }
            Ok(None) | Err(_) => {
                muxer.lock().conclude();
                release_pending_requests(
                    &muxers,
                    &pending,
                    &cmd_tx,
                    &muxer_key,
                    blocking_timeout_us,
                    directives_max_age,
                    partial_segment_max_age,
                )
                .await;
                let _ = content_notify_tx.try_send(muxer_key.clone());
                break;
            }
        }
    }

    // Cleanup on stream end
    if file_output.cleanup_on_unpublish {
        if let Some(writer) = &file_writer {
            writer.cleanup_stream(&app, &stream).await;
        }
    }

    // Hold the concluded muxer in the map for `concluded_retention_secs` so late-joining
    // viewers can still pull the EXT-X-ENDLIST playlist + already-finalised segments.
    // Without this, every short clip (<10s) would race with publisher disconnect and
    // hand subscribers an empty "not ready" playlist immediately after EOS.
    if concluded_retention_secs > 0 {
        let deadline = cheetah_codec::MonoTime::from_micros(
            engine.runtime_api.now().as_micros() + concluded_retention_secs * 1_000_000,
        );
        engine.runtime_api.sleep_until(deadline).wait().await;
    }
    muxers.lock().remove(&muxer_key);
    debug!("HLS subscriber ended for {muxer_key}");
}

/// Release pending blocking requests that are now satisfied.
async fn release_pending_requests(
    muxers: &MuxerMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    stream_key: &str,
    blocking_timeout_us: u64,
    directives_max_age: i32,
    partial_segment_max_age: i32,
) {
    // Collect all data under locks, then drop locks before await
    let (playlist_responses, released_parts, expired_playlists, expired_parts) = {
        let mut pmap = pending.lock();
        let Some(stream_pending) = pmap.get_mut(stream_key) else {
            return;
        };

        if stream_pending.playlists.is_empty() && stream_pending.parts.is_empty() {
            return;
        }

        let muxer_arc = {
            let muxer_map = muxers.lock();
            muxer_map.get(stream_key).cloned()
        };
        match muxer_arc {
            Some(muxer_arc) => {
                let mux = muxer_arc.lock();

                // Release satisfied playlist requests
                let mut released_pl = Vec::new();
                let mut i = 0;
                while i < stream_pending.playlists.len() {
                    let req = &stream_pending.playlists[i];
                    let satisfied = match req.lane {
                        Some(lane) => {
                            mux.is_track_blocking_satisfied(lane, req.target_msn, req.target_part)
                        }
                        None => mux.is_blocking_satisfied(req.target_msn, req.target_part),
                    };
                    if satisfied {
                        released_pl.push(stream_pending.playlists.swap_remove(i));
                    } else {
                        i += 1;
                    }
                }

                // Release satisfied part requests
                let mut released_pt: Vec<(PendingPartRequest, bytes::Bytes)> = Vec::new();
                let mut expired_pt = Vec::new();
                let mut i = 0;
                while i < stream_pending.parts.len() {
                    let req = &stream_pending.parts[i];
                    let data = match req.lane {
                        Some(lane) => mux.track_part(lane, req.target_part_seq),
                        None => mux.get_part(req.target_part_seq),
                    };
                    if let Some(data) = data {
                        let removed = stream_pending.parts.swap_remove(i);
                        released_pt.push((removed, data));
                    } else if mux.is_concluded() {
                        expired_pt.push(stream_pending.parts.swap_remove(i));
                    } else {
                        i += 1;
                    }
                }

                // Expire timed-out requests
                let now_us = current_time_us();

                let mut expired_pl = Vec::new();
                let mut i = 0;
                while i < stream_pending.playlists.len() {
                    if blocking_timeout_us > 0
                        && now_us.saturating_sub(stream_pending.playlists[i].created_at_us)
                            >= blocking_timeout_us
                    {
                        expired_pl.push(stream_pending.playlists.swap_remove(i));
                    } else {
                        i += 1;
                    }
                }
                let mut i = 0;
                while i < stream_pending.parts.len() {
                    if blocking_timeout_us > 0
                        && now_us.saturating_sub(stream_pending.parts[i].created_at_us)
                            >= blocking_timeout_us
                    {
                        expired_pt.push(stream_pending.parts.swap_remove(i));
                    } else {
                        i += 1;
                    }
                }

                let playlist_responses = released_pl
                    .iter()
                    .chain(expired_pl.iter())
                    .map(|req| build_pending_playlist_response(&mux, req, directives_max_age))
                    .collect::<Vec<_>>();

                (playlist_responses, released_pt, Vec::new(), expired_pt)
            }
            None => {
                let (expired_pl, expired_pt) = drain_pending_for_missing_muxer(stream_pending);
                (Vec::new(), Vec::new(), expired_pl, expired_pt)
            }
        }
    };
    // All locks dropped here

    for response in playlist_responses {
        let _ = cmd_tx
            .send(HlsDriverCommand::SendResponse {
                connection_id: response.connection_id,
                status: 200,
                content_type: "application/vnd.apple.mpegurl",
                body: response.body,
                headers: response.headers,
            })
            .await;
    }

    // Send released part responses
    for (req, data) in released_parts {
        let _ = cmd_tx
            .send(HlsDriverCommand::SendResponse {
                connection_id: req.connection_id,
                status: 200,
                content_type: "video/mp4",
                body: data,
                headers: cors_headers_with_max_age(partial_segment_max_age),
            })
            .await;
    }

    for req in expired_playlists {
        let _ = cmd_tx
            .send(HlsDriverCommand::SendResponse {
                connection_id: req.connection_id,
                status: 404,
                content_type: "text/plain",
                body: bytes::Bytes::from_static(b"Stream Not Found"),
                headers: cors_headers_no_cache(),
            })
            .await;
    }

    // Send 404 for expired part requests
    for req in expired_parts {
        let _ = cmd_tx
            .send(HlsDriverCommand::SendResponse {
                connection_id: req.connection_id,
                status: 404,
                content_type: "text/plain",
                body: bytes::Bytes::from_static(b"Part Not Found"),
                headers: cors_headers(),
            })
            .await;
    }
}

async fn release_all_pending_requests(
    muxers: &MuxerMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    blocking_timeout_us: u64,
    directives_max_age: i32,
    partial_segment_max_age: i32,
) {
    let keys = {
        let pmap = pending.lock();
        pmap.keys().cloned().collect::<Vec<_>>()
    };
    for key in keys {
        release_pending_requests(
            muxers,
            pending,
            cmd_tx,
            &key,
            blocking_timeout_us,
            directives_max_age,
            partial_segment_max_age,
        )
        .await;
    }
}

fn drain_pending_for_missing_muxer(
    pending: &mut StreamPendingRequests,
) -> (Vec<PendingPlaylistRequest>, Vec<PendingPartRequest>) {
    (
        std::mem::take(&mut pending.playlists),
        std::mem::take(&mut pending.parts),
    )
}

fn pending_timeout_deadline(
    runtime_api: &dyn RuntimeApi,
    pending: &PendingMap,
    blocking_timeout_us: u64,
) -> cheetah_codec::MonoTime {
    let now = runtime_api.now();
    if blocking_timeout_us == 0 {
        return cheetah_codec::MonoTime::from_micros(now.as_micros() + 60_000_000);
    }

    let next_timeout = {
        let pmap = pending.lock();
        next_pending_timeout_us(&pmap, blocking_timeout_us)
    };
    let Some(deadline_us) = next_timeout else {
        return cheetah_codec::MonoTime::from_micros(now.as_micros() + 60_000_000);
    };

    let wall_now_us = current_time_us();
    let delta_us = deadline_us.saturating_sub(wall_now_us);
    cheetah_codec::MonoTime::from_micros(now.as_micros() + delta_us)
}

fn next_pending_timeout_us(
    pending: &HashMap<String, StreamPendingRequests>,
    blocking_timeout_us: u64,
) -> Option<u64> {
    if blocking_timeout_us == 0 {
        return None;
    }
    pending
        .values()
        .flat_map(|stream| {
            stream
                .playlists
                .iter()
                .map(|req| req.created_at_us)
                .chain(stream.parts.iter().map(|req| req.created_at_us))
        })
        .min()
        .map(|created_at_us| created_at_us.saturating_add(blocking_timeout_us))
}

fn classify_part_request(
    state: Option<(Option<bytes::Bytes>, u64, bool)>,
    requested_seq: u64,
) -> PartRequestDecision {
    let Some((part_data, next_seq, is_ll_hls)) = state else {
        return PartRequestDecision::NotFound;
    };
    if part_data.is_some() {
        return PartRequestDecision::Ready;
    }
    if is_ll_hls && requested_seq == next_seq {
        PartRequestDecision::Pending
    } else {
        PartRequestDecision::NotFound
    }
}

#[allow(dead_code)]
struct PendingPlaylistResponse {
    connection_id: HlsConnectionId,
    content: String,
    body: bytes::Bytes,
    gzipped: bool,
    headers: Vec<(&'static str, String)>,
}

fn build_pending_playlist_response(
    mux: &StreamMuxer,
    req: &PendingPlaylistRequest,
    directives_max_age: i32,
) -> PendingPlaylistResponse {
    let content = if let Some(lane) = req.lane {
        // Per-track playlist for demuxed mode
        mux.track_playlist(lane, req.session_id, req.include_stream_key)
            .unwrap_or_default()
    } else if req.rewind {
        mux.playlist_rewind_with_token(req.session_id, req.include_stream_key)
    } else {
        mux.playlist_with_options_and_token(req.session_id, req.legacy, req.include_stream_key)
    };
    let (body, gzipped) = playlist_response_body(&content, req.accept_gzip);
    let mut headers = cors_headers_with_max_age(directives_max_age);
    if gzipped {
        push_gzip_response_headers(&mut headers);
    }
    PendingPlaylistResponse {
        connection_id: req.connection_id,
        content,
        body,
        gzipped,
        headers,
    }
}

/// Gzip compress a byte slice. Returns compressed bytes or original on failure.
fn gzip_bytes(data: &[u8]) -> bytes::Bytes {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), Compression::fast());
    if encoder.write_all(data).is_ok() {
        if let Ok(compressed) = encoder.finish() {
            return bytes::Bytes::from(compressed);
        }
    }
    bytes::Bytes::copy_from_slice(data)
}

/// Build playlist response body, applying gzip if requested.
fn playlist_response_body(content: &str, accept_gzip: bool) -> (bytes::Bytes, bool) {
    if accept_gzip && content.len() > 100 {
        (gzip_bytes(content.as_bytes()), true)
    } else {
        (bytes::Bytes::from(content.to_owned()), false)
    }
}

fn push_gzip_response_headers(headers: &mut Vec<(&'static str, String)>) {
    headers.push(("Content-Encoding", "gzip".to_string()));
    if !headers.iter().any(|(name, _)| *name == "Vary") {
        headers.push(("Vary", "Accept-Encoding".to_string()));
    }
}

fn build_master_playlist_content(
    stream_key: &StreamKeyParts,
    muxer: Option<&StreamMuxer>,
    session_id: u64,
    include_stream_key: bool,
) -> String {
    if let Some(mux) = muxer {
        if mux.is_demuxed() {
            let sk = if include_stream_key {
                mux.stream_key().to_owned()
            } else {
                String::new()
            };
            let (w, h) = mux.video_dimensions();
            return cheetah_hls_core::DemuxedMasterPlaylist::build(
                Some(&cheetah_hls_core::MediaRenditionInfo {
                    codecs: codec_string(mux.video_codec(), mux.video_extradata()),
                    bandwidth: 2000000,
                    width: if w > 0 { Some(w as u32) } else { None },
                    height: if h > 0 { Some(h as u32) } else { None },
                    frame_rate: None,
                    channels: None,
                }),
                Some(&cheetah_hls_core::MediaRenditionInfo {
                    codecs: codec_string(mux.audio_codec(), mux.audio_extradata()),
                    bandwidth: 128000,
                    width: None,
                    height: None,
                    frame_rate: None,
                    channels: Some(mux.audio_channels()),
                }),
                &stream_key.stream_path,
                Some(session_id),
                include_stream_key,
                &sk,
            );
        }
    }
    PlaylistBuilder::build_master(&stream_key.stream_path, session_id)
}

async fn wait_for_demuxed_master_muxer(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    key: &str,
) {
    if parse_container(&config.container) != HlsContainer::Fmp4
        || !config.ll_hls_enabled
        || cheetah_hls_core::LlHlsPackagingMode::parse(&config.ll_hls_packaging_mode)
            != cheetah_hls_core::LlHlsPackagingMode::DemuxedAv
    {
        return;
    }

    for _ in 0..50 {
        let ready = muxers
            .lock()
            .get(key)
            .map(|m| {
                let mux = m.lock();
                mux.is_demuxed() && mux.is_ready()
            })
            .unwrap_or(false);
        if ready {
            return;
        }
        let deadline =
            cheetah_codec::MonoTime::from_micros(engine.runtime_api.now().as_micros() + 100_000);
        engine.runtime_api.sleep_until(deadline).wait().await;
    }
}

/// Current wall-clock time in microseconds (for timeout calculations).
fn current_time_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn stream_key_string(parts: &StreamKeyParts) -> String {
    format!("{}/{}", parts.namespace, parts.stream_path)
}

fn codec_string(codec: cheetah_codec::CodecId, extradata: &[u8]) -> String {
    use cheetah_codec::CodecId;
    match codec {
        CodecId::H264 => {
            // avcC: [version, profile, compat, level, ...]
            // Generate "avc1.PPCCLL" from actual profile/compat/level
            if extradata.len() >= 4 {
                let profile = extradata[1];
                let compat = extradata[2];
                let level = extradata[3];
                format!("avc1.{profile:02x}{compat:02x}{level:02x}")
            } else {
                "avc1.64001f".to_string()
            }
        }
        CodecId::H265 => "hvc1.1.6.L93.B0".to_string(),
        CodecId::VP9 => "vp09.00.10.08".to_string(),
        CodecId::AV1 => "av01.0.01M.08".to_string(),
        CodecId::AAC => {
            // Parse audio_object_type from ASC to determine AAC profile
            if extradata.len() >= 2 {
                let aot = (extradata[0] >> 3) & 0x1f;
                format!("mp4a.40.{aot}")
            } else {
                "mp4a.40.2".to_string()
            }
        }
        CodecId::Opus => "Opus".to_string(),
        CodecId::MP3 => "mp4a.40.34".to_string(),
        _ => String::new(),
    }
}

fn cors_headers() -> Vec<(&'static str, String)> {
    vec![
        ("Access-Control-Allow-Origin", "*".to_string()),
        (
            "Access-Control-Allow-Methods",
            "GET, HEAD, OPTIONS".to_string(),
        ),
        (
            "Access-Control-Allow-Headers",
            "Origin, Range, Accept-Encoding, Referer".to_string(),
        ),
    ]
}

fn cors_headers_no_cache() -> Vec<(&'static str, String)> {
    let mut h = cors_headers();
    h.push(("Cache-Control", "no-cache".to_string()));
    h
}

/// Build CORS headers with Cache-Control based on config max_age value.
/// -1 = no Cache-Control header, 0 = no-cache/no-store, >0 = max-age=N.
fn cors_headers_with_max_age(max_age: i32) -> Vec<(&'static str, String)> {
    let mut h = cors_headers();
    if max_age == 0 {
        h.push(("Cache-Control", "no-cache, no-store".to_string()));
    } else if max_age > 0 {
        h.push(("Cache-Control", format!("max-age={max_age}")));
    }
    // max_age < 0: don't set Cache-Control
    h
}

fn segment_response_headers(segment_name: &str, max_age: i32) -> Vec<(&'static str, String)> {
    let mut headers = cors_headers_with_max_age(max_age);
    headers.push(("ETag", format!("\"{segment_name}\"")));
    headers.push(("Accept-Ranges", "bytes".to_string()));
    headers
}

fn parse_container(s: &str) -> HlsContainer {
    match s {
        "fmp4" | "fMP4" | "mp4" => HlsContainer::Fmp4,
        _ => HlsContainer::Ts,
    }
}

/// Check if request has valid CDN Bearer token authorization.
fn is_cdn_authorized(authorization: &Option<String>, cdn_secret: &str) -> bool {
    if cdn_secret.is_empty() {
        return false;
    }
    match authorization {
        Some(auth) => {
            if let Some(token) = auth.strip_prefix("Bearer ") {
                token.trim() == cdn_secret
            } else {
                false
            }
        }
        None => false,
    }
}

/// Check if User-Agent indicates an iOS device.
#[allow(dead_code)]
fn is_ios_user_agent(ua: &Option<String>) -> bool {
    match ua {
        Some(ua) => {
            ua.contains("iPhone")
                || ua.contains("iPad")
                || ua.contains("iPod")
                || (ua.contains("Mac OS") && ua.contains("Safari") && !ua.contains("Chrome"))
        }
        None => false,
    }
}

/// Generate CORS headers appropriate for CDN requests (no cache restrictions).
fn cors_headers_cdn() -> Vec<(&'static str, String)> {
    cors_headers()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_codec::{
        AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
        TrackInfo,
    };

    use crate::muxer::generate_stream_validation_key;

    use super::*;

    fn ready_muxer() -> StreamMuxer {
        let mut muxer = StreamMuxer::new(StreamMuxerConfig {
            segment_duration_ms: 2000,
            segment_count: 3,
            ready_threshold: 1,
            force_segment_after_ms: 10000,
            fast_register: true,
            container: HlsContainer::Fmp4,
            ll_hls_enabled: true,
            part_target_ms: 200,
            max_completed_segments: 5,
            ll_hls_packaging_mode: cheetah_hls_core::LlHlsPackagingMode::VideoOnly,
            origin_mode: false,
            stream_name: String::new(),
        });
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };
        muxer.set_tracks(&[track]);

        for (idx, keyframe) in [(0, true), (33_000, true)] {
            let mut frame = AVFrame::new(
                TrackId(1),
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                idx,
                idx,
                Timebase::new(1, 1_000_000),
                Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb]),
            );
            if keyframe {
                frame.flags |= FrameFlags::KEY;
            }
            muxer.push_frame(&frame);
        }
        muxer
    }

    fn ready_demuxed_muxer() -> StreamMuxer {
        let mut muxer = StreamMuxer::new(StreamMuxerConfig {
            segment_duration_ms: 2000,
            segment_count: 3,
            ready_threshold: 1,
            force_segment_after_ms: 10000,
            fast_register: true,
            container: HlsContainer::Fmp4,
            ll_hls_enabled: true,
            part_target_ms: 200,
            max_completed_segments: 5,
            ll_hls_packaging_mode: cheetah_hls_core::LlHlsPackagingMode::DemuxedAv,
            origin_mode: false,
            stream_name: String::new(),
        });
        let mut video = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90000);
        video.width = Some(1920);
        video.height = Some(1080);
        video.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x38])],
            avcc: None,
        };
        let mut audio = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48000);
        audio.sample_rate = Some(48000);
        audio.channels = Some(2);
        audio.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x11, 0x90]),
        };
        muxer.set_tracks(&[video, audio]);
        muxer
    }

    #[test]
    fn master_playlist_content_uses_demuxed_audio_rendition() {
        let muxer = ready_demuxed_muxer();
        let stream_key = StreamKeyParts {
            namespace: "live".to_string(),
            stream_path: "test".to_string(),
        };

        let content = build_master_playlist_content(&stream_key, Some(&muxer), 7, false);

        assert!(content.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
        assert!(content.contains("chunklist_audio.m3u8?uid=7"));
        assert!(content.contains("chunklist_video.m3u8?uid=7"));
        assert!(!content.contains("index.m3u8"));
    }

    #[test]
    fn track_playlist_requests_refresh_session_activity() {
        let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
        sessions
            .lock()
            .entry("live/test".to_string())
            .or_default()
            .insert(
                3,
                SessionState {
                    last_request_us: 100,
                    bytes_sent: 0,
                },
            );

        refresh_session_activity(&sessions, "live/test", Some(3), 200);
        refresh_session_activity(&sessions, "live/test", Some(4), 300);
        refresh_session_activity(&sessions, "live/test", None, 400);

        let map = sessions.lock();
        let stream_sessions = map.get("live/test").unwrap();
        assert_eq!(stream_sessions.get(&3).unwrap().last_request_us, 200);
        assert_eq!(stream_sessions.get(&4).unwrap().last_request_us, 300);
        assert_eq!(stream_sessions.len(), 2);
    }

    #[test]
    fn pending_playlist_response_preserves_session_and_gzip_preference() {
        let muxer = ready_muxer();
        let req = PendingPlaylistRequest {
            connection_id: 1,
            target_msn: 0,
            target_part: Some(0),
            session_id: Some(9),
            lane: None,
            legacy: false,
            rewind: false,
            accept_gzip: false,
            include_stream_key: false,
            created_at_us: 0,
        };

        let response = build_pending_playlist_response(&muxer, &req, 60);

        assert!(!response.gzipped);
        assert!(response
            .content
            .contains("#EXT-X-MAP:URI=\"init.mp4?uid=9\""));
        assert!(response.content.contains("seg_0.m4s?uid=9"));
        assert!(!response
            .headers
            .iter()
            .any(|(name, _)| *name == "Content-Encoding"));
    }

    #[test]
    fn gzipped_pending_playlist_response_sets_vary_accept_encoding() {
        let muxer = ready_muxer();
        let req = PendingPlaylistRequest {
            connection_id: 1,
            target_msn: 0,
            target_part: Some(0),
            session_id: Some(9),
            lane: None,
            legacy: false,
            rewind: false,
            accept_gzip: true,
            include_stream_key: false,
            created_at_us: 0,
        };

        let response = build_pending_playlist_response(&muxer, &req, 60);

        assert!(response.gzipped);
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| *name == "Content-Encoding" && value == "gzip"));
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| *name == "Vary" && value == "Accept-Encoding"));
    }

    #[test]
    fn part_request_without_muxer_returns_not_found_instead_of_pending() {
        assert_eq!(
            classify_part_request(None, 0),
            PartRequestDecision::NotFound
        );
    }

    #[test]
    fn non_ll_part_request_returns_not_found_instead_of_pending() {
        assert_eq!(
            classify_part_request(Some((None, 0, false)), 0),
            PartRequestDecision::NotFound
        );
    }

    #[test]
    fn pending_timeout_deadline_uses_oldest_pending_request() {
        let mut pending = HashMap::new();
        pending.insert(
            "live/test".to_string(),
            StreamPendingRequests {
                playlists: vec![PendingPlaylistRequest {
                    connection_id: 1,
                    target_msn: 0,
                    target_part: Some(0),
                    session_id: None,
                    lane: None,
                    legacy: false,
                    rewind: false,
                    accept_gzip: false,
                    include_stream_key: false,
                    created_at_us: 20_000,
                }],
                parts: vec![PendingPartRequest {
                    connection_id: 2,
                    target_part_seq: 3,
                    lane: None,
                    created_at_us: 10_000,
                }],
            },
        );

        assert_eq!(next_pending_timeout_us(&pending, 5_000), Some(15_000));
    }

    #[test]
    fn stream_validation_key_is_random_high_entropy_hex() {
        let first = generate_stream_validation_key();
        let second = generate_stream_validation_key();

        assert_eq!(first.len(), 32);
        assert_eq!(second.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn missing_muxer_pending_requests_are_drained_for_response() {
        let mut pending = StreamPendingRequests {
            playlists: vec![PendingPlaylistRequest {
                connection_id: 11,
                target_msn: 0,
                target_part: Some(0),
                session_id: None,
                lane: None,
                legacy: false,
                rewind: false,
                accept_gzip: false,
                include_stream_key: false,
                created_at_us: 0,
            }],
            parts: vec![PendingPartRequest {
                connection_id: 12,
                target_part_seq: 0,
                lane: None,
                created_at_us: 0,
            }],
        };

        let (playlists, parts) = drain_pending_for_missing_muxer(&mut pending);

        assert_eq!(playlists[0].connection_id, 11);
        assert_eq!(parts[0].connection_id, 12);
        assert!(pending.playlists.is_empty());
        assert!(pending.parts.is_empty());
    }

    #[test]
    fn cache_control_headers_follow_configured_max_age_values() {
        let master = cors_headers_with_max_age(30);
        assert!(master
            .iter()
            .any(|(name, value)| *name == "Cache-Control" && value == "max-age=30"));

        let no_cache = cors_headers_with_max_age(0);
        assert!(no_cache
            .iter()
            .any(|(name, value)| *name == "Cache-Control" && value == "no-cache, no-store"));

        let unset = cors_headers_with_max_age(-1);
        assert!(!unset.iter().any(|(name, _)| *name == "Cache-Control"));
    }

    #[test]
    fn segment_headers_use_configured_cache_control() {
        let headers = segment_response_headers("seg_1", 120);

        assert!(headers
            .iter()
            .any(|(name, value)| *name == "Cache-Control" && value == "max-age=120"));
        assert!(headers
            .iter()
            .any(|(name, value)| *name == "ETag" && value == "\"seg_1\""));
        assert!(headers
            .iter()
            .any(|(name, value)| *name == "Accept-Ranges" && value == "bytes"));
    }

    #[test]
    fn origin_mode_disables_driver_session_cookies() {
        let mut config = HlsModuleConfig::default();
        config.origin_mode = true;

        let driver_config = hls_driver_config(&config);

        assert!(!driver_config.set_session_cookie);
    }
}
