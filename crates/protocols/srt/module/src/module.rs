use std::collections::{BTreeMap, HashMap};
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
    format_srt_version, parse_srt_stream_id, parse_srt_stream_id_with_options, parse_srt_url,
    parse_srt_version, version_at_least, SrtEncryptionOptions, SrtKeyLength, SrtPayloadKind,
    SrtRole, SrtSessionOptions, SrtStreamMode, StreamIdParseOptions,
};
use cheetah_srt_driver_tokio::{
    spawn_driver, SrtDriverCommand, SrtDriverConfig, SrtDriverEncryption, SrtDriverEvent,
    SrtDriverFecConfig, SrtDriverHandle, SrtDriverStats, SrtPeerId,
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
                    .map_err(|err| err.to_string())
                    .and_then(|c| c.validate())
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
        let config = SrtModuleConfig::from_value(ctx.initial_config)
            .map_err(|err| SdkError::InvalidArgument(err.to_string()))?;
        config.validate().map_err(SdkError::InvalidArgument)?;
        self.config = config;
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
        next.validate().map_err(SdkError::InvalidArgument)?;
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
    let srt_version = parse_srt_version(&config.local_srt_version).map_err(|err| {
        SdkError::InvalidArgument(format!("invalid srt.local_srt_version: {err}"))
    })?;
    Ok(SrtDriverConfig {
        listen,
        max_connections: config.max_connections,
        idle_timeout_ms: config.idle_timeout_ms,
        connect_timeout_ms: config.connect_timeout_ms,
        latency_ms: config.latency_ms,
        stats_interval_ms: config.stats_interval_ms,
        recv_buffer_packets: config.pkt_buf_size,
        send_queue_capacity: config.egress.send_queue_capacity,
        srt_version,
        encryption: SrtDriverEncryption {
            enabled: config.encryption.enabled,
            passphrase: config.encryption.passphrase.clone(),
            key_length,
        },
        fec: SrtDriverFecConfig {
            enabled: config.fec.enabled,
            required: config.fec.required,
            cols: config.fec.cols,
            rows: config.fec.rows,
        },
    })
}
include!("auth.rs");
include!("stream_classify.rs");
include!("jobs.rs");
include!("ingress_session.rs");
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
include!("egress_session.rs");
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
            peer_version,
        } => {
            info!(peer_id = peer_id.0, %remote, ?stream_id, "SRT peer connected");
            // The underlying driver does not expose a packet-filter / FEC API as of
            // shiguredo_srt = "=2026.1.0-canary.1"; when FEC is required, reject the
            // connection immediately.
            if config.fec.required {
                warn!(
                    peer_id = peer_id.0,
                    "SRT FEC required but driver lacks FEC support"
                );
                metrics.inc_fec_negotiate_fail();
                driver
                    .send(SrtDriverCommand::Close {
                        peer_id,
                        reason: "reject:fec_required".to_string(),
                    })
                    .await;
                return;
            }
            let classified = worker_state
                .forced_modes
                .remove(&peer_id)
                .map(|forced| Ok(classified_from_forced(config, forced, remote)))
                .unwrap_or_else(|| {
                    classify_stream(config, stream_id.as_deref(), Some(remote), peer_version)
                });
            if let Some(job) = worker_state.jobs.get_mut(&peer_id) {
                job.retry_attempt = 0;
            }
            match classified {
                Ok(classify) => {
                    info!(
                        peer_id = peer_id.0,
                        %remote,
                        vhost = %classify.auth.vhost,
                        app = %classify.auth.app,
                        stream = %classify.auth.stream,
                        mode = ?classify.mode,
                        "SRT stream classified"
                    );
                    match classify.mode {
                        SrtStreamMode::Publish => {
                            metrics.inc_connection(SrtStreamMode::Publish);
                            worker_state
                                .active_modes
                                .insert(peer_id, SrtStreamMode::Publish);
                            match ctx
                                .publisher_api
                                .acquire_publisher(
                                    classify.stream_key.clone(),
                                    PublisherOptions::default(),
                                )
                                .await
                            {
                                Ok((lease, publisher)) => {
                                    worker_state.ingress_sessions.insert(
                                        peer_id,
                                        SrtIngressSession {
                                            stream_key: classify.stream_key,
                                            lease,
                                            publisher,
                                            demuxer: MpegTsDemuxer::new(
                                                MpegTsDemuxerConfig::default(),
                                            ),
                                            tracks: Vec::new(),
                                            tracks_published: false,
                                        },
                                    );
                                }
                                Err(err) => {
                                    warn!(peer_id = peer_id.0, %err, "SRT publish lease rejected");
                                    let reason = format!("reject:publish_conflict: {err}");
                                    metrics.inc_handshake_reject(&reason);
                                    driver
                                        .send(SrtDriverCommand::Close { peer_id, reason })
                                        .await;
                                }
                            }
                        }
                        mode @ (SrtStreamMode::Request | SrtStreamMode::Play) => {
                            metrics.inc_connection(mode);
                            worker_state.active_modes.insert(peer_id, mode);
                            let ctx = ctx.clone();
                            let runtime = ctx.runtime_api.clone();
                            let driver = driver.clone();
                            let config = config.clone();
                            let stream_key = classify.stream_key;
                            runtime.spawn(Box::pin(async move {
                                run_play_session(ctx, config, driver, peer_id, stream_key, cancel)
                                    .await;
                            }));
                        }
                    }
                }
                Err(err) => {
                    warn!(peer_id = peer_id.0, %err, "SRT stream classification failed");
                    metrics.inc_handshake_reject(&err);
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
include!("module_tests.rs");
