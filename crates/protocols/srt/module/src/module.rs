use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::{
    MonoTime, MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig, MpegTsMuxEvent, MpegTsMuxer,
    MpegTsMuxerConfig, TrackInfo,
};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, HttpMethod,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, PublishLease, PublisherOptions, PublisherSink, SdkError,
    StreamKey, SubscriberOptions,
};
use cheetah_srt_core::{
    parse_srt_stream_id, parse_srt_url, ParsedSrtStreamId, SrtEncryptionOptions, SrtKeyLength,
    SrtPayloadKind, SrtRole, SrtSessionOptions, SrtStreamMode,
};
use cheetah_srt_driver_tokio::{
    spawn_driver, SrtDriverCommand, SrtDriverConfig, SrtDriverEncryption, SrtDriverEvent,
    SrtDriverHandle, SrtDriverStats, SrtPeerId,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{debug, info, warn};

use crate::config::SrtModuleConfig;
use crate::http::SrtHttpService;
use crate::metrics::SrtModuleMetrics;

const MODULE_ID: &str = "srt";

/// Factory that creates `SrtModule` instances and registers the module manifest.
///
/// 创建 `SrtModule` 实例并注册模块清单的工厂。
pub struct SrtModuleFactory;

/// `ModuleFactory` implementation for the SRT module.
///
/// SRT 模块的 `ModuleFactory` 实现。
impl ModuleFactory for SrtModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "SRT Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "srt".to_string(),
            routes_prefix: "/srt".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(SrtModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "srt-module".to_string(),
            default_value: SrtModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                SrtModuleConfig::from_value(value.clone())
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })),
        })
    }
}

/// SRT module runtime state: lifecycle, config, engine context, and metrics.
///
/// SRT 模块运行时状态：生命周期、配置、引擎上下文与指标。
struct SrtModule {
    state: ModuleState,
    config: SrtModuleConfig,
    ctx: Option<EngineContext>,
    metrics: Arc<SrtModuleMetrics>,
}

/// `SrtModule` internal construction helpers.
///
/// `SrtModule` 内部构造辅助。
impl SrtModule {
    fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: SrtModuleConfig::default(),
            ctx: None,
            metrics: SrtModuleMetrics::new(),
        }
    }
}

/// `Module` implementation for SRT: init, start, stop, config, and HTTP routes.
///
/// SRT 的 `Module` 实现：初始化、启动、停止、配置与 HTTP 路由。
#[async_trait]
impl Module for SrtModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "SRT Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = SrtModuleConfig::from_value(ctx.initial_config)
            .map_err(|err| SdkError::InvalidArgument(err.to_string()))?;
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        if !self.config.enabled {
            return Ok(());
        }
        let ctx = self
            .ctx
            .clone()
            .ok_or_else(|| SdkError::Unavailable("SRT module not initialized".to_string()))?;
        let driver_config = driver_config(&self.config)?;
        let (driver, mut events) = spawn_driver(driver_config, cancel.clone());
        let config = self.config.clone();
        let job_plan = build_job_plan(&config)?;
        let runtime = ctx.runtime_api.clone();
        let metrics = self.metrics.clone();
        runtime.spawn(Box::pin(async move {
            let mut worker_state = SrtEventWorkerState {
                forced_modes: job_plan.forced_modes,
                jobs: job_plan.jobs,
                ingress_sessions: HashMap::new(),
                active_modes: HashMap::new(),
                last_stats: HashMap::new(),
            };
            for connect in job_plan.connects {
                driver.send(connect).await;
            }
            loop {
                let cancel_fut = cancel.cancelled().fuse();
                let event_fut = events.recv().fuse();
                pin_mut!(cancel_fut, event_fut);

                let event = select_biased! {
                    _ = cancel_fut => break,
                    event = event_fut => event,
                };
                let Some(event) = event else { break };
                handle_driver_event(
                    &ctx,
                    &config,
                    &driver,
                    metrics.as_ref(),
                    &mut worker_state,
                    event,
                    cancel.clone(),
                )
                .await;
            }
        }));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = SrtModuleConfig::from_value(change.next)
            .map_err(|err| SdkError::InvalidArgument(err.to_string()))?;
        if next == self.config {
            Ok(ConfigEffect::Immediate)
        } else {
            self.config = next;
            Ok(ConfigEffect::ModuleRestartRequired)
        }
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/metrics".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/metrics.json".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(SrtHttpService {
            metrics: self.metrics.clone(),
        }))
    }
}

/// Convert module config into `SrtDriverConfig` for the Tokio driver.
///
/// 将模块配置转换为给 Tokio 驱动的 `SrtDriverConfig`。
fn driver_config(config: &SrtModuleConfig) -> Result<SrtDriverConfig, SdkError> {
    let listen = config
        .listen
        .parse()
        .map_err(|err| SdkError::InvalidArgument(format!("invalid srt.listen: {err}")))?;
    if config.encryption.enabled && config.encryption.passphrase.is_empty() {
        return Err(SdkError::InvalidArgument(
            "srt.encryption.passphrase must be set when encryption is enabled".to_string(),
        ));
    }
    let key_length = match config.encryption.key_length {
        16 => SrtKeyLength::Aes128,
        32 => SrtKeyLength::Aes256,
        other => {
            return Err(SdkError::InvalidArgument(format!(
                "unsupported srt.encryption.key_length: {other}"
            )));
        }
    };
    Ok(SrtDriverConfig {
        listen,
        max_connections: config.max_connections,
        idle_timeout_ms: config.idle_timeout_ms,
        connect_timeout_ms: config.connect_timeout_ms,
        latency_ms: config.latency_ms,
        stats_interval_ms: config.stats_interval_ms,
        recv_buffer_packets: 1024,
        send_queue_capacity: config.egress.send_queue_capacity,
        encryption: SrtDriverEncryption {
            enabled: config.encryption.enabled,
            passphrase: config.encryption.passphrase.clone(),
            key_length,
        },
    })
}

/// State for an active SRT publish ingress session.
///
/// 活跃 SRT 发布入口会话的状态。
struct SrtIngressSession {
    stream_key: StreamKey,
    lease: PublishLease,
    publisher: Box<dyn PublisherSink>,
    demuxer: MpegTsDemuxer,
    tracks: Vec<TrackInfo>,
    tracks_published: bool,
}

#[derive(Clone)]
/// Job-defined mode and stream key override for a peer.
///
/// 为对端指定的任务级模式与流密钥覆盖。
struct ForcedSrtMode {
    mode: SrtStreamMode,
    stream_key: StreamKey,
}

/// Pre-built set of driver connect commands and per-peer runtime metadata.
///
/// 预构建的驱动连接命令集合与每个对端的运行时元数据。
struct SrtJobPlan {
    connects: Vec<SrtDriverCommand>,
    forced_modes: HashMap<SrtPeerId, ForcedSrtMode>,
    jobs: HashMap<SrtPeerId, SrtJobRuntime>,
}

#[derive(Clone)]
/// Retry state and original command for a configured job peer.
///
/// 配置任务对端的重试状态与原始命令。
struct SrtJobRuntime {
    command: SrtDriverCommand,
    forced_mode: ForcedSrtMode,
    retry_backoff_ms: u64,
    max_retry_backoff_ms: u64,
    retry_attempt: u32,
}

/// Mutable state of the background event worker that coordinates SRT sessions.
///
/// 协调 SRT 会话的后台事件工作者的可变状态。
struct SrtEventWorkerState {
    forced_modes: HashMap<SrtPeerId, ForcedSrtMode>,
    jobs: HashMap<SrtPeerId, SrtJobRuntime>,
    ingress_sessions: HashMap<SrtPeerId, SrtIngressSession>,
    active_modes: HashMap<SrtPeerId, SrtStreamMode>,
    last_stats: HashMap<SrtPeerId, SrtDriverStats>,
}

/// Dispatch driver events to ingress/egress logic, metrics, and retry scheduling.
///
/// 将驱动事件分发到入口/出口逻辑、指标与重试调度。
async fn handle_driver_event(
    ctx: &EngineContext,
    config: &SrtModuleConfig,
    driver: &SrtDriverHandle,
    metrics: &SrtModuleMetrics,
    worker_state: &mut SrtEventWorkerState,
    event: SrtDriverEvent,
    cancel: CancellationToken,
) {
    match event {
        SrtDriverEvent::ListenerStarted { local_addr } => {
            info!(%local_addr, "SRT listener started");
        }
        SrtDriverEvent::CallerConnecting { peer_id, remote } => {
            debug!(peer_id = peer_id.0, %remote, "SRT caller connecting");
        }
        SrtDriverEvent::Connected {
            peer_id,
            remote,
            stream_id,
        } => {
            info!(peer_id = peer_id.0, %remote, ?stream_id, "SRT peer connected");
            let classified = worker_state
                .forced_modes
                .remove(&peer_id)
                .map(|forced| Ok((forced.mode, forced.stream_key)))
                .unwrap_or_else(|| classify_stream(config, stream_id.as_deref()));
            if let Some(job) = worker_state.jobs.get_mut(&peer_id) {
                job.retry_attempt = 0;
            }
            match classified {
                Ok((SrtStreamMode::Publish, stream_key)) => {
                    metrics.inc_connection(SrtStreamMode::Publish);
                    worker_state
                        .active_modes
                        .insert(peer_id, SrtStreamMode::Publish);
                    match ctx
                        .publisher_api
                        .acquire_publisher(stream_key.clone(), PublisherOptions::default())
                        .await
                    {
                        Ok((lease, publisher)) => {
                            worker_state.ingress_sessions.insert(
                                peer_id,
                                SrtIngressSession {
                                    stream_key,
                                    lease,
                                    publisher,
                                    demuxer: MpegTsDemuxer::new(MpegTsDemuxerConfig::default()),
                                    tracks: Vec::new(),
                                    tracks_published: false,
                                },
                            );
                        }
                        Err(err) => {
                            warn!(peer_id = peer_id.0, %err, "SRT publish lease rejected");
                            driver
                                .send(SrtDriverCommand::Close {
                                    peer_id,
                                    reason: format!("publish rejected: {err}"),
                                })
                                .await;
                        }
                    }
                }
                Ok((mode @ (SrtStreamMode::Request | SrtStreamMode::Play), stream_key)) => {
                    metrics.inc_connection(mode);
                    worker_state.active_modes.insert(peer_id, mode);
                    let ctx = ctx.clone();
                    let runtime = ctx.runtime_api.clone();
                    let driver = driver.clone();
                    let config = config.clone();
                    runtime.spawn(Box::pin(async move {
                        run_play_session(ctx, config, driver, peer_id, stream_key, cancel).await;
                    }));
                }
                Err(err) => {
                    warn!(peer_id = peer_id.0, %err, "invalid SRT stream id");
                    driver
                        .send(SrtDriverCommand::Close {
                            peer_id,
                            reason: err,
                        })
                        .await;
                }
            }
        }
        SrtDriverEvent::Payload { peer_id, payload } => {
            debug!(
                peer_id = peer_id.0,
                bytes = payload.len(),
                "SRT payload received"
            );
            if let Some(session) = worker_state.ingress_sessions.get_mut(&peer_id) {
                handle_ingress_payload(session, &payload);
            }
        }
        SrtDriverEvent::KeyRefreshNeeded { peer_id } => {
            metrics.inc_key_refresh();
            debug!(peer_id = peer_id.0, "SRT key refresh needed");
        }
        SrtDriverEvent::Stats { peer_id, stats } => {
            metrics.add_stats_delta(worker_state.last_stats.get(&peer_id), &stats);
            worker_state.last_stats.insert(peer_id, stats.clone());
            debug!(peer_id = peer_id.0, ?stats, "SRT stats");
        }
        SrtDriverEvent::Disconnected { peer_id, reason } => {
            info!(peer_id = peer_id.0, %reason, "SRT peer disconnected");
            if worker_state.active_modes.remove(&peer_id).is_some() {
                metrics.dec_connection();
            }
            worker_state.last_stats.remove(&peer_id);
            if let Some(session) = worker_state.ingress_sessions.remove(&peer_id) {
                let _ = ctx.publisher_api.release_publisher(&session.lease).await;
            }
            schedule_job_retry(ctx, driver, worker_state, peer_id, cancel);
        }
        SrtDriverEvent::Error { peer_id, message } => {
            metrics.inc_driver_error(&message);
            warn!(peer_id = peer_id.map(|id| id.0), %message, "SRT driver error");
            if let Some(peer_id) = peer_id {
                schedule_job_retry(ctx, driver, worker_state, peer_id, cancel);
            }
        }
    }
}

/// Schedule a retry for a configured job after a disconnect or error.
///
/// 在断开或错误后为配置任务安排重试。
fn schedule_job_retry(
    ctx: &EngineContext,
    driver: &SrtDriverHandle,
    worker_state: &mut SrtEventWorkerState,
    peer_id: SrtPeerId,
    cancel: CancellationToken,
) {
    let Some(job) = worker_state.jobs.get_mut(&peer_id) else {
        return;
    };
    let delay_ms = retry_delay_ms(
        job.retry_backoff_ms,
        job.max_retry_backoff_ms,
        job.retry_attempt,
    );
    job.retry_attempt = job.retry_attempt.saturating_add(1);
    worker_state
        .forced_modes
        .insert(peer_id, job.forced_mode.clone());

    let command = job.command.clone();
    let driver = driver.clone();
    let runtime = ctx.runtime_api.clone();
    let deadline = MonoTime::from_micros(
        runtime
            .now()
            .as_micros()
            .saturating_add(delay_ms.saturating_mul(1_000)),
    );
    let mut timer = runtime.sleep_until(deadline);
    runtime.spawn(Box::pin(async move {
        let cancel_fut = cancel.cancelled().fuse();
        let sleep_fut = timer.wait().fuse();
        pin_mut!(cancel_fut, sleep_fut);
        select_biased! {
            _ = cancel_fut => {}
            _ = sleep_fut => {
                driver.send(command).await;
            }
        }
    }));
}

/// Exponential backoff capped at `max_ms`.
///
/// 上限为 `max_ms` 的指数退避。
fn retry_delay_ms(base_ms: u64, max_ms: u64, attempt: u32) -> u64 {
    let base_ms = base_ms.max(1);
    let max_ms = max_ms.max(base_ms);
    let shift = attempt.min(32);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    base_ms.saturating_mul(multiplier).min(max_ms)
}

/// Classify the stream id into a mode and validated stream key.
///
/// 将 stream id 分类为模式并校验流密钥。
fn classify_stream(
    config: &SrtModuleConfig,
    stream_id: Option<&str>,
) -> Result<(SrtStreamMode, StreamKey), String> {
    let parsed = match stream_id {
        Some(value) if !value.is_empty() => {
            Some(parse_srt_stream_id(value).map_err(|e| e.to_string())?)
        }
        _ => None,
    };
    let default_mode = match config.ingress.default_mode.as_str() {
        "request" => SrtStreamMode::Request,
        "play" => SrtStreamMode::Play,
        _ => SrtStreamMode::Publish,
    };
    let mode = parsed.as_ref().and_then(|p| p.mode).unwrap_or(default_mode);
    let stream_key = parsed
        .as_ref()
        .map(|p| p.stream_key.clone())
        .or_else(|| {
            (!config.ingress.default_publish_stream_key.is_empty())
                .then(|| config.ingress.default_publish_stream_key.clone())
        })
        .ok_or_else(|| "missing SRT stream key".to_string())?;
    authorize_stream(config, mode, parsed.as_ref())?;
    Ok((mode, stream_key_from_string(&stream_key)))
}

/// Validate the stream against global or per-user auth tokens.
///
/// 使用全局或每个用户的 token 校验流。
fn authorize_stream(
    config: &SrtModuleConfig,
    mode: SrtStreamMode,
    parsed: Option<&ParsedSrtStreamId>,
) -> Result<(), String> {
    if !config.auth.enabled {
        return Ok(());
    }

    let token = parsed.and_then(|stream_id| stream_id.extras.get("token"));
    let global_token = match mode {
        SrtStreamMode::Publish => &config.auth.publish_token,
        SrtStreamMode::Request | SrtStreamMode::Play => &config.auth.request_token,
    };
    if !global_token.is_empty() && token.is_some_and(|value| value == global_token) {
        return Ok(());
    }

    if let Some(stream_id) = parsed {
        if let (Some(user), Some(token)) = (stream_id.user.as_deref(), token) {
            if config
                .auth
                .users
                .iter()
                .any(|entry| entry.username == user && entry.token == *token)
            {
                return Ok(());
            }
        }
    }

    let action = match mode {
        SrtStreamMode::Publish => "publish",
        SrtStreamMode::Request | SrtStreamMode::Play => "request",
    };
    Err(format!("SRT {action} auth rejected"))
}

/// Build `SrtJobPlan` from ingress, egress, and relay config jobs.
///
/// 从入口、出口与中继配置任务构建 `SrtJobPlan`。
fn build_job_plan(config: &SrtModuleConfig) -> Result<SrtJobPlan, SdkError> {
    let mut next_peer_id = 1_000_000_u64;
    let connects_capacity =
        config.ingress_jobs.len() + config.egress_jobs.len() + config.relay_jobs.len() * 2;
    let mut connects = Vec::with_capacity(connects_capacity);
    let mut forced_modes = HashMap::with_capacity(connects_capacity);
    let mut jobs = HashMap::with_capacity(connects_capacity);

    for job in &config.ingress_jobs {
        if !job.enabled {
            continue;
        }
        let peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.source_url,
            SrtStreamMode::Request,
            job.target_stream_key.clone(),
            config,
        )?;
        let command = SrtDriverCommand::ConnectCaller {
            peer_id,
            remote,
            stream_id,
            options,
        };
        let forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Publish,
            stream_key: stream_key_from_string(&job.target_stream_key),
        };
        connects.push(command.clone());
        forced_modes.insert(peer_id, forced_mode.clone());
        jobs.insert(
            peer_id,
            SrtJobRuntime {
                command,
                forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    for job in &config.egress_jobs {
        if !job.enabled {
            continue;
        }
        let peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.target_url,
            SrtStreamMode::Publish,
            job.source_stream_key.clone(),
            config,
        )?;
        let command = SrtDriverCommand::ConnectCaller {
            peer_id,
            remote,
            stream_id,
            options,
        };
        let forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Play,
            stream_key: stream_key_from_string(&job.source_stream_key),
        };
        connects.push(command.clone());
        forced_modes.insert(peer_id, forced_mode.clone());
        jobs.insert(
            peer_id,
            SrtJobRuntime {
                command,
                forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    for job in &config.relay_jobs {
        if !job.enabled {
            continue;
        }
        let relay_stream_key = if job.stream_key.is_empty() {
            format!("relay/{}", job.name)
        } else {
            job.stream_key.clone()
        };

        let ingress_peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.source_url,
            SrtStreamMode::Request,
            relay_stream_key.clone(),
            config,
        )?;
        let ingress_command = SrtDriverCommand::ConnectCaller {
            peer_id: ingress_peer_id,
            remote,
            stream_id,
            options,
        };
        let ingress_forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Publish,
            stream_key: stream_key_from_string(&relay_stream_key),
        };
        connects.push(ingress_command.clone());
        forced_modes.insert(ingress_peer_id, ingress_forced_mode.clone());
        jobs.insert(
            ingress_peer_id,
            SrtJobRuntime {
                command: ingress_command,
                forced_mode: ingress_forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );

        let egress_peer_id = SrtPeerId(next_peer_id);
        next_peer_id += 1;
        let (remote, stream_id, options) = caller_connect_parts(
            &job.target_url,
            SrtStreamMode::Publish,
            relay_stream_key.clone(),
            config,
        )?;
        let egress_command = SrtDriverCommand::ConnectCaller {
            peer_id: egress_peer_id,
            remote,
            stream_id,
            options,
        };
        let egress_forced_mode = ForcedSrtMode {
            mode: SrtStreamMode::Play,
            stream_key: stream_key_from_string(&relay_stream_key),
        };
        connects.push(egress_command.clone());
        forced_modes.insert(egress_peer_id, egress_forced_mode.clone());
        jobs.insert(
            egress_peer_id,
            SrtJobRuntime {
                command: egress_command,
                forced_mode: egress_forced_mode,
                retry_backoff_ms: job.retry_backoff_ms,
                max_retry_backoff_ms: job.max_retry_backoff_ms,
                retry_attempt: 0,
            },
        );
    }

    Ok(SrtJobPlan {
        connects,
        forced_modes,
        jobs,
    })
}

/// Parse an SRT caller URL and build `SrtSessionOptions` for a job.
///
/// 解析 SRT caller URL 并为任务构建 `SrtSessionOptions`。
fn caller_connect_parts(
    url: &str,
    default_mode: SrtStreamMode,
    stream_key: String,
    config: &SrtModuleConfig,
) -> Result<(SocketAddr, Option<String>, SrtSessionOptions), SdkError> {
    let parsed = parse_srt_url(url).map_err(|err| SdkError::InvalidArgument(err.to_string()))?;
    if parsed.mode.is_some_and(|mode| mode != SrtRole::Caller) {
        return Err(SdkError::InvalidArgument(format!(
            "SRT job URL must use mode=caller: {url}"
        )));
    }
    let host = parsed.host.as_deref().unwrap_or("127.0.0.1");
    let remote = (host, parsed.port)
        .to_socket_addrs()
        .map_err(|err| SdkError::InvalidArgument(format!("resolve {host}:{}: {err}", parsed.port)))?
        .next()
        .ok_or_else(|| SdkError::InvalidArgument(format!("no address resolved for {url}")))?;
    if parsed.passphrase.as_deref().is_some_and(str::is_empty) {
        return Err(SdkError::InvalidArgument(
            "SRT URL passphrase must not be empty".to_string(),
        ));
    }
    let encryption = SrtEncryptionOptions {
        enabled: parsed.passphrase.is_some() || config.encryption.enabled,
        passphrase: parsed
            .passphrase
            .clone()
            .unwrap_or_else(|| config.encryption.passphrase.clone()),
        key_length: parsed
            .key_length
            .unwrap_or(match config.encryption.key_length {
                32 => SrtKeyLength::Aes256,
                _ => SrtKeyLength::Aes128,
            }),
    };
    let stream_id = merge_url_token_into_stream_id(parsed.stream_id, parsed.extras.get("token"))?;
    Ok((
        remote,
        stream_id,
        SrtSessionOptions {
            role: SrtRole::Caller,
            mode: default_mode,
            stream_key,
            latency_ms: parsed.latency_ms.unwrap_or(config.latency_ms),
            payload: SrtPayloadKind::MpegTs,
            encryption,
        },
    ))
}

/// Inject an URL `token` query into an access-control stream id.
///
/// 将 URL `token` 查询参数注入访问控制流 id。
fn merge_url_token_into_stream_id(
    stream_id: Option<String>,
    url_token: Option<&String>,
) -> Result<Option<String>, SdkError> {
    if let Some(stream_id) = stream_id
        .as_deref()
        .filter(|value| value.starts_with("#!::"))
    {
        parse_srt_stream_id(stream_id)
            .map_err(|err| SdkError::InvalidArgument(format!("invalid SRT streamid: {err}")))?;
    }
    let Some(token) = url_token.filter(|value| !value.is_empty()) else {
        return Ok(stream_id);
    };
    let token = percent_encode_stream_id_field(token);
    Ok(match stream_id {
        Some(stream_id) if stream_id.starts_with("#!::") => {
            if parse_srt_stream_id(&stream_id)
                .ok()
                .is_some_and(|parsed| parsed.extras.contains_key("token"))
            {
                Some(stream_id)
            } else {
                Some(format!("{stream_id},token={token}"))
            }
        }
        Some(stream_id) => {
            let stream_id = percent_encode_stream_id_field(&stream_id);
            Some(format!("#!::r={stream_id},token={token}"))
        }
        None => None,
    })
}

/// Percent-encode a value so it is safe inside an access-control stream id field.
///
/// 对值进行百分号编码，使其在访问控制流 id 字段中安全。
fn percent_encode_stream_id_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Parse a `namespace/path` or bare stream key into a `StreamKey`.
///
/// 将 `namespace/path` 或裸流密钥解析为 `StreamKey`。
fn stream_key_from_string(value: &str) -> StreamKey {
    match value.split_once('/') {
        Some((namespace, path)) if !namespace.is_empty() && !path.is_empty() => {
            StreamKey::new(namespace, path)
        }
        _ => StreamKey::new("live", value),
    }
}

/// Demux an MPEG-TS payload and push frames into the publisher sink.
///
/// 解复用 MPEG-TS 负载并将帧推入发布者接收端。
fn handle_ingress_payload(session: &mut SrtIngressSession, payload: &[u8]) {
    for event in session.demuxer.push(payload) {
        match event {
            MpegTsDemuxEvent::TrackFound(track) => {
                if merge_track_update(&mut session.tracks, track) {
                    session.tracks_published = false;
                }
            }
            MpegTsDemuxEvent::Frame(frame) => {
                if !session.tracks_published
                    && !session.tracks.is_empty()
                    && session
                        .publisher
                        .update_tracks(session.tracks.clone())
                        .is_ok()
                {
                    session.tracks_published = true;
                }
                let _ = session.publisher.push_frame(Arc::new(frame));
            }
            MpegTsDemuxEvent::Diagnostic(diagnostic) => {
                debug!(stream_key = %session.stream_key, ?diagnostic, "SRT TS demux diagnostic");
            }
        }
    }
}

/// Insert or update a track in the session track list and return whether it changed.
///
/// 在会话轨道列表中插入或更新轨道，并返回是否发生变化。
fn merge_track_update(tracks: &mut Vec<TrackInfo>, track: TrackInfo) -> bool {
    if let Some(existing) = tracks
        .iter_mut()
        .find(|existing| existing.track_id == track.track_id)
    {
        if *existing == track {
            return false;
        }
        *existing = track;
        return true;
    }

    tracks.push(track);
    true
}

/// Subscribe to a stream, mux to MPEG-TS, and send payloads to the peer.
///
/// 订阅流，复用为 MPEG-TS，并向对端发送负载。
async fn run_play_session(
    ctx: EngineContext,
    config: SrtModuleConfig,
    driver: SrtDriverHandle,
    peer_id: SrtPeerId,
    stream_key: StreamKey,
    cancel: CancellationToken,
) {
    let Some(snapshot) = wait_for_stream(&ctx, &stream_key, &config, &cancel).await else {
        driver
            .send(SrtDriverCommand::Close {
                peer_id,
                reason: "stream not found or tracks not ready".to_string(),
            })
            .await;
        return;
    };

    let queue_capacity = config
        .egress
        .subscriber_queue_capacity
        .max(config.egress.bootstrap_max_frames.max(1));
    let mut subscriber = match ctx
        .subscriber_api
        .subscribe(
            stream_key.clone(),
            SubscriberOptions {
                queue_capacity,
                backpressure: config.egress.subscriber_backpressure,
                bootstrap_policy: BootstrapPolicy::live_tail(
                    config.egress.bootstrap_max_frames,
                    None,
                ),
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            driver
                .send(SrtDriverCommand::Close {
                    peer_id,
                    reason: format!("subscribe failed: {err}"),
                })
                .await;
            return;
        }
    };

    let tracks = snapshot.tracks;
    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
    send_muxer_tables(&driver, peer_id, &mut muxer).await;

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);
        let frame = select_biased! {
            _ = cancel_fut => break,
            frame = recv_fut => frame,
        };
        match frame {
            Ok(Some(frame)) => {
                for event in muxer.push_frame(frame.as_ref()) {
                    if let MpegTsMuxEvent::Packet(payload) = event {
                        driver
                            .send(SrtDriverCommand::SendPayload { peer_id, payload })
                            .await;
                    }
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    let _ = subscriber.close().await;
}

/// Wait for a stream to exist and its tracks to become ready for egress.
///
/// 等待流存在且其轨道准备好 egress。
async fn wait_for_stream(
    ctx: &EngineContext,
    stream_key: &StreamKey,
    config: &SrtModuleConfig,
    cancel: &CancellationToken,
) -> Option<cheetah_sdk::StreamSnapshot> {
    let stream_deadline = ctx.runtime_api.now().as_micros().saturating_add(
        config
            .egress
            .play_wait_source_timeout_ms
            .saturating_mul(1_000),
    );
    let mut track_deadline = None;
    loop {
        let now = ctx.runtime_api.now().as_micros();
        if cancel.is_cancelled() || now >= stream_deadline {
            return None;
        }
        if let Ok(Some(snapshot)) = ctx.stream_manager_api.get_stream(stream_key).await {
            if tracks_ready_for_egress(&snapshot.tracks) {
                return Some(snapshot);
            }
            let deadline = *track_deadline.get_or_insert_with(|| {
                now.saturating_add(config.egress.track_ready_timeout_ms.saturating_mul(1_000))
            });
            if now >= deadline {
                return None;
            }
        }
        let deadline = cheetah_codec::MonoTime::from_micros(
            ctx.runtime_api.now().as_micros().saturating_add(100_000),
        );
        let mut sleep = ctx.runtime_api.sleep_until(deadline);
        let cancel_fut = cancel.cancelled().fuse();
        let sleep_fut = sleep.wait().fuse();
        pin_mut!(cancel_fut, sleep_fut);
        select_biased! {
            _ = cancel_fut => return None,
            _ = sleep_fut => {}
        }
    }
}

/// Check that all tracks are non-empty and individually ready.
///
/// 检查所有轨道非空且各自就绪。
fn tracks_ready_for_egress(tracks: &[TrackInfo]) -> bool {
    !tracks.is_empty() && tracks.iter().all(TrackInfo::is_ready)
}

/// Send the MPEG-TS PAT/PMT tables before the first media packets.
///
/// 在第一个媒体包之前发送 MPEG-TS PAT/PMT 表。
async fn send_muxer_tables(driver: &SrtDriverHandle, peer_id: SrtPeerId, muxer: &mut MpegTsMuxer) {
    for event in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(payload) = event {
            driver
                .send(SrtDriverCommand::SendPayload { peer_id, payload })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SrtAuthUserConfig, SrtRelayJobConfig};
    use bytes::Bytes;
    use cheetah_sdk::{HttpMethod, HttpRequest, ModuleFactory};

    #[test]
    fn factory_manifest_matches_srt_module_contract() {
        let manifest = SrtModuleFactory.manifest();
        assert_eq!(manifest.module_id, ModuleId::new("srt"));
        assert_eq!(manifest.config_namespace, "srt");
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::Publish)));
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::Subscribe)));
        assert!(manifest
            .capabilities
            .iter()
            .any(|capability| matches!(capability, ModuleCapability::HttpApi)));
    }

    #[test]
    fn metrics_routes_are_registered() {
        let module = SrtModule::new();
        let routes = module.http_routes();

        assert!(routes
            .iter()
            .any(|route| route.method == HttpMethod::Get && route.path == "/metrics"));
        assert!(routes
            .iter()
            .any(|route| route.method == HttpMethod::Get && route.path == "/metrics.json"));
    }

    #[test]
    fn metrics_json_endpoint_starts_at_zero() {
        let module = SrtModule::new();
        let service = module.http_service().expect("SRT HTTP service");
        let response = futures::executor::block_on(service.handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/metrics.json".to_string(),
            query: None,
            headers: Vec::new(),
            body: Default::default(),
        }))
        .expect("metrics.json response");

        assert_eq!(response.status, 200);
        let payload: serde_json::Value =
            serde_json::from_slice(&response.body).expect("metrics json body");
        assert_eq!(payload["connections_active"], 0);
        assert_eq!(payload["bytes_in_total"], 0);
        assert_eq!(payload["bytes_out_total"], 0);
        assert_eq!(payload["driver_errors_total"], 0);
    }

    #[test]
    fn ingress_track_update_replaces_existing_track_metadata() {
        let mut tracks = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        let mut updated = TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );
        updated.extradata = cheetah_codec::CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 0x64])],
            pps: vec![Bytes::from_static(&[0x68, 0xeb])],
            avcc: None,
        };
        updated.refresh_readiness();

        assert!(merge_track_update(&mut tracks, updated.clone()));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0], updated);
        assert!(tracks[0].is_ready());
    }

    #[test]
    fn ingress_track_update_keeps_distinct_tracks_with_same_codec() {
        let mut tracks = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        let second = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );

        assert!(merge_track_update(&mut tracks, second.clone()));
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1], second);
    }

    #[test]
    fn egress_wait_requires_non_empty_ready_tracks() {
        let empty = Vec::new();
        assert!(!tracks_ready_for_egress(&empty));

        let pending = vec![TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        )];
        assert!(!tracks_ready_for_egress(&pending));

        let mut ready = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::AAC,
            48_000,
        );
        ready.extradata = cheetah_codec::CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x11, 0x88]),
        };
        ready.refresh_readiness();
        assert!(tracks_ready_for_egress(&[ready]));
    }

    #[test]
    fn egress_wait_accepts_extended_passthrough_codecs() {
        let mut h266 = TrackInfo::new(
            cheetah_codec::TrackId(1),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H266,
            90_000,
        );
        h266.extradata = cheetah_codec::CodecExtradata::H266 {
            vps: vec![Bytes::from_static(&[0x00, 0x70, 0x01])],
            sps: vec![Bytes::from_static(&[0x00, 0x78, 0x01])],
            pps: vec![Bytes::from_static(&[0x00, 0x80, 0x01])],
        };
        h266.refresh_readiness();

        let mut mjpeg = TrackInfo::new(
            cheetah_codec::TrackId(2),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::MJPEG,
            90_000,
        );
        mjpeg.refresh_readiness();

        let mut adpcm = TrackInfo::new(
            cheetah_codec::TrackId(3),
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::ADPCM,
            90_000,
        );
        adpcm.refresh_readiness();

        assert!(tracks_ready_for_egress(&[h266, mjpeg, adpcm]));
    }

    #[test]
    fn relay_job_expands_to_ingress_and_egress_caller_connections() {
        let mut config = SrtModuleConfig::default();
        config.relay_jobs.push(SrtRelayJobConfig {
            name: "relay-a".to_string(),
            enabled: true,
            source_url: "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request"
                .to_string(),
            target_url: "srt://127.0.0.1:9002?mode=caller&streamid=#!::r=live/out,m=publish"
                .to_string(),
            stream_key: "relay/source-a".to_string(),
            retry_backoff_ms: 1_000,
            max_retry_backoff_ms: 30_000,
        });

        let plan = build_job_plan(&config).expect("valid relay job plan");
        assert_eq!(plan.connects.len(), 2);

        let publish_forced = plan
            .forced_modes
            .values()
            .filter(|mode| mode.mode == SrtStreamMode::Publish)
            .count();
        let play_forced = plan
            .forced_modes
            .values()
            .filter(|mode| mode.mode == SrtStreamMode::Play)
            .count();
        assert_eq!(publish_forced, 1);
        assert_eq!(play_forced, 1);
    }

    #[test]
    fn publish_auth_accepts_matching_global_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.publish_token = "publish-secret".to_string();

        let (mode, stream_key) = classify_stream(
            &config,
            Some("#!::r=live/test,m=publish,token=publish-secret"),
        )
        .expect("matching publish token should pass");

        assert_eq!(mode, SrtStreamMode::Publish);
        assert_eq!(stream_key.to_string(), "live/test");
    }

    #[test]
    fn publish_auth_rejects_missing_or_wrong_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.publish_token = "publish-secret".to_string();

        let missing = classify_stream(&config, Some("#!::r=live/test,m=publish"))
            .expect_err("missing publish token should fail");
        assert_eq!(missing, "SRT publish auth rejected");

        let wrong = classify_stream(&config, Some("#!::r=live/test,m=publish,token=wrong"))
            .expect_err("wrong publish token should fail");
        assert_eq!(wrong, "SRT publish auth rejected");
    }

    #[test]
    fn request_auth_accepts_matching_user_token() {
        let mut config = SrtModuleConfig::default();
        config.auth.enabled = true;
        config.auth.users.push(SrtAuthUserConfig {
            username: "alice".to_string(),
            token: "alice-secret".to_string(),
        });

        let (mode, stream_key) = classify_stream(
            &config,
            Some("#!::r=live/test,m=request,u=alice,token=alice-secret"),
        )
        .expect("matching user request token should pass");

        assert_eq!(mode, SrtStreamMode::Request);
        assert_eq!(stream_key.to_string(), "live/test");
    }

    #[test]
    fn caller_job_url_token_is_added_to_stream_id() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");

        assert_eq!(
            stream_id.as_deref(),
            Some("#!::r=live/in,m=request,token=query-secret")
        );
    }

    #[test]
    fn caller_job_stream_id_token_wins_over_url_token() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request,token=stream-secret&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");

        assert_eq!(
            stream_id.as_deref(),
            Some("#!::r=live/in,m=request,token=stream-secret")
        );
    }

    #[test]
    fn caller_job_rejects_invalid_access_control_stream_id() {
        let config = SrtModuleConfig::default();
        let err = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=../secret,m=request&token=query-secret",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect_err("invalid access-control stream id should fail");

        assert!(err.to_string().contains("stream key must not contain `..`"));
    }

    #[test]
    fn caller_job_url_token_is_encoded_for_stream_id_field() {
        let config = SrtModuleConfig::default();
        let (_remote, stream_id, _options) = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&token=a%2Cb%3Dc%25",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect("valid caller parts");
        let stream_id = stream_id.expect("merged stream id");
        let parsed = parse_srt_stream_id(&stream_id).expect("merged stream id should parse");

        assert_eq!(
            parsed.extras.get("token").map(String::as_str),
            Some("a,b=c%")
        );
    }

    #[test]
    fn caller_job_rejects_empty_url_passphrase() {
        let config = SrtModuleConfig::default();
        let err = caller_connect_parts(
            "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request&passphrase=",
            SrtStreamMode::Request,
            "live/local".to_string(),
            &config,
        )
        .expect_err("empty URL passphrase should fail");

        assert!(err.to_string().contains("passphrase must not be empty"));
    }

    #[test]
    fn relay_job_plan_keeps_retry_metadata_for_each_caller() {
        let mut config = SrtModuleConfig::default();
        config.relay_jobs.push(SrtRelayJobConfig {
            name: "relay-retry".to_string(),
            enabled: true,
            source_url: "srt://127.0.0.1:9001?mode=caller&streamid=#!::r=live/in,m=request"
                .to_string(),
            target_url: "srt://127.0.0.1:9002?mode=caller&streamid=#!::r=live/out,m=publish"
                .to_string(),
            stream_key: "relay/retry".to_string(),
            retry_backoff_ms: 250,
            max_retry_backoff_ms: 1_000,
        });

        let plan = build_job_plan(&config).expect("valid relay job plan");

        assert_eq!(plan.jobs.len(), 2);
        assert!(plan
            .jobs
            .values()
            .all(|job| job.retry_backoff_ms == 250 && job.max_retry_backoff_ms == 1_000));
    }

    #[test]
    fn retry_backoff_uses_exponential_cap() {
        assert_eq!(retry_delay_ms(250, 1_000, 0), 250);
        assert_eq!(retry_delay_ms(250, 1_000, 1), 500);
        assert_eq!(retry_delay_ms(250, 1_000, 2), 1_000);
        assert_eq!(retry_delay_ms(250, 1_000, 3), 1_000);
    }
}
