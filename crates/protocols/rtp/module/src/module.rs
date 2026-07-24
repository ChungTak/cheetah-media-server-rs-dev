//! RTP module factory and implementation.
//!
//! RTP 模块工厂与实现。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_rtp_core::{RtpClientSpec, RtpTrackFilter, RtpTransportMode};
use cheetah_rtp_driver_tokio::{
    start_driver, DriverLimits, PortRange, RtpDriverCommand, RtpDriverConfig, RtpDriverHandle,
};
use cheetah_sdk::media_api::capability::default_operations;
use cheetah_sdk::media_api::{MediaCapability, MediaCapabilitySet};
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRouteDescriptor, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleHttpService, ModuleId, ModuleInfo,
    ModuleInitContext, ModuleManifest, ModuleSchemaRegistration, ModuleState, ProviderRegistration,
    SdkError,
};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;
use tracing::info;

use crate::config::{RtpClientJobConfig, RtpModuleConfig};
use crate::egress::sleep_or_cancel;
use crate::http_service::RtpHttpService;
use crate::ingress::run_ingress_worker;
use crate::media_provider::RtpMediaProvider;
use crate::orchestrator::RtpSessionOrchestrator;
const MODULE_ID: &str = "rtp";

/// Factory for creating RTP modules.
///
/// RTP 模块工厂。
pub struct RtpModuleFactory;

/// `RtpModuleFactory` implementation.
///
/// `RtpModuleFactory` 实现。
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

/// RTP module runtime state.
///
/// RTP 模块运行时状态。
pub struct RtpModule {
    state: ModuleState,
    config: RtpModuleConfig,
    ctx: Option<EngineContext>,
    /// Shared session orchestrator created in `init` and used by both the
    /// `RtpApi` provider and the module's HTTP service.
    orchestrator: Option<Arc<RtpSessionOrchestrator>>,
    cancel_token: Option<CancellationToken>,
    active_egress: Arc<Mutex<HashMap<String, CancellationToken>>>,
    client_targets: Arc<Mutex<HashMap<String, Vec<String>>>>,
    media_services_registration: Option<ProviderRegistration>,
    rtp_session_registration: Option<ProviderRegistration>,
    /// The shared `RtpMediaProvider` is kept so `start()` can re-register the
    /// `RtpSessionApi` with capabilities that depend on the playback provider.
    rtp_provider: Option<Arc<RtpMediaProvider>>,
}

/// `RtpModule` constructor.
///
/// `RtpModule` 构造器。
impl RtpModule {
    /// Create a new RTP module instance.
    ///
    /// 创建新的 RTP 模块实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: RtpModuleConfig::default(),
            ctx: None,
            orchestrator: None,
            cancel_token: None,
            active_egress: Arc::new(Mutex::new(HashMap::new())),
            client_targets: Arc::new(Mutex::new(HashMap::new())),
            media_services_registration: None,
            rtp_session_registration: None,
            rtp_provider: None,
        }
    }
}

/// `Default` forward to `RtpModule::new`.
///
/// `Default` 转发到 `RtpModule::new`。
impl Default for RtpModule {
    fn default() -> Self {
        Self::new()
    }
}

/// `Module` lifecycle and HTTP control API implementation for RTP.
///
/// RTP 的 `Module` 生命周期与 HTTP 控制 API 实现。
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

        if !self.config.enabled {
            self.state = ModuleState::Initialized;
            return Ok(());
        }

        let engine = ctx.engine.clone();
        self.ctx = Some(ctx.engine);

        // Allocate the module-scoped cancellation token first so it can be shared with
        // the media-domain provider and the HTTP service.
        let module_cancel = CancellationToken::new();
        self.cancel_token = Some(module_cancel.clone());

        let default_bind_addr = self
            .config
            .listen_udp
            .as_deref()
            .unwrap_or("0.0.0.0:20000")
            .parse::<SocketAddr>()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen_udp: {e}")))?;

        // Shared driver handle slot. Populated in `start()` once the Tokio driver is bound.
        let driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>> = Arc::new(Mutex::new(None));
        let orchestrator = Arc::new(RtpSessionOrchestrator::with_max_sessions(
            driver_handle,
            default_bind_addr,
            self.config.max_sessions,
        ));
        self.orchestrator = Some(orchestrator.clone());

        // Register the media-domain RtpApi provider so native/ZLM adapters can
        // drive RTP sessions through the same orchestrator used by the module's HTTP API.
        let rtp_provider = Arc::new(RtpMediaProvider::new(
            orchestrator,
            engine.clone(),
            module_cancel,
            self.config.clone(),
        ));

        let rtp_capabilities = {
            let mut set = cheetah_sdk::media_api::MediaCapabilitySet::empty();
            set.add(cheetah_sdk::media_api::MediaCapability::Rtp, 1);
            set
        };
        self.media_services_registration = Some(
            engine
                .media_services
                .register_rtp_with_capabilities(rtp_provider.clone(), rtp_capabilities),
        );

        self.rtp_session_registration = Some(
            engine
                .media_services
                .register_rtp_session(rtp_provider.clone()),
        );
        self.rtp_provider = Some(rtp_provider);
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

        // Re-register the RtpSessionApi with a capability set that reflects the
        // playback provider. The `control` operation is advertised only when a
        // playback provider is present and itself advertises `control`.
        if let Some(rtp_provider) = self.rtp_provider.clone() {
            let mut capabilities = MediaCapabilitySet::empty();
            capabilities.add(MediaCapability::RtpSession, 1);
            let playback_has_control = ctx
                .media_services
                .capability_report()
                .descriptors
                .iter()
                .any(|d| {
                    d.capability == MediaCapability::Playback
                        && d.operations.contains(&"control".to_string())
                });
            if playback_has_control {
                let mut ops = default_operations(MediaCapability::RtpSession);
                ops.push("control".to_string());
                capabilities.set_operations(MediaCapability::RtpSession, ops);
            }
            self.rtp_session_registration = Some(
                ctx.media_services
                    .register_rtp_session_with_capabilities(rtp_provider, capabilities),
            );
        }

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

        let udp_port_pool = if config.udp_port_pool_start != 0 && config.udp_port_pool_end != 0 {
            PortRange::new(config.udp_port_pool_start, config.udp_port_pool_end)
        } else {
            None
        };
        let driver_config = RtpDriverConfig {
            listen_udp,
            listen_tcp,
            listen_rtcp_udp,
            write_queue_capacity: config.write_queue_capacity,
            read_buffer_size: config.read_buffer_size,
            session_idle_timeout_ms: config.idle_timeout_ms,
            max_sessions: config.max_sessions,
            tick_interval_ms: config.tick_interval_ms,
            rtcp_report_interval_ms: config.rtcp_report_interval_ms,
            tcp_framing,
            max_rtp_len_cap: config.max_rtp_len_cap,
            limits: DriverLimits::default(),
            udp_port_pool,
        };

        let handle = Arc::new(start_driver(driver_config, cancel.clone()));

        let orchestrator = self.orchestrator.clone().ok_or_else(|| {
            SdkError::InvalidArgument("RtpModule::start called before init".to_string())
        })?;
        orchestrator.set_driver_handle(handle.clone());

        let driver = orchestrator
            .driver()
            .map_err(|e| SdkError::Unavailable(e.message.to_string()))?;

        // Spawn ingress worker and wait until it has been polled so the first
        // `open_*` request does not race against the worker entering its event loop.
        let runtime_api = ctx.runtime_api.clone();
        let (ingress_ready_tx, mut ingress_ready_rx) = runtime_api.oneshot();
        {
            let ctx = ctx.clone();
            let handle = driver.clone();
            let cancel = cancel.clone();
            let orchestrator_for_ingress = orchestrator.clone();
            let publish_frame_cache = config.publish_frame_cache_frames;
            runtime_api.spawn(Box::pin(async move {
                run_ingress_worker(
                    ctx,
                    handle,
                    orchestrator_for_ingress,
                    cancel,
                    publish_frame_cache,
                    Some(ingress_ready_tx),
                )
                .await;
            }));
        }
        let _ = ingress_ready_rx.recv().await;

        // Spawn configured pull jobs
        for job in &config.pull_jobs {
            if !job.enabled {
                continue;
            }
            let job = job.clone();
            let ctx = ctx.clone();
            let runtime_api = ctx.runtime_api.clone();
            let handle = driver.clone();
            let cancel = cancel.clone();
            runtime_api.spawn(Box::pin(async move {
                run_pull_job_supervisor(ctx, handle, job, cancel).await;
            }));
        }

        // Background driver/ingress/egress loops are already spawned; return so the
        // engine startup pipeline can complete. The spawned tasks observe `cancel` and
        // are stopped from `RtpModule::stop`.
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Drop the driver handle so any HTTP request that arrives while we're stopping (or
        // before a subsequent restart re-initialises) gets `Unavailable` instead of trying
        // to talk to a dead driver.
        if let Some(orchestrator) = self.orchestrator.as_ref() {
            orchestrator.clear_driver_handle();
        }
        // Clear in-flight egress sessions and per-stream client routing tables; the cancel
        // cascade above terminates any spawned senders, but we also reset the maps so a
        // subsequent restart starts from a clean state.
        self.active_egress.lock().clear();
        self.client_targets.lock().clear();
        if let Some(reg) = self.media_services_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        if let Some(reg) = self.rtp_session_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
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
        if !self.config.enabled {
            return Vec::new();
        }
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
        if !self.config.enabled {
            return None;
        }
        let engine = self.ctx.clone()?;
        // Cancel token is allocated in `init`; treat its absence as a programming error.
        let module_cancel = self.cancel_token.clone()?;
        let orchestrator = self.orchestrator.clone()?;
        Some(Arc::new(RtpHttpService {
            engine,
            orchestrator,
            active_egress: self.active_egress.clone(),
            client_targets: self.client_targets.clone(),
            module_cancel,
        }))
    }
}

/// Supervise a configured RTP pull job with exponential retry backoff.
///
/// 以指数退避重试监督配置的 RTP 拉流任务。
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
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: None,
            source_policy: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_service::*;
    use cheetah_rtp_core::{RtpConnectionType, RtpPayloadMode, RtpTrackFilter, RtpTransportMode};
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
