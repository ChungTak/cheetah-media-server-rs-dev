//! HLS module: engine integration, HTTP control API, and TS/fMP4 muxing.
//!
//! The module is the engine-facing boundary for the HLS protocol: it creates the
//! HTTP driver, subscribes to streams, routes HLS HTTP events to muxer state, and
//! manages player sessions, blocking requests, and pull jobs.
//!
//! HLS 模块：引擎集成、HTTP 控制 API 与 TS/fMP4 复用。
//!
//! 该模块是 HLS 协议的引擎侧边界：创建 HTTP 驱动、订阅流、将 HLS HTTP 事件路由到
//! 复用器状态，并管理播放器会话、阻塞请求和拉流任务。
//!

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{subtitle::WebVttCue, MonoTime};
use cheetah_hls_core::{
    HlsContainer, MediaRenditionInfo, PlaylistBuilder, StreamKeyParts, SubtitleRenditionInfo,
    VariantRenditionInfo, VttMuxConfig,
};
use cheetah_hls_driver_tokio::{
    start_server, HlsCommandSender, HlsConnectionId, HlsCoreEvent, HlsDriverCommand,
    HlsDriverConfig, HlsDriverEvent, HlsServerHandle,
};
use cheetah_sdk::media_api::{
    auth::{MediaScope, Principal},
    ids::{MediaKey, StreamKeyBridge},
    port::MediaRequestContext,
    processing::{
        AbrVariant, AudioCodec, AudioTarget, ProcessingJobQuery, ProcessingJobSpec,
        ProcessingJobState, VideoCodec, VideoTarget,
    },
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

/// Request context used for internal HLS module calls that need to enumerate
/// all processing jobs regardless of tenant ownership.
fn admin_request_context() -> MediaRequestContext {
    MediaRequestContext {
        principal: Some(Principal {
            identity: "__system".to_string(),
            scopes: vec![MediaScope::ServerAdmin],
            resource_grants: Vec::new(),
        }),
        source_adapter: "hls-module".to_string(),
        ..Default::default()
    }
}

/// A blocking playlist request waiting for a specific MSN/Part.
///
/// Held in the per-stream pending queue until the requested content is produced
/// or the request times out.
///
/// 等待特定 MSN/Part 的阻塞播放列表请求。
///
/// 保存在每流待处理队列中，直到请求内容生成或超时。
#[derive(Debug)]
#[allow(dead_code)]
struct PendingPlaylistRequest {
    connection_id: HlsConnectionId,
    target_msn: u64,
    target_part: Option<u64>,
    session_id: Option<u64>,
    lane: Option<cheetah_hls_core::TrackLane>,
    legacy: bool,
    rewind: bool,
    accept_gzip: bool,
    include_stream_key: bool,
    created_at_us: u64,
}

/// A blocking part request waiting for a specific part sequence.
///
/// 等待特定分片序列的阻塞分片请求。
#[derive(Debug)]
struct PendingPartRequest {
    connection_id: HlsConnectionId,
    target_part_seq: u64,
    lane: Option<cheetah_hls_core::TrackLane>,
    created_at_us: u64,
}

/// Pending blocking playlist and part requests for one stream.
///
/// 单个流的待处理阻塞播放列表与分片请求。
#[derive(Default)]
struct StreamPendingRequests {
    playlists: Vec<PendingPlaylistRequest>,
    parts: Vec<PendingPartRequest>,
}

/// Map of stream_key → pending blocking requests.
///
/// 流标识到待处理阻塞请求的映射。
type PendingMap = Arc<Mutex<HashMap<String, StreamPendingRequests>>>;

/// Decision for a part request: ready, pending, or not found.
///
/// 分片请求决策：就绪、挂起或不存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartRequestDecision {
    Ready,
    Pending,
    NotFound,
}

/// Factory that creates the HLS module instance.
///
/// Implements `ModuleFactory` so the engine can load and lifecycle-manage the module.
///
/// 创建 HLS 模块实例的工厂。
///
/// 实现 `ModuleFactory`，使引擎能够加载并生命周期管理该模块。
pub struct HlsModuleFactory;

/// Return the module manifest for the engine.
///
/// Declares module id, display name, config namespace, and subscribe capability.
///
/// 返回引擎所需的模块清单。
///
/// 声明模块 ID、显示名称、配置命名空间和订阅能力。
impl ModuleFactory for HlsModuleFactory {
    /// Return the module manifest for the engine.
    ///
    /// Declares module id, display name, config namespace, and subscribe capability.
    ///
    /// 返回引擎所需的模块清单。
    ///
    /// 声明模块 ID、显示名称、配置命名空间和订阅能力。
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

    /// Create a new `HlsModule` instance.
    ///
    /// 创建新的 `HlsModule` 实例。
    fn create(&self) -> Box<dyn Module> {
        Box::new(HlsModule::new())
    }

    /// Register the JSON schema and validator for `HlsModuleConfig`.
    ///
    /// 注册 `HlsModuleConfig` 的 JSON schema 与校验器。
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

/// HLS module runtime state.
///
/// Holds the engine context, config, cancellation token, and runtime task handles.
///
/// HLS 模块运行时状态。
///
/// 保存引擎上下文、配置、取消令牌与运行时任务句柄。
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
/// Return the module metadata.
///
/// 返回模块元数据。
impl Module for HlsModule {
    /// Return the module metadata.
    ///
    /// 返回模块元数据。
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    /// Return the current module state.
    ///
    /// 返回当前模块状态。
    fn state(&self) -> ModuleState {
        self.state
    }

    /// Initialize the module with the provided config and engine context.
    ///
    /// Parses `HlsModuleConfig` from the initial JSON and stores the engine context.
    ///
    /// 使用提供的配置与引擎上下文初始化模块。
    ///
    /// 从初始 JSON 解析 `HlsModuleConfig` 并保存引擎上下文。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = HlsModuleConfig::from_value(ctx.initial_config)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    /// Start the HLS module: bind HTTP server, register service, spawn loops.
    ///
    /// Launches the Tokio HLS driver, registers the `hls://` service, and spawns the
    /// main event loop plus one pull job task per enabled pull job.
    ///
    /// 启动 HLS 模块：绑定 HTTP 服务、注册服务、启动循环。
    ///
    /// 启动 Tokio HLS 驱动，注册 `hls://` 服务，并启动主事件循环及每个启用拉流任务。
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

    /// Stop the HLS module and clean up resources.
    ///
    /// Cancels runtime tasks, waits for shutdown, and unregisters the service.
    ///
    /// 停止 HLS 模块并清理资源。
    ///
    /// 取消运行时任务、等待关闭并注销服务。
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

    /// Apply a config change; restarts the module if the config differs.
    ///
    /// 应用配置变更；如果配置不同则要求模块重启。
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

/// Spawn a future on the runtime and return a oneshot completion receiver.
///
/// 在运行时上启动一个 future 并返回 oneshot 完成接收器。
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

/// Build the HLS driver config from the module config.
///
/// Origin mode disables per-connection session cookies.
///
/// 根据模块配置构建 HLS 驱动配置。
///
/// 源站模式禁用以连接为单位的会话 cookie。
fn hls_driver_config(config: &HlsModuleConfig) -> HlsDriverConfig {
    HlsDriverConfig {
        set_session_cookie: !config.origin_mode,
        ..HlsDriverConfig::default()
    }
}

/// Session UID generator.
///
/// 会话唯一 ID 生成器。
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Return the next unique session identifier.
///
/// 返回下一个唯一会话标识符。
fn new_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Per-stream muxer state managed by the server loop.
///
/// 服务循环管理的每个流复用器状态。
type MuxerMap = Arc<Mutex<HashMap<String, Arc<Mutex<StreamMuxer>>>>>;

/// Per-session tracking state.
///
/// 每个会话的跟踪状态。
struct SessionState {
    last_request_us: u64,
    bytes_sent: u64,
}

/// Player session map: stream_key → {session_id → state}.
///
/// 播放器会话映射：stream_key → {session_id → state}。
type SessionMap = Arc<Mutex<HashMap<String, HashMap<u64, SessionState>>>>;

/// Update a session's last request timestamp.
///
/// Creates the session entry lazily if it does not exist.
///
/// 更新会话的最后请求时间戳。
///
/// 如果会话不存在则惰性创建。
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

/// Main HLS server loop: process driver events and content notifications.
///
/// Maintains muxer and session maps, spawns cleanup and subscriber tasks, and
/// dispatches `HlsCoreEvent` requests to the appropriate handler.
///
/// HLS 主服务循环：处理驱动事件和内容通知。
///
/// 维护复用器与会话映射，启动清理和订阅任务，并将 `HlsCoreEvent` 请求分派到对应处理函数。
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
                            cancel.clone(),
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

/// Evict player sessions that have not requested recently.
///
/// Emits `on_none_reader` lifecycle events and, in `hls_demand` mode, disables
/// muxers for streams with no active viewers.
///
/// 驱逐最近未请求的播放器会话。
///
/// 发出 `on_none_reader` 生命周期事件，并在 `hls_demand` 模式下禁用无活跃观众的流复用器。
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

/// Dispatch a single HTTP driver event to the right handler.
///
/// Covers master/media playlists, segments, init segments, parts, demuxed
/// tracks, blocking requests, and stream-key validation.
///
/// 将单个 HTTP 驱动事件分派到对应的处理函数。
///
/// 覆盖主/媒体播放列表、分段、init segment、分片、分离轨道、阻塞请求和流密钥验证。
#[allow(clippy::too_many_arguments)]
async fn handle_core_event(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    sessions: &SessionMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    cancel: CancellationToken,
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
                cancel.clone(),
                &stream_key,
                content_notify_tx,
            );
            if config.hls_demand {
                if let Some(m) = muxers.lock().get(&key) {
                    m.lock().enabled = true;
                }
            }
            wait_for_demuxed_master_muxer(engine, config, muxers, &key).await;
            let variants = collect_abr_variant_renditions(
                engine,
                config,
                muxers,
                pending,
                cmd_tx,
                content_notify_tx,
                cancel.clone(),
                &stream_key,
                session_id,
            )
            .await;

            let content = {
                let map = muxers.lock();
                let muxer = map.get(&key).map(|m| m.lock());
                build_master_playlist_content(
                    &stream_key,
                    muxer.as_deref(),
                    &variants,
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
                cancel.clone(),
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
                cancel.clone(),
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
                cancel.clone(),
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
        HlsCoreEvent::SubtitleMediaPlaylistRequested {
            stream_key,
            session_id,
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
                cancel.clone(),
                &stream_key,
                content_notify_tx,
            );

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

            let content = {
                let map = muxers.lock();
                map.get(&key)
                    .and_then(|m| {
                        let mux = m.lock();
                        mux.vtt_media_playlist(session_id, config.stream_key_validation)
                    })
                    .unwrap_or_else(|| {
                        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n"
                            .to_string()
                    })
            };
            let _ = cmd_tx
                .send(HlsDriverCommand::SendResponse {
                    connection_id,
                    status: 200,
                    content_type: "application/vnd.apple.mpegurl",
                    body: bytes::Bytes::from(content),
                    headers: cors_headers_with_max_age(config.cache_control.chunklist_max_age),
                })
                .await;
        }
        HlsCoreEvent::SubtitleSegmentRequested {
            stream_key,
            segment_name,
            session_id,
            key_token,
            ..
        } => {
            let key = stream_key_string(&stream_key);
            if let Some(uid) = session_id {
                let now_us = engine.runtime_api.now().as_micros();
                sessions
                    .lock()
                    .entry(key.clone())
                    .or_default()
                    .entry(uid)
                    .and_modify(|s| s.last_request_us = now_us)
                    .or_insert(SessionState {
                        last_request_us: now_us,
                        bytes_sent: 0,
                    });
            }

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

            let segment_data = {
                let map = muxers.lock();
                map.get(&key)
                    .and_then(|m| m.lock().get_vtt_segment(&segment_name))
            };

            match segment_data {
                Some(data) => {
                    let _ = cmd_tx
                        .send(HlsDriverCommand::SendResponse {
                            connection_id,
                            status: 200,
                            content_type: "text/vtt",
                            body: data,
                            headers: segment_response_headers(
                                &segment_name,
                                config.cache_control.segment_max_age,
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
                            body: bytes::Bytes::from_static(b"Subtitle Segment Not Found"),
                            headers: cors_headers(),
                        })
                        .await;
                }
            }
        }
    }
}

/// Create the muxer and subscriber task for a stream if not present.
///
/// Called on the first playlist request for a stream. It inserts a `StreamMuxer`
/// and spawns a subscriber that pulls frames from the engine, plus a caption
/// discovery task if a `MediaProcessingApi` is registered.
///
/// 如果尚不存在，则为流创建复用器和订阅任务。
#[allow(clippy::too_many_arguments)]
fn ensure_muxer(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    cancel: CancellationToken,
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
        vtt_config: Some(VttMuxConfig {
            segment_duration_ms: config.segment_duration_ms,
            max_segments: config.segment_count.max(1),
        }),
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
    let muxer_for_subscriber = muxer.clone();
    let _ = runtime_api.spawn(Box::pin(async move {
        run_subscriber(
            engine2,
            sdk_stream_key,
            muxer_for_subscriber,
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

    // Spawn caption discovery/subscriber task for this stream.
    // Use a Weak reference so the task exits when the muxer is removed from the map,
    // preventing leaked background tasks that poll forever after the stream ends.
    let engine3 = engine.clone();
    let stream_key2 = stream_key.clone();
    let cancel2 = cancel.clone();
    let muxer_for_caption = Arc::downgrade(&muxer);
    let _ = runtime_api.spawn(Box::pin(async move {
        run_caption_subscriber(engine3, stream_key2, muxer_for_caption, cancel2).await;
    }));
}

/// Subscribe to a stream and feed frames into the muxer.
///
/// Retries subscription for a short window, applies track info, handles EOS by
/// concluding the muxer, optionally writes files to disk, and retains the
/// concluded muxer for late viewers.
///
/// 订阅流并将帧送入复用器。
///
/// 短时间重试订阅、应用轨道信息、在 EOS 时结束复用器、可选写入磁盘，并在结束后
/// 为延迟观众保留复用器。
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

/// Discover a running `CaptionExtract` job for the source stream, subscribe to its
/// target stream, and push deserialized `WebVttCue` frames into the HLS muxer.
///
/// Loops while the stream/muxer is alive: it retries discovery when no job is found,
/// resubscribes if the caption publisher goes away, and stops when the cancellation
/// token fires.
///
/// 发现并订阅 `CaptionExtract` 派生字幕流，将 `WebVttCue` 推入 HLS 复用器。
async fn run_caption_subscriber(
    engine: EngineContext,
    stream_key: StreamKeyParts,
    muxer: std::sync::Weak<Mutex<StreamMuxer>>,
    cancel: CancellationToken,
) {
    let processing_api = match engine.media_services.processing() {
        Some(api) => api,
        None => return,
    };

    let source_key = match StreamKeyBridge::from_namespace_path(
        &stream_key.namespace,
        &stream_key.stream_path,
    ) {
        Ok(k) => k,
        Err(e) => {
            warn!("hls caption subscriber: invalid stream key {stream_key:?}: {e}");
            return;
        }
    };

    const DISCOVERY_INTERVAL_US: u64 = 2_000_000;
    const SUBSCRIBE_RETRY_INTERVAL_MS: u64 = 200;
    const SUBSCRIBE_RETRY_TOTAL: usize = 30;
    let ctx = admin_request_context();

    while !cancel.is_cancelled() {
        // Discovery loop: wait for a running CaptionExtract job whose source matches.
        let mut target_key: Option<MediaKey> = None;
        while target_key.is_none() && !cancel.is_cancelled() {
            // Exit if the muxer/stream is no longer alive.
            if muxer.upgrade().is_none() {
                return;
            }

            let mut query = ProcessingJobQuery {
                vhost: Some(source_key.vhost.0.clone()),
                app: Some(source_key.app.0.clone()),
                stream: Some(source_key.stream.0.clone()),
                state: Some(ProcessingJobState::Running),
                page: 1,
                page_size: 20,
            };
            query.clamp_page_size();

            if let Ok(page) = processing_api.list_jobs(&ctx, query).await {
                target_key = page.items.into_iter().find_map(|j| match j.spec {
                    ProcessingJobSpec::CaptionExtract { target, .. } => Some(target),
                    _ => None,
                });
            }

            if target_key.is_none() {
                let deadline = MonoTime::from_micros(
                    engine.runtime_api.now().as_micros() + DISCOVERY_INTERVAL_US,
                );
                let mut timer = engine.runtime_api.sleep_until(deadline);
                let timer_fut = timer.wait().fuse();
                let cancel_fut = cancel.cancelled().fuse();
                pin_mut!(cancel_fut, timer_fut);
                select_biased! {
                    _ = cancel_fut => break,
                    _ = timer_fut => {}
                }
            }
        }

        let Some(target) = target_key else { break };
        let (ns, path) = StreamKeyBridge::to_namespace_path(&target);
        let target_stream_key = StreamKey::new(&ns, &path);

        // Try to subscribe to the caption target stream.
        let mut subscriber = None;
        for attempt in 0..SUBSCRIBE_RETRY_TOTAL {
            match engine
                .subscriber_api
                .subscribe(
                    target_stream_key.clone(),
                    SubscriberOptions {
                        queue_capacity: 256,
                        bootstrap_policy: BootstrapPolicy::live_tail(150, Some(5_000)),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(s) => {
                    subscriber = Some(s);
                    break;
                }
                Err(SdkError::NotFound(_)) if attempt + 1 < SUBSCRIBE_RETRY_TOTAL => {
                    let deadline = MonoTime::from_micros(
                        engine.runtime_api.now().as_micros() + SUBSCRIBE_RETRY_INTERVAL_MS * 1000,
                    );
                    let mut timer = engine.runtime_api.sleep_until(deadline);
                    let timer_fut = timer.wait().fuse();
                    let cancel_fut = cancel.cancelled().fuse();
                    pin_mut!(cancel_fut, timer_fut);
                    select_biased! {
                        _ = cancel_fut => break,
                        _ = timer_fut => {}
                    }
                }
                Err(e) => {
                    warn!("hls caption subscriber: subscribe to {ns}/{path} failed: {e}");
                    break;
                }
            }
        }

        let Some(mut sub) = subscriber else {
            // Exit if the muxer/stream is no longer alive.
            if muxer.upgrade().is_none() {
                return;
            }

            // Pause before re-discovering in case the target stream name changed.
            let deadline =
                MonoTime::from_micros(engine.runtime_api.now().as_micros() + DISCOVERY_INTERVAL_US);
            let mut timer = engine.runtime_api.sleep_until(deadline);
            let timer_fut = timer.wait().fuse();
            let cancel_fut = cancel.cancelled().fuse();
            pin_mut!(cancel_fut, timer_fut);
            select_biased! {
                _ = cancel_fut => break,
                _ = timer_fut => {}
            }
            continue;
        };

        // Feed cues into the muxer until the caption stream ends or we are cancelled.
        let mut stop_task = false;
        loop {
            let cancel_fut = cancel.cancelled().fuse();
            let recv_fut = sub.recv().fuse();
            pin_mut!(cancel_fut, recv_fut);

            let frame = select_biased! {
                _ = cancel_fut => {
                    stop_task = true;
                    break;
                }
                frame = recv_fut => frame,
            };

            match frame {
                Ok(Some(frame))
                    if frame.media_kind == cheetah_codec::MediaKind::Subtitle
                        && frame.codec == cheetah_codec::CodecId::WebVtt =>
                {
                    match serde_json::from_slice::<WebVttCue>(&frame.payload) {
                        Ok(cue) => {
                            if let Some(m) = muxer.upgrade() {
                                if let Err(e) = m.lock().push_cue(cue) {
                                    warn!("hls caption subscriber: push_cue failed: {e}");
                                }
                            } else {
                                stop_task = true;
                                break;
                            }
                        }
                        Err(e) => warn!("hls caption subscriber: invalid WebVTT cue payload: {e}"),
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }

        let _ = sub.close().await;
        if stop_task || cancel.is_cancelled() {
            return;
        }
    }
}

/// Release pending blocking requests that are now satisfied or timed out.
///
/// Gathers response data under a short lock, then sends the responses outside
/// the lock to avoid blocking the muxer.
///
/// 释放已满足或超时的待处理阻塞请求。
///
/// 在短暂加锁下收集响应数据，然后在锁外发送响应以避免阻塞复用器。
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

/// Release all pending requests across every stream.
///
/// 释放所有流的待处理请求。
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

/// Drain pending requests for a stream whose muxer has disappeared.
///
/// 将复用器已消失的流的待处理请求全部取出。
fn drain_pending_for_missing_muxer(
    pending: &mut StreamPendingRequests,
) -> (Vec<PendingPlaylistRequest>, Vec<PendingPartRequest>) {
    (
        std::mem::take(&mut pending.playlists),
        std::mem::take(&mut pending.parts),
    )
}

/// Compute the next timer deadline for pending request expiration.
///
/// Returns a default 60 s deadline when no pending requests are present.
///
/// 计算待处理请求过期的下一个定时器截止时间。
///
/// 当无待处理请求时返回默认 60 秒截止时间。
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

/// Return the earliest pending request timeout in microseconds.
///
/// 返回最早的待处理请求超时时间（微秒）。
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

/// Classify a part request as ready, pending, or not found.
///
/// Only LL-HLS requests for the next expected sequence are held as pending.
///
/// 将分片请求分类为就绪、挂起或不存在。
///
/// 只有 LL-HLS 对下一个预期序列号的请求会被挂起。
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

/// Built response for a pending blocking playlist request.
///
/// 待处理阻塞播放列表请求的已构建响应。
#[allow(dead_code)]
struct PendingPlaylistResponse {
    connection_id: HlsConnectionId,
    content: String,
    body: bytes::Bytes,
    gzipped: bool,
    headers: Vec<(&'static str, String)>,
}

/// Build the HTTP response for a released pending playlist request.
///
/// Handles demuxed per-track, rewind, and legacy playlist variants.
///
/// 为已释放的待处理播放列表请求构建 HTTP 响应。
///
/// 处理 demuxed 每轨、回退和 legacy 播放列表变体。
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

/// Gzip-compress data, falling back to original bytes on failure.
///
/// 对数据进行 gzip 压缩，失败时回退到原始字节。
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

/// Build the playlist response body, applying gzip if requested.
///
/// 构建播放列表响应体，若请求则应用 gzip。
fn playlist_response_body(content: &str, accept_gzip: bool) -> (bytes::Bytes, bool) {
    if accept_gzip && content.len() > 100 {
        (gzip_bytes(content.as_bytes()), true)
    } else {
        (bytes::Bytes::from(content.to_owned()), false)
    }
}

/// Add `Content-Encoding` and `Vary` headers for gzip responses.
///
/// 为 gzip 响应添加 `Content-Encoding` 和 `Vary` 头。
fn push_gzip_response_headers(headers: &mut Vec<(&'static str, String)>) {
    headers.push(("Content-Encoding", "gzip".to_string()));
    if !headers.iter().any(|(name, _)| *name == "Vary") {
        headers.push(("Vary", "Accept-Encoding".to_string()));
    }
}

/// Build the master playlist for a stream.
///
/// Produces a demuxed master playlist when the muxer is in demuxed mode,
/// otherwise a simple master playlist.
///
/// 构建流的主播放列表。
///
/// 当复用器处于 demuxed 模式时生成 demuxed 主播放列表，否则生成简单主播放列表。
fn build_master_playlist_content(
    stream_key: &StreamKeyParts,
    muxer: Option<&StreamMuxer>,
    variants: &[VariantRenditionInfo],
    session_id: u64,
    include_stream_key: bool,
) -> String {
    let subtitle_info = muxer
        .filter(|m| m.vtt_ready())
        .map(|_| SubtitleRenditionInfo {
            name: "English".to_string(),
            language: "en".to_string(),
            is_default: true,
            autoselect: true,
        });
    if let Some(mux) = muxer {
        if mux.is_demuxed() {
            let sk = if include_stream_key {
                mux.stream_key().to_owned()
            } else {
                String::new()
            };
            let (w, h) = mux.video_dimensions();
            return cheetah_hls_core::DemuxedMasterPlaylist::build_with_subtitles(
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
                subtitle_info.as_ref(),
                &stream_key.stream_path,
                Some(session_id),
                include_stream_key,
                &sk,
            );
        }
    }

    if !variants.is_empty() {
        let subtitle_token = muxer.filter(|_| include_stream_key).map(|m| m.stream_key());
        let subtitle_uri = subtitle_info.as_ref().map(|_| {
            let token = subtitle_token.unwrap_or("");
            let key_suffix = if include_stream_key && !token.is_empty() {
                format!("&k={token}")
            } else {
                String::new()
            };
            format!(
                "{}/subtitle.m3u8?uid={session_id}{key_suffix}",
                stream_key.stream_path
            )
        });
        return PlaylistBuilder::build_master_with_variants(
            variants,
            subtitle_info.as_ref(),
            subtitle_uri.as_deref(),
        );
    }

    let subtitle_token = muxer.filter(|_| include_stream_key).map(|m| m.stream_key());
    PlaylistBuilder::build_master_with_subtitles(
        &stream_key.stream_path,
        session_id,
        subtitle_info.as_ref(),
        subtitle_token,
    )
}

/// Wait briefly for the demuxed muxer to be ready before building a master playlist.
///
/// Avoids returning a master playlist with no available chunklists.
///
/// 在构建主播放列表前短暂等待 demuxed 复用器就绪。
///
/// 避免返回无可用分片列表的主播放列表。
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
///
/// 用于超时计算的当前墙上时间（微秒）。
fn current_time_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Build the engine stream key string from namespace and path.
///
/// 由命名空间和路径构建引擎流标识字符串。
fn stream_key_string(parts: &StreamKeyParts) -> String {
    format!("{}/{}", parts.namespace, parts.stream_path)
}

/// Build the HLS codec string for a track.
///
/// Generates avc1/hvc1/vp09/av01/mp4a/Opus/MP3 identifiers from codec and extradata.
///
/// 构建轨道的 HLS codec 字符串。
///
/// 根据编解码器和 extradata 生成 avc1/hvc1/vp09/av01/mp4a/Opus/MP3 标识。
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

/// Build an HLS video codec string from a processing `VideoTarget`.
///
/// Returns an empty string for codecs that are not usable in HLS (e.g. MJPEG).
fn hls_video_codec_string(video: &VideoTarget) -> String {
    match video.codec {
        VideoCodec::H264 => hls_h264_profile_string(video.profile.as_deref()),
        VideoCodec::H265 => "hvc1.1.6.L93.B0".to_string(),
        VideoCodec::MJPEG => String::new(),
    }
}

/// Pick a conservative H.264 codec string from a textual profile name.
fn hls_h264_profile_string(profile: Option<&str>) -> String {
    match profile {
        Some(p) if p.eq_ignore_ascii_case("baseline") => "avc1.42001e".to_string(),
        Some(p) if p.eq_ignore_ascii_case("main") => "avc1.4d001e".to_string(),
        Some(p) if p.eq_ignore_ascii_case("high") => "avc1.64001f".to_string(),
        _ => "avc1.64001f".to_string(),
    }
}

/// Build an HLS audio codec string from a processing `AudioTarget`.
fn hls_audio_codec_string(audio: &AudioTarget) -> String {
    match audio.codec {
        AudioCodec::Aac => "mp4a.40.2".to_string(),
        AudioCodec::Opus => "Opus".to_string(),
        _ => String::new(),
    }
}

/// Compute the frame rate in Hz from a processing `VideoTarget`.
fn hls_frame_rate(video: &VideoTarget) -> Option<f64> {
    match (video.frame_rate_num, video.frame_rate_den) {
        (Some(n), Some(d)) if d != 0 => Some(f64::from(n) / f64::from(d)),
        _ => None,
    }
}

/// Compute a variant bandwidth in bits per second from the video/audio targets.
fn hls_variant_bandwidth(video: &VideoTarget, audio: Option<&AudioTarget>) -> u64 {
    let video_bw = video
        .bit_rate
        .unwrap_or_else(|| match (video.width, video.height) {
            (Some(w), Some(h)) => default_bit_rate_for_resolution(w, h),
            _ => 1_000_000,
        });
    let audio_bw = audio.and_then(|a| a.bit_rate).unwrap_or(128000);
    video_bw + audio_bw
}

/// Conservative default bit rate for H.264 ABR variants when not configured.
fn default_bit_rate_for_resolution(width: u32, height: u32) -> u64 {
    let pixels = u64::from(width) * u64::from(height);
    if pixels >= 1920 * 1080 {
        4500000
    } else if pixels >= 1280 * 720 {
        2500000
    } else if pixels >= 854 * 480 {
        1200000
    } else if pixels >= 640 * 360 {
        800000
    } else {
        500000
    }
}

/// Build the `MediaRenditionInfo` attributes for an `AbrVariant`.
fn build_variant_rendition_info(variant: &AbrVariant) -> Option<MediaRenditionInfo> {
    let video_codec = hls_video_codec_string(&variant.video);
    if video_codec.is_empty() {
        return None;
    }
    let audio_codec = variant
        .audio
        .as_ref()
        .map(hls_audio_codec_string)
        .filter(|s| !s.is_empty());
    let codecs = match audio_codec {
        Some(ref audio) => format!("{video_codec},{audio}"),
        None => video_codec,
    };
    let bandwidth = hls_variant_bandwidth(&variant.video, variant.audio.as_ref());
    Some(MediaRenditionInfo {
        codecs,
        bandwidth,
        width: variant.video.width,
        height: variant.video.height,
        frame_rate: hls_frame_rate(&variant.video),
        channels: None,
    })
}

/// Build the media-playlist URI for a variant.
///
/// `token` is the variant muxer's `stream_key()` validation token. It is only
/// embedded when `include_stream_key` is true.
fn build_variant_media_uri(
    source: &StreamKeyParts,
    variant_ns: &str,
    variant_path: &str,
    session_id: u64,
    include_stream_key: bool,
    token: Option<&str>,
) -> String {
    let base = if variant_ns == source.namespace {
        format!("{variant_path}/index.m3u8")
    } else {
        format!("../{variant_ns}/{variant_path}/index.m3u8")
    };
    let mut uri = format!("{base}?uid={session_id}");
    if include_stream_key {
        if let Some(t) = token {
            uri.push_str(&format!("&k={t}"));
        }
    }
    uri
}

/// Query `MediaProcessingApi` for running `AbrLadder` jobs whose source is the requested stream.
///
/// If the processing API is unavailable or the source key is invalid, returns an empty list.
async fn collect_abr_variants(
    engine: &EngineContext,
    stream_key: &StreamKeyParts,
) -> Vec<AbrVariant> {
    let processing_api = match engine.media_services.processing() {
        Some(api) => api,
        None => return Vec::new(),
    };
    let source_key = match StreamKeyBridge::from_namespace_path(
        &stream_key.namespace,
        &stream_key.stream_path,
    ) {
        Ok(k) => k,
        Err(e) => {
            warn!("hls master: invalid source stream key {stream_key:?}: {e}");
            return Vec::new();
        }
    };
    let mut query = ProcessingJobQuery {
        vhost: Some(source_key.vhost.0.clone()),
        app: Some(source_key.app.0.clone()),
        stream: Some(source_key.stream.0.clone()),
        state: Some(ProcessingJobState::Running),
        page: 1,
        page_size: 20,
    };
    query.clamp_page_size();

    let ctx = admin_request_context();
    let page = match processing_api.list_jobs(&ctx, query).await {
        Ok(p) => p,
        Err(e) => {
            warn!("hls master: failed to list processing jobs: {e}");
            return Vec::new();
        }
    };

    let mut variants = Vec::new();
    for job in page.items {
        if job.state != ProcessingJobState::Running {
            continue;
        }
        if let ProcessingJobSpec::AbrLadder {
            variants: ref ladder,
            ..
        } = job.spec
        {
            for variant in ladder {
                variants.push(variant.clone());
            }
        }
    }
    variants
}

/// Collect running ABR variants, ensure a `StreamMuxer` exists for each variant output,
/// and build `VariantRenditionInfo` entries using each variant muxer's validation token.
#[allow(clippy::too_many_arguments)]
async fn collect_abr_variant_renditions(
    engine: &EngineContext,
    config: &HlsModuleConfig,
    muxers: &MuxerMap,
    pending: &PendingMap,
    cmd_tx: &HlsCommandSender,
    content_notify_tx: &futures::channel::mpsc::Sender<String>,
    cancel: CancellationToken,
    stream_key: &StreamKeyParts,
    session_id: u64,
) -> Vec<VariantRenditionInfo> {
    let abr_variants = collect_abr_variants(engine, stream_key).await;
    let include_stream_key = config.stream_key_validation;
    let mut renditions = Vec::new();
    for variant in abr_variants {
        let Some(info) = build_variant_rendition_info(&variant) else {
            continue;
        };
        let (variant_ns, variant_path) = StreamKeyBridge::to_namespace_path(&variant.target);
        let variant_key_str = format!("{variant_ns}/{variant_path}");
        let variant_parts = StreamKeyParts {
            namespace: variant_ns.clone(),
            stream_path: variant_path.clone(),
        };
        ensure_muxer(
            engine,
            config,
            muxers,
            pending,
            cmd_tx,
            cancel.child_token(),
            &variant_parts,
            content_notify_tx,
        );
        let token = if include_stream_key {
            muxers
                .lock()
                .get(&variant_key_str)
                .map(|m| m.lock().stream_key().to_owned())
        } else {
            None
        };
        let uri = build_variant_media_uri(
            stream_key,
            &variant_ns,
            &variant_path,
            session_id,
            include_stream_key,
            token.as_deref(),
        );
        renditions.push(VariantRenditionInfo { info, uri });
    }
    renditions
}

/// Build default CORS headers for HLS responses.
///
/// 构建 HLS 响应的默认 CORS 头。
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

/// Build CORS headers with `Cache-Control: no-cache`.
///
/// 构建带 `Cache-Control: no-cache` 的 CORS 头。
fn cors_headers_no_cache() -> Vec<(&'static str, String)> {
    let mut h = cors_headers();
    h.push(("Cache-Control", "no-cache".to_string()));
    h
}

/// Build CORS headers with `Cache-Control` based on the configured max age.
///
/// -1 = no header, 0 = no-cache/no-store, >0 = max-age=N.
///
/// 根据配置的最大缓存时间构建 CORS 头。
///
/// -1 = 不设置头部，0 = no-cache/no-store，>0 = max-age=N。
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

/// Build CORS and cache headers for a segment response.
///
/// 为分段响应构建 CORS 与缓存头。
fn segment_response_headers(segment_name: &str, max_age: i32) -> Vec<(&'static str, String)> {
    let mut headers = cors_headers_with_max_age(max_age);
    headers.push(("ETag", format!("\"{segment_name}\"")));
    headers.push(("Accept-Ranges", "bytes".to_string()));
    headers
}

/// Parse a container string into `HlsContainer`.
///
/// Defaults to TS unless `fmp4`, `fMP4`, or `mp4` is specified.
///
/// 将容器字符串解析为 `HlsContainer`。
///
/// 除非指定 `fmp4`、`fMP4` 或 `mp4`，否则默认 TS。
fn parse_container(s: &str) -> HlsContainer {
    match s {
        "fmp4" | "fMP4" | "mp4" => HlsContainer::Fmp4,
        _ => HlsContainer::Ts,
    }
}

/// Check whether the request has a valid CDN Bearer token.
///
/// 检查请求是否带有有效的 CDN Bearer 令牌。
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

/// Check whether the User-Agent indicates an iOS device.
///
/// 检查 User-Agent 是否表明为 iOS 设备。
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

/// Build CORS headers for CDN requests.
///
/// 为 CDN 请求构建 CORS 头。
fn cors_headers_cdn() -> Vec<(&'static str, String)> {
    cors_headers()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_codec::{
        subtitle::WebVttCue, AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind,
        Timebase, TrackId, TrackInfo,
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
            vtt_config: Some(VttMuxConfig::default()),
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
            vtt_config: Some(VttMuxConfig::default()),
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

        let content = build_master_playlist_content(&stream_key, Some(&muxer), &[], 7, false);

        assert!(content.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
        assert!(content.contains("chunklist_audio.m3u8?uid=7"));
        assert!(content.contains("chunklist_video.m3u8?uid=7"));
        assert!(!content.contains("index.m3u8"));
    }

    #[test]
    fn master_playlist_content_includes_subtitle_rendition_when_vtt_ready() {
        let mut muxer = ready_demuxed_muxer();
        let _ = muxer.push_cue(WebVttCue {
            id: None,
            start_ms: 500,
            end_ms: 2_500,
            payload: "caption".to_string(),
            settings: None,
        });

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            2_000_000,
            2_000_000,
            Timebase::new(1, 1_000_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb]),
        );
        frame.flags |= FrameFlags::KEY;
        muxer.push_frame(&frame);
        muxer.conclude();

        let stream_key = StreamKeyParts {
            namespace: "live".to_string(),
            stream_path: "test".to_string(),
        };

        let content = build_master_playlist_content(&stream_key, Some(&muxer), &[], 7, false);

        assert!(content.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));
        assert!(content.contains("chunklist_subtitles.m3u8?uid=7"));
        assert!(content.contains("SUBTITLES=\"subs\""));
    }

    #[test]
    fn subtitle_media_playlist_and_segment_can_be_served_from_muxer() {
        let mut muxer = ready_muxer();
        let _ = muxer.push_cue(WebVttCue {
            id: None,
            start_ms: 500,
            end_ms: 2_500,
            payload: "caption".to_string(),
            settings: None,
        });

        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            2_000_000,
            2_000_000,
            Timebase::new(1, 1_000_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb]),
        );
        frame.flags |= FrameFlags::KEY;
        muxer.push_frame(&frame);
        muxer.conclude();

        let vtt_mux = muxer.vtt_mux().unwrap();
        let playlist = cheetah_hls_core::PlaylistBuilder::build_vtt_media(vtt_mux, Some(7));
        assert!(playlist.contains("sub0.vtt?uid=7"));

        let segment = muxer.get_vtt_segment("sub0.vtt").expect("sub0.vtt");
        assert!(String::from_utf8_lossy(&segment).contains("WEBVTT"));
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
        let config = HlsModuleConfig {
            origin_mode: true,
            ..Default::default()
        };

        let driver_config = hls_driver_config(&config);

        assert!(!driver_config.set_session_cookie);
    }

    #[test]
    fn variant_rendition_builds_relative_uri_and_codec_string() {
        let target = MediaKey::with_default_vhost("live", "test_480p", None).unwrap();
        let video = VideoTarget {
            codec: VideoCodec::H264,
            width: Some(854),
            height: Some(480),
            frame_rate_num: Some(30),
            frame_rate_den: Some(1),
            bit_rate: Some(1_200_000),
            gop_size: None,
            profile: Some("high".to_string()),
        };
        let audio = AudioTarget {
            codec: AudioCodec::Aac,
            sample_rate: Some(48000),
            channels: Some(2),
            bit_rate: Some(128000),
        };
        let variant = AbrVariant {
            target,
            video,
            audio: Some(audio),
        };
        let source = StreamKeyParts {
            namespace: "live".to_string(),
            stream_path: "test".to_string(),
        };

        let info = build_variant_rendition_info(&variant).expect("rendition");
        let uri = build_variant_media_uri(&source, "live", "test_480p", 5, true, Some("deadbeef"));

        assert!(uri.contains("test_480p/index.m3u8?uid=5&k=deadbeef"));
        assert!(info.codecs.contains("avc1.64001f"));
        assert!(info.codecs.contains("mp4a.40.2"));
        assert_eq!(info.width, Some(854));
        assert_eq!(info.height, Some(480));
        assert!(info.bandwidth >= 1_200_000 + 128_000);
    }

    #[test]
    fn master_playlist_with_abr_variants_lists_each_variant() {
        let target = MediaKey::with_default_vhost("live", "test_480p", None).unwrap();
        let variant = AbrVariant {
            target,
            video: VideoTarget {
                codec: VideoCodec::H264,
                width: Some(854),
                height: Some(480),
                frame_rate_num: Some(30),
                frame_rate_den: Some(1),
                bit_rate: Some(1_200_000),
                gop_size: None,
                profile: Some("high".to_string()),
            },
            audio: Some(AudioTarget {
                codec: AudioCodec::Aac,
                sample_rate: Some(48000),
                channels: Some(2),
                bit_rate: Some(128000),
            }),
        };
        let source = StreamKeyParts {
            namespace: "live".to_string(),
            stream_path: "test".to_string(),
        };
        let info = build_variant_rendition_info(&variant).unwrap();
        let uri = build_variant_media_uri(&source, "live", "test_480p", 7, false, None);
        let variants = vec![VariantRenditionInfo { info, uri }];

        let content = build_master_playlist_content(&source, None, &variants, 7, false);

        assert!(content.contains("#EXT-X-STREAM-INF:BANDWIDTH="));
        assert!(content.contains("test_480p/index.m3u8?uid=7"));
        assert!(content.contains("CODECS=\"avc1.64001f,mp4a.40.2\""));
        assert!(content.contains("RESOLUTION=854x480"));
    }

    #[test]
    fn mjpeg_variant_is_excluded_from_master_playlist() {
        let target = MediaKey::with_default_vhost("live", "test_mjpeg", None).unwrap();
        let variant = AbrVariant {
            target,
            video: VideoTarget {
                codec: VideoCodec::MJPEG,
                width: Some(640),
                height: Some(360),
                frame_rate_num: Some(30),
                frame_rate_den: Some(1),
                bit_rate: Some(500_000),
                gop_size: None,
                profile: None,
            },
            audio: None,
        };

        assert!(build_variant_rendition_info(&variant).is_none());
    }
}
