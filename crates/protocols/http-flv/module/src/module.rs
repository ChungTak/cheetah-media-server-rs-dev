use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::{
    build_track_bootstrap_payloads, map_frame_to_rtmp_flv_payload, track_list_has_audio, FlvHeader,
    FlvTag, FlvTagType, MediaKind, RtmpFlvPayloadKind, RtmpFlvPlayMode, TrackInfo,
};
use cheetah_http_flv_core::StreamKeyParts;
use cheetah_http_flv_driver_tokio::{
    start_server, HttpFlvConnectionId, HttpFlvCoreCommandSender, HttpFlvDriverConfig,
    HttpFlvDriverEvent, HttpFlvEvent, HttpFlvServerHandle,
};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability,
    ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, OneShotReceiver, RuntimeApi, SdkError,
    ServiceDescriptor, StreamKey, StreamSnapshot, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::warn;

use crate::config::HttpFlvModuleConfig;
use crate::pull::{run_pull_job_supervisor, PullReadLimits};

const MODULE_ID: &str = "http-flv";

/// Factory that builds `HttpFlvModule` instances and advertises the module
/// manifest to the engine.
///
/// 构建 `HttpFlvModule` 实例并向引擎注册模块 manifest 的工厂。
pub struct HttpFlvModuleFactory;

impl ModuleFactory for HttpFlvModuleFactory {
    /// Return the module manifest: ID, display name, HTTP route prefix, and
    /// capabilities (subscribe, publish, background job).
    ///
    /// 返回模块 manifest：ID、显示名称、HTTP 路由前缀以及能力
    ///（订阅、发布、后台任务）。
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "HTTP-FLV Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "http_flv".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![
                ModuleCapability::Subscribe,
                ModuleCapability::Publish,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    /// Create a fresh `HttpFlvModule` instance.
    ///
    /// 创建一个全新的 `HttpFlvModule` 实例。
    fn create(&self) -> Box<dyn Module> {
        Box::new(HttpFlvModule::new())
    }

    /// Register the JSON schema for `http_flv` configuration and a validator
    /// that converts JSON into `HttpFlvModuleConfig` and checks it.
    ///
    /// 注册 `http_flv` 配置的 JSON schema，并提供校验器将 JSON 转换为
    /// `HttpFlvModuleConfig` 并校验。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "http-flv-module".to_string(),
            default_value: HttpFlvModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                HttpFlvModuleConfig::from_value(value.clone())
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })),
        })
    }
}

/// HTTP-FLV module instance.
///
/// Owns the parsed configuration, the `EngineContext` handle, the cancellation
/// token for spawned tasks, and the one-shot join handles for the server event
/// loop and pull supervisors.
///
/// HTTP-FLV 模块实例。
///
/// 持有解析后的配置、`EngineContext` 句柄、已生成任务的取消令牌，
/// 以及服务器事件循环和拉流监管器的一次性完成句柄。
pub struct HttpFlvModule {
    info: ModuleInfo,
    state: ModuleState,
    engine: Option<EngineContext>,
    config: HttpFlvModuleConfig,
    runtime_cancel: Option<CancellationToken>,
    runtime_loops: Vec<OneShotReceiver>,
}

impl HttpFlvModule {
    /// Create an HTTP-FLV module in the `Created` state.
    ///
    /// 创建一个处于 `Created` 状态的 HTTP-FLV 模块。
    pub fn new() -> Self {
        Self {
            info: ModuleInfo {
                module_id: ModuleId::new(MODULE_ID),
                display_name: "HTTP-FLV Module".to_string(),
                state: ModuleState::Created,
            },
            state: ModuleState::Created,
            engine: None,
            config: HttpFlvModuleConfig::default(),
            runtime_cancel: None,
            runtime_loops: Vec::new(),
        }
    }
}

impl Default for HttpFlvModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for HttpFlvModule {
    /// Return a clone of the module's runtime info.
    ///
    /// 返回模块运行时信息的副本。
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    /// Return the current module lifecycle state.
    ///
    /// 返回模块当前生命周期状态。
    fn state(&self) -> ModuleState {
        self.state
    }

    /// Parse the initial JSON configuration and capture the engine context.
    ///
    /// 解析初始 JSON 配置并保存引擎上下文。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = HttpFlvModuleConfig::from_value(ctx.initial_config)?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    /// Start the HTTP-FLV driver and pull supervisors.
    ///
    /// If disabled, the module transitions to `Running` without binding any
    /// sockets. Otherwise the driver is started on the configured `listen`
    /// address, the service is registered, and the server event loop and all
    /// enabled pull jobs are spawned as background tasks.
    ///
    /// 启动 HTTP-FLV 驱动和拉流监管器。
    ///
    /// 如果禁用，模块直接进入 `Running` 状态而不绑定任何 socket；否则在
    /// 配置的 `listen` 地址上启动驱动，注册服务，并将服务器事件循环和所有
    /// 已启用的拉流任务作为后台任务生成。
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        let Some(engine) = self.engine.clone() else {
            return Err(SdkError::Unavailable(
                "http-flv module is not initialized".to_string(),
            ));
        };

        if !self.config.enabled {
            self.runtime_cancel = Some(cancel);
            self.state = ModuleState::Running;
            return Ok(());
        }

        let listen: SocketAddr =
            self.config.listen.parse().map_err(|err| {
                SdkError::InvalidArgument(format!("invalid http_flv.listen: {err}"))
            })?;

        let server_cancel = cancel.child_token();
        let driver = start_server(
            engine.runtime_api.clone(),
            listen,
            HttpFlvDriverConfig {
                write_queue_capacity: self.config.write_queue_capacity,
                read_buffer_size: self.config.read_buffer_size,
                ..HttpFlvDriverConfig::default()
            },
            server_cancel.clone(),
        )
        .map_err(|err| SdkError::Internal(format!("start http-flv driver failed: {err}")))?;

        if let Err(err) = engine.service_registry.register(ServiceDescriptor {
            name: MODULE_ID.to_string(),
            endpoint: format!("http-flv://{}", driver.local_addr()),
            metadata: Default::default(),
        }) {
            let failed_driver = driver;
            failed_driver.shutdown();
            let _ = failed_driver.wait().await;
            return Err(SdkError::Internal(format!(
                "register http-flv service failed: {err}"
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
        runtime_loops.extend(spawn_pull_job_loops(
            engine.runtime_api.clone(),
            self.config.clone(),
            server_cancel.clone(),
        ));

        self.runtime_cancel = Some(server_cancel);
        self.runtime_loops = runtime_loops;
        self.state = ModuleState::Running;
        Ok(())
    }

    /// Stop the module by cancelling runtime tasks and waiting for them.
    ///
    /// 取消运行时任务并等待其结束，然后注销服务。
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

    /// Apply a new configuration.
    ///
    /// If the configuration is unchanged the effect is `Immediate`; otherwise
    /// the module stores the new config and signals `ModuleRestartRequired` so
    /// the engine rebuilds the module.
    ///
    /// 应用新配置。
    ///
    /// 如果配置未变，则效果为 `Immediate`；否则保存新配置并返回
    /// `ModuleRestartRequired`，让引擎重建模块。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = HttpFlvModuleConfig::from_value(change.next)?;
        if next == self.config {
            return Ok(ConfigEffect::Immediate);
        }
        self.config = next;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}

/// Spawn a background pull supervisor for every enabled pull job.
///
/// Each job runs its own retry loop with a dedicated child cancellation token.
///
/// 为每个已启用的拉流任务生成后台拉流监管器。
///
/// 每个任务都使用独立的子取消令牌运行自己的重试循环。
fn spawn_pull_job_loops(
    runtime_api: Arc<dyn RuntimeApi>,
    config: HttpFlvModuleConfig,
    cancel: CancellationToken,
) -> Vec<OneShotReceiver> {
    let limits = PullReadLimits {
        read_buffer_size: config.read_buffer_size,
        ..PullReadLimits::default()
    };
    let mut loops = Vec::new();
    for job in config.pull_jobs.iter().filter(|job| job.enabled).cloned() {
        let loop_cancel = cancel.child_token();
        loops.push(spawn_runtime_task(
            runtime_api.clone(),
            run_pull_job_supervisor(runtime_api.clone(), job, loop_cancel, limits),
        ));
    }
    loops
}

/// Spawn a runtime task and return a one-shot receiver that completes when it
/// finishes.
///
/// 生成一个运行时任务，并返回一个在其完成时触发的一次性接收器。
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

/// A play session that is currently running for a connection.
///
/// Holds the cancellation token and the one-shot join handle for the session
/// task so the server loop can cleanly shut it down.
///
/// 为某个连接正在运行的播放会话。
///
/// 保存会话任务的取消令牌和一次性完成句柄，以便服务器循环可以干净地关闭它。
struct ActivePlaySession {
    cancel: CancellationToken,
    done: OneShotReceiver,
}

/// Main server loop: dispatch driver events and manage play sessions.
///
/// The loop waits for `HttpFlvDriverEvent`s. On `PlayRequested` it spawns a
/// `run_play_session` task for the connection. On `ConnectionClosed` or
/// `PeerClosed` it cancels the matching session. When `cancel` fires, the
/// driver is shut down and the loop drains all active sessions before exiting.
///
/// 主服务器循环：分发驱动事件并管理播放会话。
///
/// 循环等待 `HttpFlvDriverEvent`。收到 `PlayRequested` 时，为对应连接生成
/// `run_play_session` 任务；收到 `ConnectionClosed` 或 `PeerClosed` 时取消对应会话。
/// 当 `cancel` 触发时，关闭驱动并排空所有活动会话后退出。
async fn run_server_loop(
    engine: EngineContext,
    config: HttpFlvModuleConfig,
    mut driver: HttpFlvServerHandle,
    cancel: CancellationToken,
) {
    let mut sessions = HashMap::<HttpFlvConnectionId, ActivePlaySession>::new();
    let command_tx = driver.core_command_sender();
    let mut shutdown_requested = false;
    loop {
        if shutdown_requested {
            let Some(event) = driver.recv_event().await else {
                break;
            };
            if let Some(connection_id) = event_connection_id(&event) {
                if let Some(session) = sessions.remove(&connection_id) {
                    session.cancel.cancel();
                    let mut done = session.done;
                    let _ = done.recv().await;
                }
            }
            continue;
        }

        let mut next_event = None;
        let should_shutdown = {
            let mut flag = false;
            let cancel_fut = cancel.cancelled().fuse();
            let event_fut = driver.recv_event().fuse();
            pin_mut!(cancel_fut, event_fut);
            select_biased! {
                _ = cancel_fut => {
                    flag = true;
                }
                maybe_event = event_fut => {
                    next_event = maybe_event;
                }
            }
            flag
        };

        if should_shutdown {
            driver.shutdown();
            shutdown_requested = true;
            for (_, session) in sessions.drain() {
                session.cancel.cancel();
                let mut done = session.done;
                let _ = done.recv().await;
            }
            continue;
        }

        let Some(event) = next_event else {
            break;
        };
        if let Some(connection_id) = event_connection_id(&event) {
            if matches!(&event, HttpFlvDriverEvent::ConnectionClosed { .. }) {
                if let Some(session) = sessions.remove(&connection_id) {
                    session.cancel.cancel();
                    let mut done = session.done;
                    let _ = done.recv().await;
                }
                continue;
            }
        }

        if let HttpFlvDriverEvent::Core {
            connection_id,
            event:
                HttpFlvEvent::PlayRequested {
                    stream_key,
                    play_mode,
                    ..
                },
        } = event
        {
            if let Some(previous) = sessions.remove(&connection_id) {
                previous.cancel.cancel();
            }
            let session_cancel = cancel.child_token();
            let done = spawn_runtime_task(
                engine.runtime_api.clone(),
                run_play_session(
                    engine.clone(),
                    config.clone(),
                    command_tx.clone(),
                    connection_id,
                    stream_key,
                    play_mode,
                    session_cancel.clone(),
                ),
            );
            sessions.insert(
                connection_id,
                ActivePlaySession {
                    cancel: session_cancel,
                    done,
                },
            );
        } else if let HttpFlvDriverEvent::Core {
            connection_id,
            event: HttpFlvEvent::PeerClosed,
        } = event
        {
            if let Some(session) = sessions.remove(&connection_id) {
                session.cancel.cancel();
            }
        } else if let HttpFlvDriverEvent::Core {
            event: HttpFlvEvent::PublishRequested { .. } | HttpFlvEvent::PullTag(_),
            ..
        } = event
        {
            // HTTP-FLV POST publish and pull tag processing not yet integrated.
            // Events are acknowledged but not acted upon.
        }
    }

    for (_, session) in sessions.drain() {
        session.cancel.cancel();
        let mut done = session.done;
        let _ = done.recv().await;
    }
    let _ = driver.wait().await;
}

/// Extract the connection ID carried by any `HttpFlvDriverEvent` variant.
///
/// 从任意 `HttpFlvDriverEvent` 变体中提取连接 ID。
fn event_connection_id(event: &HttpFlvDriverEvent) -> Option<HttpFlvConnectionId> {
    match event {
        HttpFlvDriverEvent::ConnectionOpened { connection_id, .. }
        | HttpFlvDriverEvent::ConnectionClosed { connection_id, .. }
        | HttpFlvDriverEvent::Core { connection_id, .. } => Some(*connection_id),
    }
}

/// Run a single HTTP-FLV play session.
///
/// Waits for the source stream to appear, subscribes to it, sends the FLV
/// header and bootstrap payloads, then loops forwarding frames to the driver
/// command sender. On a key frame it checks whether the track list has changed
/// and re-issues bootstrap payloads if needed. If a frame cannot be mapped, it
/// refreshes the stream snapshot and tries once more.
///
/// 运行单个 HTTP-FLV 播放会话。
///
/// 等待源流出现、订阅它、发送 FLV 头部和启动负载，然后循环将帧转发给驱动命令发送器。
/// 在关键帧时检查轨道列表是否变化，需要时重新发送启动负载。如果帧无法映射，
/// 则刷新流快照并重试一次。
async fn run_play_session(
    engine: EngineContext,
    config: HttpFlvModuleConfig,
    command_tx: HttpFlvCoreCommandSender,
    connection_id: HttpFlvConnectionId,
    stream_key: StreamKeyParts,
    play_mode: RtmpFlvPlayMode,
    cancel: CancellationToken,
) {
    let stream_key = StreamKey::new(stream_key.namespace, stream_key.stream_path);
    let wait_timeout = if config.play_wait_source_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(config.play_wait_source_timeout_ms))
    };

    let Some(mut snapshot) =
        wait_for_stream_snapshot(&engine, &stream_key, &cancel, wait_timeout).await
    else {
        let _ = command_tx.close_connection(connection_id).await;
        return;
    };

    // Auto-derive H.264/AAC when source is not already FLV-playable; share job with RTMP play.
    let mut play_stream_key = stream_key.clone();
    let mut processing_job_id = None;
    match crate::processing::ensure_derived_play_source(&engine, stream_key.clone(), &cancel).await
    {
        Ok(derived) => {
            play_stream_key = derived.stream_key;
            processing_job_id = derived.processing_job_id;
            if processing_job_id.is_some() {
                if let Ok(Some(derived_snapshot)) =
                    engine.stream_manager_api.get_stream(&play_stream_key).await
                {
                    if !derived_snapshot.tracks.is_empty() {
                        snapshot = derived_snapshot;
                    }
                }
            }
        }
        Err(err) => {
            warn!(%stream_key, %connection_id, %err, "http-flv derived play source failed; using source");
        }
    }

    let queue_capacity = config
        .subscriber_queue_capacity
        .max(config.bootstrap_max_frames.max(1));
    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            play_stream_key.clone(),
            SubscriberOptions {
                queue_capacity,
                backpressure: config.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::live_tail(config.bootstrap_max_frames, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            if let Some(job_id) = processing_job_id.take() {
                crate::processing::stop_derived_job(&engine, job_id).await;
            }
            warn!(%play_stream_key, %connection_id, %err, "http-flv subscribe failed");
            let _ = command_tx.close_connection(connection_id).await;
            return;
        }
    };

    if send_play_header_and_bootstrap(
        &command_tx,
        connection_id,
        &snapshot.tracks,
        play_mode,
        &config,
    )
    .await
    .is_err()
    {
        let _ = subscriber.close().await;
        if let Some(job_id) = processing_job_id.take() {
            crate::processing::stop_derived_job(&engine, job_id).await;
        }
        return;
    }

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        let next = select_biased! {
            _ = cancel_fut => break,
            recv = recv_fut => recv,
        };

        match next {
            Ok(Some(frame)) => {
                if frame.is_key_frame() {
                    if let Ok(Some(next_snapshot)) =
                        engine.stream_manager_api.get_stream(&play_stream_key).await
                    {
                        if next_snapshot.tracks != snapshot.tracks {
                            snapshot = next_snapshot;
                            if send_bootstrap_payloads(
                                &command_tx,
                                connection_id,
                                &snapshot.tracks,
                                play_mode,
                                &config,
                            )
                            .await
                            .is_err()
                            {
                                break;
                            }
                        }
                    }
                }

                let mut payload =
                    map_frame_to_rtmp_flv_payload(frame.as_ref(), play_mode, &snapshot.tracks);
                if payload.is_none() {
                    if let Ok(Some(next_snapshot)) =
                        engine.stream_manager_api.get_stream(&play_stream_key).await
                    {
                        snapshot = next_snapshot;
                        payload = map_frame_to_rtmp_flv_payload(
                            frame.as_ref(),
                            play_mode,
                            &snapshot.tracks,
                        );
                    }
                }
                let Some(payload) = payload else {
                    continue;
                };
                let tag_type = match payload.kind {
                    RtmpFlvPayloadKind::Audio => FlvTagType::Audio,
                    RtmpFlvPayloadKind::Video => FlvTagType::Video,
                    RtmpFlvPayloadKind::Data => FlvTagType::Script,
                };
                let tag = FlvTag {
                    tag_type,
                    timestamp_ms: payload.timestamp_ms,
                    payload: payload.payload,
                };
                if command_tx
                    .send_flv_bytes(connection_id, tag.encode_with_previous_tag_size())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                warn!(%play_stream_key, %connection_id, %err, "http-flv subscriber recv failed");
                break;
            }
        }
    }

    let _ = subscriber.close().await;
    if let Some(job_id) = processing_job_id.take() {
        crate::processing::stop_derived_job(&engine, job_id).await;
    }
    let _ = command_tx.close_connection(connection_id).await;
}

/// Send the FLV header and bootstrap payloads for the current track list.
///
/// 发送当前轨道列表的 FLV 头部与启动负载。
async fn send_play_header_and_bootstrap(
    command_tx: &HttpFlvCoreCommandSender,
    connection_id: HttpFlvConnectionId,
    tracks: &[TrackInfo],
    play_mode: RtmpFlvPlayMode,
    config: &HttpFlvModuleConfig,
) -> Result<(), ()> {
    let flv_header = FlvHeader {
        has_audio: track_list_has_audio(tracks),
        has_video: tracks
            .iter()
            .any(|track| track.media_kind == MediaKind::Video),
    }
    .encode();
    command_tx
        .send_flv_bytes(connection_id, flv_header)
        .await
        .map_err(|_| ())?;
    send_bootstrap_payloads(command_tx, connection_id, tracks, play_mode, config).await
}

/// Build and send the bootstrap payloads needed for a player to start.
///
/// 生成并发送播放器启动所需的启动负载。
async fn send_bootstrap_payloads(
    command_tx: &HttpFlvCoreCommandSender,
    connection_id: HttpFlvConnectionId,
    tracks: &[TrackInfo],
    play_mode: RtmpFlvPlayMode,
    config: &HttpFlvModuleConfig,
) -> Result<(), ()> {
    let payloads = build_track_bootstrap_payloads(
        tracks,
        play_mode,
        config.enable_add_mute,
        config.emit_play_metadata,
    );
    for payload in payloads {
        let tag_type = match payload.kind {
            RtmpFlvPayloadKind::Audio => FlvTagType::Audio,
            RtmpFlvPayloadKind::Video => FlvTagType::Video,
            RtmpFlvPayloadKind::Data => FlvTagType::Script,
        };
        let tag = FlvTag {
            tag_type,
            timestamp_ms: payload.timestamp_ms,
            payload: payload.payload,
        };
        command_tx
            .send_flv_bytes(connection_id, tag.encode_with_previous_tag_size())
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// Wait until the stream exists or the timeout/cancellation fires.
///
/// Polls `stream_manager_api.get_stream` every 100 ms, checking `cancel` and
/// the elapsed time against `timeout`. Returns `None` on cancellation or
/// timeout.
///
/// 等待流出现，直到超时或取消触发。
///
/// 每 100 毫秒轮询 `stream_manager_api.get_stream`，检查 `cancel` 和已用时间是否超过
/// `timeout`。取消或超时时返回 `None`。
async fn wait_for_stream_snapshot(
    engine: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
    timeout: Option<Duration>,
) -> Option<StreamSnapshot> {
    let start_micros = engine.runtime_api.now().as_micros();
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        if let Ok(Some(snapshot)) = engine.stream_manager_api.get_stream(stream_key).await {
            return Some(snapshot);
        }

        if let Some(timeout) = timeout {
            let elapsed_micros = engine
                .runtime_api
                .now()
                .as_micros()
                .saturating_sub(start_micros);
            let timeout_micros = u64::try_from(timeout.as_micros()).unwrap_or(u64::MAX);
            if elapsed_micros >= timeout_micros {
                return None;
            }
        }
        if sleep_or_cancel(
            engine.runtime_api.as_ref(),
            cancel,
            Duration::from_millis(100),
        )
        .await
        {
            return None;
        }
    }
}

/// Sleep for `duration` but return early if `cancel` is triggered.
///
/// Returns `true` when cancelled, `false` when the timer completes.
///
/// 睡眠 `duration`，但如果 `cancel` 被触发则提前返回。
///
/// 取消时返回 `true`，计时器完成时返回 `false`。
async fn sleep_or_cancel(
    runtime_api: &dyn RuntimeApi,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let now = runtime_api.now().as_micros();
    let delta = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
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
