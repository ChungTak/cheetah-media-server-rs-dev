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

/// `HttpFlvModuleFactory` data structure.
/// `HttpFlvModuleFactory` 数据结构.
pub struct HttpFlvModuleFactory;

impl ModuleFactory for HttpFlvModuleFactory {
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

    fn create(&self) -> Box<dyn Module> {
        Box::new(HttpFlvModule::new())
    }

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

/// `HttpFlvModule` data structure.
/// `HttpFlvModule` 数据结构.
pub struct HttpFlvModule {
    /// `info` field of type `ModuleInfo`.
    /// `info` 字段，类型为 `ModuleInfo`.
    info: ModuleInfo,
    /// `state` field of type `ModuleState`.
    /// `state` 字段，类型为 `ModuleState`.
    state: ModuleState,
    /// `engine` field.
    /// `engine` 字段.
    engine: Option<EngineContext>,
    /// `config` field of type `HttpFlvModuleConfig`.
    /// `config` 字段，类型为 `HttpFlvModuleConfig`.
    config: HttpFlvModuleConfig,
    /// `runtime_cancel` field.
    /// `runtime_cancel` 字段.
    runtime_cancel: Option<CancellationToken>,
    /// `runtime_loops` field.
    /// `runtime_loops` 字段.
    runtime_loops: Vec<OneShotReceiver>,
}

impl HttpFlvModule {
    /// Creates a new instance.
    /// 创建 新的 实例.
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
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = HttpFlvModuleConfig::from_value(ctx.initial_config)?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

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
            endpoint: format!("http-flv://{}", self.config.listen),
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
        let next = HttpFlvModuleConfig::from_value(change.next)?;
        if next == self.config {
            return Ok(ConfigEffect::Immediate);
        }
        self.config = next;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}

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

struct ActivePlaySession {
    cancel: CancellationToken,
    done: OneShotReceiver,
}

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

fn event_connection_id(event: &HttpFlvDriverEvent) -> Option<HttpFlvConnectionId> {
    match event {
        HttpFlvDriverEvent::ConnectionOpened { connection_id, .. }
        | HttpFlvDriverEvent::ConnectionClosed { connection_id, .. }
        | HttpFlvDriverEvent::Core { connection_id, .. } => Some(*connection_id),
    }
}

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

    let queue_capacity = config
        .subscriber_queue_capacity
        .max(config.bootstrap_max_frames.max(1));
    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            stream_key.clone(),
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
            warn!(%stream_key, %connection_id, %err, "http-flv subscribe failed");
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
                        engine.stream_manager_api.get_stream(&stream_key).await
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
                        engine.stream_manager_api.get_stream(&stream_key).await
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
                warn!(%stream_key, %connection_id, %err, "http-flv subscriber recv failed");
                break;
            }
        }
    }

    let _ = subscriber.close().await;
    let _ = command_tx.close_connection(connection_id).await;
}

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
