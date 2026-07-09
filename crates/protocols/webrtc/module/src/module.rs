//! WebRTC module factory and lifecycle.

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_codec::MonoTime;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRouteDescriptor, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleHttpService, ModuleId, ModuleInfo,
    ModuleInitContext, ModuleManifest, ModuleSchemaRegistration, ModuleState, SdkError, StreamKey,
};
use cheetah_webrtc_core::{
    MidLabel, WebRtcCloseReason, WebRtcRequestKeyframeKind, WebRtcSessionId, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{spawn_driver, WebRtcDriverEvent, WebRtcDriverHandle};
use futures::FutureExt;
use parking_lot::Mutex;
use tracing::{debug, error, info, warn};

use crate::bridge::{WebRtcBridgeRegistry, WebRtcPublishBridge};
use crate::compat::{
    parse_ome_webrtc_path_query_with_default_transport, OmeDirection, OmeTransportMode,
};
use crate::config::WebRtcModuleConfig;
use crate::http::{AnswerDispatcher, WebRtcHttpService};
use crate::ome_signaling::{
    handle_established_message, handle_request_offer, render_error_response, OmeWsRequestOfferInput,
};
use crate::ome_ws::{
    run_ome_ws_server, OmeWsConnectionHandler, OmeWsInboundConnection, OmeWsServerConfig,
    WebSocketOmeTransport,
};
use crate::session::{WebRtcModuleSessionState, WebRtcSessionIdAllocator, WebRtcSessionRegistry};

const MODULE_ID: &str = "webrtc";
const ROUTES_PREFIX: &str = "/api/v1/rtc";

pub struct WebRtcModuleFactory;

impl ModuleFactory for WebRtcModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "WebRTC Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "webrtc".to_string(),
            routes_prefix: ROUTES_PREFIX.to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(WebRtcModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "webrtc-module".to_string(),
            default_value: WebRtcModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg = WebRtcModuleConfig::from_value(value.clone())?;
                cfg.validate()
            })),
        })
    }
}

pub struct WebRtcModule {
    state: ModuleState,
    config: Arc<Mutex<WebRtcModuleConfig>>,
    ctx: Option<EngineContext>,
    driver: Arc<Mutex<Option<Arc<WebRtcDriverHandle>>>>,
    cancel: Option<CancellationToken>,
    allocator: Arc<WebRtcSessionIdAllocator>,
    registry: Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    answer_dispatcher: Arc<AnswerDispatcher>,
    jobs: Arc<Mutex<crate::jobs::WebRtcJobRegistry>>,
    http_client: crate::http_client::WhipWhepHttpClient,
    metrics: Arc<crate::metrics::WebRtcModuleMetrics>,
    /// P2P signaling room keeper registry. Phase 05 follow-up:
    /// stores keeper bookkeeping for the upcoming WebSocket
    /// signaling client. Today the registry is reachable through
    /// the `/api/v1/rtc/p2p/keeper/*` HTTP endpoints; the actual
    /// keeper task that drives reconnect/check-in/etc. is wired in
    /// the next round.
    keepers: Arc<crate::p2p::P2pRoomKeeperRegistry>,
    /// Lifecycle dispatcher fed by the driver event worker. Cheap
    /// to clone; the upcoming P2P bridge tasks subscribe to it via
    /// `BridgeLifecycleSource`.
    lifecycle_dispatcher: Arc<crate::p2p::LifecycleDispatcher>,
    /// P2P client job registry — backs `/pull/start` and
    /// `/push/start` for `signaling_protocols=1` URLs. The HTTP
    /// service consults the registry when handling P2P URLs and
    /// spawns background supervisor tasks via `p2p_jobs::spawn`.
    p2p_jobs: Arc<crate::p2p_jobs::P2pClientJobRegistry>,
    /// Per-session "previous Stats snapshot" cache. Used by the
    /// event worker to compute deltas between consecutive
    /// `WebRtcCoreEvent::Stats` messages so the aggregate counters
    /// stay strictly increasing.
    last_session_stats: Arc<
        Mutex<
            std::collections::HashMap<
                cheetah_webrtc_core::WebRtcSessionId,
                cheetah_webrtc_core::WebRtcSessionStats,
            >,
        >,
    >,
}

impl WebRtcModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Arc::new(Mutex::new(WebRtcModuleConfig::default())),
            ctx: None,
            driver: Arc::new(Mutex::new(None)),
            cancel: None,
            allocator: Arc::new(WebRtcSessionIdAllocator::new()),
            registry: Arc::new(Mutex::new(WebRtcSessionRegistry::default())),
            bridges: Arc::new(Mutex::new(WebRtcBridgeRegistry::default())),
            answer_dispatcher: Arc::new(AnswerDispatcher::new()),
            jobs: Arc::new(Mutex::new(crate::jobs::WebRtcJobRegistry::default())),
            http_client: crate::http_client::WhipWhepHttpClient::new(),
            metrics: crate::metrics::WebRtcModuleMetrics::new(),
            keepers: Arc::new(crate::p2p::P2pRoomKeeperRegistry::default()),
            lifecycle_dispatcher: crate::p2p::LifecycleDispatcher::new(),
            p2p_jobs: crate::p2p_jobs::P2pClientJobRegistry::new(),
            last_session_stats: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Operator-facing snapshot of the documented metrics surface
    /// (phase-04 §4.8). Combines monotonic counters bumped by the
    /// driver event worker with live gauges from the session
    /// registry. Cheap to call from a Prometheus exporter / admin
    /// HTTP route — atomic loads only, plus one read of the
    /// registry's HashMap length.
    pub fn metrics_snapshot(&self) -> crate::metrics::WebRtcModuleMetricsSnapshot {
        let counters = self.metrics.snapshot_counters();
        let (sessions_active, publish_sessions, play_sessions) = {
            use cheetah_webrtc_core::WebRtcSessionRole;
            let reg = self.registry.lock();
            let mut publish = 0usize;
            let mut play = 0usize;
            for s in reg.sessions.values() {
                match s.role {
                    WebRtcSessionRole::Publisher => publish += 1,
                    WebRtcSessionRole::Player => play += 1,
                    WebRtcSessionRole::Bidirectional => {
                        // P2P sessions count as both publish and play
                        // because they hold both lease + subscriber.
                        publish += 1;
                        play += 1;
                    }
                }
            }
            (reg.sessions.len(), publish, play)
        };
        crate::metrics::WebRtcModuleMetricsSnapshot::assemble(
            counters,
            sessions_active,
            publish_sessions,
            play_sessions,
        )
    }

    /// Lifecycle dispatcher fed by the driver event worker. Public
    /// so external code (e.g. the P2P pull/push entry path) can hand
    /// this to `run_bridge_with_lifecycle` as a `BridgeLifecycleSource`.
    pub fn lifecycle_dispatcher(&self) -> Arc<crate::p2p::LifecycleDispatcher> {
        self.lifecycle_dispatcher.clone()
    }
}

impl Default for WebRtcModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for WebRtcModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "WebRTC Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let cfg = WebRtcModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(SdkError::InvalidArgument)?;
        cfg.validate().map_err(SdkError::InvalidArgument)?;
        *self.config.lock() = cfg;
        self.ctx = Some(ctx.engine);
        self.cancel = Some(CancellationToken::new());
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        let cfg = self.config.lock().clone();
        if !cfg.enabled {
            self.state = ModuleState::Running;
            return Ok(());
        }

        let module_cancel = self.cancel.clone().unwrap_or_default();
        if self.cancel.is_none() {
            self.cancel = Some(module_cancel.clone());
        }
        let ctx = self
            .ctx
            .clone()
            .ok_or_else(|| SdkError::InvalidArgument("WebRtcModule started before init".into()))?;

        // Bridge engine root cancel into our module-scoped cancel.
        {
            let module_cancel_in = module_cancel.clone();
            let module_cancel_out = module_cancel.clone();
            let cancel_outer = cancel.clone();
            ctx.runtime_api.spawn(Box::pin(async move {
                let engine = cancel_outer.cancelled().fuse();
                let stop = module_cancel_in.cancelled().fuse();
                futures::pin_mut!(engine, stop);
                futures::select_biased! {
                    _ = engine => module_cancel_out.cancel(),
                    _ = stop => {}
                }
            }));
        }
        let cancel = module_cancel.clone();

        let driver_config = cfg.to_driver_config().map_err(SdkError::InvalidArgument)?;
        let handle = match spawn_driver(driver_config, cancel.clone()).await {
            Ok(h) => h,
            Err(err) => {
                error!("WebRTC driver bind failed: {err}");
                return Err(SdkError::Internal(format!(
                    "webrtc driver bind failed: {err}"
                )));
            }
        };
        info!(
            "WebRTC module started: udp={}, sessions={}",
            handle.local_udp_addr(),
            handle.session_count()
        );
        *self.driver.lock() = Some(handle.clone());

        if let Some(listen) = cfg.ome_ws_listen.clone() {
            let (listener, local_addr) = cheetah_webrtc_driver_tokio::bind_ws_server(&listen)
                .await
                .map_err(|err| {
                    SdkError::Internal(format!("OME WebSocket bind failed on {listen}: {err}"))
                })?;
            let handler = build_ome_ws_connection_handler(
                handle.clone(),
                self.answer_dispatcher.clone(),
                self.registry.clone(),
                self.bridges.clone(),
                self.allocator.clone(),
                self.config.clone(),
                ctx.clone(),
            );
            let server_config = OmeWsServerConfig {
                max_connections: cfg.ome_ws_max_connections,
                accept_timeout: std::time::Duration::from_millis(cfg.ome_ws_handshake_timeout_ms),
                ..Default::default()
            };
            let cancel_for_ws = cancel.clone();
            ctx.runtime_api.spawn(Box::pin(async move {
                if let Err(err) =
                    run_ome_ws_server(listener, server_config, handler, cancel_for_ws).await
                {
                    warn!("OME WebSocket server stopped with error: {err}");
                }
            }));
            info!("OME WebSocket signaling listener started: {local_addr}");
        }

        if cfg.fir_interval_ms > 0 {
            let handle_for_fir = handle.clone();
            let registry_for_fir = self.registry.clone();
            let bridges_for_fir = self.bridges.clone();
            let cancel_for_fir = cancel.clone();
            let interval_ms = cfg.fir_interval_ms;
            let ctx_for_fir = ctx.clone();
            ctx.runtime_api.spawn(Box::pin(async move {
                run_periodic_fir_worker(
                    handle_for_fir,
                    registry_for_fir,
                    bridges_for_fir,
                    ctx_for_fir,
                    cancel_for_fir,
                    std::time::Duration::from_millis(interval_ms),
                )
                .await;
            }));
        }

        // Spawn driver event worker. The worker terminates when the
        // module-scoped cancel token fires, so it does not keep the
        // module running past `stop()`.
        {
            let answer_dispatcher = self.answer_dispatcher.clone();
            let registry = self.registry.clone();
            let bridges = self.bridges.clone();
            let cancel = cancel.clone();
            let handle = handle.clone();
            let ctx_for_worker = ctx.clone();
            let metrics = self.metrics.clone();
            let config = self.config.clone();
            let last_session_stats = self.last_session_stats.clone();
            let lifecycle_dispatcher = self.lifecycle_dispatcher.clone();
            ctx.runtime_api.spawn(Box::pin(async move {
                run_driver_event_worker(
                    handle,
                    answer_dispatcher,
                    registry,
                    bridges,
                    ctx_for_worker,
                    cancel,
                    metrics,
                    config,
                    last_session_stats,
                    lifecycle_dispatcher,
                )
                .await;
            }));
        }

        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        *self.driver.lock() = None;
        let removed_sessions: Vec<_> = {
            let mut reg = self.registry.lock();
            reg.sessions.drain().map(|(_, session)| session).collect()
        };
        if let Some(ctx) = self.ctx.as_ref() {
            let min_duration = {
                let cfg = self.config.lock();
                std::time::Duration::from_millis(cfg.play_disconnect_min_duration_ms)
            };
            let now = std::time::Instant::now();
            for session in &removed_sessions {
                crate::play_disconnect::observe_play_session_cleanup(
                    ctx.event_bus.as_ref(),
                    self.metrics.as_ref(),
                    session,
                    crate::play_disconnect::PlayDisconnectReason::ServerShutdown,
                    min_duration,
                    now,
                );
            }
        }
        crate::bridge::close_all(self.bridges.clone());
        self.jobs.lock().cancel_all();
        self.p2p_jobs.stop_all();
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg =
            WebRtcModuleConfig::from_value(change.next).map_err(SdkError::InvalidArgument)?;
        new_cfg.validate().map_err(SdkError::InvalidArgument)?;
        let current = self.config.lock().clone();
        if current == new_cfg {
            return Ok(ConfigEffect::Immediate);
        }
        *self.config.lock() = new_cfg;
        Ok(ConfigEffect::ModuleRestartRequired)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/publish".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/play".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/whip".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/whep".into(),
            },
            // ABL-style WHEP path aliases.
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "//rtc/v1/whep".into(),
            },
            // ABL-compatible OPTIONS preflight for WHEP paths.
            HttpRouteDescriptor {
                method: HttpMethod::Options,
                path: "//rtc/v1/whep".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Options,
                path: "/whep".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Delete,
                path: "/session/*".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Patch,
                path: "/session/*".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/session/*/ice-restart".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/session/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/metrics".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/metrics.json".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/session/*".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/pull/start".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/pull/stop".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/pull/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/push/start".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/push/stop".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/push/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/add".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/remove".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/p2p/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/stop".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/keeper/add".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/keeper/remove".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/p2p/keeper/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/p2p/rooms".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Get,
                path: "/p2p/client/list".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/p2p/client/stop".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/echo/start".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/echo/stop".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/echo".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/zlm".into(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/session/*/datachannel/send".into(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        Some(Arc::new(WebRtcHttpService {
            driver: self.driver.clone(),
            config: self.config.clone(),
            allocator: self.allocator.clone(),
            registry: self.registry.clone(),
            bridges: self.bridges.clone(),
            answer_dispatcher: self.answer_dispatcher.clone(),
            engine: self.ctx.clone(),
            jobs: self.jobs.clone(),
            http_client: self.http_client.clone(),
            jobs_cancel: self.cancel.clone().unwrap_or_default(),
            metrics: self.metrics.clone(),
            keepers: self.keepers.clone(),
            p2p_jobs: self.p2p_jobs.clone(),
            lifecycle_dispatcher: self.lifecycle_dispatcher.clone(),
        }))
    }
}

fn build_ome_ws_connection_handler(
    driver: Arc<WebRtcDriverHandle>,
    answer_dispatcher: Arc<AnswerDispatcher>,
    registry: Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    allocator: Arc<WebRtcSessionIdAllocator>,
    config: Arc<Mutex<WebRtcModuleConfig>>,
    engine: EngineContext,
) -> OmeWsConnectionHandler {
    Arc::new(move |info, transport| {
        let driver = driver.clone();
        let answer_dispatcher = answer_dispatcher.clone();
        let registry = registry.clone();
        let bridges = bridges.clone();
        let allocator = allocator.clone();
        let config = config.clone();
        let engine = engine.clone();
        Box::pin(async move {
            run_ome_ws_connection(
                info,
                transport,
                driver,
                answer_dispatcher,
                registry,
                bridges,
                allocator,
                config,
                engine,
            )
            .await;
        })
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_ome_ws_connection(
    info: OmeWsInboundConnection,
    transport: WebSocketOmeTransport,
    driver: Arc<WebRtcDriverHandle>,
    answer_dispatcher: Arc<AnswerDispatcher>,
    registry: Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    allocator: Arc<WebRtcSessionIdAllocator>,
    config: Arc<Mutex<WebRtcModuleConfig>>,
    engine: EngineContext,
) {
    let (path, query) = split_path_and_query(&info.path_and_query);
    let (default_transport, tcp_relay_force, ice_servers, offer_timeout) = {
        let cfg = config.lock();
        (
            cfg.ome_default_transport_mode()
                .unwrap_or(OmeTransportMode::UdpTcp),
            cfg.ome_tcp_relay_force,
            cfg.ome_ice_servers.clone(),
            std::time::Duration::from_millis(cfg.wait_stream_timeout_ms),
        )
    };
    let target =
        match parse_ome_webrtc_path_query_with_default_transport(path, query, default_transport) {
            Ok(target) => target,
            Err(err) => {
                let _ = transport
                    .send_text(
                        serde_json::json!({
                            "command": "error",
                            "reason": format!("invalid OME WebSocket URL: {err}")
                        })
                        .to_string(),
                    )
                    .await;
                transport.close().await;
                return;
            }
        };

    let first = match transport.recv_message().await {
        Ok(Some(message)) => message,
        Ok(None) => return,
        Err(err) => {
            warn!(
                "OME WebSocket receive failed from {}: {err}",
                info.remote_addr
            );
            transport.close().await;
            return;
        }
    };
    let (request_id, peer_id) = match first {
        crate::ome_signaling::OmeWsMessage::RequestOffer { id, peer_id } => (id, peer_id),
        _ => {
            let _ = transport
                .send_text(
                    serde_json::json!({
                        "command": "error",
                        "reason": "first OME WebSocket message must be request_offer"
                    })
                    .to_string(),
                )
                .await;
            transport.close().await;
            return;
        }
    };

    let session_id = allocator.allocate();
    let stream_key = StreamKey::new(&target.app, &target.stream);
    let publish_bridge = if matches!(target.direction, OmeDirection::Send | OmeDirection::Whip) {
        match acquire_ome_ws_publish_bridge(&engine, &config, stream_key.clone()).await {
            Ok(bridge) => Some(bridge),
            Err(reason) => {
                let _ = transport
                    .send_text(
                        render_error_response(request_id.unwrap_or(0), peer_id, reason)
                            .unwrap_or_else(|err| {
                                format!(r#"{{"command":"error","reason":"{err}"}}"#)
                            }),
                    )
                    .await;
                transport.close().await;
                return;
            }
        }
    } else {
        None
    };
    let waiter = crate::http::OmeAnswerWaiter::new(answer_dispatcher, engine.runtime_api.clone());
    let outcome = match handle_request_offer(
        OmeWsRequestOfferInput {
            target: &target,
            session_id,
            request_id,
            peer_id,
            tcp_relay_force,
            ice_server_configs: &ice_servers,
            offer_timeout,
        },
        &driver,
        &waiter,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            if let Some(bridge) = publish_bridge {
                bridge.close();
            }
            driver
                .send_command(
                    cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                        session_id,
                        reason: WebRtcCloseReason::Internal(err.to_string()),
                    },
                )
                .await;
            let _ = transport
                .send_text(
                    render_error_response(request_id.unwrap_or(0), peer_id, err.to_string())
                        .unwrap_or_else(|err| format!(r#"{{"command":"error","reason":"{err}"}}"#)),
                )
                .await;
            transport.close().await;
            return;
        }
    };
    registry.lock().insert(outcome.session);
    if let Some(bridge) = publish_bridge {
        bridges.lock().insert_publish(session_id, bridge);
    } else {
        spawn_ome_ws_play(
            engine.clone(),
            driver.clone(),
            bridges.clone(),
            config.clone(),
            session_id,
            stream_key,
        )
        .await;
    }
    if let Err(err) = transport.send_text(outcome.response_json).await {
        warn!(
            "OME WebSocket offer send failed for session {} from {}: {err}",
            session_id, info.remote_addr
        );
        driver
            .send_command(
                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal(err.to_string()),
                },
            )
            .await;
        cleanup_ome_ws_allocated_session(&registry, &bridges, session_id);
        return;
    }
    let signaling_id = request_id.unwrap_or(session_id.value());

    loop {
        match transport.recv_message().await {
            Ok(Some(message)) => {
                match handle_established_message(session_id, signaling_id, message, &driver).await {
                    Ok(outcome) => {
                        if outcome.closed {
                            break;
                        }
                    }
                    Err(err) => {
                        warn!(
                            "OME WebSocket signaling message rejected for session {} from {}: {err}",
                            session_id, info.remote_addr
                        );
                        driver
                            .send_command(
                                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                                    session_id,
                                    reason: WebRtcCloseReason::Internal(err.to_string()),
                                },
                            )
                            .await;
                        let _ = transport
                            .send_text(
                                render_error_response(signaling_id, peer_id, err.to_string())
                                    .unwrap_or_else(|err| {
                                        format!(r#"{{"command":"error","reason":"{err}"}}"#)
                                    }),
                            )
                            .await;
                        break;
                    }
                }
            }
            Ok(None) => {
                driver
                    .send_command(
                        cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                            session_id,
                            reason: WebRtcCloseReason::PeerClosed,
                        },
                    )
                    .await;
                break;
            }
            Err(err) => {
                warn!(
                    "OME WebSocket receive failed for session {} from {}: {err}",
                    session_id, info.remote_addr
                );
                driver
                    .send_command(
                        cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                            session_id,
                            reason: WebRtcCloseReason::Internal(err.to_string()),
                        },
                    )
                    .await;
                break;
            }
        }
    }
    transport.close().await;
}

async fn acquire_ome_ws_publish_bridge(
    engine: &EngineContext,
    config: &Arc<Mutex<WebRtcModuleConfig>>,
    stream_key: StreamKey,
) -> Result<WebRtcPublishBridge, String> {
    let (policy, bwe_thresholds, rtcp_based_timestamp) = {
        let cfg = config.lock();
        (
            cfg.simulcast_policy(),
            cfg.bwe_thresholds_bps(),
            cfg.rtcp_based_timestamp,
        )
    };
    WebRtcPublishBridge::acquire(
        &engine.publisher_api,
        stream_key,
        policy,
        bwe_thresholds,
        rtcp_based_timestamp,
    )
    .await
    .map_err(|err| err.to_string())
}

async fn spawn_ome_ws_play(
    engine: EngineContext,
    driver: Arc<WebRtcDriverHandle>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    config: Arc<Mutex<WebRtcModuleConfig>>,
    session_id: WebRtcSessionId,
    stream_key: StreamKey,
) {
    let (
        bootstrap_frames,
        bootstrap_max_age_ms,
        wait_stream_timeout_ms,
        h264_bframe_filter,
        audio_policy,
        timing_policy,
    ) = {
        let cfg = config.lock();
        (
            cfg.bootstrap_frame_count,
            cfg.bootstrap_max_age_ms,
            cfg.wait_stream_timeout_ms,
            cfg.h264_bframe_filter,
            crate::bridge::PlaybackAudioPolicy {
                profile: cfg.codec_profile,
                strategy: cfg.audio_strategy(),
            },
            crate::bridge::PlaybackTimingPolicy {
                jitter_buffer_ms: cfg.play_jitter_buffer_ms,
                playout_delay_min_ms: cfg.playout_delay_min_ms,
                playout_delay_max_ms: cfg.playout_delay_max_ms,
            },
        )
    };
    let cancel = CancellationToken::new();
    bridges.lock().insert_play(session_id, cancel.clone());
    let runtime_api = engine.runtime_api.clone();
    let driver_for_task = driver.clone();
    runtime_api.spawn(Box::pin(async move {
        let start_instant = std::time::Instant::now();
        if let Err(err) = crate::bridge::spawn_play_subscriber(
            engine,
            driver_for_task.clone(),
            bridges,
            session_id,
            stream_key.clone(),
            bootstrap_frames,
            bootstrap_max_age_ms,
            wait_stream_timeout_ms,
            h264_bframe_filter,
            audio_policy,
            timing_policy,
            cancel,
            start_instant,
        )
        .await
        {
            warn!("OME WebSocket play subscriber failed for {stream_key}: {err}");
            driver_for_task
                .send_command(
                    cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession {
                        session_id,
                        reason: WebRtcCloseReason::Internal(err.to_string()),
                    },
                )
                .await;
        }
    }));
}

fn cleanup_ome_ws_allocated_session(
    registry: &Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: &Arc<Mutex<WebRtcBridgeRegistry>>,
    session_id: WebRtcSessionId,
) {
    registry.lock().remove(session_id);
    let mut bridges = bridges.lock();
    if let Some(bridge) = bridges.remove_publish(session_id) {
        bridge.close();
    }
    if let Some(cancel) = bridges.remove_play(session_id) {
        cancel.cancel();
    }
}

fn split_path_and_query(path_and_query: &str) -> (&str, Option<&str>) {
    match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    }
}

fn rewrite_local_sdp_for_compat(sdp: &str, cfg: &WebRtcModuleConfig) -> String {
    let mut rewritten = sdp.to_string();
    if cfg.play_jitter_buffer_ms != 0
        || cfg.playout_delay_min_ms != 0
        || cfg.playout_delay_max_ms != 0
    {
        rewritten = cheetah_webrtc_core::sdp_compat::ensure_playout_delay_extmap(&rewritten);
    }
    if !cfg.enable_red_ulpfec {
        rewritten = cheetah_webrtc_core::sdp_compat::strip_red_ulpfec_payloads(&rewritten);
    }
    rewritten
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PeriodicFirTarget {
    RemoteWebRtc {
        session_id: WebRtcSessionId,
        mid: MidLabel,
    },
    UpstreamStream {
        stream_key: StreamKey,
    },
}

fn collect_periodic_fir_targets(
    sessions: &[(WebRtcSessionId, WebRtcSessionRole, StreamKey)],
    bridges: &WebRtcBridgeRegistry,
) -> Vec<PeriodicFirTarget> {
    let mut targets = Vec::new();
    let mut upstream_seen = std::collections::BTreeSet::<StreamKey>::new();
    for (session_id, role, stream_key) in sessions {
        if matches!(
            role,
            WebRtcSessionRole::Publisher | WebRtcSessionRole::Bidirectional
        ) {
            if let Some(mid) =
                bridges.play_track_for(*session_id, cheetah_webrtc_core::WebRtcMediaKind::Video)
            {
                targets.push(PeriodicFirTarget::RemoteWebRtc {
                    session_id: *session_id,
                    mid,
                });
            }
        }
        if matches!(
            role,
            WebRtcSessionRole::Player | WebRtcSessionRole::Bidirectional
        ) && upstream_seen.insert(stream_key.clone())
        {
            targets.push(PeriodicFirTarget::UpstreamStream {
                stream_key: stream_key.clone(),
            });
        }
    }
    targets
}

async fn run_periodic_fir_worker(
    handle: Arc<WebRtcDriverHandle>,
    registry: Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    ctx: EngineContext,
    cancel: CancellationToken,
    interval: std::time::Duration,
) {
    if interval.is_zero() {
        return;
    }
    let interval_us = u64::try_from(interval.as_micros()).unwrap_or(u64::MAX);
    loop {
        let deadline = MonoTime::from_micros(
            ctx.runtime_api
                .now()
                .as_micros()
                .saturating_add(interval_us),
        );
        let mut timer = ctx.runtime_api.sleep_until(deadline);
        let cancelled = cancel.cancelled().fuse();
        let tick = timer.wait().fuse();
        futures::pin_mut!(cancelled, tick);
        let ticked = futures::select_biased! {
            _ = cancelled => false,
            _ = tick => true,
        };
        if !ticked {
            break;
        }
        let connected_sessions: Vec<(WebRtcSessionId, WebRtcSessionRole, StreamKey)> = {
            let reg = registry.lock();
            reg.sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    if !matches!(session.state, WebRtcModuleSessionState::Connected) {
                        return None;
                    }
                    Some((*session_id, session.role, session.stream_key.clone()))
                })
                .collect()
        };
        let targets = {
            let guard = bridges.lock();
            collect_periodic_fir_targets(&connected_sessions, &guard)
        };
        for target in targets {
            match target {
                PeriodicFirTarget::RemoteWebRtc { session_id, mid } => {
                    handle
                        .send_command(
                            cheetah_webrtc_driver_tokio::WebRtcDriverCommand::RequestKeyframe {
                                session_id,
                                mid,
                                kind: WebRtcRequestKeyframeKind::Fir,
                            },
                        )
                        .await;
                }
                PeriodicFirTarget::UpstreamStream { stream_key } => {
                    if let Err(err) = ctx.stream_manager_api.request_keyframe(&stream_key).await {
                        debug!(
                            "periodic FIR upstream keyframe request failed for {stream_key}: {err}"
                        );
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_driver_event_worker(
    handle: Arc<WebRtcDriverHandle>,
    dispatcher: Arc<AnswerDispatcher>,
    registry: Arc<Mutex<WebRtcSessionRegistry>>,
    bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    ctx: cheetah_sdk::EngineContext,
    cancel: CancellationToken,
    metrics: Arc<crate::metrics::WebRtcModuleMetrics>,
    config: Arc<Mutex<WebRtcModuleConfig>>,
    last_session_stats: Arc<
        Mutex<
            std::collections::HashMap<
                cheetah_webrtc_core::WebRtcSessionId,
                cheetah_webrtc_core::WebRtcSessionStats,
            >,
        >,
    >,
    lifecycle_dispatcher: Arc<crate::p2p::LifecycleDispatcher>,
) {
    use cheetah_webrtc_core::{
        WebRtcCoreEvent, WebRtcMediaEvent, WebRtcRtcpFeedback, WebRtcSessionLifecycle,
    };

    loop {
        let evt = {
            let cancelled = cancel.cancelled().fuse();
            let recv = handle.recv_event().fuse();
            futures::pin_mut!(cancelled, recv);
            futures::select_biased! {
                _ = cancelled => break,
                evt = recv => evt,
            }
        };
        match evt {
            Some(WebRtcDriverEvent::AnswerReady { session_id, sdp }) => {
                debug!("WebRTC driver delivered answer for {session_id}");
                let sdp = {
                    let cfg = config.lock();
                    rewrite_local_sdp_for_compat(&sdp, &cfg)
                };
                dispatcher.deliver_sdp(session_id, sdp);
            }
            Some(WebRtcDriverEvent::OfferReady { session_id, sdp }) => {
                debug!("WebRTC driver delivered local offer for {session_id}");
                // Both server-side answers (`AnswerReady`) and
                // client-side offers (`OfferReady`) are delivered
                // to the same per-session waiter; the supervisor
                // / HTTP handler that subscribed for `session_id`
                // can disambiguate by which command it dispatched.
                let sdp = {
                    let cfg = config.lock();
                    rewrite_local_sdp_for_compat(&sdp, &cfg)
                };
                dispatcher.deliver_sdp(session_id, sdp);
            }
            Some(WebRtcDriverEvent::SessionClosed { session_id, reason }) => {
                debug!("WebRTC driver closed session {session_id}: {reason:?}");
                let mut reg = registry.lock();
                if let Some(session) = reg.sessions.get_mut(&session_id) {
                    session.state = WebRtcModuleSessionState::Closed;
                }
                let removed = reg.remove(session_id);
                drop(reg);
                let mut bridges_guard = bridges.lock();
                if let Some(bridge) = bridges_guard.remove_publish(session_id) {
                    bridge.close();
                }
                if let Some(cancel) = bridges_guard.remove_play(session_id) {
                    cancel.cancel();
                }
                drop(bridges_guard);
                if let Some(session) = removed.as_ref() {
                    let min_duration = {
                        let cfg = config.lock();
                        std::time::Duration::from_millis(cfg.play_disconnect_min_duration_ms)
                    };
                    let play_reason =
                        crate::play_disconnect::close_reason_to_play_disconnect_reason(&reason);
                    crate::play_disconnect::observe_play_session_cleanup(
                        ctx.event_bus.as_ref(),
                        metrics.as_ref(),
                        session,
                        play_reason,
                        min_duration,
                        std::time::Instant::now(),
                    );
                }
                // Drop the cached previous stats snapshot so
                // a future session id reuse does not leak old
                // counters into the delta calculation.
                last_session_stats.lock().remove(&session_id);
                // If a request was still waiting for an answer at
                // this point, surface the failure.
                dispatcher.deliver_failure(
                    session_id,
                    format!("session closed before answer: {reason:?}"),
                );
            }
            Some(WebRtcDriverEvent::Core(event)) => {
                match &event {
                    WebRtcCoreEvent::Lifecycle { session_id, state } => {
                        // Phase 05 follow-up: feed `Connected`
                        // / `Closed` lifecycle transitions to
                        // any `run_bridge_with_lifecycle`
                        // subscriber. WHIP/WHEP and SMS-style
                        // sessions don't subscribe and the
                        // dispatcher silently drops their
                        // events.
                        match state {
                            WebRtcSessionLifecycle::Connected => {
                                lifecycle_dispatcher.deliver_connected(*session_id);
                            }
                            WebRtcSessionLifecycle::Closed => {
                                lifecycle_dispatcher.deliver_closed(*session_id, "session closed");
                            }
                            WebRtcSessionLifecycle::Failed => {
                                lifecycle_dispatcher.deliver_closed(*session_id, "session failed");
                            }
                            WebRtcSessionLifecycle::Created
                            | WebRtcSessionLifecycle::LocalDescriptionReady
                            | WebRtcSessionLifecycle::Disconnected => {
                                // Intermediate states aren't
                                // surfaced to bridges yet; the
                                // job's `Connected` and
                                // `Failed` transitions are
                                // driven by the terminal
                                // events.
                            }
                        }
                    }
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback: WebRtcRtcpFeedback::Pli { .. } | WebRtcRtcpFeedback::Fir { .. },
                    } => {
                        // Phase 04 §4.8: PLI / FIR bump
                        // `pli_total` / `fir_total` on the
                        // aggregator. The per-session
                        // telemetry tracks the same value
                        // separately for `/session/{id}` GET.
                        if matches!(
                            event,
                            WebRtcCoreEvent::RtcpFeedback {
                                feedback: WebRtcRtcpFeedback::Pli { .. },
                                ..
                            }
                        ) {
                            metrics
                                .pli
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            metrics
                                .fir
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        let stream_key_opt = {
                            let reg = registry.lock();
                            reg.sessions.get(session_id).map(|s| s.stream_key.clone())
                        };
                        if let Some(stream_key) = stream_key_opt {
                            let stream_manager = ctx.stream_manager_api.clone();
                            ctx.runtime_api.spawn(Box::pin(async move {
                                if let Err(err) = stream_manager.request_keyframe(&stream_key).await
                                {
                                    debug!("stream_manager.request_keyframe failed: {err}");
                                }
                            }));
                        }
                    }
                    WebRtcCoreEvent::RtpExtensionObserved {
                        session_id,
                        mappings,
                    } => {
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.record_rtp_extensions(mappings.clone());
                        }
                    }
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback: WebRtcRtcpFeedback::SenderReport,
                    } => {
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.inc_rtcp_sr();
                        }
                    }
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback: WebRtcRtcpFeedback::ReceiverReport,
                    } => {
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.inc_rtcp_rr();
                        }
                    }
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback: WebRtcRtcpFeedback::Nack { count, .. },
                    } => {
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.add_rtcp_nack(*count);
                        }
                    }
                    WebRtcCoreEvent::RtcpFeedback {
                        session_id,
                        feedback: WebRtcRtcpFeedback::Remb { bitrate_bps, .. },
                    } => {
                        let auto_abr = { config.lock().webrtc_auto_abr };
                        // Phase 04: surface remote REMB so
                        // operators see the receiver's view of
                        // the bitrate ceiling alongside the
                        // local BWE estimate.
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.record_remb(*bitrate_bps);
                        }
                        drop(reg);
                        // Thread the REMB cap into the publish
                        // bridge so the adaptive simulcast
                        // policy uses `min(bwe, remb)` instead
                        // of silently overshooting the
                        // receiver-suggested ceiling.
                        if auto_abr {
                            bridges
                                .lock()
                                .set_publish_remb_cap(*session_id, *bitrate_bps);
                        }
                        metrics.record_remb(*bitrate_bps);
                    }
                    WebRtcCoreEvent::Stats {
                        session_id,
                        snapshot,
                    } => {
                        // Phase 04: aggregate ingress/egress
                        // stats. `core` emits both directions
                        // separately so we merge instead of
                        // overwriting.
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.merge_stats(snapshot);
                        }
                        drop(reg);
                        // Compute deltas vs. the previous
                        // Stats sample for the same session
                        // and add them to the aggregator. The
                        // delta is monotonic by construction
                        // because str0m's per-session counters
                        // are cumulative.
                        let delta = {
                            let mut last = last_session_stats.lock();
                            let prev = last
                                .insert(*session_id, snapshot.clone())
                                .unwrap_or_default();
                            crate::metrics::WebRtcSessionStatsDelta {
                                packets_in: snapshot.packets_in.saturating_sub(prev.packets_in),
                                packets_out: snapshot.packets_out.saturating_sub(prev.packets_out),
                                nack_in: snapshot.nack_in.saturating_sub(prev.nack_in),
                                nack_out: snapshot.nack_out.saturating_sub(prev.nack_out),
                                rtx_sent: snapshot.rtx_sent.saturating_sub(prev.rtx_sent),
                                rtx_miss: snapshot.rtx_miss.saturating_sub(prev.rtx_miss),
                                // PLI/FIR are tracked
                                // separately by the
                                // RtcpFeedback arm; do not
                                // double-count here.
                                pli: 0,
                                fir: 0,
                            }
                        };
                        metrics.add_stats_delta(&delta);
                        // Feed the egress NACK counter into
                        // the publish bridge's storm
                        // detector. `nack_in` is cumulative
                        // (str0m emits the running total), so
                        // the bridge tracks the delta between
                        // samples to detect a burst. A storm
                        // pins the simulcast policy to the
                        // lowest layer for a recovery window.
                        if snapshot.nack_in != 0 {
                            let storm_tripped = bridges
                                .lock()
                                .record_publish_nack_in(*session_id, snapshot.nack_in);
                            if storm_tripped {
                                warn!(
                                            "WebRTC NACK storm detected on session {}: \
                                             nack_in={}, simulcast pinned to lowest layer for recovery window",
                                            session_id, snapshot.nack_in
                                        );
                            }
                        }
                    }
                    WebRtcCoreEvent::Bwe {
                        session_id,
                        snapshot,
                    } => {
                        let auto_abr = { config.lock().webrtc_auto_abr };
                        let mut reg = registry.lock();
                        if let Some(session) = reg.sessions.get_mut(session_id) {
                            session.telemetry.merge_bwe(snapshot);
                        }
                        drop(reg);
                        // Thread the estimate into the publish
                        // bridge so `SimulcastPolicy::Adaptive`
                        // can re-elect the layer on the next
                        // inbound frame. We only forward the
                        // primary `estimated_bitrate_bps`; the
                        // separate `target_bitrate_bps` is
                        // surfaced through telemetry already.
                        if auto_abr {
                            if let Some(bps) = snapshot.estimated_bitrate_bps {
                                bridges.lock().set_publish_bwe_estimate(*session_id, bps);
                            }
                        }
                        if let Some(bps) = snapshot.estimated_bitrate_bps {
                            metrics.record_bwe(bps);
                        }
                        // Phase 04 §4.8: TWCC feedback delivery
                        // counter. `core` emits a `Bwe` event
                        // for every TWCC-driven estimate
                        // refresh, so we treat that as a
                        // feedback delivery sample.
                        metrics.inc_twcc_feedback();
                    }
                    WebRtcCoreEvent::MediaTrackAdded { session_id, track } => {
                        bridges.lock().record_play_track(
                            *session_id,
                            track.mid.clone(),
                            track.kind,
                        );
                    }
                    WebRtcCoreEvent::Media {
                        session_id,
                        event: media_evt,
                    } => {
                        if let WebRtcMediaEvent::Frame { .. } = media_evt {
                            let mut bridges_guard = bridges.lock();
                            let _ =
                                bridges_guard.push_publish_frame(*session_id, media_evt.clone());
                            // If the adaptive simulcast policy just
                            // upgraded to a higher layer, request a
                            // keyframe so the new layer starts with
                            // a decodable frame (PLI).
                            if bridges_guard.take_publish_layer_upgrade(*session_id) {
                                let stream_key_opt = {
                                    let reg = registry.lock();
                                    reg.sessions.get(session_id).map(|s| s.stream_key.clone())
                                };
                                if let Some(stream_key) = stream_key_opt {
                                    let stream_manager = ctx.stream_manager_api.clone();
                                    ctx.runtime_api.spawn(Box::pin(async move {
                                        let _ = stream_manager.request_keyframe(&stream_key).await;
                                    }));
                                }
                            }
                            // MultiStream: check if new RIDs need
                            // sub-stream sinks acquired. We collect
                            // pending RIDs under the lock, then
                            // spawn async acquisition outside.
                            let pending_rids = bridges_guard.pending_multistream_rids(*session_id);
                            drop(bridges_guard);
                            if !pending_rids.is_empty() {
                                let bridges_clone = bridges.clone();
                                let sid = *session_id;
                                // Collect the publisher_api and stream_key
                                // from the bridge while we have the lock,
                                // and mark RIDs as in-flight to prevent
                                // duplicate acquire calls.
                                let acquire_info = {
                                    let mut guard = bridges_clone.lock();
                                    if let Some(b) = guard.publish_mut(sid) {
                                        b.mark_multistream_inflight(&pending_rids);
                                        b.publisher_api_and_stream_key()
                                    } else {
                                        None
                                    }
                                };
                                if let Some((pub_api, base_key)) = acquire_info {
                                    ctx.runtime_api.spawn(Box::pin(async move {
                                                for rid in pending_rids {
                                                    let sub_key = crate::bridge::derive_multistream_key(&base_key, &rid);
                                                    match pub_api
                                                        .acquire_publisher(sub_key.clone(), cheetah_sdk::PublisherOptions::default())
                                                        .await
                                                    {
                                                        Ok((lease, sink)) => {
                                                            let mut guard = bridges_clone.lock();
                                                            if let Some(bridge) = guard.publish_mut(sid) {
                                                                bridge.insert_multistream_sink(rid, sub_key, lease, sink);
                                                            }
                                                        }
                                                        Err(e) => {
                                                            tracing::warn!(
                                                                "MultiStream sink acquire for rid={rid} session={sid} failed: {e}"
                                                            );
                                                            // Clear in-flight marker so the
                                                            // RID becomes pending again on
                                                            // the next frame.
                                                            let mut guard = bridges_clone.lock();
                                                            if let Some(bridge) = guard.publish_mut(sid) {
                                                                bridge.clear_multistream_inflight(&rid);
                                                            }
                                                        }
                                                    }
                                                }
                                            }));
                                }
                            }
                        }
                    }
                    WebRtcCoreEvent::DataChannel {
                        session_id,
                        event: dc_evt,
                    } => {
                        use cheetah_webrtc_core::{WebRtcDataChannelEvent, WebRtcDataChannelOut};
                        if let WebRtcDataChannelEvent::Message {
                            id,
                            payload,
                            binary,
                        } = dc_evt
                        {
                            let echo_enabled = {
                                let reg = registry.lock();
                                reg.sessions
                                    .get(session_id)
                                    .map(|s| s.echo.data_channel)
                                    .unwrap_or(false)
                            };
                            if echo_enabled {
                                let out = WebRtcDataChannelOut {
                                    session_id: *session_id,
                                    channel: *id,
                                    payload: payload.clone(),
                                    binary: *binary,
                                };
                                let driver_handle = handle.clone();
                                ctx.runtime_api.spawn(Box::pin(async move {
                                            driver_handle
                                                .send_command(
                                                    cheetah_webrtc_driver_tokio::WebRtcDriverCommand::SendDataChannel(out),
                                                )
                                                .await;
                                        }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(WebRtcDriverEvent::RouteUpdated(update)) => {
                debug!(
                    "WebRTC route migration session={} new={}",
                    update.session_id, update.new_addr
                );
                metrics.inc_route_migration();
            }
            Some(WebRtcDriverEvent::TcpAccepted { remote_addr }) => {
                debug!("WebRTC TCP peer connected: {remote_addr}");
            }
            Some(WebRtcDriverEvent::TcpClosed {
                remote_addr,
                reason,
            }) => {
                debug!("WebRTC TCP peer disconnected: {remote_addr} ({reason:?})");
            }
            Some(WebRtcDriverEvent::Diagnostic(diag)) => {
                if matches!(
                    diag.kind,
                    cheetah_webrtc_driver_tokio::WebRtcDriverDiagnosticKind::Lifecycle
                ) {
                    // Lifecycle failures with a session id should
                    // bubble up to any pending HTTP waiter so
                    // they fail fast instead of timing out. The
                    // dispatcher silently drops the failure when
                    // there is no subscriber for that session.
                    if let Some(session_id) = diag.session_id {
                        if diag.message.contains("failed") {
                            dispatcher.deliver_failure(session_id, diag.message.clone());
                        }
                    }
                } else {
                    warn!("WebRTC driver diagnostic: {} {:?}", diag.message, diag.kind);
                }
            }
            Some(WebRtcDriverEvent::Backpressure { queue, pending }) => {
                warn!("WebRTC driver backpressure on {queue}: {pending} pending");
            }
            Some(WebRtcDriverEvent::ShardStopped { shard_id, reason }) => {
                // Operators rely on this signal to tell
                // graceful drain ("cancelled" / "exited")
                // apart from a crashed shard ("panic: ..").
                // We log at warn for non-cancellation
                // reasons so cancellation noise stays
                // low-priority.
                if reason.contains("cancelled") || reason.contains("exited") {
                    debug!("WebRTC shard {shard_id} stopped: {reason}");
                } else {
                    warn!("WebRTC shard {shard_id} stopped unexpectedly: {reason}");
                }
            }
            Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id,
                counts,
            }) => {
                // Phase 02 follow-up §5 / plan-27 task 5.4:
                // Record Prometheus counters BEFORE the debug
                // log so both metric and log fire on every
                // snapshot (dual-write).
                metrics.record_local_candidate_snapshot(counts);

                // Phase 02 follow-up §3 / plan-27 task 3.5:
                // the driver emits a candidate snapshot
                // alongside each local description. Surface
                // the per-type / per-transport / per-family
                // counts as a structured `debug` event so
                // operators can wire them straight into a
                // dashboard. No business logic — logging
                // only.
                tracing::debug!(
                    target: "webrtc.driver",
                    shard_id = %shard_id,
                    session_id = %session_id,
                    host = counts.host,
                    srflx = counts.srflx,
                    prflx = counts.prflx,
                    relay = counts.relay,
                    udp = counts.udp,
                    tcp = counts.tcp,
                    ipv4 = counts.ipv4,
                    ipv6 = counts.ipv6,
                    "local candidate snapshot",
                );
            }
            None => break,
        }
    }
    debug!("WebRTC driver event worker terminated");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::{BridgeLifecycleEvent, BridgeLifecycleSource};
    use cheetah_webrtc_core::WebRtcSessionId;

    /// `WebRtcModule::lifecycle_dispatcher()` must return a working
    /// `BridgeLifecycleSource`. Subscribers receive events delivered
    /// to the dispatcher — this is the contract the upcoming P2P
    /// HTTP entry path (`/pull/start?signaling_protocols=1`) relies
    /// on once the WebSocket transport lands.
    #[tokio::test(flavor = "current_thread")]
    async fn module_exposes_working_lifecycle_dispatcher() {
        let module = WebRtcModule::new();
        let dispatcher = module.lifecycle_dispatcher();
        let session_id = WebRtcSessionId::new(7);
        let mut rx = dispatcher.subscribe(session_id).await;

        // Deliver a `Connected` event through the public accessor.
        dispatcher.deliver_connected(session_id);
        match futures::StreamExt::next(&mut rx).await {
            Some(BridgeLifecycleEvent::Connected) => {}
            other => panic!("expected Connected, got {other:?}"),
        }

        // After a delivered `Closed`, the subscription is removed.
        let session_id_b = WebRtcSessionId::new(8);
        let mut rx_b = dispatcher.subscribe(session_id_b).await;
        dispatcher.deliver_closed(session_id_b, "test");
        match futures::StreamExt::next(&mut rx_b).await {
            Some(BridgeLifecycleEvent::Closed { reason }) => assert_eq!(reason, "test"),
            other => panic!("expected Closed, got {other:?}"),
        }
        // Subscribing again works (entry was cleaned up).
        let _rx_again = dispatcher.subscribe(session_id_b).await;
    }

    #[test]
    fn local_sdp_compat_injects_playout_delay_when_configured() {
        let cfg = WebRtcModuleConfig {
            play_jitter_buffer_ms: 60,
            ..Default::default()
        };
        let input = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=extmap:2 urn:ietf:params:rtp-hdrext:sdes:mid\r\n",
        );
        let out = rewrite_local_sdp_for_compat(input, &cfg);
        assert!(out.contains("http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"));
    }

    #[test]
    fn local_sdp_compat_strips_red_ulpfec_by_default() {
        let cfg = WebRtcModuleConfig::default();
        let input = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 116 117\r\n",
            "a=rtpmap:96 VP8/90000\r\n",
            "a=rtpmap:116 red/90000\r\n",
            "a=rtpmap:117 ulpfec/90000\r\n",
        );
        let out = rewrite_local_sdp_for_compat(input, &cfg);
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n"));
        assert!(!out.contains("a=rtpmap:116 red/90000"));
        assert!(!out.contains("a=rtpmap:117 ulpfec/90000"));
    }

    #[test]
    fn periodic_fir_targets_cover_remote_publishers_and_playback_streams() {
        let publisher_id = WebRtcSessionId::new(11);
        let player_id = WebRtcSessionId::new(12);
        let bidirectional_id = WebRtcSessionId::new(13);
        let stream_key = StreamKey::new("live", "cam");
        let mut bridges = WebRtcBridgeRegistry::default();
        bridges.record_play_track(
            publisher_id,
            MidLabel::new("pub-video"),
            cheetah_webrtc_core::WebRtcMediaKind::Video,
        );
        bridges.record_play_track(
            bidirectional_id,
            MidLabel::new("bidi-video"),
            cheetah_webrtc_core::WebRtcMediaKind::Video,
        );

        let targets = collect_periodic_fir_targets(
            &[
                (
                    publisher_id,
                    WebRtcSessionRole::Publisher,
                    stream_key.clone(),
                ),
                (player_id, WebRtcSessionRole::Player, stream_key.clone()),
                (
                    bidirectional_id,
                    WebRtcSessionRole::Bidirectional,
                    stream_key.clone(),
                ),
            ],
            &bridges,
        );

        assert!(targets.contains(&PeriodicFirTarget::RemoteWebRtc {
            session_id: publisher_id,
            mid: MidLabel::new("pub-video"),
        }));
        assert!(targets.contains(&PeriodicFirTarget::RemoteWebRtc {
            session_id: bidirectional_id,
            mid: MidLabel::new("bidi-video"),
        }));
        let upstream_count = targets
            .iter()
            .filter(|target| {
                matches!(
                    target,
                    PeriodicFirTarget::UpstreamStream { stream_key: key } if key == &stream_key
                )
            })
            .count();
        assert_eq!(upstream_count, 1);
    }

    #[test]
    fn ome_ws_allocated_session_cleanup_removes_registry_and_play_bridge() {
        let session_id = WebRtcSessionId::new(21);
        let stream_key = StreamKey::new("live", "cam");
        let registry = Arc::new(Mutex::new(WebRtcSessionRegistry::default()));
        let bridges = Arc::new(Mutex::new(WebRtcBridgeRegistry::default()));
        registry
            .lock()
            .insert(crate::session::WebRtcModuleSession::new(
                session_id,
                stream_key,
                WebRtcSessionRole::Player,
                crate::session::WebRtcApiKind::OmeWs,
            ));
        let cancel = CancellationToken::new();
        bridges.lock().insert_play(session_id, cancel.clone());

        cleanup_ome_ws_allocated_session(&registry, &bridges, session_id);

        assert!(!registry.lock().sessions.contains_key(&session_id));
        assert!(cancel.is_cancelled());
        assert!(bridges.lock().remove_play(session_id).is_none());
    }
}
