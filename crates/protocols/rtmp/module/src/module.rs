use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::{AVFrame, CodecExtradata, CodecId, FrameFlags, MediaKind, MonoTime};
use cheetah_rtmp_core::{RtmpClientState, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, start_server, start_tls_client, start_tls_server, ClientDriverEvent,
    ClientSendError, DriverConfig, DriverEvent, RtmpClientCommandSender, RtmpClientDriverConfig,
    RtmpClientMode, RtmpConnectionId, RtmpCoreCommand, RtmpCoreCommandSender, RtmpEvent,
    RtmpMediaType, RtmpServerHandle, RtmpTlsClientConfig, RtmpTlsConfig,
};
use cheetah_sdk::media_api::ids::MediaSchema;
use cheetah_sdk::media_api::output::MediaOutputEndpoint;
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability,
    ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, OneShotReceiver, PublisherOptions, RuntimeApi, SdkError,
    ServiceDescriptor, StreamKey, SubscriberOptions, TrackSelection,
};
use cheetah_sdk::{HttpMethod, HttpRequest, HttpResponse, HttpRouteDescriptor, ModuleHttpService};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;

use crate::config::{RtmpModuleConfig, RtmpPullJobConfig, RtmpPushJobConfig, RtmpRelayJobConfig};
use crate::egress::{
    build_track_bootstrap_commands, frame_dts_to_rtmp_timestamp_ms, map_frame_to_rtmp_with_tracks,
    maybe_make_mute_audio, play_accept_flags, rtmp_playback_codec_supported, send_track_bootstrap,
    should_delay_publish_release_for_h264, should_force_close_play_on_source_end,
    track_list_has_audio, track_list_has_codec, track_list_has_supported_playback_codec,
    track_list_has_video,
};
use crate::ingest::{
    apply_metadata_to_tracks, handle_audio_ingest_with_alert_threshold, handle_data_ingest,
    handle_video_ingest_with_alert_threshold, should_emit_alert_threshold,
};
use crate::route::{parse_stream_key_spec, parse_stream_route, RtmpPlayMode, StreamRoute};
use crate::session::{
    with_publish_session, FrameRateEstimator, KeepaliveSession, PlaySession, PublishSession,
    PublishTimestampStates, PublishTracks,
};

#[cfg(test)]
use crate::egress::{
    build_h266_config, build_video_config_payload, map_non_h264_video, use_enhanced_video_mode,
};
#[cfg(test)]
use crate::ingest::{
    annexb_to_length_prefixed, annexb_to_length_prefixed_with_size, apply_video_config,
    attach_raw_rtmp_video_payload, handle_audio_ingest, handle_video_ingest,
    length_prefixed_to_annexb, length_prefixed_to_annexb_with_size, parse_avcc_parameter_sets,
    parse_hvcc_parameter_sets,
};
#[cfg(test)]
use cheetah_rtmp_core::parse_video_ingress_header;

#[cfg(test)]
use bytes::Bytes;
#[cfg(test)]
use cheetah_codec::{
    rtmp_fourcc_from_codec, FrameFormat, RtmpTimestamp, SourceTimestamp, Timebase, TrackId,
    TrackInfo,
};
#[cfg(test)]
use cheetah_rtmp_core::{Amf0Value as WireAmf0Value, AmfValue};

const MODULE_ID: &str = "rtmp";
const H264_RELEASE_GRACE_MS: u64 = 800;
const RTMP_EGRESS_BACKWARD_REPAIR_THRESHOLD_MS: u32 = 3_000;
const RTMP_PLAY_PACING_MAX_FORWARD_DELTA_MS: u32 = 30_000;

/// Factory that creates RTMP module instances and registers them with the engine.
///
/// 创建 RTMP 模块实例并向引擎注册的工厂。
pub struct RtmpModuleFactory;

/// `ModuleFactory` implementation exposing the RTMP manifest, factory, and config schema.
///
/// `ModuleFactory` 实现，暴露 RTMP manifest、工厂与配置 schema。
impl ModuleFactory for RtmpModuleFactory {
    /// Returns the module manifest: id, display name, and capabilities.
    ///
    /// 返回模块 manifest：id、显示名称与能力。
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "RTMP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "rtmp".to_string(),
            routes_prefix: "/rtmp".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::BackgroundJob,
                ModuleCapability::HttpApi,
            ],
        }
    }

    /// Creates a new `RtmpModule` instance.
    ///
    /// 创建新的 `RtmpModule` 实例。
    fn create(&self) -> Box<dyn Module> {
        Box::new(RtmpModule::new())
    }

    /// Returns the JSON schema and validator for the RTMP module config.
    ///
    /// 返回 RTMP 模块配置的 JSON schema 与校验器。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "rtmp-module".to_string(),
            default_value: RtmpModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                RtmpModuleConfig::from_value(value.clone())
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })),
        })
    }
}

/// RTMP module runtime state and lifecycle holder.
///
/// Tracks the module state, engine context, config, and active runtime loops.
///
/// RTMP 模块运行时状态与生命周期持有器。
///
/// 跟踪模块状态、引擎上下文、配置与活跃运行时循环。
pub struct RtmpModule {
    info: ModuleInfo,
    state: ModuleState,
    engine: Option<EngineContext>,
    config: RtmpModuleConfig,
    runtime_cancel: Option<CancellationToken>,
    runtime_loops: Vec<OneShotReceiver>,
    output_endpoint_ids: Vec<String>,
}

impl RtmpModule {
    /// Creates a new module in the `Created` state.
    ///
    /// 创建处于 `Created` 状态的新模块。
    pub fn new() -> Self {
        Self {
            info: ModuleInfo {
                module_id: ModuleId::new(MODULE_ID),
                display_name: "RTMP Module".to_string(),
                state: ModuleState::Created,
            },
            state: ModuleState::Created,
            engine: None,
            config: RtmpModuleConfig::default(),
            runtime_cancel: None,
            runtime_loops: Vec::new(),
            output_endpoint_ids: Vec::new(),
        }
    }
}

impl Default for RtmpModule {
    /// Returns a default module instance.
    ///
    /// 返回默认模块实例。
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for RtmpModule {
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = RtmpModuleConfig::from_value(ctx.initial_config)?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        let Some(engine) = self.engine.clone() else {
            return Err(SdkError::Unavailable(
                "rtmp module is not initialized".to_string(),
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
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtmp.listen: {err}")))?;

        let server_cancel = cancel.child_token();
        let driver = start_server(
            engine.runtime_api.clone(),
            listen,
            DriverConfig {
                write_queue_capacity: self.config.write_queue_capacity,
                ..DriverConfig::default()
            },
            server_cancel.clone(),
        )
        .map_err(|err| SdkError::Internal(format!("start rtmp driver failed: {err}")))?;

        if let Err(err) = engine.service_registry.register(ServiceDescriptor {
            name: MODULE_ID.to_string(),
            endpoint: format!("rtmp://{}", driver.local_addr()),
            metadata: Default::default(),
        }) {
            driver.shutdown();
            let _ = driver.wait().await;
            return Err(SdkError::Internal(format!(
                "register rtmp service failed: {err}"
            )));
        }

        if let Some(registry) = engine.media_services.output_registry() {
            let local = driver.local_addr();
            let host = if local.ip().is_unspecified() {
                "127.0.0.1".to_string()
            } else {
                local.ip().to_string()
            };
            let endpoint = MediaOutputEndpoint::new(
                MODULE_ID,
                MediaSchema::Rtmp,
                host,
                local.port(),
                false,
                "{app}/{stream}",
            );
            match registry.register_endpoint(endpoint).await {
                Ok(id) => self.output_endpoint_ids.push(id),
                Err(err) => {
                    driver.shutdown();
                    let _ = driver.wait().await;
                    return Err(SdkError::Internal(format!(
                        "register rtmp output endpoint failed: {err}"
                    )));
                }
            }
        }

        // Start RTMPS (TLS) server if configured
        let tls_driver = if self.config.tls.enabled {
            let tls_listen: SocketAddr = self.config.tls.listen.parse().map_err(|err| {
                SdkError::InvalidArgument(format!("invalid rtmp.tls.listen: {err}"))
            })?;
            let tls_config = RtmpTlsConfig::from_pem_files(
                std::path::Path::new(&self.config.tls.cert_path),
                std::path::Path::new(&self.config.tls.key_path),
            )
            .map_err(|err| SdkError::Internal(format!("load rtmps tls config: {err}")))?;
            let tls_timeout = Duration::from_millis(self.config.tls.handshake_timeout_ms.max(1000));
            let tls_handle = start_tls_server(
                engine.runtime_api.clone(),
                tls_listen,
                DriverConfig {
                    write_queue_capacity: self.config.write_queue_capacity,
                    ..DriverConfig::default()
                },
                tls_config,
                tls_timeout,
                server_cancel.clone(),
            )
            .map_err(|err| SdkError::Internal(format!("start rtmps driver failed: {err}")))?;

            let _ = engine.service_registry.register(ServiceDescriptor {
                name: format!("{MODULE_ID}-tls"),
                endpoint: format!("rtmps://{}", tls_handle.local_addr()),
                metadata: Default::default(),
            });

            if let Some(registry) = engine.media_services.output_registry() {
                let local = tls_handle.local_addr();
                let host = if local.ip().is_unspecified() {
                    "127.0.0.1".to_string()
                } else {
                    local.ip().to_string()
                };
                let endpoint = MediaOutputEndpoint::new(
                    "rtmp-tls",
                    MediaSchema::Rtmp,
                    host,
                    local.port(),
                    true,
                    "{app}/{stream}",
                );
                if let Ok(id) = registry.register_endpoint(endpoint).await {
                    self.output_endpoint_ids.push(id);
                }
            }

            Some(tls_handle)
        } else {
            None
        };

        let event_task = spawn_runtime_task(
            engine.runtime_api.clone(),
            run_event_loop(
                engine.clone(),
                self.config.clone(),
                driver,
                server_cancel.clone(),
            ),
        );
        let mut runtime_loops = vec![event_task];

        if let Some(tls_handle) = tls_driver {
            let tls_event_task = spawn_runtime_task(
                engine.runtime_api.clone(),
                run_event_loop(
                    engine.clone(),
                    self.config.clone(),
                    tls_handle,
                    server_cancel.clone(),
                ),
            );
            runtime_loops.push(tls_event_task);
        }

        runtime_loops.extend(spawn_static_job_loops(
            engine,
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
        // Drop join handles — tasks will finish asynchronously after cancellation.
        self.runtime_loops.clear();
        if let Some(engine) = self.engine.as_ref() {
            if let Some(registry) = engine.media_services.output_registry() {
                for id in self.output_endpoint_ids.drain(..) {
                    let _ = registry.unregister_endpoint(&id).await;
                }
            } else {
                self.output_endpoint_ids.clear();
            }
            let _ = engine.service_registry.unregister(MODULE_ID);
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = RtmpModuleConfig::from_value(change.next)?;
        if next == self.config {
            return Ok(ConfigEffect::Immediate);
        }
        self.config = next;
        Ok(ConfigEffect::ModuleRestartRequired)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/streams".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/stats".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        let engine = self.engine.clone()?;
        Some(Arc::new(RtmpHttpService { engine }))
    }
}

/// HTTP service handler for the RTMP module's REST endpoints.
///
/// RTMP 模块 REST 端点的 HTTP 服务处理器。
struct RtmpHttpService {
    engine: EngineContext,
}

/// Checks whether a publish request is authorized by the configured token.
///
/// 检查发布请求是否被配置的 token 授权。
fn check_publish_auth(config: &RtmpModuleConfig, stream_name: &str) -> bool {
    if !config.auth.enabled || config.auth.publish_token.is_empty() {
        return true;
    }
    let token = crate::route::extract_token_from_stream_name(stream_name);
    token == Some(config.auth.publish_token.as_str())
}

/// Checks whether a play request is authorized by the configured token.
///
/// 检查播放请求是否被配置的 token 授权。
fn check_play_auth(config: &RtmpModuleConfig, stream_name: &str) -> bool {
    if !config.auth.enabled || config.auth.play_token.is_empty() {
        return true;
    }
    let token = crate::route::extract_token_from_stream_name(stream_name);
    token == Some(config.auth.play_token.as_str())
}

#[async_trait]
impl ModuleHttpService for RtmpHttpService {
    /// Handles `/streams` and `/stats` HTTP requests by querying the stream manager.
    ///
    /// 通过查询流管理器处理 `/streams` 与 `/stats` HTTP 请求。
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/streams") => {
                let streams = self.engine.stream_manager_api.list_streams().await?;
                let list: Vec<serde_json::Value> = streams
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "stream_key": format!("{}", s.key),
                            "tracks": s.tracks.len(),
                        })
                    })
                    .collect();
                let body = serde_json::to_vec(&serde_json::json!({ "streams": list }))
                    .map_err(|e| SdkError::Internal(format!("json: {e}")))?;
                Ok(HttpResponse::ok_json(body))
            }
            (HttpMethod::Get, "/stats") => {
                let streams = self.engine.stream_manager_api.list_streams().await?;
                let body = serde_json::to_vec(&serde_json::json!({
                    "streams_active": streams.len(),
                }))
                .map_err(|e| SdkError::Internal(format!("json: {e}")))?;
                Ok(HttpResponse::ok_json(body))
            }
            _ => Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: bytes::Bytes::from_static(b"{\"error\":\"not found\"}"),
            }),
        }
    }
}

/// Spawns a future on the runtime and returns a one-shot receiver for completion.
///
/// 在运行时上生成一个 future，并返回用于完成通知的 one-shot 接收端。
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

/// Spawns a future on the runtime without waiting for completion.
///
/// 在运行时上生成一个无需等待完成的 future。
fn spawn_runtime_detached<F>(runtime_api: Arc<dyn RuntimeApi>, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let _ = runtime_api.spawn(Box::pin(fut));
}

/// Returns the current runtime time in microseconds.
///
/// 返回当前运行时时间（微秒）。
fn runtime_now_micros(runtime_api: &Arc<dyn RuntimeApi>) -> u64 {
    runtime_api.now().as_micros()
}

/// Computes the pending-play source wait timeout from the config, if enabled.
///
/// 根据配置计算待播放源等待超时（如果启用）。
fn pending_play_source_wait_timeout(config: &RtmpModuleConfig) -> Option<Duration> {
    if config.play_wait_source_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(config.play_wait_source_timeout_ms))
    }
}

/// Computes a `MonoTime` deadline `duration` after the current runtime time.
///
/// 计算当前运行时时间之后 `duration` 的 `MonoTime` 截止时间。
fn runtime_deadline_after(runtime_api: &Arc<dyn RuntimeApi>, duration: Duration) -> MonoTime {
    let duration_micros = duration.as_micros();
    let delta = u64::try_from(duration_micros).unwrap_or(u64::MAX);
    MonoTime::from_micros(runtime_now_micros(runtime_api).saturating_add(delta))
}

/// Sleeps for the given duration using the runtime's timer API.
///
/// 使用运行时的定时器 API 睡眠指定时长。
async fn runtime_sleep(runtime_api: &Arc<dyn RuntimeApi>, duration: Duration) {
    let mut timer = runtime_api.sleep_until(runtime_deadline_after(runtime_api, duration));
    timer.wait().await;
}

/// Spawns supervisor loops for all enabled pull, push, and relay jobs.
///
/// 为所有启用的拉流、推流与转发任务生成监控循环。
fn spawn_static_job_loops(
    engine: EngineContext,
    config: RtmpModuleConfig,
    cancel: CancellationToken,
) -> Vec<OneShotReceiver> {
    let mut loops = Vec::new();
    for job in config.pull_jobs.iter().filter(|job| job.enabled).cloned() {
        let runtime_api = engine.runtime_api.clone();
        let job_cancel = cancel.child_token();
        loops.push(spawn_runtime_task(
            runtime_api,
            run_pull_job_supervisor(engine.clone(), config.clone(), job, job_cancel),
        ));
    }
    for job in config.push_jobs.iter().filter(|job| job.enabled).cloned() {
        let runtime_api = engine.runtime_api.clone();
        let job_cancel = cancel.child_token();
        loops.push(spawn_runtime_task(
            runtime_api,
            run_push_job_supervisor(engine.clone(), config.clone(), job, job_cancel),
        ));
    }
    for job in config.relay_jobs.iter().filter(|job| job.enabled).cloned() {
        let runtime_api = engine.runtime_api.clone();
        let job_cancel = cancel.child_token();
        loops.push(spawn_runtime_task(
            runtime_api,
            run_relay_job_supervisor(engine.clone(), config.clone(), job, job_cancel),
        ));
    }
    loops
}

/// Waits for `duration` or until the cancellation token fires, returning `true` on cancel.
///
/// 等待 `duration` 或直到取消 token 触发，取消时返回 `true`。
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

/// Computes the next retry backoff, doubling up to the configured cap.
///
/// 计算下一次重试退避，按倍数增长直到配置上限。
fn next_retry_backoff_ms(current_ms: u64, max_ms: u64) -> u64 {
    let cap = max_ms.max(1);
    current_ms.saturating_mul(2).min(cap)
}

/// Supervisor loop for an RTMP pull job: connect, ingest, retry with backoff.
///
/// Stops when the job is cancelled or the target stream is already occupied.
///
/// RTMP 拉流任务监控循环：连接、摄取、按退避重试。
///
/// 任务取消或目标流已被占用时停止。
async fn run_pull_job_supervisor(
    engine: EngineContext,
    module_config: RtmpModuleConfig,
    job: RtmpPullJobConfig,
    cancel: CancellationToken,
) {
    let Some(target_stream_key) = parse_stream_key_spec(&job.target_stream_key) else {
        return;
    };
    let Ok(source_url) = RtmpUrl::parse(job.source_url.trim()) else {
        return;
    };

    let base_backoff_ms = job.retry_backoff_ms.max(1);
    let max_backoff_ms = job.max_retry_backoff_ms.max(base_backoff_ms);
    let mut backoff_ms = base_backoff_ms;

    while !cancel.is_cancelled() {
        let result = run_pull_job_once(
            &engine,
            &module_config,
            &job,
            source_url.clone(),
            target_stream_key.clone(),
            cancel.child_token(),
        )
        .await;
        if cancel.is_cancelled() || result == PullJobResult::Occupied {
            break;
        }
        if wait_or_cancel(
            &engine.runtime_api,
            &cancel,
            Duration::from_millis(backoff_ms),
        )
        .await
        {
            break;
        }
        backoff_ms = next_retry_backoff_ms(backoff_ms, max_backoff_ms);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullJobResult {
    Ok,
    Occupied,
}

/// Starts an RTMP client, using TLS when the URL scheme is `rtmps://`.
///
/// 启动 RTMP 客户端；URL 方案为 `rtmps://` 时使用 TLS。
fn start_rtmp_client(
    runtime_api: Arc<dyn RuntimeApi>,
    url: RtmpUrl,
    mode: RtmpClientMode,
    config: RtmpClientDriverConfig,
    cancel: CancellationToken,
) -> std::io::Result<cheetah_rtmp_driver_tokio::RtmpClientHandle> {
    if url.tls {
        let tls_config = RtmpTlsClientConfig::with_native_roots()?;
        start_tls_client(runtime_api, url, mode, config, tls_config, cancel)
    } else {
        start_client(runtime_api, url, mode, config, cancel)
    }
}

/// Executes a single pull attempt: connect, acquire publisher, and ingest events.
///
/// 执行单次拉流尝试：连接、获取发布者并摄取事件。
async fn run_pull_job_once(
    engine: &EngineContext,
    module_config: &RtmpModuleConfig,
    job: &RtmpPullJobConfig,
    source_url: RtmpUrl,
    target_stream_key: cheetah_sdk::StreamKey,
    cancel: CancellationToken,
) -> PullJobResult {
    let mut client = match start_rtmp_client(
        engine.runtime_api.clone(),
        source_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig {
            write_queue_capacity: module_config.write_queue_capacity,
            ..RtmpClientDriverConfig::default()
        },
        cancel.child_token(),
    ) {
        Ok(client) => client,
        Err(_) => return PullJobResult::Ok,
    };

    let (lease, sink) = match engine
        .publisher_api
        .acquire_publisher(target_stream_key, PublisherOptions::default())
        .await
    {
        Ok(v) => v,
        Err(SdkError::AlreadyExists(_)) => {
            tracing::warn!(
                job_name = job.name,
                "pull job stopping: target stream key already has an active publisher"
            );
            client.shutdown();
            let _ = client.wait().await;
            return PullJobResult::Occupied;
        }
        Err(_) => {
            client.shutdown();
            let _ = client.wait().await;
            return PullJobResult::Ok;
        }
    };
    let mut session = PublishSession {
        lease,
        sink,
        tracks: PublishTracks::default(),
        timestamp_states: PublishTimestampStates::default(),
        fps_estimator: FrameRateEstimator::default(),
    };
    let mut queue_drop_count: u64 = 0;

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = client.recv_event().fuse();
        pin_mut!(cancel_fut, recv_fut);
        let next = select_biased! {
            _ = cancel_fut => None,
            event = recv_fut => event,
        };
        let Some(event) = next else {
            break;
        };
        match event {
            ClientDriverEvent::Connected { .. } => {}
            ClientDriverEvent::Closed { .. } => break,
            ClientDriverEvent::Core { event } => match event {
                RtmpEvent::Metadata { values, .. } => {
                    apply_metadata_to_tracks(&mut session, &values);
                    let _ = session.sink.update_tracks(session.tracks.list());
                }
                RtmpEvent::MediaData {
                    timestamp_ms,
                    media_type,
                    payload,
                    ..
                } => {
                    let frame = match media_type {
                        RtmpMediaType::Video
                            if job.track_selection == TrackSelection::AudioOnly =>
                        {
                            None
                        }
                        RtmpMediaType::Audio
                            if job.track_selection == TrackSelection::VideoOnly =>
                        {
                            None
                        }
                        RtmpMediaType::Video => handle_video_ingest_with_alert_threshold(
                            &mut session,
                            timestamp_ms,
                            &payload,
                            module_config.alert_thresholds.timestamp_repair_count,
                        ),
                        RtmpMediaType::Audio => handle_audio_ingest_with_alert_threshold(
                            &mut session,
                            timestamp_ms,
                            &payload,
                            module_config.alert_thresholds.timestamp_repair_count,
                        ),
                        RtmpMediaType::Data => {
                            handle_data_ingest(&mut session, &payload);
                            None
                        }
                    };
                    if let Some(frame) = frame {
                        if session.sink.update_tracks(session.tracks.list()).is_err() {
                            break;
                        }
                        let fields = frame_observability_fields(&frame);
                        match session.sink.push_frame(Arc::new(frame)) {
                            Ok(cheetah_sdk::DispatchResult::Accepted) => {
                                queue_drop_count = 0;
                            }
                            Ok(cheetah_sdk::DispatchResult::DroppedByPolicy) => {
                                queue_drop_count = queue_drop_count.saturating_add(1);
                                tracing::warn!(
                                    stream_key = %session.lease.stream_key,
                                    track_id = fields.track_id,
                                    codec = ?fields.codec,
                                    pts = fields.pts,
                                    dts = fields.dts,
                                    queue_drop_count,
                                    "pull job frame dropped by backpressure policy"
                                );
                                if should_emit_alert_threshold(
                                    queue_drop_count,
                                    module_config.alert_thresholds.queue_drop_count,
                                ) {
                                    tracing::warn!(
                                        stream_key = %session.lease.stream_key,
                                        track_id = fields.track_id,
                                        codec = ?fields.codec,
                                        pts = fields.pts,
                                        dts = fields.dts,
                                        queue_drop_count,
                                        queue_drop_alert_threshold =
                                            module_config.alert_thresholds.queue_drop_count,
                                        "rtmp ingest queue buildup alert threshold reached"
                                    );
                                }
                            }
                            Ok(cheetah_sdk::DispatchResult::RejectedClosed) | Err(_) => {
                                break;
                            }
                        }
                    }
                }
                RtmpEvent::ClientDisconnectRequested { .. }
                | RtmpEvent::PeerClosed
                | RtmpEvent::StreamClosed { .. } => break,
                _ => {}
            },
        }
    }

    client.shutdown();
    let _ = client.wait().await;
    let _ = session.sink.close();
    let _ = engine.publisher_api.release_publisher(&session.lease).await;
    PullJobResult::Ok
}

/// Supervisor loop for an RTMP push job: subscribe, connect, and forward frames.
///
/// RTMP 推流任务监控循环：订阅、连接并转发帧。
async fn run_push_job_supervisor(
    engine: EngineContext,
    module_config: RtmpModuleConfig,
    job: RtmpPushJobConfig,
    cancel: CancellationToken,
) {
    let Some(source_stream_key) = parse_stream_key_spec(&job.source_stream_key) else {
        return;
    };
    let Ok(target_url) = RtmpUrl::parse(job.target_url.trim()) else {
        return;
    };

    let base_backoff_ms = job.retry_backoff_ms.max(1);
    let max_backoff_ms = job.max_retry_backoff_ms.max(base_backoff_ms);
    let mut backoff_ms = base_backoff_ms;

    while !cancel.is_cancelled() {
        run_push_job_once(
            &engine,
            &module_config,
            job.name.as_str(),
            source_stream_key.clone(),
            target_url.clone(),
            job.track_selection == TrackSelection::AudioOnly,
            job.track_selection == TrackSelection::VideoOnly,
            cancel.child_token(),
        )
        .await;
        if cancel.is_cancelled() {
            break;
        }
        if wait_or_cancel(
            &engine.runtime_api,
            &cancel,
            Duration::from_millis(backoff_ms),
        )
        .await
        {
            break;
        }
        backoff_ms = next_retry_backoff_ms(backoff_ms, max_backoff_ms);
    }
}

#[allow(clippy::too_many_arguments)]
/// Executes a single push attempt: subscribe to source, connect to target, forward frames.
///
/// 执行单次推流尝试：订阅源、连接目标并转发帧。
async fn run_push_job_once(
    engine: &EngineContext,
    module_config: &RtmpModuleConfig,
    job_name: &str,
    source_stream_key: cheetah_sdk::StreamKey,
    target_url: RtmpUrl,
    disable_video: bool,
    disable_audio: bool,
    cancel: CancellationToken,
) {
    let current_tracks = engine
        .stream_manager_api
        .get_stream(&source_stream_key)
        .await
        .ok()
        .flatten()
        .map(|snapshot| snapshot.tracks)
        .unwrap_or_default();
    let bootstrap_max_frames = push_bootstrap_max_frames(module_config, &current_tracks);
    let queue_capacity = play_subscriber_queue_capacity(module_config, bootstrap_max_frames);

    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            source_stream_key.clone(),
            SubscriberOptions {
                queue_capacity,
                backpressure: module_config.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::live_tail(bootstrap_max_frames, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            tracing::warn!(
                job_name,
                stream_key = %source_stream_key,
                queue_capacity,
                bootstrap_max_frames,
                "push job subscribe failed: {err}"
            );
            return;
        }
    };

    let mut client = match start_rtmp_client(
        engine.runtime_api.clone(),
        target_url,
        RtmpClientMode::Publish,
        RtmpClientDriverConfig {
            write_queue_capacity: module_config.write_queue_capacity,
            ..RtmpClientDriverConfig::default()
        },
        cancel.child_token(),
    ) {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(
                job_name,
                stream_key = %source_stream_key,
                "push job start publish client failed: {err}"
            );
            let _ = subscriber.close().await;
            return;
        }
    };
    let command_tx = client.core_command_sender();
    let stream_id = 1u32;
    let mut current_tracks = current_tracks;
    let mut publish_ready = false;
    let mut last_mute_ts: Option<u32> = None;
    let mut last_media_timestamp = MediaTimestampState::default();

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let event_fut = client.recv_event().fuse();
        let frame_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, event_fut, frame_fut);
        let action = select_biased! {
            _ = cancel_fut => PushLoopAction::Stop,
            event = event_fut => PushLoopAction::Driver(event),
            frame = frame_fut => PushLoopAction::Frame(frame),
        };

        match action {
            PushLoopAction::Stop => break,
            PushLoopAction::Driver(None) => break,
            PushLoopAction::Driver(Some(ClientDriverEvent::Connected { .. })) => {}
            PushLoopAction::Driver(Some(ClientDriverEvent::Closed { .. })) => break,
            PushLoopAction::Driver(Some(ClientDriverEvent::Core { event })) => match event {
                RtmpEvent::ClientStateChanged {
                    state: RtmpClientState::Publishing,
                } => {
                    publish_ready = true;
                    if let Ok(Some(snapshot)) = engine
                        .stream_manager_api
                        .get_stream(&source_stream_key)
                        .await
                    {
                        current_tracks = snapshot.tracks;
                    }
                    if let Err(err) = send_client_bootstrap(
                        &command_tx,
                        stream_id,
                        &current_tracks,
                        module_config.enable_add_mute,
                        module_config.emit_play_metadata,
                    )
                    .await
                    {
                        tracing::warn!(
                            job_name,
                            stream_key = %source_stream_key,
                            "push job bootstrap send failed after publishing ready: {err:?}"
                        );
                        break;
                    }
                }
                RtmpEvent::ClientDisconnectRequested { .. }
                | RtmpEvent::PeerClosed
                | RtmpEvent::StreamClosed { .. } => break,
                _ => {}
            },
            PushLoopAction::Frame(Ok(Some(frame))) => {
                if !publish_ready {
                    continue;
                }
                if frame.is_key_frame() {
                    if let Ok(Some(snapshot)) = engine
                        .stream_manager_api
                        .get_stream(&source_stream_key)
                        .await
                    {
                        if snapshot.tracks != current_tracks {
                            current_tracks = snapshot.tracks;
                            if let Err(err) = send_client_bootstrap(
                                &command_tx,
                                stream_id,
                                &current_tracks,
                                module_config.enable_add_mute,
                                module_config.emit_play_metadata,
                            )
                            .await
                            {
                                tracing::warn!(
                                    job_name,
                                    stream_key = %source_stream_key,
                                    "push job bootstrap refresh send failed: {err:?}"
                                );
                                break;
                            }
                        }
                    }
                }

                if let Some(mut command) = map_frame_to_rtmp_with_tracks(
                    stream_id,
                    frame.clone(),
                    RtmpPlayMode::Normal,
                    &current_tracks,
                ) {
                    // Skip media types filtered out by track selection.
                    if disable_video && frame.media_kind == MediaKind::Video {
                        continue;
                    }
                    if disable_audio && frame.media_kind == MediaKind::Audio {
                        continue;
                    }
                    let fields = frame_observability_fields(frame.as_ref());
                    if frame.flags.contains(FrameFlags::DISCONTINUITY)
                        && should_reset_rtmp_egress_timeline_for_discontinuity(
                            &command,
                            &mut last_media_timestamp,
                        )
                    {
                        reset_rtmp_egress_timeline_state(
                            None,
                            &mut last_media_timestamp,
                            &mut last_mute_ts,
                        );
                    }
                    clamp_media_command_timestamp(&mut command, &mut last_media_timestamp);
                    if command_tx.send_core(command).await.is_err() {
                        tracing::warn!(
                            job_name,
                            stream_key = %source_stream_key,
                            track_id = fields.track_id,
                            codec = ?fields.codec,
                            pts = fields.pts,
                            dts = fields.dts,
                            "push job media send failed: client command channel closed"
                        );
                        break;
                    }
                } else {
                    let fields = frame_observability_fields(frame.as_ref());
                    tracing::warn!(
                        job_name,
                        stream_key = %source_stream_key,
                        track_id = fields.track_id,
                        media_kind = ?frame.media_kind,
                        codec = ?fields.codec,
                        pts = fields.pts,
                        dts = fields.dts,
                        "push job frame mapping to RTMP command failed"
                    );
                }
                if module_config.enable_add_mute
                    && frame.media_kind == MediaKind::Video
                    && !track_list_has_audio(&current_tracks)
                {
                    if let Some(mut mute_command) = maybe_make_mute_audio(
                        stream_id,
                        frame_dts_to_rtmp_timestamp_ms(frame.as_ref()),
                        &mut last_mute_ts,
                    ) {
                        clamp_media_command_timestamp(&mut mute_command, &mut last_media_timestamp);
                        if command_tx.send_core(mute_command).await.is_err() {
                            let fields = frame_observability_fields(frame.as_ref());
                            tracing::warn!(
                                job_name,
                                stream_key = %source_stream_key,
                                track_id = fields.track_id,
                                codec = ?fields.codec,
                                pts = fields.pts,
                                dts = fields.dts,
                                "push job mute-audio send failed: client command channel closed"
                            );
                            break;
                        }
                    }
                }
            }
            PushLoopAction::Frame(Ok(None)) => break,
            PushLoopAction::Frame(Err(err)) => {
                tracing::warn!(
                    job_name,
                    stream_key = %source_stream_key,
                    "push job subscriber recv failed: {err}"
                );
                break;
            }
        }
    }

    client.shutdown();
    let _ = subscriber.close().await;
    let _ = client.wait().await;
}

enum PushLoopAction {
    Stop,
    Driver(Option<ClientDriverEvent>),
    Frame(Result<Option<Arc<cheetah_codec::AVFrame>>, SdkError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameObservabilityFields {
    track_id: u32,
    codec: CodecId,
    pts: i64,
    dts: i64,
}

/// Extracts common frame fields used for logging and tracing.
///
/// 提取用于日志与跟踪的公共帧字段。
fn frame_observability_fields(frame: &AVFrame) -> FrameObservabilityFields {
    FrameObservabilityFields {
        track_id: frame.track_id.0,
        codec: frame.codec,
        pts: frame.pts,
        dts: frame.dts,
    }
}

/// Sends sequence headers and metadata to a client before media frames.
///
/// 在发送媒体帧前向客户端发送序列头与元数据。
async fn send_client_bootstrap(
    command_tx: &RtmpClientCommandSender,
    stream_id: u32,
    tracks: &[cheetah_codec::TrackInfo],
    enable_add_mute: bool,
    emit_play_metadata: bool,
) -> Result<(), ClientSendError> {
    let commands = build_track_bootstrap_commands(
        stream_id,
        tracks,
        RtmpPlayMode::Normal,
        enable_add_mute,
        emit_play_metadata,
    );
    for command in commands {
        command_tx.send_core(command).await?;
    }
    Ok(())
}

/// Tracks the last emitted RTMP timestamp per media type for egress.
///
/// 跟踪输出时每种媒体类型最后发送的 RTMP 时间戳。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MediaTimestampState {
    video_last_ms: Option<u32>,
    audio_last_ms: Option<u32>,
    metadata_last_ms: Option<u32>,
}

impl MediaTimestampState {
    /// Clears the remembered last timestamps.
    ///
    /// 清除记录的最后时间戳。
    fn reset(&mut self) {
        self.video_last_ms = None;
        self.audio_last_ms = None;
        self.metadata_last_ms = None;
    }
}

/// Rebases playback timestamps to start near zero for each play session.
///
/// 将播放时间戳重置为每个播放会话接近零的起始值。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PlayTimestampRebaseState {
    base_media_ms: Option<u32>,
}

/// Monitors audio/video timestamp drift and applies micro-corrections.
///
/// Checks every 500ms; if drift exceeds threshold, adjusts video DTS by ±step_ms.
///
/// 监控音视频时间戳漂移并应用微调修正。
///
/// 每 500ms 检查一次；若漂移超过阈值，则按 ±step_ms 调整视频 DTS。
#[derive(Debug, Clone, Copy, Default)]
struct AvSyncState {
    last_check_micros: u64,
    last_video_ms: u32,
    last_audio_ms: u32,
    correction_ms: i32,
}

impl AvSyncState {
    const CHECK_INTERVAL_US: u64 = 500_000; // 500ms
    const DRIFT_THRESHOLD_MS: i32 = 100;
    const STEP_MS: i32 = 10;

    fn on_video(&mut self, ts_ms: u32) {
        self.last_video_ms = ts_ms;
    }

    fn on_audio(&mut self, ts_ms: u32) {
        self.last_audio_ms = ts_ms;
    }

    /// Returns the correction to apply to the video timestamp (ms) for A/V sync.
    ///
    /// 返回用于音视频同步的、应应用到视频时间戳（毫秒）的修正值。
    fn check(&mut self, now_micros: u64) -> i32 {
        if self.last_check_micros == 0 {
            self.last_check_micros = now_micros;
            return 0;
        }
        if now_micros.saturating_sub(self.last_check_micros) < Self::CHECK_INTERVAL_US {
            return self.correction_ms;
        }
        self.last_check_micros = now_micros;

        if self.last_audio_ms == 0 || self.last_video_ms == 0 {
            return self.correction_ms;
        }

        let drift = self.last_video_ms as i32 - self.last_audio_ms as i32;
        if drift > Self::DRIFT_THRESHOLD_MS {
            self.correction_ms = self.correction_ms.saturating_sub(Self::STEP_MS);
        } else if drift < -Self::DRIFT_THRESHOLD_MS {
            self.correction_ms = self.correction_ms.saturating_add(Self::STEP_MS);
        }
        self.correction_ms
    }
}

impl PlayTimestampRebaseState {
    /// Rebases the given timestamp relative to the session start.
    ///
    /// 将给定时间戳相对于会话起点重新计算。
    fn rebase(&mut self, timestamp_ms: u32) -> u32 {
        let base = *self.base_media_ms.get_or_insert(timestamp_ms);
        timestamp_ms.saturating_sub(base)
    }

    /// Clears the rebase anchor.
    ///
    /// 清除 rebase 锚点。
    fn reset(&mut self) {
        self.base_media_ms = None;
    }
}

/// Paces the start of a play stream by aligning media time with runtime time.
///
/// 通过将媒体时间与运行时时间对齐，控制播放流的起始节奏。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PlayStartPacingState {
    anchor_media_ms: Option<u32>,
    anchor_runtime_micros: u64,
    last_media_ms: Option<u32>,
}

/// Enforces a minimum interval between consecutive sends to smooth bursty output.
///
/// 强制两次发送之间的最小间隔，以平滑突发输出。
#[derive(Debug, Clone, Copy)]
struct PacedSenderState {
    min_interval_micros: u64,
    last_send_micros: u64,
}

impl PacedSenderState {
    fn new(paced_sender_ms: u64) -> Self {
        Self {
            min_interval_micros: paced_sender_ms.saturating_mul(1_000),
            last_send_micros: 0,
        }
    }

    fn is_enabled(&self) -> bool {
        self.min_interval_micros > 0
    }

    /// Returns the additional delay needed to enforce the minimum send interval.
    ///
    /// 返回强制最小发送间隔所需的额外延迟。
    fn delay_for(&mut self, now_micros: u64) -> Duration {
        if self.min_interval_micros == 0 {
            return Duration::ZERO;
        }
        if self.last_send_micros == 0 {
            self.last_send_micros = now_micros;
            return Duration::ZERO;
        }
        let elapsed = now_micros.saturating_sub(self.last_send_micros);
        if elapsed >= self.min_interval_micros {
            self.last_send_micros = now_micros;
            Duration::ZERO
        } else {
            let wait = self.min_interval_micros - elapsed;
            self.last_send_micros = self
                .last_send_micros
                .saturating_add(self.min_interval_micros);
            Duration::from_micros(wait)
        }
    }
}

impl PlayStartPacingState {
    /// Returns the pacing delay needed to keep playback aligned with real time.
    ///
    /// 返回使播放与实时对齐所需的节奏延迟。
    fn delay_for(
        &mut self,
        media_timestamp_ms: u32,
        now_micros: u64,
        force_reset: bool,
    ) -> Duration {
        if force_reset || self.anchor_media_ms.is_none() {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        }

        let Some(anchor_media_ms) = self.anchor_media_ms else {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        };
        if let Some(last_media_ms) = self.last_media_ms {
            if media_timestamp_ms < last_media_ms
                && last_media_ms.wrapping_sub(media_timestamp_ms)
                    > RTMP_EGRESS_BACKWARD_REPAIR_THRESHOLD_MS
            {
                self.reset_anchor(media_timestamp_ms, now_micros);
                return Duration::ZERO;
            }
        }

        let elapsed_media_ms = media_timestamp_ms.wrapping_sub(anchor_media_ms);
        if elapsed_media_ms > RTMP_PLAY_PACING_MAX_FORWARD_DELTA_MS {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        }

        self.last_media_ms = Some(media_timestamp_ms);
        let elapsed_media_micros = u64::from(elapsed_media_ms).saturating_mul(1_000);
        let target_runtime_micros = self
            .anchor_runtime_micros
            .saturating_add(elapsed_media_micros);
        if now_micros >= target_runtime_micros {
            Duration::ZERO
        } else {
            Duration::from_micros(target_runtime_micros - now_micros)
        }
    }

    /// Resets the play pacing anchor to the current media and runtime time.
    ///
    /// 将播放节奏锚点重置为当前媒体与运行时时间。
    fn reset_anchor(&mut self, media_timestamp_ms: u32, now_micros: u64) {
        self.anchor_media_ms = Some(media_timestamp_ms);
        self.anchor_runtime_micros = now_micros;
        self.last_media_ms = Some(media_timestamp_ms);
    }
}

/// Repairs monotonic timestamps on egress commands to avoid backward jumps.
///
/// 修复输出命令上的单调时间戳，避免向后跳变。
fn clamp_media_command_timestamp(command: &mut RtmpCoreCommand, state: &mut MediaTimestampState) {
    let (timestamp, last_timestamp_ms) = match command {
        RtmpCoreCommand::SendVideo { timestamp_ms, .. } => (timestamp_ms, &mut state.video_last_ms),
        RtmpCoreCommand::SendAudio { timestamp_ms, .. } => (timestamp_ms, &mut state.audio_last_ms),
        RtmpCoreCommand::SendMetadata { timestamp_ms, .. } => {
            (timestamp_ms, &mut state.metadata_last_ms)
        }
        _ => return,
    };

    if let Some(last) = *last_timestamp_ms {
        let repaired = cheetah_codec::repair_monotonic_timestamp(
            *timestamp,
            Some(last),
            RTMP_EGRESS_BACKWARD_REPAIR_THRESHOLD_MS,
        );
        *timestamp = repaired.timestamp;
    }
    *last_timestamp_ms = Some(*timestamp);
}

/// Rebases audio/video command timestamps to a near-zero base for each play session.
///
/// 将每个播放会话的音频/视频命令时间戳重置为接近零的基准。
fn rebase_play_media_command_timestamp(
    command: &mut RtmpCoreCommand,
    state: &mut PlayTimestampRebaseState,
) {
    match command {
        RtmpCoreCommand::SendVideo { timestamp_ms, .. }
        | RtmpCoreCommand::SendAudio { timestamp_ms, .. } => {
            *timestamp_ms = state.rebase(*timestamp_ms);
        }
        _ => {}
    }
}

/// Extracts the media timestamp from an audio/video RTMP core command.
///
/// 从音频/视频 RTMP 核心命令中提取媒体时间戳。
fn command_media_timestamp_ms(command: &RtmpCoreCommand) -> Option<u32> {
    match command {
        RtmpCoreCommand::SendVideo { timestamp_ms, .. }
        | RtmpCoreCommand::SendAudio { timestamp_ms, .. } => Some(*timestamp_ms),
        _ => None,
    }
}

/// Applies an A/V sync correction to a video command timestamp.
///
/// 将音视频同步修正应用到视频命令时间戳。
fn apply_timestamp_correction(command: &mut RtmpCoreCommand, correction_ms: i32) {
    let ts = match command {
        RtmpCoreCommand::SendVideo { timestamp_ms, .. } => timestamp_ms,
        _ => return,
    };
    *ts = (*ts as i64 + correction_ms as i64).max(0) as u32;
}

/// Returns the mutable "last timestamp" slot for the given command type.
///
/// 返回给定命令类型对应的“最后时间戳”可变槽位。
fn command_last_timestamp_slot_mut<'a>(
    command: &RtmpCoreCommand,
    state: &'a mut MediaTimestampState,
) -> Option<&'a mut Option<u32>> {
    match command {
        RtmpCoreCommand::SendVideo { .. } => Some(&mut state.video_last_ms),
        RtmpCoreCommand::SendAudio { .. } => Some(&mut state.audio_last_ms),
        RtmpCoreCommand::SendMetadata { .. } => Some(&mut state.metadata_last_ms),
        _ => None,
    }
}

/// Detects a large backward timestamp jump that requires an egress timeline reset.
///
/// 检测需要重置输出时间轴的大幅向后时间戳跳变。
fn should_reset_rtmp_egress_timeline_for_discontinuity(
    command: &RtmpCoreCommand,
    state: &mut MediaTimestampState,
) -> bool {
    let Some(timestamp_ms) = command_media_timestamp_ms(command) else {
        return false;
    };
    let Some(last_slot) = command_last_timestamp_slot_mut(command, state) else {
        return false;
    };
    let Some(last_ms) = *last_slot else {
        return false;
    };
    timestamp_ms < last_ms
        && last_ms.wrapping_sub(timestamp_ms) > RTMP_EGRESS_BACKWARD_REPAIR_THRESHOLD_MS
}

/// Resets the egress timeline state, including rebase, clamp, and mute-audio markers.
///
/// 重置输出时间轴状态，包括 rebase、clamp 与静音音频标记。
fn reset_rtmp_egress_timeline_state(
    rebase: Option<&mut PlayTimestampRebaseState>,
    clamp: &mut MediaTimestampState,
    last_mute_ts: &mut Option<u32>,
) {
    if let Some(rebase) = rebase {
        rebase.reset();
    }
    clamp.reset();
    *last_mute_ts = None;
}

/// Supervisor loop for an RTMP relay job: pull from source and push to target.
///
/// Builds synthetic pull/push jobs and runs them concurrently, cancelling both when one stops.
///
/// RTMP 转发任务监控循环：从源拉流并推向目标。
///
/// 构造合成拉流/推流任务并并发运行，任一任务停止时取消两者。
async fn run_relay_job_supervisor(
    engine: EngineContext,
    module_config: RtmpModuleConfig,
    job: RtmpRelayJobConfig,
    cancel: CancellationToken,
) {
    let Ok(source_url) = RtmpUrl::parse(job.source_url.trim()) else {
        return;
    };
    let Ok(_target_url) = RtmpUrl::parse(job.target_url.trim()) else {
        return;
    };

    // Derive local stream key: use explicit config or extract from source URL.
    let stream_key_str = if job.stream_key.is_empty() {
        format!("{}/{}", source_url.app, source_url.stream_name)
    } else {
        job.stream_key.clone()
    };
    let Some(_local_stream_key) = parse_stream_key_spec(&stream_key_str) else {
        return;
    };

    // Construct synthetic pull and push job configs to reuse existing supervisors.
    let pull_job = RtmpPullJobConfig {
        name: format!("{}-pull", job.name),
        enabled: true,
        source_url: job.source_url.clone(),
        target_stream_key: stream_key_str.clone(),
        track_selection: TrackSelection::All,
        processing_policy: cheetah_sdk::ProcessingPolicy::Passthrough,
        retry_backoff_ms: job.retry_backoff_ms,
        max_retry_backoff_ms: job.max_retry_backoff_ms,
    };
    let push_job = RtmpPushJobConfig {
        name: format!("{}-push", job.name),
        enabled: true,
        source_stream_key: stream_key_str,
        target_url: job.target_url.clone(),
        track_selection: TrackSelection::All,
        processing_policy: cheetah_sdk::ProcessingPolicy::Passthrough,
        retry_backoff_ms: job.retry_backoff_ms,
        max_retry_backoff_ms: job.max_retry_backoff_ms,
    };

    // Run pull and push supervisors concurrently; cancel both when either stops.
    let relay_cancel = cancel.child_token();
    let pull_fut = run_pull_job_supervisor(
        engine.clone(),
        module_config.clone(),
        pull_job,
        relay_cancel.child_token(),
    )
    .fuse();
    let push_fut = run_push_job_supervisor(
        engine.clone(),
        module_config,
        push_job,
        relay_cancel.child_token(),
    )
    .fuse();
    let cancel_fut = cancel.cancelled().fuse();

    pin_mut!(pull_fut, push_fut, cancel_fut);
    select_biased! {
        _ = cancel_fut => {}
        _ = pull_fut => {}
        _ = push_fut => {}
    }
    relay_cancel.cancel();
}

/// Main RTMP server event loop: dispatches driver events and manages shutdown.
///
/// 主 RTMP 服务事件循环：分发驱动事件并管理关闭。
async fn run_event_loop(
    engine: EngineContext,
    config: RtmpModuleConfig,
    mut driver: RtmpServerHandle,
    cancel: CancellationToken,
) {
    let command_tx = driver.core_command_sender();
    let publish_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PublishSession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let play_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PlaySession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let keepalive_sessions: Arc<Mutex<HashMap<StreamKey, KeepaliveSession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut publish_queue_drop_counts: HashMap<RtmpConnectionId, u64> = HashMap::new();
    let mut shutdown_requested = false;

    loop {
        if shutdown_requested {
            let event_fut = driver.recv_event().fuse();
            let timeout_fut = runtime_sleep(&engine.runtime_api, Duration::from_secs(1)).fuse();
            pin_mut!(event_fut, timeout_fut);
            let next_event = select_biased! {
                maybe = event_fut => maybe,
                _ = timeout_fut => None,
            };
            let Some(event) = next_event else {
                break;
            };
            handle_driver_event(
                event,
                &engine,
                &config,
                &engine.runtime_api,
                &command_tx,
                publish_sessions.clone(),
                play_sessions.clone(),
                keepalive_sessions.clone(),
                &mut publish_queue_drop_counts,
            )
            .await;
            continue;
        }

        let mut next_event: Option<DriverEvent> = None;
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
            continue;
        }
        let Some(event) = next_event else {
            break;
        };
        handle_driver_event(
            event,
            &engine,
            &config,
            &engine.runtime_api,
            &command_tx,
            publish_sessions.clone(),
            play_sessions.clone(),
            keepalive_sessions.clone(),
            &mut publish_queue_drop_counts,
        )
        .await;
    }

    let drained_publishes: Vec<PublishSession> = {
        let mut publishes = publish_sessions.lock();
        publishes.drain().map(|(_, session)| session).collect()
    };
    for session in drained_publishes {
        let _ = session.sink.close();
        let _ = engine.publisher_api.release_publisher(&session.lease).await;
    }

    // Drain keepalive sessions on shutdown.
    let drained_keepalives: Vec<KeepaliveSession> = {
        let mut keepalives = keepalive_sessions.lock();
        keepalives.drain().map(|(_, session)| session).collect()
    };
    for session in drained_keepalives {
        let _ = session.sink.close();
        let _ = engine.publisher_api.release_publisher(&session.lease).await;
    }

    let drained_plays: Vec<PlaySession> = {
        let mut plays = play_sessions.lock();
        plays.drain().map(|(_, play)| play).collect()
    };
    for play in drained_plays {
        play.cancel.cancel();
        play.join.abort();
    }
}

#[allow(clippy::too_many_arguments)]
/// Dispatches a single RTMP driver event to publish, play, or connection cleanup logic.
///
/// 将单个 RTMP 驱动事件分发给发布、播放或连接清理逻辑。
async fn handle_driver_event(
    event: DriverEvent,
    engine: &EngineContext,
    config: &RtmpModuleConfig,
    runtime_api: &Arc<dyn RuntimeApi>,
    command_tx: &RtmpCoreCommandSender,
    publish_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PublishSession>>>,
    play_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PlaySession>>>,
    keepalive_sessions: Arc<Mutex<HashMap<StreamKey, KeepaliveSession>>>,
    publish_queue_drop_counts: &mut HashMap<RtmpConnectionId, u64>,
) {
    match event {
        DriverEvent::ConnectionOpened { .. } => {}
        DriverEvent::ConnectionClosed { connection_id, .. } => {
            publish_queue_drop_counts.remove(&connection_id);
            cleanup_connection(
                connection_id,
                engine,
                runtime_api,
                publish_sessions.clone(),
                play_sessions.clone(),
                keepalive_sessions.clone(),
                config.publish_keepalive_ms,
            )
            .await;
        }
        DriverEvent::Core {
            connection_id,
            event,
        } => match event {
            RtmpEvent::Connected { .. } => {}
            RtmpEvent::PublishRequested {
                stream_id,
                app,
                stream_name,
                ..
            } => {
                if !check_publish_auth(config, &stream_name) {
                    send_reject_then_close(
                        runtime_api,
                        command_tx,
                        connection_id,
                        RtmpCoreCommand::RejectPublish {
                            stream_id,
                            description: "authorization failed".to_string(),
                        },
                    )
                    .await;
                    return;
                }
                let route = parse_stream_route(&app, &stream_name);
                // Check if there's a keepalive session for this stream key.
                let keepalive = keepalive_sessions.lock().remove(&route.stream_key);
                if let Some(keepalive) = keepalive {
                    // Reuse the existing lease — the timer will find the session removed.
                    let previous_publish = publish_sessions.lock().insert(
                        connection_id,
                        PublishSession {
                            lease: keepalive.lease,
                            sink: keepalive.sink,
                            tracks: keepalive.tracks,
                            timestamp_states: PublishTimestampStates::default(),
                            fps_estimator: FrameRateEstimator::default(),
                        },
                    );
                    if let Some(previous_publish) = previous_publish {
                        let _ = previous_publish.sink.close();
                        let _ = engine
                            .publisher_api
                            .release_publisher(&previous_publish.lease)
                            .await;
                    }
                    let _ = command_tx
                        .send_core(connection_id, RtmpCoreCommand::AcceptPublish { stream_id })
                        .await;
                    tracing::info!(
                        stream_key = %route.stream_key,
                        "publisher reconnected within keepalive window"
                    );
                    return;
                }
                match engine
                    .publisher_api
                    .acquire_publisher(route.stream_key.clone(), PublisherOptions::default())
                    .await
                {
                    Ok((lease, sink)) => {
                        let previous_publish = publish_sessions.lock().insert(
                            connection_id,
                            PublishSession {
                                lease,
                                sink,
                                tracks: PublishTracks::default(),
                                timestamp_states: PublishTimestampStates::default(),
                                fps_estimator: FrameRateEstimator::default(),
                            },
                        );
                        if let Some(previous_publish) = previous_publish {
                            let _ = previous_publish.sink.close();
                            let _ = engine
                                .publisher_api
                                .release_publisher(&previous_publish.lease)
                                .await;
                        }
                        let _ = command_tx
                            .send_core(connection_id, RtmpCoreCommand::AcceptPublish { stream_id })
                            .await;
                    }
                    Err(err) => {
                        send_reject_then_close(
                            runtime_api,
                            command_tx,
                            connection_id,
                            RtmpCoreCommand::RejectPublish {
                                stream_id,
                                description: err.to_string(),
                            },
                        )
                        .await;
                    }
                }
            }
            RtmpEvent::PlayRequested {
                stream_id,
                app,
                stream_name,
                ..
            } => {
                if !check_play_auth(config, &stream_name) {
                    send_reject_then_close(
                        runtime_api,
                        command_tx,
                        connection_id,
                        RtmpCoreCommand::RejectPlay {
                            stream_id,
                            description: "authorization failed".to_string(),
                        },
                    )
                    .await;
                    return;
                }
                let route = parse_stream_route(&app, &stream_name);
                let snapshot_opt = engine
                    .stream_manager_api
                    .get_stream(&route.stream_key)
                    .await
                    .ok()
                    .flatten();

                match snapshot_opt {
                    Some(snapshot) => {
                        if !track_list_has_supported_playback_codec(&snapshot.tracks) {
                            send_reject_then_close(
                                runtime_api,
                                command_tx,
                                connection_id,
                                RtmpCoreCommand::RejectPlay {
                                    stream_id,
                                    description: "stream has no RTMP/FLV playable media track"
                                        .to_string(),
                                },
                            )
                            .await;
                            return;
                        }
                        if !track_list_ready_for_rtmp_play_bootstrap(&snapshot.tracks) {
                            let pending = spawn_pending_play(
                                engine.clone(),
                                config.clone(),
                                runtime_api.clone(),
                                command_tx.clone(),
                                connection_id,
                                stream_id,
                                route.clone(),
                            );
                            replace_play_session(&play_sessions, connection_id, pending);
                            return;
                        }
                        let play_cancel = CancellationToken::new();
                        let play_cancel_child = play_cancel.child_token();
                        let join = runtime_api.spawn(Box::pin(run_play_stream(
                            PlayTaskContext {
                                engine: engine.clone(),
                                config: config.clone(),
                                runtime_api: runtime_api.clone(),
                                command_tx: command_tx.clone(),
                                connection_id,
                                stream_id,
                                route: route.clone(),
                                subscribe_reject_description: None,
                            },
                            snapshot.tracks,
                            play_cancel_child,
                        )));

                        replace_play_session(
                            &play_sessions,
                            connection_id,
                            PlaySession {
                                cancel: play_cancel,
                                join,
                            },
                        );
                    }
                    _ => {
                        let pending = spawn_pending_play(
                            engine.clone(),
                            config.clone(),
                            runtime_api.clone(),
                            command_tx.clone(),
                            connection_id,
                            stream_id,
                            route.clone(),
                        );
                        replace_play_session(&play_sessions, connection_id, pending);
                    }
                }
            }
            RtmpEvent::MediaData {
                stream_id: _,
                timestamp_ms,
                media_type,
                payload,
            } => {
                let frame = with_publish_session(connection_id, &publish_sessions, |session| {
                    let frame = match media_type {
                        RtmpMediaType::Video => handle_video_ingest_with_alert_threshold(
                            session,
                            timestamp_ms,
                            &payload,
                            config.alert_thresholds.timestamp_repair_count,
                        ),
                        RtmpMediaType::Audio => handle_audio_ingest_with_alert_threshold(
                            session,
                            timestamp_ms,
                            &payload,
                            config.alert_thresholds.timestamp_repair_count,
                        ),
                        RtmpMediaType::Data => {
                            handle_data_ingest(session, &payload);
                            None
                        }
                    };
                    if let Some(ref f) = frame {
                        if f.media_kind == cheetah_codec::MediaKind::Video {
                            if let Some(fps) = session.fps_estimator.on_video_frame(f.dts) {
                                if let Some(track) = session.tracks.video.as_mut() {
                                    track.fps = Some(cheetah_codec::Rational32::new(
                                        (fps * 1000.0) as u32,
                                        1000,
                                    ));
                                }
                            }
                        }
                    }
                    frame
                })
                .flatten();
                if let Some(frame) = frame {
                    let mut sessions = publish_sessions.lock();
                    if let Some(session) = sessions.get_mut(&connection_id) {
                        let tracks = session.tracks.list();
                        let _ = session.sink.update_tracks(tracks);
                        let fields = frame_observability_fields(&frame);
                        match session.sink.push_frame(Arc::new(frame)) {
                            Ok(cheetah_sdk::DispatchResult::Accepted) => {
                                publish_queue_drop_counts.remove(&connection_id);
                            }
                            Ok(cheetah_sdk::DispatchResult::DroppedByPolicy) => {
                                let queue_drop_count = publish_queue_drop_counts
                                    .entry(connection_id)
                                    .and_modify(|count| *count = count.saturating_add(1))
                                    .or_insert(1);
                                tracing::warn!(
                                    %connection_id,
                                    stream_key = %session.lease.stream_key,
                                    track_id = fields.track_id,
                                    codec = ?fields.codec,
                                    pts = fields.pts,
                                    dts = fields.dts,
                                    queue_drop_count = *queue_drop_count,
                                    "ingest frame dropped by backpressure policy"
                                );
                                if should_emit_alert_threshold(
                                    *queue_drop_count,
                                    config.alert_thresholds.queue_drop_count,
                                ) {
                                    tracing::warn!(
                                        %connection_id,
                                        stream_key = %session.lease.stream_key,
                                        track_id = fields.track_id,
                                        codec = ?fields.codec,
                                        pts = fields.pts,
                                        dts = fields.dts,
                                        queue_drop_count = *queue_drop_count,
                                        queue_drop_alert_threshold =
                                            config.alert_thresholds.queue_drop_count,
                                        "rtmp ingest queue buildup alert threshold reached"
                                    );
                                }
                            }
                            Ok(cheetah_sdk::DispatchResult::RejectedClosed) | Err(_) => {}
                        }
                    }
                } else if matches!(media_type, RtmpMediaType::Video | RtmpMediaType::Audio) {
                    with_publish_session(connection_id, &publish_sessions, |session| {
                        // Sync track metadata even when this packet only carries codec config.
                        let tracks = session.tracks.list();
                        let _ = session.sink.update_tracks(tracks);
                    });
                }
            }
            RtmpEvent::Metadata { values, .. } => {
                with_publish_session(connection_id, &publish_sessions, |session| {
                    apply_metadata_to_tracks(session, &values);
                    let tracks = session.tracks.list();
                    let _ = session.sink.update_tracks(tracks);
                });
            }
            RtmpEvent::Notify { .. } => {}
            RtmpEvent::StreamCreated { .. }
            | RtmpEvent::CommandIgnored { .. }
            | RtmpEvent::MessageIgnored { .. }
            | RtmpEvent::UserControlIgnored { .. }
            | RtmpEvent::AckReceived { .. }
            | RtmpEvent::LocalAckWindowUpdated { .. }
            | RtmpEvent::PeerAckWindowUpdated { .. }
            | RtmpEvent::ClientStateChanged { .. }
            | RtmpEvent::ClientDisconnectRequested { .. }
            | RtmpEvent::SeekRequested { .. }
            | RtmpEvent::PauseRequested { .. }
            | RtmpEvent::ReceiveVideo { .. }
            | RtmpEvent::ReceiveAudio { .. } => {}
            RtmpEvent::StreamClosed { .. } | RtmpEvent::PeerClosed => {
                publish_queue_drop_counts.remove(&connection_id);
                cleanup_connection(
                    connection_id,
                    engine,
                    runtime_api,
                    publish_sessions.clone(),
                    play_sessions.clone(),
                    keepalive_sessions.clone(),
                    config.publish_keepalive_ms,
                )
                .await;
            }
        },
    }
}

/// Cleans up a closed connection: release publish lease or keep it alive briefly.
///
/// On publish disconnect, enters keepalive state if configured, or releases immediately.
/// Stops and aborts play sessions.
///
/// 清理关闭的连接：释放发布租约或短暂保持。
///
/// 发布端断开时，若配置则进入保活状态，否则立即释放；停止并中止播放会话。
async fn cleanup_connection(
    connection_id: RtmpConnectionId,
    engine: &EngineContext,
    runtime_api: &Arc<dyn RuntimeApi>,
    publish_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PublishSession>>>,
    play_sessions: Arc<Mutex<HashMap<RtmpConnectionId, PlaySession>>>,
    keepalive_sessions: Arc<Mutex<HashMap<StreamKey, KeepaliveSession>>>,
    publish_keepalive_ms: u64,
) {
    let publish = publish_sessions.lock().remove(&connection_id);
    if let Some(publish) = publish {
        if publish_keepalive_ms > 0 {
            // Enter keepalive state: hold the lease open for reconnection.
            let stream_key = publish.lease.stream_key.clone();
            let keepalive = KeepaliveSession {
                lease: publish.lease,
                sink: publish.sink,
                tracks: publish.tracks,
            };
            keepalive_sessions
                .lock()
                .insert(stream_key.clone(), keepalive);

            // Spawn a timer that releases the lease if no reconnection occurs.
            let keepalive_sessions_clone = keepalive_sessions.clone();
            let publisher_api = engine.publisher_api.clone();
            let runtime_api_clone = runtime_api.clone();
            let stream_key_for_timer = stream_key.clone();
            spawn_runtime_detached(runtime_api.clone(), async move {
                runtime_sleep(
                    &runtime_api_clone,
                    Duration::from_millis(publish_keepalive_ms),
                )
                .await;
                // After timeout, check if the keepalive session is still present
                // (it will have been removed if a reconnection consumed it).
                let expired = keepalive_sessions_clone
                    .lock()
                    .remove(&stream_key_for_timer);
                if let Some(expired) = expired {
                    let _ = expired.sink.close();
                    let _ = publisher_api.release_publisher(&expired.lease).await;
                    tracing::info!(
                        %stream_key_for_timer,
                        keepalive_ms = publish_keepalive_ms,
                        "publish keepalive expired, stream released"
                    );
                }
            });
            tracing::debug!(
                %stream_key,
                keepalive_ms = publish_keepalive_ms,
                "publisher disconnected, entering keepalive state"
            );
        } else if should_delay_publish_release_for_h264(&publish) {
            let publisher_api = engine.publisher_api.clone();
            let runtime_api = runtime_api.clone();
            spawn_runtime_detached(runtime_api.clone(), async move {
                // Keep short H264 streams discoverable for pending-play polling.
                runtime_sleep(&runtime_api, Duration::from_millis(H264_RELEASE_GRACE_MS)).await;
                let _ = publish.sink.close();
                let _ = publisher_api.release_publisher(&publish.lease).await;
            });
        } else {
            let _ = publish.sink.close();
            let _ = engine.publisher_api.release_publisher(&publish.lease).await;
        }
    }

    let play = play_sessions.lock().remove(&connection_id);
    if let Some(play) = play {
        play.cancel.cancel();
        play.join.abort();
    }
}

/// Replaces an active or pending play session, cancelling the previous one.
///
/// 替换活跃或待播放会话，取消上一个会话。
fn replace_play_session(
    play_sessions: &Arc<Mutex<HashMap<RtmpConnectionId, PlaySession>>>,
    connection_id: RtmpConnectionId,
    next: PlaySession,
) {
    let previous = play_sessions.lock().insert(connection_id, next);
    if let Some(previous) = previous {
        previous.cancel.cancel();
        previous.join.abort();
    }
}

/// Sends a reject command and closes the connection after a short flush delay.
///
/// 发送拒绝命令并在短暂刷新延迟后关闭连接。
async fn send_reject_then_close(
    runtime_api: &Arc<dyn RuntimeApi>,
    command_tx: &RtmpCoreCommandSender,
    connection_id: RtmpConnectionId,
    command: RtmpCoreCommand,
) {
    let _ = command_tx.send_core(connection_id, command).await;
    let close_tx = command_tx.clone();
    let runtime_api = runtime_api.clone();
    spawn_runtime_detached(runtime_api.clone(), async move {
        // Keep a short gap so the error status has a chance to flush first.
        runtime_sleep(&runtime_api, Duration::from_millis(50)).await;
        let _ = close_tx.close_connection(connection_id).await;
    });
}

/// Computes the bootstrap frame count for a play session.
///
/// 计算播放会话的引导帧数。
fn play_bootstrap_max_frames(
    config: &RtmpModuleConfig,
    tracks: &[cheetah_codec::TrackInfo],
) -> usize {
    let floor = video_bootstrap_floor(tracks);
    config.bootstrap_max_frames.max(floor)
}

/// Computes the bootstrap frame count for a push job.
///
/// 计算推流任务的引导帧数。
fn push_bootstrap_max_frames(
    config: &RtmpModuleConfig,
    tracks: &[cheetah_codec::TrackInfo],
) -> usize {
    if tracks.is_empty() {
        // Push jobs often subscribe before upstream tracks are fully announced.
        // Keep an H26x-class GOP window in that pending state.
        return config.bootstrap_max_frames.max(1024);
    }
    play_bootstrap_max_frames(config, tracks)
}

/// Returns the codec-dependent bootstrap floor to ensure a full GOP is buffered.
///
/// 返回依赖编解码器的引导下限，确保缓存完整 GOP。
fn video_bootstrap_floor(tracks: &[cheetah_codec::TrackInfo]) -> usize {
    if !track_list_has_video(tracks) {
        return 0;
    }
    if track_list_has_codec(tracks, CodecId::VP9)
        || track_list_has_codec(tracks, CodecId::AV1)
        || track_list_has_codec(tracks, CodecId::H265)
        || track_list_has_codec(tracks, CodecId::H266)
    {
        return 2048;
    }
    1024
}

/// Checks whether the track list has enough metadata to start an RTMP play bootstrap.
///
/// 判断轨道列表是否具备足够元数据以启动 RTMP 播放引导。
fn track_list_ready_for_rtmp_play_bootstrap(tracks: &[cheetah_codec::TrackInfo]) -> bool {
    if tracks.is_empty() {
        return false;
    }
    if !track_list_has_video(tracks) {
        return track_list_has_supported_playback_codec(tracks);
    }

    tracks
        .iter()
        .filter(|track| track.media_kind == MediaKind::Video)
        .any(video_track_ready_for_rtmp_play_bootstrap)
}

/// Checks whether a single video track has enough metadata to start an RTMP play bootstrap.
///
/// 判断单个视频轨道是否具备足够元数据以启动 RTMP 播放引导。
fn video_track_ready_for_rtmp_play_bootstrap(track: &cheetah_codec::TrackInfo) -> bool {
    if !rtmp_playback_codec_supported(track.media_kind, track.codec) {
        return false;
    }
    if !codec_requires_strict_play_bootstrap(track.codec) {
        return true;
    }
    match (&track.codec, &track.extradata) {
        (
            CodecId::H264,
            CodecExtradata::H264 {
                avcc: Some(avcc), ..
            },
        ) => !avcc.is_empty(),
        (CodecId::H264, CodecExtradata::H264 { sps, pps, .. }) => {
            !sps.is_empty() && !pps.is_empty()
        }
        (
            CodecId::H265,
            CodecExtradata::H265 {
                hvcc: Some(hvcc), ..
            },
        ) => !hvcc.is_empty(),
        (CodecId::H265, CodecExtradata::H265 { vps, sps, pps, .. })
        | (CodecId::H266, CodecExtradata::H266 { vps, sps, pps }) => {
            !vps.is_empty() && !sps.is_empty() && !pps.is_empty()
        }
        (
            CodecId::AV1,
            CodecExtradata::AV1 {
                codec_config: Some(config),
                ..
            },
        ) => !config.is_empty(),
        (
            CodecId::VP9,
            CodecExtradata::VP9 {
                config: Some(config),
            },
        )
        | (
            CodecId::VP8,
            CodecExtradata::VP8 {
                config: Some(config),
            },
        ) => !config.is_empty(),
        (CodecId::VP8 | CodecId::VP9, _) => true,
        _ => false,
    }
}

/// Returns true if the codec requires a complete config for play bootstrap.
///
/// 判断该编解码器是否需要完整配置才能进行播放引导。
fn codec_requires_strict_play_bootstrap(codec: CodecId) -> bool {
    matches!(codec, CodecId::AV1)
}

/// Computes the subscriber queue capacity, ensuring it is at least the bootstrap size.
///
/// 计算订阅者队列容量，确保至少为引导大小。
fn play_subscriber_queue_capacity(config: &RtmpModuleConfig, bootstrap_max_frames: usize) -> usize {
    config.subscriber_queue_capacity.max(bootstrap_max_frames)
}

/// Returns true if the codec requires a keyframe before play bootstrap is complete.
///
/// 判断该编解码器是否需要在播放引导完成前收到关键帧。
fn rtmp_play_codec_requires_keyframe_bootstrap(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::H264 | CodecId::H265 | CodecId::H266 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9
    )
}

/// Returns true if the play session must wait for a video keyframe before starting.
///
/// 判断播放会话是否必须在开始前等待视频关键帧。
fn rtmp_play_waits_for_video_keyframe(tracks: &[cheetah_codec::TrackInfo]) -> bool {
    tracks.iter().any(|track| {
        track.media_kind == MediaKind::Video
            && rtmp_play_codec_requires_keyframe_bootstrap(track.codec)
    })
}

/// Returns true if play can start immediately without waiting for a video keyframe.
///
/// 判断播放是否可以无需等待视频关键帧立即开始。
fn initial_rtmp_play_video_started(tracks: &[cheetah_codec::TrackInfo]) -> bool {
    if tracks.is_empty() {
        return false;
    }
    !rtmp_play_waits_for_video_keyframe(tracks)
}

/// Recomputes the "video started" gate after track metadata changes.
///
/// Re-arms the gate when transitioning from audio-only/unknown into a keyframe-required codec.
///
/// 在轨道元数据变化后重新计算“视频已开始”门控。
///
/// 当从仅音频/未知过渡到需要关键帧的编解码器时重新打开门控。
fn reconcile_rtmp_play_video_started_on_track_refresh(
    video_started: bool,
    previous_tracks: &[cheetah_codec::TrackInfo],
    updated_tracks: &[cheetah_codec::TrackInfo],
) -> bool {
    if updated_tracks.is_empty() {
        // Unknown track model: keep gate closed to avoid forwarding delta frames as startup.
        return false;
    }

    let previous_wait_for_keyframe = rtmp_play_waits_for_video_keyframe(previous_tracks);
    let updated_wait_for_keyframe = rtmp_play_waits_for_video_keyframe(updated_tracks);
    if !updated_wait_for_keyframe {
        return true;
    }
    if !previous_wait_for_keyframe {
        // Transition from unknown/audio-only into video stream: re-arm gate.
        return false;
    }
    video_started
}

/// Decides whether a frame should be forwarded to an RTMP player.
///
/// Drops non-key frames before the `video_started` gate has opened.
///
/// 判断帧是否应该转发给 RTMP 播放器。
///
/// 在 `video_started` 门控打开前丢弃非关键帧。
fn should_forward_rtmp_play_frame(
    current_tracks: &[cheetah_codec::TrackInfo],
    video_started: &mut bool,
    frame: &AVFrame,
) -> bool {
    if *video_started {
        return true;
    }
    if frame.media_kind != MediaKind::Video {
        return false;
    }

    let codec = current_tracks
        .iter()
        .find(|track| track.track_id == frame.track_id && track.media_kind == MediaKind::Video)
        .map(|track| track.codec)
        .unwrap_or(frame.codec);
    if rtmp_play_codec_requires_keyframe_bootstrap(codec) && !frame.is_key_frame() {
        return false;
    }

    *video_started = true;
    true
}

/// Decides whether to refresh the play bootstrap (sequence headers) on a key frame.
///
/// 判断是否在关键帧处刷新播放引导（序列头）。
fn should_refresh_play_bootstrap(
    frame: &AVFrame,
    current_tracks: &[cheetah_codec::TrackInfo],
) -> bool {
    frame.is_key_frame()
        || !track_list_has_video(current_tracks)
        || (frame.media_kind == MediaKind::Video
            && !track_list_ready_for_rtmp_play_bootstrap(current_tracks))
}

/// Context passed to the play stream task for a single RTMP player.
///
/// 传递给单个 RTMP 播放器播放流任务的上下文。
struct PlayTaskContext {
    engine: EngineContext,
    config: RtmpModuleConfig,
    runtime_api: Arc<dyn RuntimeApi>,
    command_tx: RtmpCoreCommandSender,
    connection_id: RtmpConnectionId,
    stream_id: u32,
    route: StreamRoute,
    subscribe_reject_description: Option<&'static str>,
}

/// Main loop for an RTMP play stream: subscribe, bootstrap, then forward frames.
///
/// Handles A/V sync, pacing, mute-audio injection, and bootstrap refresh.
///
/// RTMP 播放流主循环：订阅、引导，然后转发帧。
///
/// 处理音视频同步、节奏控制、静音音频注入与引导刷新。
async fn run_play_stream(
    ctx: PlayTaskContext,
    mut current_tracks: Vec<cheetah_codec::TrackInfo>,
    play_cancel_child: CancellationToken,
) {
    let PlayTaskContext {
        engine,
        config,
        runtime_api,
        command_tx,
        connection_id,
        stream_id,
        route,
        subscribe_reject_description,
    } = ctx;

    let bootstrap_max_frames = play_bootstrap_max_frames(&config, &current_tracks);
    let queue_capacity = play_subscriber_queue_capacity(&config, bootstrap_max_frames);
    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            route.stream_key.clone(),
            SubscriberOptions {
                queue_capacity,
                backpressure: config.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::live_tail(bootstrap_max_frames, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            let description = subscribe_reject_description
                .map(str::to_owned)
                .unwrap_or_else(|| err.to_string());
            send_reject_then_close(
                &runtime_api,
                &command_tx,
                connection_id,
                RtmpCoreCommand::RejectPlay {
                    stream_id,
                    description,
                },
            )
            .await;
            return;
        }
    };

    let (emit_play_status, emit_sample_access) = play_accept_flags(&current_tracks);
    let _ = command_tx
        .send_core(
            connection_id,
            RtmpCoreCommand::AcceptPlayConfigured {
                stream_id,
                emit_play_status,
                emit_sample_access,
            },
        )
        .await;

    if let Err(err) = send_track_bootstrap(
        connection_id,
        stream_id,
        &current_tracks,
        route.play_mode,
        config.enable_add_mute,
        config.emit_play_metadata,
        &command_tx,
    )
    .await
    {
        tracing::warn!(
            %connection_id,
            stream_id,
            stream_key = %route.stream_key,
            "play bootstrap send failed: {err:?}"
        );
        let _ = subscriber.close().await;
        return;
    }

    let stream_api = engine.stream_manager_api.clone();
    let stream_key_for_task = route.stream_key;
    let play_mode = route.play_mode;
    let enable_add_mute = config.enable_add_mute;
    let emit_play_metadata = config.emit_play_metadata;
    let force_close_on_source_end = should_force_close_play_on_source_end(&current_tracks);

    let mut last_mute_ts: Option<u32> = None;
    let mut play_timestamp_rebase = PlayTimestampRebaseState::default();
    let mut last_media_timestamp = MediaTimestampState::default();
    let mut play_start_pacing = PlayStartPacingState::default();
    let mut paced_sender = PacedSenderState::new(config.paced_sender_ms);
    let mut av_sync = AvSyncState::default();
    let mut video_started = initial_rtmp_play_video_started(&current_tracks);
    let mut force_close_connection = false;
    let mut last_source_check_micros = runtime_now_micros(&runtime_api);
    loop {
        let cancel_fut = play_cancel_child.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        select_biased! {
            _ = cancel_fut => {
                break;
            }
            recv = recv_fut => {
                match recv {
                    Ok(Some(frame)) => {
                        let now_micros = runtime_now_micros(&runtime_api);
                        if force_close_on_source_end
                            && now_micros.saturating_sub(last_source_check_micros)
                                >= Duration::from_millis(200).as_micros() as u64
                        {
                            last_source_check_micros = now_micros;
                            if let Ok(None) = stream_api
                                .get_stream(&stream_key_for_task)
                                .await
                            {
                                // For HEVC pulls, stop immediately after source teardown
                                // instead of draining stale backlog.
                                force_close_connection = true;
                                break;
                            }
                        }

                        if should_refresh_play_bootstrap(frame.as_ref(), &current_tracks) {
                            let updated = stream_api
                                .get_stream(&stream_key_for_task)
                                .await;
                            if let Ok(Some(updated)) = updated {
                                if updated.tracks != current_tracks {
                                    let next_video_started =
                                        reconcile_rtmp_play_video_started_on_track_refresh(
                                            video_started,
                                            &current_tracks,
                                            &updated.tracks,
                                        );
                                    if let Err(err) = send_track_bootstrap(
                                        connection_id,
                                        stream_id,
                                        &updated.tracks,
                                        play_mode,
                                        enable_add_mute,
                                        emit_play_metadata,
                                        &command_tx,
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            %connection_id,
                                            stream_id,
                                            stream_key = %stream_key_for_task,
                                            "play bootstrap refresh send failed: {err:?}"
                                        );
                                        break;
                                    }
                                    current_tracks = updated.tracks;
                                    video_started = next_video_started;
                                }
                            }
                        }

                        if !rtmp_playback_codec_supported(frame.media_kind, frame.codec) {
                            continue;
                        }

                        if !should_forward_rtmp_play_frame(
                            &current_tracks,
                            &mut video_started,
                            frame.as_ref(),
                        ) {
                            continue;
                        }

                        if let Some(mut command) = map_frame_to_rtmp_with_tracks(
                            stream_id,
                            frame.clone(),
                            play_mode,
                            &current_tracks,
                        ) {
                            let fields = frame_observability_fields(frame.as_ref());
                            if frame.flags.contains(FrameFlags::DISCONTINUITY)
                                && should_reset_rtmp_egress_timeline_for_discontinuity(
                                    &command,
                                    &mut last_media_timestamp,
                                )
                            {
                                reset_rtmp_egress_timeline_state(
                                    Some(&mut play_timestamp_rebase),
                                    &mut last_media_timestamp,
                                    &mut last_mute_ts,
                                );
                            }
                            rebase_play_media_command_timestamp(
                                &mut command,
                                &mut play_timestamp_rebase,
                            );
                            clamp_media_command_timestamp(&mut command, &mut last_media_timestamp);
                            // A/V sync: track timestamps and apply correction to video.
                            if let Some(ts) = command_media_timestamp_ms(&command) {
                                if frame.media_kind == MediaKind::Video {
                                    av_sync.on_video(ts);
                                    let correction = av_sync.check(now_micros);
                                    if correction != 0 {
                                        apply_timestamp_correction(&mut command, correction);
                                    }
                                } else if frame.media_kind == MediaKind::Audio {
                                    av_sync.on_audio(ts);
                                }
                            }
                            if let Some(command_timestamp_ms) = command_media_timestamp_ms(&command) {
                                let should_reset_pacing =
                                    frame.flags.contains(FrameFlags::DISCONTINUITY);
                                let pacing_delay = play_start_pacing.delay_for(
                                    command_timestamp_ms,
                                    now_micros,
                                    should_reset_pacing,
                                );
                                if !pacing_delay.is_zero()
                                    && wait_or_cancel(
                                        &runtime_api,
                                        &play_cancel_child,
                                        pacing_delay,
                                    )
                                    .await
                                {
                                    break;
                                }
                            }
                            // Paced sender: enforce minimum interval between sends.
                            if paced_sender.is_enabled() {
                                let send_delay = paced_sender.delay_for(
                                    runtime_now_micros(&runtime_api),
                                );
                                if !send_delay.is_zero()
                                    && wait_or_cancel(
                                        &runtime_api,
                                        &play_cancel_child,
                                        send_delay,
                                    )
                                    .await
                                {
                                    break;
                                }
                            }
                            if command_tx.send_core(connection_id, command).await.is_err() {
                                tracing::warn!(
                                    %connection_id,
                                    stream_id,
                                    stream_key = %stream_key_for_task,
                                    track_id = fields.track_id,
                                    codec = ?fields.codec,
                                    pts = fields.pts,
                                    dts = fields.dts,
                                    "play media send failed: command channel closed"
                                );
                                break;
                            }
                        } else {
                            let fields = frame_observability_fields(frame.as_ref());
                            tracing::warn!(
                                %connection_id,
                                stream_id,
                                stream_key = %stream_key_for_task,
                                track_id = fields.track_id,
                                media_kind = ?frame.media_kind,
                                codec = ?fields.codec,
                                pts = fields.pts,
                                dts = fields.dts,
                                "play frame mapping to RTMP command failed"
                            );
                        }

                        if enable_add_mute
                            && frame.media_kind == MediaKind::Video
                            && !track_list_has_audio(&current_tracks)
                        {
                            if let Some(mut mute_command) = maybe_make_mute_audio(
                                stream_id,
                                frame_dts_to_rtmp_timestamp_ms(frame.as_ref()),
                                &mut last_mute_ts,
                            ) {
                                rebase_play_media_command_timestamp(
                                    &mut mute_command,
                                    &mut play_timestamp_rebase,
                                );
                                clamp_media_command_timestamp(
                                    &mut mute_command,
                                    &mut last_media_timestamp,
                                );
                                if let Some(command_timestamp_ms) =
                                    command_media_timestamp_ms(&mute_command)
                                {
                                    let pacing_delay = play_start_pacing.delay_for(
                                        command_timestamp_ms,
                                        runtime_now_micros(&runtime_api),
                                        false,
                                    );
                                    if !pacing_delay.is_zero()
                                        && wait_or_cancel(
                                            &runtime_api,
                                            &play_cancel_child,
                                            pacing_delay,
                                        )
                                        .await
                                    {
                                        break;
                                    }
                                }
                                if command_tx
                                    .send_core(connection_id, mute_command)
                                    .await
                                    .is_err()
                                {
                                    let fields = frame_observability_fields(frame.as_ref());
                                    tracing::warn!(
                                        %connection_id,
                                        stream_id,
                                        stream_key = %stream_key_for_task,
                                        track_id = fields.track_id,
                                        codec = ?fields.codec,
                                        pts = fields.pts,
                                        dts = fields.dts,
                                        "play mute-audio send failed: command channel closed"
                                    );
                                    break;
                                }
                            }
                        }

                    }
                    Ok(None) => {
                        if force_close_on_source_end {
                            force_close_connection = true;
                        }
                        break;
                    }
                    Err(err) => {
                        tracing::warn!(
                            %connection_id,
                            stream_id,
                            stream_key = %stream_key_for_task,
                            "play subscriber recv failed: {err}"
                        );
                        if force_close_on_source_end {
                            force_close_connection = true;
                        }
                        break;
                    }
                }
            }
        }
    }

    let _ = subscriber.close().await;
    if force_close_connection {
        let _ = command_tx.close_connection(connection_id).await;
    } else {
        let _ = command_tx
            .send_core(connection_id, RtmpCoreCommand::CloseStream { stream_id })
            .await;
    }
}

/// Spawns a background task that waits for the source stream to become ready.
///
/// If the source does not appear in time, the play request is rejected.
///
/// 生成后台任务等待源流就绪。
///
/// 若源未在超时内出现，则拒绝播放请求。
fn spawn_pending_play(
    engine: EngineContext,
    config: RtmpModuleConfig,
    runtime_api: Arc<dyn RuntimeApi>,
    command_tx: RtmpCoreCommandSender,
    connection_id: RtmpConnectionId,
    stream_id: u32,
    route: StreamRoute,
) -> PlaySession {
    let play_cancel = CancellationToken::new();
    let play_cancel_child = play_cancel.child_token();
    let runtime_api_in_task = runtime_api.clone();
    let source_wait_timeout = pending_play_source_wait_timeout(&config);
    let startup_alert_threshold_ms = config.alert_thresholds.startup_timeout_ms;
    let join = runtime_api.spawn(Box::pin(async move {
        let mut source_missing_since_micros = runtime_now_micros(&runtime_api_in_task);
        let mut startup_alert_emitted = false;
        let snapshot = loop {
            if play_cancel_child.is_cancelled() {
                return;
            }

            let snapshot_opt = engine
                .stream_manager_api
                .get_stream(&route.stream_key)
                .await
                .ok()
                .flatten();
            if let Some(snapshot) = snapshot_opt {
                source_missing_since_micros = runtime_now_micros(&runtime_api_in_task);
                startup_alert_emitted = false;
                if track_list_ready_for_rtmp_play_bootstrap(&snapshot.tracks) {
                    break snapshot;
                }
                if !snapshot.tracks.is_empty()
                    && !track_list_has_supported_playback_codec(&snapshot.tracks)
                {
                    let _ = command_tx
                        .send_core(
                            connection_id,
                            RtmpCoreCommand::RejectPlay {
                                stream_id,
                                description: "stream has no RTMP/FLV playable media track"
                                    .to_string(),
                            },
                        )
                        .await;
                    runtime_sleep(&runtime_api_in_task, Duration::from_millis(50)).await;
                    let _ = command_tx.close_connection(connection_id).await;
                    return;
                }
            } else if let Some(timeout) = source_wait_timeout {
                let now = runtime_now_micros(&runtime_api_in_task);
                let wait_micros = now.saturating_sub(source_missing_since_micros);
                if !startup_alert_emitted && wait_micros / 1_000 >= startup_alert_threshold_ms {
                    startup_alert_emitted = true;
                    tracing::warn!(
                        %connection_id,
                        stream_key = %route.stream_key,
                        startup_elapsed_ms = wait_micros / 1_000,
                        startup_alert_threshold_ms,
                        source_wait_timeout_ms =
                            u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                        "rtmp play source wait exceeded startup alert threshold"
                    );
                }
                if now.saturating_sub(source_missing_since_micros) >= timeout.as_micros() as u64 {
                    let _ = command_tx
                        .send_core(
                            connection_id,
                            RtmpCoreCommand::RejectPlay {
                                stream_id,
                                description: "stream source not ready before timeout".to_string(),
                            },
                        )
                        .await;
                    runtime_sleep(&runtime_api_in_task, Duration::from_millis(50)).await;
                    let _ = command_tx.close_connection(connection_id).await;
                    return;
                }
            }
            let cancel_fut = play_cancel_child.cancelled().fuse();
            let sleep_fut = runtime_sleep(&runtime_api_in_task, Duration::from_millis(20)).fuse();
            pin_mut!(cancel_fut, sleep_fut);
            select_biased! {
                _ = cancel_fut => {
                    return;
                }
                _ = sleep_fut => {}
            }
        };

        run_play_stream(
            PlayTaskContext {
                engine: engine.clone(),
                config: config.clone(),
                runtime_api: runtime_api_in_task.clone(),
                command_tx: command_tx.clone(),
                connection_id,
                stream_id,
                route: route.clone(),
                subscribe_reject_description: Some("stream source unavailable during subscribe"),
            },
            snapshot.tracks,
            play_cancel_child,
        )
        .await;
    }));

    PlaySession {
        cancel: play_cancel,
        join,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_sdk::{PublishLease, PublisherSink, StreamKey};

    struct TestPublisherSink;

    impl PublisherSink for TestPublisherSink {
        fn update_tracks(&self, _tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            Ok(())
        }

        fn push_frame(
            &self,
            _frame: Arc<AVFrame>,
        ) -> Result<cheetah_sdk::DispatchResult, SdkError> {
            Ok(cheetah_sdk::DispatchResult::Accepted)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    #[test]
    fn frame_observability_fields_cover_required_keys() {
        let frame = AVFrame::new(
            TrackId(11),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            90_100,
            90_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x26]),
        );

        let fields = frame_observability_fields(&frame);
        assert_eq!(fields.track_id, 11);
        assert_eq!(fields.codec, CodecId::H265);
        assert_eq!(fields.pts, 90_100);
        assert_eq!(fields.dts, 90_000);
    }

    #[test]
    fn should_emit_alert_threshold_on_threshold_and_multiples() {
        assert!(!should_emit_alert_threshold(31, 32));
        assert!(should_emit_alert_threshold(32, 32));
        assert!(should_emit_alert_threshold(64, 32));
    }

    #[test]
    fn stream_key_strips_query_and_slashes() {
        let route = parse_stream_route("/live/", "/cam/main?token=abc");
        assert_eq!(route.stream_key.namespace, "live");
        assert_eq!(route.stream_key.path, "cam/main");
        assert_eq!(route.play_mode, RtmpPlayMode::Normal);
    }

    #[test]
    fn stream_key_parses_play_mode_from_query() {
        let route = parse_stream_route("/live/", "/cam/main?token=abc&type=enhanced");
        assert_eq!(route.stream_key.namespace, "live");
        assert_eq!(route.stream_key.path, "cam/main");
        assert_eq!(route.play_mode, RtmpPlayMode::Enhanced);
    }

    #[test]
    fn stream_key_strips_query_from_connect_app() {
        let route = parse_stream_route("/live?token=pub-session/", "/cam/main?token=sub-session");
        assert_eq!(route.stream_key.namespace, "live");
        assert_eq!(route.stream_key.path, "cam/main");
    }

    #[test]
    fn avcc_parameter_sets_are_extracted() {
        let avcc = [
            1, 0x64, 0x00, 0x1f, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1f, 0x01, 0x00, 0x02,
            0x68, 0xce,
        ];
        let (sps, pps) = parse_avcc_parameter_sets(&avcc);
        assert_eq!(sps.len(), 1);
        assert_eq!(pps.len(), 1);
        assert_eq!(sps[0], Bytes::from_static(&[0x67, 0x42, 0x00, 0x1f]));
        assert_eq!(pps[0], Bytes::from_static(&[0x68, 0xce]));
    }

    #[test]
    fn annexb_length_prefixed_roundtrip() {
        let annexb = Bytes::from_static(&[
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 1, 0x68, 4, 5, 0, 0, 1, 0x65, 6, 7,
        ]);
        let avcc = annexb_to_length_prefixed(&annexb);
        let back = length_prefixed_to_annexb(&avcc);
        let normalized = Bytes::from_static(&[
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5, 0, 0, 0, 1, 0x65, 6, 7,
        ]);
        assert_eq!(back, normalized);
    }

    #[test]
    fn annexb_length_prefixed_roundtrip_len_size_two() {
        let annexb = Bytes::from_static(&[
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 1, 0x68, 4, 5, 0, 0, 1, 0x65, 6, 7,
        ]);
        let avcc = annexb_to_length_prefixed_with_size(&annexb, 2);
        let back = length_prefixed_to_annexb_with_size(&avcc, 2);
        let normalized = Bytes::from_static(&[
            0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5, 0, 0, 0, 1, 0x65, 6, 7,
        ]);
        assert_eq!(back, normalized);
    }

    #[test]
    fn ingest_h264_uses_avcc_nal_length_size_two() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "len2_h264"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let avcc = [1, 0x64, 0x00, 0x1f, 0xfd, 0xe0, 0x00];
        apply_video_config(&mut session, CodecId::H264, &avcc);

        let mut payload = vec![0x17, 0x01, 0x00, 0x00, 0x00];
        payload.extend_from_slice(&[0x00, 0x03, 0x65, 0x88, 0x99, 0x00, 0x01, 0x06]);
        let frame = handle_video_ingest(&mut session, 0, &payload).expect("h264 frame");

        assert_eq!(
            frame.payload,
            Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88, 0x99, 0, 0, 0, 1, 0x06])
        );
        assert_eq!(
            frame.source_timestamp(),
            Some(SourceTimestamp::Rtmp(RtmpTimestamp::new(0, 0)))
        );
    }

    #[test]
    fn ingest_audio_attaches_rtmp_source_timestamp_side_data() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "audio_source_ts"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let frame =
            handle_audio_ingest(&mut session, 42, &[0xaf, 0x01, 0x11, 0x22]).expect("aac frame");
        assert_eq!(
            frame.source_timestamp(),
            Some(SourceTimestamp::Rtmp(RtmpTimestamp::new(42, 42)))
        );
    }

    #[test]
    fn egress_h264_uses_avcc_nal_length_size_one() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa, 0xbb, 0, 0, 1, 0x06]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: Vec::new(),
            pps: Vec::new(),
            avcc: Some(Bytes::from_static(&[1, 0x64, 0x00, 0x1f, 0xfc, 0xe0, 0x00])),
        };

        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                .expect("h264 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(&payload[0..5], &[0x17, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&payload[5..], &[0x03, 0x65, 0xaa, 0xbb, 0x01, 0x06]);
    }

    #[test]
    fn bootstrap_h264_sequence_header_uses_parameter_sets_without_avcc() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[
                0x67, 0x42, 0x00, 0x1f, 0x96, 0x54, 0x05, 0x01, 0xed, 0x00, 0xf0, 0x88, 0x45, 0x80,
            ])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x06, 0xe2])],
            avcc: None,
        };

        let commands =
            build_track_bootstrap_commands(1, &[track], RtmpPlayMode::Normal, false, false);
        let sequence = commands.iter().find_map(|command| match command {
            RtmpCoreCommand::SendVideo { payload, .. } => Some(payload),
            _ => None,
        });
        let payload = sequence.expect("h264 bootstrap video sequence header");
        assert_eq!(&payload[..2], &[0x17, 0x00]);
    }

    #[test]
    fn bootstrap_h265_sequence_header_uses_parameter_sets_without_hvcc() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        let vps = Bytes::from_static(&[0x40, 0x01, 0x0c, 0x01, 0xff]);
        let sps = Bytes::from_static(&[0x42, 0x01, 0x01, 0x01, 0x60]);
        let pps = Bytes::from_static(&[0x44, 0x01, 0xc0, 0xf1]);
        track.extradata = CodecExtradata::H265 {
            vps: vec![vps.clone()],
            sps: vec![sps.clone()],
            pps: vec![pps.clone()],
            hvcc: None,
        };

        let commands =
            build_track_bootstrap_commands(1, &[track], RtmpPlayMode::Normal, false, false);
        let payload = commands.iter().find_map(|command| match command {
            RtmpCoreCommand::SendVideo { payload, .. } => Some(payload),
            _ => None,
        });
        let payload = payload.expect("h265 bootstrap video sequence header");
        assert_eq!(payload[0], 0x90);
        assert_eq!(
            &payload[1..5],
            &rtmp_fourcc_from_codec(CodecId::H265)
                .expect("h265 fourcc")
                .to_be_bytes()
        );

        let (parsed_vps, parsed_sps, parsed_pps) =
            parse_hvcc_parameter_sets(&payload[5..], CodecId::H265);
        assert_eq!(parsed_vps, vec![vps]);
        assert_eq!(parsed_sps, vec![sps]);
        assert_eq!(parsed_pps, vec![pps]);
    }

    #[test]
    fn bootstrap_h265_without_hvcc_and_parameter_sets_skips_sequence_header() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        track.extradata = CodecExtradata::H265 {
            vps: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
            hvcc: None,
        };

        let commands =
            build_track_bootstrap_commands(1, &[track], RtmpPlayMode::Normal, false, false);
        let video_sequence = commands.iter().find(|command| {
            matches!(
                command,
                RtmpCoreCommand::SendVideo { payload, .. } if !payload.is_empty() && payload[0] == 0x90
            )
        });
        assert!(
            video_sequence.is_none(),
            "missing h265 config must not emit malformed sequence headers"
        );
    }

    #[test]
    fn bootstrap_enhanced_video_config_covers_vp8_vp9_av1() {
        let mut vp8 = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::VP8, 90_000);
        vp8.extradata = CodecExtradata::VP8 {
            config: Some(Bytes::from_static(&[0x01, 0x02])),
        };

        let mut vp9 = TrackInfo::new(TrackId(2), MediaKind::Video, CodecId::VP9, 90_000);
        vp9.extradata = CodecExtradata::VP9 {
            config: Some(Bytes::from_static(&[0x11, 0x22, 0x33])),
        };

        let mut av1 = TrackInfo::new(TrackId(3), MediaKind::Video, CodecId::AV1, 90_000);
        av1.extradata = CodecExtradata::AV1 {
            sequence_header: Some(Bytes::from_static(&[0xaa])),
            codec_config: Some(Bytes::from_static(&[0xbb, 0xcc])),
        };

        let commands = build_track_bootstrap_commands(
            1,
            &[vp8.clone(), vp9.clone(), av1.clone()],
            RtmpPlayMode::Normal,
            false,
            false,
        );

        let expected = [
            (CodecId::VP8, vec![0x01, 0x02]),
            (CodecId::VP9, vec![0x11, 0x22, 0x33]),
            (CodecId::AV1, vec![0xbb, 0xcc]),
        ];
        for (codec, config_tail) in expected {
            let fourcc = rtmp_fourcc_from_codec(codec).expect("fourcc").to_be_bytes();
            let payload = commands.iter().find_map(|command| match command {
                RtmpCoreCommand::SendVideo { payload, .. }
                    if payload.len() >= 5 && payload[0] == 0x90 && payload[1..5] == fourcc =>
                {
                    Some(payload)
                }
                _ => None,
            });
            let payload = payload.expect("enhanced bootstrap config payload");
            assert_eq!(&payload[5..], config_tail.as_slice());
        }
    }

    #[test]
    fn audio_only_play_refreshes_bootstrap_without_video_keyframe() {
        let audio_frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x11, 0x22]),
        );
        let audio_only_tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            48_000,
        )];
        assert!(should_refresh_play_bootstrap(
            &audio_frame,
            &audio_only_tracks
        ));

        let av_tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        assert!(!should_refresh_play_bootstrap(&audio_frame, &av_tracks));
    }

    #[test]
    fn video_keyframe_play_still_refreshes_bootstrap() {
        let mut video_frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        video_frame.flags.insert(FrameFlags::KEY);
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        assert!(should_refresh_play_bootstrap(&video_frame, &tracks));
    }

    #[test]
    fn incomplete_video_track_checks_refresh_on_delta_frame() {
        let video_frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            33,
            33,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x12, 0x00]),
        );
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::AV1,
            90_000,
        )];

        assert!(should_refresh_play_bootstrap(&video_frame, &tracks));
    }

    #[test]
    fn av1_track_requires_config_but_not_optional_fps_before_play_bootstrap() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);
        assert!(!track_list_ready_for_rtmp_play_bootstrap(&[track.clone()]));

        track.extradata = CodecExtradata::AV1 {
            sequence_header: Some(Bytes::from_static(&[0x0a, 0x01, 0x00])),
            codec_config: Some(Bytes::from_static(&[0x81, 0x00, 0x00, 0x00])),
        };
        assert!(track_list_ready_for_rtmp_play_bootstrap(&[track.clone()]));

        track.fps = Some(cheetah_codec::Rational32::new(30, 1));
        assert!(track_list_ready_for_rtmp_play_bootstrap(&[track]));
    }

    #[test]
    fn rtmp_play_start_gate_waits_for_video_keyframe_before_forwarding_audio_or_delta() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        let mut video_started = !rtmp_play_waits_for_video_keyframe(&tracks);

        let audio = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x11, 0x22]),
        );
        assert!(!should_forward_rtmp_play_frame(
            &tracks,
            &mut video_started,
            &audio
        ));
        assert!(!video_started);

        let delta = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            33,
            33,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0, 0, 0, 1, 0x41]),
        );
        assert!(!should_forward_rtmp_play_frame(
            &tracks,
            &mut video_started,
            &delta
        ));
        assert!(!video_started);

        let mut key = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            66,
            66,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        key.flags.insert(FrameFlags::KEY);
        assert!(should_forward_rtmp_play_frame(
            &tracks,
            &mut video_started,
            &key
        ));
        assert!(video_started);
    }

    #[test]
    fn rtmp_play_track_refresh_from_unknown_to_h264_rearms_keyframe_gate() {
        let unknown_tracks: Vec<TrackInfo> = Vec::new();
        let h264_tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let audio_tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            48_000,
        )];

        assert!(
            !initial_rtmp_play_video_started(&unknown_tracks),
            "unknown track state must keep startup gate closed"
        );
        assert!(
            initial_rtmp_play_video_started(&audio_tracks),
            "audio-only streams should not wait for a video keyframe gate"
        );
        assert!(
            !initial_rtmp_play_video_started(&h264_tracks),
            "h264 streams must wait for keyframe on startup"
        );

        let rearmed =
            reconcile_rtmp_play_video_started_on_track_refresh(true, &unknown_tracks, &h264_tracks);
        assert!(
            !rearmed,
            "transition from unknown tracks to h264 must re-arm keyframe gate"
        );

        let resumed = reconcile_rtmp_play_video_started_on_track_refresh(
            false,
            &unknown_tracks,
            &audio_tracks,
        );
        assert!(
            resumed,
            "transition to audio-only tracks should open playback immediately"
        );
    }

    #[test]
    fn rtmp_play_start_gate_allows_audio_only_streams() {
        let tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            48_000,
        )];
        let mut video_started = !rtmp_play_waits_for_video_keyframe(&tracks);
        let audio = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x11, 0x22]),
        );

        assert!(should_forward_rtmp_play_frame(
            &tracks,
            &mut video_started,
            &audio
        ));
    }

    #[test]
    fn h264_aac_play_bootstrap_covers_multi_second_gop() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];

        let max_frames = play_bootstrap_max_frames(&RtmpModuleConfig::default(), &tracks);

        assert!(
            max_frames >= 1024,
            "rtmp play bootstrap must cover multi-second GOPs with interleaved audio"
        );
    }

    #[test]
    fn play_bootstrap_floor_covers_all_rtmp_video_codecs() {
        let video_codecs = [
            (CodecId::H264, 1024usize),
            (CodecId::VP8, 1024usize),
            (CodecId::H265, 2048usize),
            (CodecId::H266, 2048usize),
            (CodecId::AV1, 2048usize),
            (CodecId::VP9, 2048usize),
        ];

        for (codec, expected_floor) in video_codecs {
            let tracks = vec![TrackInfo::new(TrackId(1), MediaKind::Video, codec, 90_000)];
            let max_frames = play_bootstrap_max_frames(&RtmpModuleConfig::default(), &tracks);
            assert!(
                max_frames >= expected_floor,
                "{codec:?} bootstrap window should be at least {expected_floor}"
            );
        }
    }

    #[test]
    fn push_bootstrap_uses_video_floor_when_tracks_unknown() {
        let config = RtmpModuleConfig::default();
        let bootstrap_max_frames = push_bootstrap_max_frames(&config, &[]);
        let queue_capacity = play_subscriber_queue_capacity(&config, bootstrap_max_frames);

        assert!(
            bootstrap_max_frames >= 1024,
            "push bootstrap should keep a video-class GOP window before tracks are announced"
        );
        assert!(
            queue_capacity >= bootstrap_max_frames,
            "push queue capacity must cover push bootstrap window"
        );
    }

    #[test]
    fn play_subscriber_queue_capacity_covers_bootstrap_window() {
        let config = RtmpModuleConfig::default();
        let bootstrap_max_frames = 1024;

        let queue_capacity = play_subscriber_queue_capacity(&config, bootstrap_max_frames);

        assert!(
            queue_capacity >= bootstrap_max_frames,
            "play subscriber queue must not truncate GOP bootstrap frames"
        );
    }

    #[test]
    fn egress_h265_uses_hvcc_nal_length_size_two() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0, 0, 0, 1, 0x26, 0x01]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let mut hvcc = [0u8; 22];
        hvcc[21] = 0xfd;
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
        track.extradata = CodecExtradata::H265 {
            vps: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
            hvcc: Some(Bytes::copy_from_slice(&hvcc)),
        };

        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                .expect("h265 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        let header = parse_video_ingress_header(&payload).expect("h265 header");
        assert_eq!(header.codec, CodecId::H265);
        assert_eq!(header.payload_offset, 8);
        assert_eq!(&payload[header.payload_offset..], &[0x00, 0x02, 0x26, 0x01]);
    }

    #[test]
    fn vp8_and_h265_default_to_enhanced_mode() {
        assert!(use_enhanced_video_mode(RtmpPlayMode::Normal, CodecId::VP8));
        assert!(use_enhanced_video_mode(RtmpPlayMode::Normal, CodecId::H265));
    }

    #[test]
    fn h265_egress_normal_mode_avoids_negative_composition_time() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            100,
            160,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01]),
        );
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("h265 video");
        let RtmpCoreCommand::SendVideo {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 100);
        let header = parse_video_ingress_header(&payload).expect("h265 header");
        assert_eq!(header.codec, CodecId::H265);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn h265_egress_enhanced_mode_preserves_negative_composition_time() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            100,
            160,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01]),
        );
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Enhanced).expect("h265 video");
        let RtmpCoreCommand::SendVideo {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 160);
        let header = parse_video_ingress_header(&payload).expect("h265 header");
        assert_eq!(header.codec, CodecId::H265);
        assert_eq!(header.cts, -60);
    }

    #[test]
    fn parses_enhanced_vp8_ingress() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::VP8).expect("vp8 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x00,
            0x00,
            0x01,
            0x12,
            0x34,
        ];
        let header = parse_video_ingress_header(&payload).expect("vp8 header");
        assert_eq!(header.codec, CodecId::VP8);
        assert_eq!(header.payload_offset, 8);
        assert_eq!(header.cts, 1);
    }

    #[test]
    fn parses_enhanced_vp8_ingress_without_cts_keeps_payload_prefix() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::VP8).expect("vp8 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x11,
            0x22,
            0x33,
            0x44,
        ];
        let header = parse_video_ingress_header(&payload).expect("vp8 header without cts");
        assert_eq!(header.codec, CodecId::VP8);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn parses_enhanced_h264_ingress_packet_type_1_with_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::H264).expect("h264 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0xff,
            0xff,
            0xfe,
            0x65,
            0x88,
        ];
        let header = parse_video_ingress_header(&payload).expect("h264 header");
        assert_eq!(header.codec, CodecId::H264);
        assert_eq!(header.payload_offset, 8);
        assert_eq!(header.cts, -2);
        assert_eq!(&payload[header.payload_offset..], &[0x65, 0x88]);
    }

    #[test]
    fn rejects_short_non_enhanced_video_config_packet() {
        let payload = [0x17, 0x00, 0x00, 0x00];
        assert!(parse_video_ingress_header(&payload).is_none());
    }

    #[test]
    fn parses_enhanced_vp9_ingress_packet_type_1_with_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::VP9).expect("vp9 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x00,
            0x01,
            0x2c,
            0x82,
            0x00,
        ];
        let header = parse_video_ingress_header(&payload).expect("vp9 header");
        assert_eq!(header.codec, CodecId::VP9);
        assert_eq!(header.payload_offset, 8);
        assert_eq!(header.cts, 300);
    }

    #[test]
    fn parses_enhanced_vp9_ingress_packet_type_1_without_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::VP9).expect("vp9 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x82,
            0x49,
            0x83,
            0x00,
        ];
        let header = parse_video_ingress_header(&payload).expect("vp9 header without cts");
        assert_eq!(header.codec, CodecId::VP9);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn parses_enhanced_vp9_ingress_ambiguous_payload_as_without_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::VP9).expect("vp9 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x82,
            0x49,
            0x83,
            0x82,
            0x00,
        ];
        let header = parse_video_ingress_header(&payload).expect("vp9 ambiguous header");
        assert_eq!(header.codec, CodecId::VP9);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn parses_enhanced_av1_ingress_packet_type_1_with_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x00,
            0x00,
            0x03,
            0x0a,
            0x01,
            0x4a,
        ];
        let header = parse_video_ingress_header(&payload).expect("av1 header");
        assert_eq!(header.codec, CodecId::AV1);
        assert_eq!(header.payload_offset, 8);
        assert_eq!(header.cts, 3);
    }

    #[test]
    fn parses_enhanced_av1_ingress_packet_type_1_without_cts() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x0a,
            0x01,
            0xaa,
        ];
        let header = parse_video_ingress_header(&payload).expect("av1 header without cts");
        assert_eq!(header.codec, CodecId::AV1);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn parses_enhanced_av1_ingress_packet_type_4_as_frame_payload() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x94,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x02,
            0x00,
            0x09,
        ];
        let header = parse_video_ingress_header(&payload).expect("av1 packet_type=4 header");
        assert_eq!(header.codec, CodecId::AV1);
        assert_eq!(header.packet_type, 4);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn builds_av1_enhanced_config_without_prefix_injection() {
        let payload = build_video_config_payload(CodecId::AV1, &[0x11, 0x22], RtmpPlayMode::Normal)
            .expect("av1 config");
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc");
        assert_eq!(payload[0], 0x90);
        assert_eq!(&payload[1..5], &fourcc.to_be_bytes());
        assert_eq!(&payload[5..], &[0x11, 0x22]);
    }

    #[test]
    fn vp9_config_payload_preserves_enhanced_prefix() {
        let raw = [0x01, 0x00, 0x00, 0x00, 0x00, 0x28, 0x80];
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "cam/main"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        apply_video_config(&mut session, CodecId::VP9, &raw);
        let track = session.tracks.video.expect("video track");
        let CodecExtradata::VP9 { config } = track.extradata else {
            panic!("expected vp9 extradata");
        };
        assert_eq!(config.expect("vp9 config"), Bytes::copy_from_slice(&raw));
    }

    #[test]
    fn h265_empty_payload_uses_enhanced_packet_type_3() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::new(),
        );
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("h265 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(payload, Bytes::from_static(&[0xa3, b'h', b'v', b'c', b'1']));
    }

    #[test]
    fn empty_h265_enhanced_frame_is_not_marked_as_key() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::H265).expect("h265 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let payload = [
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x00,
            0x00,
            0x64,
        ];
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "cam/main"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let frame = handle_video_ingest(&mut session, 33, &payload).expect("video frame");
        assert!(frame.payload.is_empty());
        assert!(!frame.is_key_frame());
    }

    #[test]
    fn ingest_video_does_not_trust_mislabelled_rtmp_key_flag() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "mislabelled_h264"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x17, 0x01, 0x00, 0x00, 0x00, 0, 0, 0, 2, 0x41, 0x88];
        let frame = handle_video_ingest(&mut session, 33, &payload).expect("video frame");

        assert_eq!(frame.payload.as_ref(), &[0, 0, 0, 1, 0x41, 0x88]);
        assert!(!frame.is_key_frame());
    }

    #[test]
    fn ingest_video_marks_key_only_after_payload_random_access_verification() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "verified_h264"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x17, 0x01, 0x00, 0x00, 0x00, 0, 0, 0, 2, 0x65, 0x88];
        let frame = handle_video_ingest(&mut session, 33, &payload).expect("video frame");

        assert!(frame.is_key_frame());
    }

    #[test]
    fn ingest_enhanced_av1_does_not_trust_mislabelled_key_flag() {
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc");
        let fourcc_bytes = fourcc.to_be_bytes();
        let mut payload = vec![
            0x91,
            fourcc_bytes[0],
            fourcc_bytes[1],
            fourcc_bytes[2],
            fourcc_bytes[3],
            0x1a,
            0x01,
            0x40,
        ];
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "mislabelled_av1"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let frame = handle_video_ingest(&mut session, 33, &payload).expect("video frame");
        assert!(!frame.is_key_frame());

        payload[7] = 0x00;
        let key = handle_video_ingest(&mut session, 66, &payload).expect("video frame");
        assert!(key.is_key_frame());
    }

    #[test]
    fn ingest_video_repairs_backward_timestamps_to_monotonic_dts() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "video_monotonic"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x27, 0x01, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x65];

        let first = handle_video_ingest(&mut session, 100, &payload).expect("first frame");
        let second = handle_video_ingest(&mut session, 90, &payload).expect("second frame");

        assert!(second.dts > first.dts, "video dts should stay monotonic");
        assert_eq!(first.pts - first.dts, 20);
        assert_eq!(second.pts - second.dts, 20);
    }

    #[test]
    fn ingest_audio_repairs_backward_timestamps_to_monotonic_pts_dts() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "audio_monotonic"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x2f, 0xaa, 0xbb];

        let first = handle_audio_ingest(&mut session, 100, &payload).expect("first frame");
        let second = handle_audio_ingest(&mut session, 90, &payload).expect("second frame");

        assert!(second.dts > first.dts, "audio dts should stay monotonic");
        assert_eq!(second.pts, second.dts, "audio pts/dts should stay aligned");
    }

    #[test]
    fn ingest_video_wraparound_keeps_monotonic_dts() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "video_wrap"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x27, 0x01, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x65];

        let first = handle_video_ingest(&mut session, u32::MAX - 10, &payload).expect("first");
        let second = handle_video_ingest(&mut session, 20, &payload).expect("second");

        assert!(
            second.dts > first.dts,
            "video dts should stay monotonic across u32 wrap"
        );
    }

    #[test]
    fn ingest_video_large_backward_reset_restarts_timeline_with_discontinuity() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "video_reset"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x27, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x65];

        let first = handle_video_ingest(&mut session, 120_000, &payload).expect("first");
        let second = handle_video_ingest(&mut session, 0, &payload).expect("second");

        assert_eq!(first.dts, 0);
        assert_eq!(second.dts, 0);
        assert!(
            second.flags.contains(FrameFlags::DISCONTINUITY),
            "timestamp reset should be marked as discontinuity"
        );
    }

    #[test]
    fn ingest_h264_long_run_keeps_monotonic_timeline_for_ten_minutes() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "long_run_h264"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        // Non-enhanced H264 video payload with cts=0 and one short NALU.
        let payload = [0x27, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x65];
        let total_frames = (10 * 60 * 1000) / 33;
        let mut last_dts = None;

        for i in 0..total_frames {
            let frame = handle_video_ingest(&mut session, (i * 33) as u32, &payload)
                .expect("h264 frame in long run");
            if let Some(previous) = last_dts {
                assert!(
                    frame.dts > previous,
                    "h264 dts must stay strictly monotonic in long run"
                );
            }
            assert_eq!(frame.pts, frame.dts, "cts=0 should keep pts==dts");
            last_dts = Some(frame.dts);
        }
    }

    #[test]
    fn ingest_audio_monotonic_repair_covers_aac_opus_g711_mp3() {
        let cases: [(&str, [u8; 3]); 4] = [
            ("aac", [0xaf, 0x01, 0x55]),
            ("opus", [0xd3, 0x01, 0x02]),
            ("g711a", [0x7f, 0x01, 0x02]),
            ("mp3", [0x2f, 0xaa, 0xbb]),
        ];
        for (name, payload) in cases {
            let mut session = PublishSession {
                lease: PublishLease {
                    stream_id: cheetah_sdk::StreamId(1),
                    stream_key: StreamKey::new("live", format!("audio_monotonic_{name}")),
                    lease_id: 1,
                },
                sink: Box::new(TestPublisherSink),
                tracks: PublishTracks::default(),
                timestamp_states: PublishTimestampStates::default(),
                fps_estimator: FrameRateEstimator::default(),
            };

            let first = handle_audio_ingest(&mut session, 100, &payload).expect("first frame");
            let second = handle_audio_ingest(&mut session, 90, &payload).expect("second frame");

            assert!(second.dts > first.dts, "{name} dts should stay monotonic");
            assert_eq!(second.pts, second.dts, "{name} pts/dts should stay aligned");
            assert!(
                !second.flags.contains(FrameFlags::DISCONTINUITY),
                "{name} small timestamp repair must not be marked discontinuity"
            );
        }
    }

    #[test]
    fn ingest_audio_derives_duration_from_sample_rate_and_samples() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "audio_duration"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        let _ = handle_audio_ingest(&mut session, 0, &[0xaf, 0x00, 0x11, 0x90]);
        let aac = handle_audio_ingest(&mut session, 20, &[0xaf, 0x01, 0x55]).expect("aac frame");
        assert_eq!(aac.duration, 21, "aac 1024/48k should be ~21ms");

        let mut g711_payload = vec![0x70];
        g711_payload.extend(std::iter::repeat_n(0u8, 160));
        let g711 = handle_audio_ingest(&mut session, 20, &g711_payload).expect("g711 frame");
        assert_eq!(g711.duration, 20, "g711 160 samples@8k should be 20ms");

        let mp3 = handle_audio_ingest(&mut session, 46, &[0x2f, 0xaa, 0xbb]).expect("mp3 frame");
        assert_eq!(mp3.duration, 26, "mp3 1152/44.1k should be ~26ms");
    }

    #[test]
    fn aac_sequence_header_with_pce_derives_multichannel_layout() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "aac_pce_channels"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        let sequence_header = [
            0xaf, 0x00, 0x11, 0x80, 0x04, 0xc8, 0x44, 0x00, 0x20, 0x00, 0xc4, 0x0c, 0x4c, 0x61,
            0x76, 0x63, 0x36, 0x31, 0x2e, 0x33, 0x2e, 0x31, 0x30, 0x30, 0x56, 0xe5, 0x00,
        ];

        assert!(handle_audio_ingest(&mut session, 0, &sequence_header).is_none());
        let track = session.tracks.audio.as_ref().expect("audio track");
        assert_eq!(track.sample_rate, Some(48_000));
        assert_eq!(track.channels, Some(6));
        assert_eq!(track.clock_rate, 48_000);
    }

    #[test]
    fn map_non_h264_video_prefers_raw_rtmp_payload_side_data() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01]),
        );
        let raw_payload = [0x94, b'h', b'v', b'c', b'1', 0x02, 0x00, 0x09];
        attach_raw_rtmp_video_payload(&mut frame, &raw_payload);
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("h265 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(payload, Bytes::copy_from_slice(&raw_payload));
    }

    #[test]
    fn map_frame_to_rtmp_emits_vp8_video() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::VP8,
            FrameFormat::CanonicalVp8Frame,
            100,
            100,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x10, 0x00, 0x00, 0x9d, 0x01, 0x2a]),
        );
        frame.flags.insert(FrameFlags::KEY);
        let command = map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[])
            .expect("vp8 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        let header = parse_video_ingress_header(&payload).expect("vp8 header");
        assert_eq!(header.codec, CodecId::VP8);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
        assert_eq!(&payload[5..], &[0x10, 0x00, 0x00, 0x9d, 0x01, 0x2a]);
    }

    #[test]
    fn enhanced_vp9_egress_payload_does_not_insert_composition_time_prefix() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            105,
            100,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x82, 0x49, 0x83, 0x42, 0x00, 0x11]),
        );
        frame.flags.insert(FrameFlags::KEY);
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("vp9 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        let header = parse_video_ingress_header(&payload).expect("vp9 header");
        assert_eq!(header.codec, CodecId::VP9);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
        assert_eq!(&payload[5..8], &[0x82, 0x49, 0x83]);
        assert_eq!(
            &payload[header.payload_offset..header.payload_offset + 3],
            &[0x82, 0x49, 0x83]
        );
    }

    #[test]
    fn h264_egress_normal_mode_avoids_negative_composition_time() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            100,
            160,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65]),
        );
        let command = map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[])
            .expect("h264 video");
        let RtmpCoreCommand::SendVideo {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 100);
        let header = parse_video_ingress_header(&payload).expect("h264 header");
        assert_eq!(header.codec, CodecId::H264);
        assert_eq!(header.cts, 0);
        assert_eq!(&payload[2..5], &[0x00, 0x00, 0x00]);
    }

    #[test]
    fn h264_egress_enhanced_mode_preserves_negative_composition_time() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            100,
            160,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65]),
        );
        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Enhanced, &[])
                .expect("h264 video");
        let RtmpCoreCommand::SendVideo {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 160);
        let header = parse_video_ingress_header(&payload).expect("h264 header");
        assert_eq!(header.codec, CodecId::H264);
        assert_eq!(header.cts, -60);
        assert_eq!(&payload[2..5], &[0xff, 0xff, 0xc4]);
    }

    #[test]
    fn h264_egress_fast_pts_mode_keeps_canonical_dts_timestamp() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            135,
            100,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65]),
        );
        let normal =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame.clone()), RtmpPlayMode::Normal, &[])
                .expect("h264 normal");
        let fast = map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::FastPts, &[])
            .expect("h264 fast");
        let (
            RtmpCoreCommand::SendVideo {
                timestamp_ms: normal_ts,
                ..
            },
            RtmpCoreCommand::SendVideo {
                timestamp_ms: fast_ts,
                ..
            },
        ) = (normal, fast)
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(normal_ts, 100);
        assert_eq!(fast_ts, normal_ts);
    }

    #[test]
    fn enhanced_vp9_egress_negative_composition_time_does_not_prefix_payload() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            100,
            160,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x82, 0x49, 0x83]),
        );
        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("vp9 video");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected SendVideo");
        };
        let header = parse_video_ingress_header(&payload).expect("vp9 header");
        assert_eq!(header.codec, CodecId::VP9);
        assert_eq!(header.cts, 0);
        assert_eq!(&payload[5..8], &[0x82, 0x49, 0x83]);
    }

    #[test]
    fn track_list_playback_support_accepts_opus_only() {
        let tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::Opus,
            48_000,
        )];
        assert!(track_list_has_supported_playback_codec(&tracks));
    }

    #[test]
    fn track_list_playback_support_rejects_unknown_audio_only() {
        let tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::Unknown,
            48_000,
        )];
        assert!(!track_list_has_supported_playback_codec(&tracks));
    }

    #[test]
    fn track_list_playback_support_accepts_vp9_aac() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::VP9, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        assert!(track_list_has_supported_playback_codec(&tracks));
    }

    #[test]
    fn track_list_playback_support_accepts_vp8_and_h266() {
        let vp8 = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::VP8,
            90_000,
        )];
        let h266 = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H266,
            90_000,
        )];
        assert!(track_list_has_supported_playback_codec(&vp8));
        assert!(track_list_has_supported_playback_codec(&h266));
    }

    #[test]
    fn opus_ingest_uses_codec_sample_rate_not_flv_rate_bits() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "opus_rate"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0xD3, 0x01, 0x02];
        let _ = handle_audio_ingest(&mut session, 0, &payload);
        let track = session.tracks.audio.expect("audio track");
        assert_eq!(track.codec, CodecId::Opus);
        assert_eq!(track.sample_rate, Some(48_000));
        assert_eq!(track.clock_rate, 48_000);
    }

    #[test]
    fn enhanced_rtmp_opus_ingest_uses_opus_head_and_coded_frames() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "enhanced_opus"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        let sequence_header = [
            0x90, b'O', b'p', b'u', b's', b'O', b'p', b'u', b's', b'H', b'e', b'a', b'd', 1, 2,
            0x38, 0x01, 0x80, 0xbb, 0x00, 0x00, 0, 0, 0,
        ];
        assert!(handle_audio_ingest(&mut session, 0, &sequence_header).is_none());

        let track = session.tracks.audio.as_ref().expect("opus track");
        assert_eq!(track.codec, CodecId::Opus);
        assert_eq!(track.sample_rate, Some(48_000));
        assert_eq!(track.channels, Some(2));
        assert_eq!(track.clock_rate, 48_000);

        let frame = handle_audio_ingest(
            &mut session,
            20,
            &[0x91, b'O', b'p', b'u', b's', 0xfc, 0x7b, 0x19],
        )
        .expect("opus coded frame");
        assert_eq!(frame.codec, CodecId::Opus);
        assert_eq!(frame.format, FrameFormat::OpusPacket);
        assert_eq!(&frame.payload[..], &[0xfc, 0x7b, 0x19]);
    }

    #[test]
    fn g711_ingest_uses_codec_sample_rate_not_flv_rate_bits() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "g711_rate"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        let payload = [0x7F, 0x01, 0x02];
        let _ = handle_audio_ingest(&mut session, 0, &payload);
        let track = session.tracks.audio.expect("audio track");
        assert_eq!(track.codec, CodecId::G711A);
        assert_eq!(track.sample_rate, Some(8_000));
        assert_eq!(track.clock_rate, 8_000);
    }

    #[test]
    fn mp3_ingest_overrides_stale_metadata_audio_rate() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "mp3_rate_override"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        let metadata = AmfValue::amf0_object([
            ("audiocodecid", WireAmf0Value::Number(10.0)),
            ("audiosamplerate", WireAmf0Value::Number(48_000.0)),
        ]);
        apply_metadata_to_tracks(
            &mut session,
            &[
                AmfValue::Amf0(WireAmf0Value::String("onMetaData".to_string())),
                metadata,
            ],
        );

        // MP3 + rate_code=1 -> 11_025 Hz
        let _ = handle_audio_ingest(&mut session, 0, &[0x27, 0x00]);

        let track = session.tracks.audio.expect("audio track");
        assert_eq!(track.codec, CodecId::MP3);
        assert_eq!(track.sample_rate, Some(11_025));
        assert_eq!(track.clock_rate, 11_025);
    }

    #[test]
    fn mp3_ingest_overrides_stale_metadata_channels() {
        let mut session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "mp3_channels_override"),
                lease_id: 1,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks::default(),
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };

        let metadata = AmfValue::amf0_object([
            ("audiocodecid", WireAmf0Value::Number(10.0)),
            ("audiosamplerate", WireAmf0Value::Number(48_000.0)),
            ("stereo", WireAmf0Value::Boolean(true)),
        ]);
        apply_metadata_to_tracks(
            &mut session,
            &[
                AmfValue::Amf0(WireAmf0Value::String("onMetaData".to_string())),
                metadata,
            ],
        );

        // MP3 + mono tag (sound_type=0)
        let _ = handle_audio_ingest(&mut session, 0, &[0x26, 0x00]);

        let track = session.tracks.audio.expect("audio track");
        assert_eq!(track.codec, CodecId::MP3);
        assert_eq!(track.channels, Some(1));
    }

    #[test]
    fn mp3_egress_preserves_track_rate_and_channels_in_audio_flag() {
        let frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::MP3,
            FrameFormat::Mp3Frame,
            0,
            0,
            Timebase::new(1, 1000),
            Bytes::from_static(&[0x11, 0x22]),
        );
        let mut track = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::MP3, 11_025);
        track.sample_rate = Some(11_025);
        track.channels = Some(1);

        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                .expect("mp3 audio");
        let RtmpCoreCommand::SendAudio { payload, .. } = command else {
            panic!("expected SendAudio");
        };
        assert_eq!(payload[0], 0x26);
        assert_eq!(&payload[1..], &[0x11, 0x22]);
    }

    #[test]
    fn rtsp_timebase_is_normalized_to_rtmp_milliseconds_on_egress() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            12_000,
            9_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88, 0x84]),
        );
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);

        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                .expect("h264 video");
        let RtmpCoreCommand::SendVideo {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendVideo");
        };

        assert_eq!(timestamp_ms, 100);
        assert_eq!([payload[2], payload[3], payload[4]], [0x00, 0x00, 0x21]);
    }

    #[test]
    fn rtsp_video_timebase_normalization_covers_codec_matrix() {
        let cases = [
            (
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                Bytes::from_static(&[0, 0, 0, 1, 0x65, 0x88]),
                true,
            ),
            (
                CodecId::H265,
                FrameFormat::CanonicalH26x,
                Bytes::from_static(&[0, 0, 0, 1, 0x26, 0x01]),
                true,
            ),
            (
                CodecId::H266,
                FrameFormat::CanonicalH26x,
                Bytes::from_static(&[0, 0, 0, 1, 0x00, 0x80, 0x01]),
                true,
            ),
            (
                CodecId::AV1,
                FrameFormat::CanonicalAv1Obu,
                Bytes::from_static(&[0x18, 0x00, 0x12, 0x34]),
                true,
            ),
            (
                CodecId::VP8,
                FrameFormat::CanonicalVp8Frame,
                Bytes::from_static(&[0x9d, 0x01, 0x2a]),
                true,
            ),
            (
                CodecId::VP9,
                FrameFormat::CanonicalVp9Frame,
                Bytes::from_static(&[0x82, 0x49, 0x83]),
                true,
            ),
        ];

        for (codec, format, payload, key_frame) in cases {
            let mut frame = AVFrame::new(
                TrackId(11),
                MediaKind::Video,
                codec,
                format,
                12_000,
                9_000,
                Timebase::new(1, 90_000),
                payload,
            );
            if key_frame {
                frame.flags.insert(FrameFlags::KEY);
            }

            let track = TrackInfo::new(TrackId(11), MediaKind::Video, codec, 90_000);
            let command =
                map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                    .unwrap_or_else(|| panic!("missing RTMP command for {codec:?}"));

            let RtmpCoreCommand::SendVideo {
                timestamp_ms,
                payload,
                ..
            } = command
            else {
                panic!("expected SendVideo for {codec:?}");
            };

            assert_eq!(timestamp_ms, 100, "{codec:?} must normalize to 100ms");
            assert!(
                !payload.is_empty(),
                "{codec:?} must emit a decodable RTMP video payload"
            );
        }
    }

    #[test]
    fn rtsp_audio_timebase_normalization_covers_codec_matrix() {
        let cases = [
            (CodecId::AAC, FrameFormat::AacRaw, 48_000u32),
            (CodecId::G711A, FrameFormat::G711Packet, 8_000u32),
            (CodecId::G711U, FrameFormat::G711Packet, 8_000u32),
            (CodecId::MP3, FrameFormat::Mp3Frame, 11_025u32),
            (CodecId::Opus, FrameFormat::OpusPacket, 48_000u32),
        ];

        for (codec, format, clock_rate) in cases {
            let frame = AVFrame::new(
                TrackId(21),
                MediaKind::Audio,
                codec,
                format,
                i64::from(clock_rate / 10),
                i64::from(clock_rate / 10),
                Timebase::new(1, clock_rate),
                Bytes::from_static(&[0x11, 0x22]),
            );
            let mut track = TrackInfo::new(TrackId(21), MediaKind::Audio, codec, clock_rate);
            if codec == CodecId::MP3 {
                track.sample_rate = Some(clock_rate);
                track.channels = Some(1);
            }

            let command =
                map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                    .unwrap_or_else(|| panic!("missing RTMP command for {codec:?}"));

            let RtmpCoreCommand::SendAudio {
                timestamp_ms,
                payload,
                ..
            } = command
            else {
                panic!("expected SendAudio for {codec:?}");
            };

            assert_eq!(timestamp_ms, 100, "{codec:?} must normalize to 100ms");
            assert!(
                payload.len() >= 2,
                "{codec:?} audio payload must include header + media bytes"
            );
        }
    }

    #[test]
    fn rtsp_opus_audio_exports_enhanced_rtmp_audio() {
        let frame = AVFrame::new(
            TrackId(21),
            MediaKind::Audio,
            CodecId::Opus,
            FrameFormat::OpusPacket,
            4_800,
            4_800,
            Timebase::new(1, 48_000),
            Bytes::from_static(&[0x11, 0x22]),
        );
        let track = TrackInfo::new(TrackId(21), MediaKind::Audio, CodecId::Opus, 48_000);

        let command =
            map_frame_to_rtmp_with_tracks(1, Arc::new(frame), RtmpPlayMode::Normal, &[track])
                .expect("opus enhanced audio");
        let RtmpCoreCommand::SendAudio {
            timestamp_ms,
            payload,
            ..
        } = command
        else {
            panic!("expected SendAudio");
        };

        assert_eq!(timestamp_ms, 100);
        assert_eq!(payload[0], 0x91);
        assert_eq!(&payload[1..5], b"Opus");
        assert_eq!(&payload[5..], &[0x11, 0x22]);
    }

    #[test]
    fn opus_bootstrap_emits_enhanced_rtmp_sequence_header() {
        let mut track = TrackInfo::new(TrackId(21), MediaKind::Audio, CodecId::Opus, 48_000);
        track.sample_rate = Some(48_000);
        track.channels = Some(2);
        track.extradata = CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: None,
        };

        let commands =
            build_track_bootstrap_commands(1, &[track], RtmpPlayMode::Normal, false, false);
        let RtmpCoreCommand::SendAudio { payload, .. } = commands
            .iter()
            .find(|command| matches!(command, RtmpCoreCommand::SendAudio { .. }))
            .expect("opus sequence header")
        else {
            panic!("expected SendAudio");
        };

        assert_eq!(payload[0], 0x90);
        assert_eq!(&payload[1..5], b"Opus");
        assert_eq!(&payload[5..13], b"OpusHead");
        assert_eq!(payload[13], 1);
        assert_eq!(payload[14], 2);
        assert_eq!(&payload[15..17], &312u16.to_le_bytes());
        assert_eq!(&payload[17..21], &48_000u32.to_le_bytes());
    }

    #[test]
    fn track_list_has_codec_detects_vp9() {
        let tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::VP9, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        assert!(track_list_has_codec(&tracks, CodecId::VP9));
        assert!(!track_list_has_codec(&tracks, CodecId::H266));
    }

    #[test]
    fn play_accept_flags_track_driven() {
        let av_tracks = vec![
            TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000),
            TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000),
        ];
        let audio_tracks = vec![TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            48_000,
        )];

        assert_eq!(play_accept_flags(&av_tracks), (true, true));
        assert_eq!(play_accept_flags(&audio_tracks), (true, true));
    }

    #[test]
    fn delay_release_for_h264_publish() {
        let session = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "h264_demo"),
                lease_id: 9,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks {
                video: Some(TrackInfo::new(
                    TrackId(1),
                    MediaKind::Video,
                    CodecId::H264,
                    90_000,
                )),
                audio: None,
            },
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        assert!(should_delay_publish_release_for_h264(&session));
    }

    #[test]
    fn no_delay_release_for_non_h264_publish() {
        let h264 = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "h264_aac"),
                lease_id: 9,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks {
                video: Some(TrackInfo::new(
                    TrackId(1),
                    MediaKind::Video,
                    CodecId::H264,
                    90_000,
                )),
                audio: None,
            },
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        assert!(should_delay_publish_release_for_h264(&h264));

        let h265 = PublishSession {
            lease: PublishLease {
                stream_id: cheetah_sdk::StreamId(1),
                stream_key: StreamKey::new("live", "h265_demo"),
                lease_id: 9,
            },
            sink: Box::new(TestPublisherSink),
            tracks: PublishTracks {
                video: Some(TrackInfo::new(
                    TrackId(1),
                    MediaKind::Video,
                    CodecId::H265,
                    90_000,
                )),
                audio: None,
            },
            timestamp_states: PublishTimestampStates::default(),
            fps_estimator: FrameRateEstimator::default(),
        };
        assert!(!should_delay_publish_release_for_h264(&h265));
    }

    #[test]
    fn clamp_media_timestamp_repairs_small_backward_jitter_without_freezing_followups() {
        let mut command = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 900,
            payload: Bytes::new(),
        };
        let mut last = MediaTimestampState {
            video_last_ms: Some(1_000),
            ..MediaTimestampState::default()
        };
        clamp_media_command_timestamp(&mut command, &mut last);

        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 1_001);
        assert_eq!(last.video_last_ms, Some(1_001));

        let mut next = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 933,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut next, &mut last);

        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = next else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 1_002);
        assert_eq!(last.video_last_ms, Some(1_002));
    }

    #[test]
    fn clamp_media_timestamp_allows_u32_wraparound_progress() {
        let mut command = RtmpCoreCommand::SendAudio {
            stream_id: 1,
            timestamp_ms: 12,
            payload: Bytes::new(),
        };
        let mut last = MediaTimestampState {
            audio_last_ms: Some(u32::MAX - 5),
            ..MediaTimestampState::default()
        };
        clamp_media_command_timestamp(&mut command, &mut last);

        let RtmpCoreCommand::SendAudio { timestamp_ms, .. } = command else {
            panic!("expected SendAudio");
        };
        assert_eq!(timestamp_ms, 12);
        assert_eq!(last.audio_last_ms, Some(12));
    }

    #[test]
    fn clamp_media_timestamp_accepts_large_backward_reset() {
        let mut command = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 0,
            payload: Bytes::new(),
        };
        let mut last = MediaTimestampState {
            video_last_ms: Some(120_000),
            ..MediaTimestampState::default()
        };
        clamp_media_command_timestamp(&mut command, &mut last);

        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 0);
        assert_eq!(last.video_last_ms, Some(0));

        let mut next = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 33,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut next, &mut last);

        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = next else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 33);
        assert_eq!(last.video_last_ms, Some(33));
    }

    #[test]
    fn clamp_media_timestamp_accepts_backward_reset_beyond_repair_threshold() {
        let mut command = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 1_000,
            payload: Bytes::new(),
        };
        let mut state = MediaTimestampState {
            video_last_ms: Some(10_000),
            ..MediaTimestampState::default()
        };
        clamp_media_command_timestamp(&mut command, &mut state);
        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = command else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 1_000);
        assert_eq!(state.video_last_ms, Some(1_000));
    }

    #[test]
    fn play_start_pacing_delays_followup_media_by_timestamp_delta() {
        let mut pacing = PlayStartPacingState::default();
        let first = pacing.delay_for(1_000, 1_000_000, false);
        assert_eq!(first, Duration::ZERO);

        let second = pacing.delay_for(1_040, 1_000_000, false);
        assert_eq!(second, Duration::from_millis(40));

        let third = pacing.delay_for(1_060, 1_030_000, false);
        assert_eq!(third, Duration::from_millis(30));
    }

    #[test]
    fn play_start_pacing_first_frame_is_immediate_even_with_large_epoch_timestamp() {
        let mut pacing = PlayStartPacingState::default();
        let first = pacing.delay_for(3_895_818, 9_000_000, false);
        assert_eq!(first, Duration::ZERO);
    }

    #[test]
    fn play_start_pacing_resets_on_discontinuity_and_large_backward_jump() {
        let mut pacing = PlayStartPacingState::default();
        assert_eq!(pacing.delay_for(5_000, 2_000_000, false), Duration::ZERO);
        assert_eq!(
            pacing.delay_for(5_200, 2_000_000, false),
            Duration::from_millis(200)
        );

        let discontinuity = pacing.delay_for(1_000, 2_300_000, true);
        assert_eq!(discontinuity, Duration::ZERO);

        let after_discontinuity = pacing.delay_for(1_100, 2_320_000, false);
        assert_eq!(after_discontinuity, Duration::from_millis(80));

        let backward_reset = pacing.delay_for(0, 2_500_000, false);
        assert_eq!(backward_reset, Duration::ZERO);
    }

    #[test]
    fn play_start_pacing_uses_single_timeline_for_audio_video_interleaving() {
        let mut pacing = PlayStartPacingState::default();
        assert_eq!(pacing.delay_for(10_000, 1_000_000, false), Duration::ZERO);

        let audio_delay = pacing.delay_for(10_020, 1_000_000, false);
        assert_eq!(audio_delay, Duration::from_millis(20));

        let video_delay = pacing.delay_for(10_033, 1_020_000, false);
        assert_eq!(video_delay, Duration::from_millis(13));
    }

    #[test]
    fn clamp_media_timestamp_repairs_duplicate_video_millis() {
        let mut state = MediaTimestampState::default();
        let mut first = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 73_900,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut first, &mut state);
        let mut second = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 73_900,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut second, &mut state);
        let RtmpCoreCommand::SendVideo {
            timestamp_ms: second_ms,
            ..
        } = second
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(second_ms, 73_901);
        assert_eq!(state.video_last_ms, Some(73_901));
    }

    #[test]
    fn clamp_media_timestamp_keeps_audio_video_timelines_independent() {
        let mut state = MediaTimestampState::default();
        let mut video = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 1_000,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut video, &mut state);

        let mut audio = RtmpCoreCommand::SendAudio {
            stream_id: 1,
            timestamp_ms: 1_000,
            payload: Bytes::new(),
        };
        clamp_media_command_timestamp(&mut audio, &mut state);
        let RtmpCoreCommand::SendAudio { timestamp_ms, .. } = audio else {
            panic!("expected SendAudio");
        };
        assert_eq!(
            timestamp_ms, 1_000,
            "audio timestamp must not be bumped by video timeline"
        );
        assert_eq!(state.video_last_ms, Some(1_000));
        assert_eq!(state.audio_last_ms, Some(1_000));
    }

    #[test]
    fn play_media_timestamp_rebase_starts_late_joiner_at_zero() {
        let mut rebase = PlayTimestampRebaseState::default();
        let mut clamp = MediaTimestampState::default();
        let mut first_video = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 27_333,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut first_video, &mut rebase);
        clamp_media_command_timestamp(&mut first_video, &mut clamp);
        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = first_video else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 0);

        let mut early_audio = RtmpCoreCommand::SendAudio {
            stream_id: 1,
            timestamp_ms: 27_136,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut early_audio, &mut rebase);
        clamp_media_command_timestamp(&mut early_audio, &mut clamp);
        let RtmpCoreCommand::SendAudio { timestamp_ms, .. } = early_audio else {
            panic!("expected SendAudio");
        };
        assert_eq!(timestamp_ms, 0);

        let mut next_video = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 27_366,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut next_video, &mut rebase);
        clamp_media_command_timestamp(&mut next_video, &mut clamp);
        let RtmpCoreCommand::SendVideo { timestamp_ms, .. } = next_video else {
            panic!("expected SendVideo");
        };
        assert_eq!(timestamp_ms, 33);
    }

    #[test]
    fn rtmp_egress_timeline_reset_restarts_rebase_and_clamp() {
        let mut rebase = PlayTimestampRebaseState::default();
        let mut clamp = MediaTimestampState::default();
        let mut last_mute_ts = Some(8_000);

        let mut first_video = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 5_000,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut first_video, &mut rebase);
        clamp_media_command_timestamp(&mut first_video, &mut clamp);

        reset_rtmp_egress_timeline_state(Some(&mut rebase), &mut clamp, &mut last_mute_ts);
        assert_eq!(last_mute_ts, None);

        let mut next_video = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 9_000,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut next_video, &mut rebase);
        clamp_media_command_timestamp(&mut next_video, &mut clamp);
        let RtmpCoreCommand::SendVideo {
            timestamp_ms: next_video_ts,
            ..
        } = next_video
        else {
            panic!("expected SendVideo");
        };
        assert_eq!(next_video_ts, 0);

        let mut next_audio = RtmpCoreCommand::SendAudio {
            stream_id: 1,
            timestamp_ms: 8_700,
            payload: Bytes::new(),
        };
        rebase_play_media_command_timestamp(&mut next_audio, &mut rebase);
        clamp_media_command_timestamp(&mut next_audio, &mut clamp);
        let RtmpCoreCommand::SendAudio {
            timestamp_ms: next_audio_ts,
            ..
        } = next_audio
        else {
            panic!("expected SendAudio");
        };
        assert_eq!(next_audio_ts, 0);
    }

    #[test]
    fn mute_audio_generation_recovers_after_timeline_reset() {
        let mut last_mute_ts = Some(10_000);
        let mut clamp = MediaTimestampState::default();

        reset_rtmp_egress_timeline_state(None, &mut clamp, &mut last_mute_ts);
        let command = maybe_make_mute_audio(1, 500, &mut last_mute_ts);

        let Some(RtmpCoreCommand::SendAudio { timestamp_ms, .. }) = command else {
            panic!("expected SendAudio mute command");
        };
        assert_eq!(timestamp_ms, 500);
    }

    #[test]
    fn discontinuity_reset_applies_only_to_large_backward_timestamp_jump() {
        let mut state = MediaTimestampState {
            video_last_ms: Some(10_000),
            ..MediaTimestampState::default()
        };
        let forward = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 1_810_000,
            payload: Bytes::new(),
        };
        assert!(
            !should_reset_rtmp_egress_timeline_for_discontinuity(&forward, &mut state),
            "large forward jump should keep egress timeline continuity"
        );

        let backward = RtmpCoreCommand::SendVideo {
            stream_id: 1,
            timestamp_ms: 0,
            payload: Bytes::new(),
        };
        assert!(
            should_reset_rtmp_egress_timeline_for_discontinuity(&backward, &mut state),
            "large backward jump should reset egress timeline on discontinuity"
        );
    }

    #[test]
    fn builds_h266_vvcc_and_parses_parameter_sets() {
        let vps = Bytes::from_static(&[0x40, 0x01, 0x0c]);
        let sps = Bytes::from_static(&[0x42, 0x01, 0x01]);
        let pps = Bytes::from_static(&[0x44, 0x01, 0xc0]);
        let config = build_h266_config(
            std::slice::from_ref(&vps),
            std::slice::from_ref(&sps),
            std::slice::from_ref(&pps),
        );

        assert_eq!(config[0], 0xfe);
        assert_eq!(config[1], 3);

        let (parsed_vps, parsed_sps, parsed_pps) =
            parse_hvcc_parameter_sets(&config, CodecId::H266);
        assert_eq!(parsed_vps, vec![vps]);
        assert_eq!(parsed_sps, vec![sps]);
        assert_eq!(parsed_pps, vec![pps]);
    }
}
