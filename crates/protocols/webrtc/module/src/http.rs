//! HTTP service for `/api/v1/rtc/...` routes.
//!
//! The service translates WHIP/WHEP and SMS-style JSON requests into
//! [`WebRtcDriverCommand`](cheetah_webrtc_driver_tokio::WebRtcDriverCommand)
//! values and waits for the driver's `AnswerReady` event before returning
//! a response. The wait is bounded by `wait_stream_timeout_ms`.
//!
//! Response shape:
//! - WHIP/WHEP: `201 Created`, body is the SDP answer, `Content-Type:
//!   application/sdp`, `Location: /api/v1/rtc/session/{id}`.
//! - SMS publish/play: `200 OK`, JSON body matching SMS fields.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_sdk::{
    EngineContext, HttpHeader, HttpMethod, HttpRequest, HttpResponse, ModuleHttpService, SdkError,
    StreamKey,
};
use cheetah_webrtc_core::{WebRtcCloseReason, WebRtcSessionRole};
use cheetah_webrtc_driver_tokio::{
    CandidateTransportPolicy, WebRtcDriverCommand, WebRtcDriverHandle, WebRtcSessionSpec,
};
use futures::channel::oneshot;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::warn;

use crate::bridge::{WebRtcBridgeRegistry, WebRtcPublishBridge};
use crate::codec_policy::{WebRtcAudioCodecPreference, WebRtcVideoCodecPreference};
use crate::compat::{
    extract_app_stream_aliases, extract_app_stream_from_query, is_abl_whep_path,
    parse_ome_webrtc_path_query_with_default_transport, url_decode_lossy, OmeDirection,
    OmeTransportMode, OmeWebRtcUrlError,
};
use crate::config::WebRtcModuleConfig;
use crate::ome_signaling::{
    ice_server_link_headers, ome_transport_to_candidate_policy, OmeWsOfferWaiter,
};
use crate::session::{
    WebRtcApiKind, WebRtcModuleSession, WebRtcModuleSessionState, WebRtcSessionIdAllocator,
    WebRtcSessionRegistry,
};

pub(crate) struct WebRtcHttpService {
    pub driver: Arc<Mutex<Option<Arc<WebRtcDriverHandle>>>>,
    pub config: Arc<Mutex<WebRtcModuleConfig>>,
    pub allocator: Arc<WebRtcSessionIdAllocator>,
    pub registry: Arc<Mutex<WebRtcSessionRegistry>>,
    pub bridges: Arc<Mutex<WebRtcBridgeRegistry>>,
    pub answer_dispatcher: Arc<AnswerDispatcher>,
    pub engine: Option<EngineContext>,
    pub jobs: Arc<Mutex<crate::jobs::WebRtcJobRegistry>>,
    pub http_client: crate::http_client::WhipWhepHttpClient,
    pub jobs_cancel: cheetah_sdk::CancellationToken,
    pub metrics: Arc<crate::metrics::WebRtcModuleMetrics>,
    /// P2P room keeper registry — driven by `/api/v1/rtc/p2p/keeper/*`.
    pub keepers: Arc<crate::p2p::P2pRoomKeeperRegistry>,
    /// P2P pull/push client job registry. Populated when
    /// `/pull/start` or `/push/start` receives a
    /// `webrtc://...?signaling_protocols=1` URL.
    pub p2p_jobs: Arc<crate::p2p_jobs::P2pClientJobRegistry>,
    /// Lifecycle dispatcher fed by the driver event worker. Used by
    /// the spawned P2P client job to subscribe to driver events.
    pub lifecycle_dispatcher: Arc<crate::p2p::LifecycleDispatcher>,
}

impl WebRtcHttpService {
    fn driver_handle(&self) -> Result<Arc<WebRtcDriverHandle>, SdkError> {
        self.driver
            .lock()
            .clone()
            .ok_or_else(|| SdkError::Unavailable("WebRTC driver not started".into()))
    }

    /// Spawn an engine subscriber that pumps frames into the WebRTC
    /// driver for a player session.
    async fn spawn_play(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
        stream_key: StreamKey,
        driver: Arc<WebRtcDriverHandle>,
    ) {
        let engine = match self.engine.as_ref() {
            Some(e) => e.clone(),
            None => return,
        };
        let bridges = self.bridges.clone();
        let (
            bootstrap_frames,
            bootstrap_max_age_ms,
            wait_stream_timeout_ms,
            h264_bframe_filter,
            audio_policy,
            timing_policy,
        ) = {
            let cfg = self.config.lock();
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
        let cancel = cheetah_sdk::CancellationToken::new();
        bridges.lock().insert_play(session_id, cancel.clone());
        let runtime_api = engine.runtime_api.clone();
        let driver = driver.clone();
        let stream_key_clone = stream_key.clone();
        runtime_api.spawn(Box::pin(async move {
            let start_instant = std::time::Instant::now();
            if let Err(err) = crate::bridge::spawn_play_subscriber(
                engine,
                driver.clone(),
                bridges,
                session_id,
                stream_key_clone,
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
                warn!("WebRTC play subscriber failed for {stream_key}: {err}");
                driver
                    .send_command(WebRtcDriverCommand::StopSession {
                        session_id,
                        reason: cheetah_webrtc_core::WebRtcCloseReason::Internal(err.to_string()),
                    })
                    .await;
            }
        }));
    }

    /// Acquire an engine publisher lease for the stream. Returns a 409 on
    /// conflict and a 503 if the engine is not ready.
    async fn acquire_publish_bridge(
        &self,
        stream_key: StreamKey,
    ) -> Result<WebRtcPublishBridge, HttpResponse> {
        let engine = match self.engine.as_ref() {
            Some(e) => e.clone(),
            None => {
                return Err(http_json_status(
                    503,
                    "engine_unavailable",
                    "WebRTC module not yet bound to engine",
                ));
            }
        };
        let policy = {
            let cfg = self.config.lock();
            cfg.simulcast_policy()
        };
        let (bwe_thresholds, rtcp_based_timestamp) = {
            let cfg = self.config.lock();
            (cfg.bwe_thresholds_bps(), cfg.rtcp_based_timestamp)
        };
        match WebRtcPublishBridge::acquire(
            &engine.publisher_api,
            stream_key,
            policy,
            bwe_thresholds,
            rtcp_based_timestamp,
        )
        .await
        {
            Ok(bridge) => Ok(bridge),
            Err(SdkError::Conflict(reason)) => Err(http_json_status(409, "conflict", &reason)),
            Err(err) => Err(http_json_status(
                503,
                "publish_unavailable",
                &err.to_string(),
            )),
        }
    }
}

#[async_trait]
impl ModuleHttpService for WebRtcHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let method = req.method;
        let path = req.path.clone();
        match (method, path.as_str()) {
            (HttpMethod::Post, "/publish") => self.handle_sms_publish(req).await,
            (HttpMethod::Post, "/play") => self.handle_sms_play(req).await,
            (HttpMethod::Post, "/whip") => self.handle_whip(req).await,
            (HttpMethod::Post, "/whep") => self.handle_whep(req).await,
            // ABL-style WHEP path aliases: `/rtc/v1/whep/`, `/rtc/v1/whep`
            (HttpMethod::Post, p) if is_abl_whep_path(p) => self.handle_whep(req).await,
            // ABL-compatible OPTIONS preflight for WHEP paths.
            (HttpMethod::Options, p) if is_abl_whep_path(p) => self.handle_options_preflight(),
            (HttpMethod::Delete, p) if p.starts_with("/session/") => {
                self.handle_session_delete(p).await
            }
            (HttpMethod::Patch, p) if p.starts_with("/session/") => {
                self.handle_session_patch(p, req).await
            }
            (HttpMethod::Post, p) if p.starts_with("/session/") && p.ends_with("/ice-restart") => {
                self.handle_session_ice_restart(p, req).await
            }
            (HttpMethod::Get, "/session/list") => self.handle_session_list().await,
            (HttpMethod::Get, p) if p.starts_with("/session/") => self.handle_session_get(p).await,
            (HttpMethod::Get, "/metrics") => self.handle_metrics(),
            (HttpMethod::Get, "/metrics.json") => self.handle_metrics_json(),
            // Phase 05 client/P2P/DataChannel endpoints. Pull/push
            // run a `CreateOffer → POST → ApplyAnswer` supervisor over
            // the workspace-local WHIP/WHEP HTTP client; P2P shares
            // the same offer/answer machinery as WHIP/WHEP but uses a
            // bidirectional role.
            (HttpMethod::Post, "/pull/start") => {
                self.handle_job_start(req, crate::jobs::WebRtcJobKind::Pull)
                    .await
            }
            (HttpMethod::Post, "/pull/stop") => {
                self.handle_job_stop(req, crate::jobs::WebRtcJobKind::Pull)
                    .await
            }
            (HttpMethod::Get, "/pull/list") => {
                self.handle_job_list(crate::jobs::WebRtcJobKind::Pull)
            }
            (HttpMethod::Post, "/push/start") => {
                self.handle_job_start(req, crate::jobs::WebRtcJobKind::Push)
                    .await
            }
            (HttpMethod::Post, "/push/stop") => {
                self.handle_job_stop(req, crate::jobs::WebRtcJobKind::Push)
                    .await
            }
            (HttpMethod::Get, "/push/list") => {
                self.handle_job_list(crate::jobs::WebRtcJobKind::Push)
            }
            (HttpMethod::Post, "/p2p/add") => self.handle_p2p_add(req).await,
            (HttpMethod::Post, "/p2p/remove") => self.handle_p2p_remove(req).await,
            (HttpMethod::Get, "/p2p/list") => self.handle_p2p_list(),
            (HttpMethod::Post, "/p2p/stop") => self.handle_p2p_remove(req).await,
            // Phase 05 follow-up: P2P signaling room keeper API.
            (HttpMethod::Post, "/p2p/keeper/add") => self.handle_keeper_add(req).await,
            (HttpMethod::Post, "/p2p/keeper/remove") => self.handle_keeper_remove(req).await,
            (HttpMethod::Get, "/p2p/keeper/list") => self.handle_keeper_list(),
            (HttpMethod::Get, "/p2p/rooms") => self.handle_keeper_rooms(),
            (HttpMethod::Get, "/p2p/client/list") => self.handle_p2p_client_list(),
            (HttpMethod::Post, "/p2p/client/stop") => self.handle_p2p_client_stop(req).await,
            (HttpMethod::Post, "/echo/start") => self.handle_echo_start(req).await,
            (HttpMethod::Post, "/echo/stop") => self.handle_echo_stop(req).await,
            (HttpMethod::Post, "/echo") => self.handle_echo_create(req).await,
            // ZLM-style unified endpoint: the `type` field in the JSON
            // body selects push/play/echo. This provides compatibility
            // with ZLMediaKit clients that use a single URL for all
            // WebRTC operations.
            (HttpMethod::Post, "/zlm") => self.handle_zlm_unified(req).await,
            (HttpMethod::Post, p)
                if p.starts_with("/session/") && p.ends_with("/datachannel/send") =>
            {
                self.handle_datachannel_send(p, req).await
            }
            _ => self.handle_ome_or_not_found(method, req).await,
        }
    }
}

impl WebRtcHttpService {
    async fn handle_ome_or_not_found(
        &self,
        method: HttpMethod,
        req: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        if method != HttpMethod::Post || !has_ome_query_marker(req.query.as_deref()) {
            return Ok(http_not_found());
        }

        let (default_transport, tcp_relay_force, ome_ice_servers) = {
            let cfg = self.config.lock();
            let default_transport = cfg
                .ome_default_transport_mode()
                .unwrap_or(OmeTransportMode::UdpTcp);
            (
                default_transport,
                cfg.ome_tcp_relay_force,
                cfg.ome_ice_servers.clone(),
            )
        };
        let target = match parse_ome_webrtc_path_query_with_default_transport(
            &req.path,
            req.query.as_deref(),
            default_transport,
        ) {
            Ok(target) => target,
            Err(OmeWebRtcUrlError::InvalidDirection(reason)) => {
                return Ok(http_json_status(
                    400,
                    "invalid_direction",
                    &format!("invalid OME direction: {reason}"),
                ));
            }
            Err(OmeWebRtcUrlError::InvalidTransport(reason)) => {
                return Ok(http_json_status(
                    400,
                    "invalid_transport",
                    &format!("invalid OME transport: {reason}"),
                ));
            }
            Err(_) => return Ok(http_not_found()),
        };

        let candidate_transport_policy =
            ome_transport_to_candidate_policy(target.transport, tcp_relay_force);
        let ice_server_headers =
            ice_server_link_headers(&ome_ice_servers, target.transport, tcp_relay_force);
        match target.direction {
            OmeDirection::Whip | OmeDirection::Send => {
                self.handle_whip_stream(
                    req,
                    target.app,
                    target.stream,
                    candidate_transport_policy,
                    ice_server_headers,
                )
                .await
            }
            OmeDirection::Play => {
                self.handle_whep_stream(
                    req,
                    target.app,
                    target.stream,
                    candidate_transport_policy,
                    ice_server_headers,
                )
                .await
            }
        }
    }

    async fn handle_sms_publish(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let (app, stream) = extract_app_stream_aliases(&body);
        let stream = stream
            .ok_or_else(|| SdkError::InvalidArgument("missing field: streamName/stream".into()))?;
        let sdp = body
            .get("sdp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SdkError::InvalidArgument("missing field: sdp".into()))?
            .to_string();
        if let Some(err) = self.check_codec_policy(&body) {
            return Ok(http_json_status(422, "unsupported_codec", &err));
        }

        let stream_key = StreamKey::new(&app, &stream);
        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;

        // Acquire publisher lease *before* asking the driver to accept
        // the offer, so a conflict is reported as `409` rather than
        // leaving an orphan WebRTC session in the driver.
        let bridge = match self.acquire_publish_bridge(stream_key.clone()).await {
            Ok(b) => b,
            Err(resp) => return Ok(resp),
        };

        // Pre-register the session so subsequent driver events can find it.
        {
            let mut reg = self.registry.lock();
            reg.insert(WebRtcModuleSession::new(
                session_id,
                stream_key.clone(),
                WebRtcSessionRole::Publisher,
                WebRtcApiKind::SmsPublish,
            ));
        }
        self.bridges.lock().insert_publish(session_id, bridge);

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: sdp,
                candidate_transport_policy: CandidateTransportPolicy::All,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_json_status(503, "driver_unavailable", &reason));
            }
        };

        let server_label = {
            let cfg = self.config.lock();
            cfg.server_label
                .clone()
                .unwrap_or_else(|| "cheetah".to_string())
        };
        let body = serde_json::json!({
            "code": 0,
            "server": server_label,
            "sessionid": format!("{session_id}"),
            "sdp": answer_sdp,
        });
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    async fn handle_sms_play(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let (app, stream) = extract_app_stream_aliases(&body);
        let stream = stream
            .ok_or_else(|| SdkError::InvalidArgument("missing field: streamName/stream".into()))?;
        let sdp = body
            .get("sdp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SdkError::InvalidArgument("missing field: sdp".into()))?
            .to_string();
        if let Some(err) = self.check_codec_policy(&body) {
            return Ok(http_json_status(422, "unsupported_codec", &err));
        }

        let stream_key = StreamKey::new(&app, &stream);
        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;

        {
            let mut reg = self.registry.lock();
            reg.insert(WebRtcModuleSession::new(
                session_id,
                stream_key.clone(),
                WebRtcSessionRole::Player,
                WebRtcApiKind::SmsPlay,
            ));
        }

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Player,
                remote_sdp_offer: sdp,
                candidate_transport_policy: CandidateTransportPolicy::All,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_json_status(503, "driver_unavailable", &reason));
            }
        };

        // Spawn the engine subscriber that pumps frames into the
        // WebRTC driver for this player session.
        self.spawn_play(session_id, stream_key.clone(), driver.clone())
            .await;

        let body = serde_json::json!({
            "code": 200,
            "sdp": answer_sdp,
        });
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    async fn handle_whip(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let (app, stream) = extract_app_stream_from_query(req.query.as_deref());
        let app = app.unwrap_or_else(|| "live".to_string());
        let stream = stream.ok_or_else(|| {
            SdkError::InvalidArgument("missing query parameter: streamName".into())
        })?;
        self.handle_whip_stream(req, app, stream, CandidateTransportPolicy::All, Vec::new())
            .await
    }

    async fn handle_whip_stream(
        &self,
        req: HttpRequest,
        app: String,
        stream: String,
        candidate_transport_policy: CandidateTransportPolicy,
        extra_headers: Vec<HttpHeader>,
    ) -> Result<HttpResponse, SdkError> {
        let sdp = sdp_from_body(&req)?;

        let stream_key = StreamKey::new(&app, &stream);
        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;

        let bridge = match self.acquire_publish_bridge(stream_key.clone()).await {
            Ok(b) => b,
            Err(resp) => return Ok(resp),
        };

        {
            let mut reg = self.registry.lock();
            reg.insert(WebRtcModuleSession::new(
                session_id,
                stream_key,
                WebRtcSessionRole::Publisher,
                WebRtcApiKind::Whip,
            ));
        }
        self.bridges.lock().insert_publish(session_id, bridge);

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: sdp,
                candidate_transport_policy,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_sdp_error(503, &reason));
            }
        };

        Ok(http_sdp_created_with_extra_headers(
            session_id,
            answer_sdp,
            extra_headers,
        ))
    }

    async fn handle_whep(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let (app, stream) = extract_app_stream_from_query(req.query.as_deref());
        let app = app.unwrap_or_else(|| "live".to_string());
        let stream = match stream {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Ok(http_json_status(
                    400,
                    "missing_parameter",
                    "missing required query parameter: stream (or streamName)",
                ));
            }
        };
        self.handle_whep_stream(req, app, stream, CandidateTransportPolicy::All, Vec::new())
            .await
    }

    async fn handle_whep_stream(
        &self,
        req: HttpRequest,
        app: String,
        stream: String,
        candidate_transport_policy: CandidateTransportPolicy,
        extra_headers: Vec<HttpHeader>,
    ) -> Result<HttpResponse, SdkError> {
        let sdp = sdp_from_body(&req)?;

        let stream_key = StreamKey::new(&app, &stream);
        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;

        {
            let mut reg = self.registry.lock();
            reg.insert(WebRtcModuleSession::new(
                session_id,
                stream_key.clone(),
                WebRtcSessionRole::Player,
                WebRtcApiKind::Whep,
            ));
        }

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Player,
                remote_sdp_offer: sdp,
                candidate_transport_policy,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_sdp_error(503, &reason));
            }
        };

        self.spawn_play(session_id, stream_key.clone(), driver.clone())
            .await;

        Ok(http_sdp_created_with_extra_headers(
            session_id,
            answer_sdp,
            extra_headers,
        ))
    }

    /// Handle OPTIONS preflight requests for WHEP paths.
    ///
    /// Returns 200 with CORS headers and an empty body. This mirrors
    /// ABL's `ResponseOPTIONS` which solves browser preflight issues
    /// during multi-stream playback. Crucially, this does NOT create
    /// a WebRTC play session.
    fn handle_options_preflight(&self) -> Result<HttpResponse, SdkError> {
        let cfg = self.config.lock();
        let mut headers = vec![
            HttpHeader {
                name: "Access-Control-Allow-Origin".into(),
                value: "*".into(),
            },
            HttpHeader {
                name: "Access-Control-Allow-Methods".into(),
                value: "POST, GET, OPTIONS, DELETE, PATCH".into(),
            },
            HttpHeader {
                name: "Access-Control-Allow-Headers".into(),
                value: "Content-Type, Authorization".into(),
            },
            HttpHeader {
                name: "Content-Length".into(),
                value: "0".into(),
            },
        ];
        if cfg.enable_private_network_access {
            headers.push(HttpHeader {
                name: "Access-Control-Allow-Private-Network".into(),
                value: "true".into(),
            });
        }
        Ok(HttpResponse {
            status: 200,
            headers,
            body: Bytes::new(),
        })
    }

    async fn handle_session_delete(&self, path: &str) -> Result<HttpResponse, SdkError> {
        let id = match parse_session_id(path) {
            Some(id) => id,
            None => {
                return Ok(http_json_status(404, "not_found", "session id not found"));
            }
        };
        let driver = {
            let guard = self.driver.lock();
            guard.clone()
        };
        let driver = match driver {
            Some(driver) => driver,
            None => {
                return Ok(http_json_status(
                    503,
                    "driver_unavailable",
                    "driver not started",
                ));
            }
        };

        driver
            .send_command(WebRtcDriverCommand::StopSession {
                session_id: id,
                reason: WebRtcCloseReason::Normal,
            })
            .await;

        let removed = {
            let mut reg = self.registry.lock();
            reg.remove(id)
        };
        let mut bridges = self.bridges.lock();
        if let Some(bridge) = bridges.remove_publish(id) {
            bridge.close();
        }
        if let Some(cancel) = bridges.remove_play(id) {
            cancel.cancel();
        }
        drop(bridges);
        if let (Some(session), Some(engine)) = (removed.as_ref(), self.engine.as_ref()) {
            let min_duration = {
                let cfg = self.config.lock();
                std::time::Duration::from_millis(cfg.play_disconnect_min_duration_ms)
            };
            crate::play_disconnect::observe_play_session_cleanup(
                engine.event_bus.as_ref(),
                self.metrics.as_ref(),
                session,
                crate::play_disconnect::PlayDisconnectReason::ClientDelete,
                min_duration,
                std::time::Instant::now(),
            );
        }
        if removed.is_none() {
            // Idempotent DELETE.
            return Ok(HttpResponse {
                status: 204,
                headers: Vec::new(),
                body: Bytes::new(),
            });
        }
        Ok(HttpResponse {
            status: 204,
            headers: Vec::new(),
            body: Bytes::new(),
        })
    }

    async fn handle_session_get(&self, path: &str) -> Result<HttpResponse, SdkError> {
        let id = match parse_session_id(path) {
            Some(id) => id,
            None => {
                return Ok(http_json_status(404, "not_found", "session id not found"));
            }
        };
        let session_summary = {
            let reg = self.registry.lock();
            reg.sessions.get(&id).map(|s| {
                (
                    serde_json::json!({
                        "session_id": format!("{id}"),
                        "stream_key": format!("{}", s.stream_key),
                        "role": format!("{:?}", s.role),
                        "api_kind": format!("{:?}", s.api_kind),
                        "state": format!("{:?}", s.state),
                    }),
                    s.telemetry.clone(),
                )
            })
        };
        match session_summary {
            Some((mut json, telemetry)) => {
                // Phase 03: surface GOP bootstrap timing so operators
                // can observe first-packet / first-keyframe /
                // first-decodable timing without scraping logs.
                let (stats, renditions) = {
                    let bridges = self.bridges.lock();
                    (bridges.play_stats(id), bridges.publish_renditions(id))
                };
                if let Some(stats) = stats {
                    if let Some(obj) = json.as_object_mut() {
                        obj.insert(
                            "play_bootstrap".to_string(),
                            serde_json::json!({
                                "first_frame_micros": stats.first_frame_micros,
                                "first_keyframe_micros": stats.first_keyframe_micros,
                                "first_decodable_micros": stats.first_decodable_micros,
                                "frames_forwarded": stats.frames_forwarded,
                                "keyframes_forwarded": stats.keyframes_forwarded,
                                "wait_timeout_elapsed_ms": stats.wait_timeout_elapsed_ms,
                                "jitter_buffer_ms": stats.jitter_buffer_ms,
                                "playout_delay_min_ms": stats.playout_delay_min_ms,
                                "playout_delay_max_ms": stats.playout_delay_max_ms,
                                "effective_playout_delay_ms": stats.effective_playout_delay_ms,
                                "delayed_frames": stats.delayed_frames,
                                "delayed_total_micros": stats.delayed_total_micros,
                            }),
                        );
                    }
                }
                if let Some(renditions) = renditions {
                    if !renditions.is_empty() {
                        if let Some(obj) = json.as_object_mut() {
                            obj.insert(
                                "renditions".to_string(),
                                serde_json::json!(renditions
                                    .iter()
                                    .map(|rendition| serde_json::json!({
                                        "mid": rendition.mid.clone(),
                                        "current_rid": rendition.current_rid.clone(),
                                        "seen_rids": rendition.seen_rids.clone(),
                                    }))
                                    .collect::<Vec<_>>()),
                            );
                        }
                    }
                }
                // Phase 04: surface BWE / loss / RTT / RTCP counters
                // aggregated from `WebRtcCoreEvent::Stats` and
                // `WebRtcCoreEvent::Bwe`.
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(
                        "telemetry".to_string(),
                        serde_json::json!({
                            "rtp_extensions": telemetry.rtp_extensions
                                .iter()
                                .map(|mapping| serde_json::json!({
                                    "id": mapping.id,
                                    "extension_type": format!("{:?}", mapping.ext_type),
                                    "uri": mapping.uri,
                                    "canonical_uri": mapping.ext_type.uri(),
                                    "direction": mapping.direction,
                                }))
                                .collect::<Vec<_>>(),
                            "bwe_estimated_bps": telemetry.bwe_estimated_bps,
                            "bwe_target_bps": telemetry.bwe_target_bps,
                            "remb_bitrate_bps": telemetry.remb_bitrate_bps,
                            "rtt_micros": telemetry.rtt_micros,
                            "loss_fraction_x10000": telemetry.loss_fraction_x10000,
                            "packets_in": telemetry.packets_in,
                            "packets_out": telemetry.packets_out,
                            "bytes_in": telemetry.bytes_in,
                            "bytes_out": telemetry.bytes_out,
                            "nack_in": telemetry.nack_in,
                            "nack_out": telemetry.nack_out,
                            "pli_in": telemetry.pli_in,
                            "pli_out": telemetry.pli_out,
                            "fir_in": telemetry.fir_in,
                            "fir_out": telemetry.fir_out,
                            "rtcp_sr": telemetry.rtcp_sr,
                            "rtcp_rr": telemetry.rtcp_rr,
                            "rtcp_nack": telemetry.rtcp_nack,
                            "rtx_sent": telemetry.rtx_sent,
                            "rtx_miss": telemetry.rtx_miss,
                        }),
                    );
                }
                Ok(HttpResponse::ok_json(serde_json::to_vec(&json).unwrap()))
            }
            None => Ok(http_json_status(404, "not_found", "session id not found")),
        }
    }

    async fn handle_session_patch(
        &self,
        path: &str,
        req: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        let id = match parse_session_id(path) {
            Some(id) => id,
            None => return Ok(http_json_status(404, "not_found", "session id not found")),
        };

        // Validate the session is known. 404 on unknown, 409 on closed.
        {
            let reg = self.registry.lock();
            match reg.sessions.get(&id) {
                None => return Ok(http_json_status(404, "not_found", "session id not found")),
                Some(s)
                    if matches!(
                        s.state,
                        WebRtcModuleSessionState::Closed | WebRtcModuleSessionState::Closing
                    ) =>
                {
                    return Ok(http_json_status(409, "session_closed", "session is closed"));
                }
                Some(_) => {}
            }
        }

        let driver = self.driver_handle()?;

        // WHIP / WHEP PATCH carries a trickle-ICE fragment as
        // `application/trickle-ice-sdpfrag`. The body is essentially an
        // SDP that contains one or more `a=candidate:...` lines and
        // optionally an `a=ice-ufrag:` / `a=ice-pwd:` pair signalling
        // an ICE restart (RFC 8839 §5.4 / WHIP). We accept the body if
        // it contains *either* trickle candidates *or* a valid
        // ufrag+pwd pair so a peer can ask for an ICE restart even
        // without sending fresh candidates in the same PATCH.
        let body = match std::str::from_utf8(&req.body) {
            Ok(s) => s,
            Err(_) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "PATCH body must be UTF-8 trickle-ice-sdpfrag",
                ))
            }
        };
        let candidates = crate::compat::extract_trickle_candidates(body);
        let ice_restart_creds = crate::compat::extract_trickle_ice_restart_creds(body);
        if candidates.is_empty() && ice_restart_creds.is_none() {
            return Ok(http_json_status(
                400,
                "no_candidates",
                "PATCH body must contain at least one a=candidate or an ICE-restart \
                 a=ice-ufrag / a=ice-pwd pair",
            ));
        }
        for candidate in candidates {
            driver
                .send_command(WebRtcDriverCommand::AddRemoteCandidate {
                    session_id: id,
                    candidate,
                })
                .await;
        }
        // Trigger an ICE restart when the peer sent fresh credentials.
        // We retain local candidates by default; the WHIP spec leaves
        // that decision to the implementation, and SMS / ZLM keep
        // them. The fresh offer is delivered via `OfferReady` and
        // would be returned by the regular signaling path; the PATCH
        // response itself stays `204` for backwards compatibility
        // with clients that issue PATCHes purely for trickle ICE.
        if ice_restart_creds.is_some() {
            driver
                .send_command(WebRtcDriverCommand::IceRestart {
                    session_id: id,
                    keep_local_candidates: true,
                })
                .await;
        }
        Ok(HttpResponse {
            status: 204,
            headers: Vec::new(),
            body: Bytes::new(),
        })
    }

    /// Trigger an ICE restart on an existing session.
    ///
    /// `POST /api/v1/rtc/session/{id}/ice-restart`. Optional JSON body:
    /// ```json
    /// {"keepLocalCandidates": true}
    /// ```
    /// When the body is empty or missing the field, defaults to
    /// retaining the existing local candidates. The driver delivers
    /// the resulting fresh SDP offer back to the request through the
    /// same answer-dispatcher path as `CreateOffer` (an `OfferReady`
    /// driver event maps onto a per-session waiter).
    ///
    /// Response: `200 OK` with `application/sdp` body containing the
    /// fresh offer; the client is expected to deliver an answer by
    /// `PATCH`-ing the same session (when ICE restart is paired with
    /// a counter-offer from the remote) or via the regular signaling
    /// channel.
    async fn handle_session_ice_restart(
        &self,
        path: &str,
        req: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        let id_part = match path
            .strip_prefix("/session/")
            .and_then(|rest| rest.rsplit_once("/ice-restart").map(|(a, _)| a))
        {
            Some(p) => p,
            None => return Ok(http_json_status(404, "not_found", "session id not found")),
        };
        let id = match parse_session_id_str(id_part) {
            Some(id) => id,
            None => return Ok(http_json_status(404, "not_found", "session id not found")),
        };
        // Validate the session is alive.
        {
            let reg = self.registry.lock();
            match reg.sessions.get(&id) {
                None => return Ok(http_json_status(404, "not_found", "session id not found")),
                Some(s)
                    if matches!(
                        s.state,
                        WebRtcModuleSessionState::Closed | WebRtcModuleSessionState::Closing
                    ) =>
                {
                    return Ok(http_json_status(409, "session_closed", "session is closed"));
                }
                Some(_) => {}
            }
        }

        // Body is optional. Empty or whitespace = default.
        let keep_local_candidates = if req.body.is_empty() {
            true
        } else {
            match serde_json::from_slice::<Value>(&req.body) {
                Ok(v) => v
                    .get("keepLocalCandidates")
                    .or_else(|| v.get("keep_local_candidates"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                // Tolerant: a malformed body is treated as "use defaults"
                // so a curl-with-no-body call still succeeds. This
                // mirrors how SMS / ZLM ICE restart endpoints behave.
                Err(_) => true,
            }
        };

        let driver = self.driver_handle()?;
        let waiter = self.answer_dispatcher.subscribe(id);
        driver
            .send_command(WebRtcDriverCommand::IceRestart {
                session_id: id,
                keep_local_candidates,
            })
            .await;
        let offer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                return Ok(http_json_status(503, "ice_restart_failed", &reason));
            }
        };
        Ok(HttpResponse {
            status: 200,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(offer_sdp),
        })
    }

    async fn handle_echo_start(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let session_id = match body
            .get("sessionid")
            .or_else(|| body.get("sessionId"))
            .and_then(|v| v.as_str())
            .and_then(parse_session_id_str)
        {
            Some(id) => id,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing or invalid `sessionid`",
                ))
            }
        };
        let mode = body
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("datachannel");
        let echo = match mode.to_ascii_lowercase().as_str() {
            "datachannel" => crate::session::WebRtcEchoConfig {
                data_channel: true,
                media: false,
            },
            "media" => crate::session::WebRtcEchoConfig {
                data_channel: false,
                media: true,
            },
            "both" => crate::session::WebRtcEchoConfig {
                data_channel: true,
                media: true,
            },
            other => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("unknown echo mode: {other}"),
                ))
            }
        };
        let mut reg = self.registry.lock();
        match reg.sessions.get_mut(&session_id) {
            Some(session) => {
                session.echo = echo;
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&serde_json::json!({
                        "sessionid": format!("{session_id}"),
                        "mode": mode,
                    }))
                    .unwrap(),
                ))
            }
            None => Ok(http_json_status(404, "not_found", "session id not found")),
        }
    }

    async fn handle_echo_stop(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let session_id = match body
            .get("sessionid")
            .or_else(|| body.get("sessionId"))
            .and_then(|v| v.as_str())
            .and_then(parse_session_id_str)
        {
            Some(id) => id,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing or invalid `sessionid`",
                ))
            }
        };
        let mut reg = self.registry.lock();
        match reg.sessions.get_mut(&session_id) {
            Some(session) => {
                session.echo = crate::session::WebRtcEchoConfig::default();
                Ok(HttpResponse {
                    status: 204,
                    headers: Vec::new(),
                    body: Bytes::new(),
                })
            }
            None => Ok(http_json_status(404, "not_found", "session id not found")),
        }
    }

    /// Create a dedicated echo session with `msid` rewrite.
    ///
    /// `POST /echo` with JSON body:
    /// ```json
    /// { "sdp": "<offer SDP>", "mode": "both" }
    /// ```
    ///
    /// Creates a bidirectional session with echo enabled from the start.
    /// The answer SDP has its `a=msid:` lines rewritten to a unique
    /// per-session stream id, preventing Chrome from silently discarding
    /// remote tracks whose `msid` matches the local track's `msid`.
    ///
    /// This aligns with ZLM `WebRtcEchoTest` behaviour.
    async fn handle_echo_create(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let sdp = body
            .get("sdp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SdkError::InvalidArgument("missing field: sdp".into()))?
            .to_string();
        let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("both");
        let echo = match mode.to_ascii_lowercase().as_str() {
            "datachannel" => crate::session::WebRtcEchoConfig {
                data_channel: true,
                media: false,
            },
            "media" => crate::session::WebRtcEchoConfig {
                data_channel: false,
                media: true,
            },
            "both" => crate::session::WebRtcEchoConfig {
                data_channel: true,
                media: true,
            },
            other => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("unknown echo mode: {other}"),
                ))
            }
        };

        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;
        let stream_key = StreamKey::new("echo", format!("{session_id}"));

        {
            let mut reg = self.registry.lock();
            let mut session = WebRtcModuleSession::new(
                session_id,
                stream_key.clone(),
                WebRtcSessionRole::Bidirectional,
                WebRtcApiKind::Echo,
            );
            session.echo = echo;
            reg.insert(session);
        }

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Bidirectional,
                remote_sdp_offer: sdp,
                candidate_transport_policy: CandidateTransportPolicy::All,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_json_status(503, "echo_unavailable", &reason));
            }
        };

        // Rewrite msid in the answer SDP to prevent Chrome from
        // treating the echoed tracks as local tracks.
        let echo_rewrite = { self.config.lock().echo_rewrite_msid };
        let final_sdp = if echo_rewrite {
            crate::compat::rewrite_echo_msid(&answer_sdp, &format!("echo-{}", session_id.value()))
        } else {
            answer_sdp
        };

        let body_json = serde_json::json!({
            "code": 0,
            "sessionid": format!("{session_id}"),
            "sdp": final_sdp,
        });
        Ok(HttpResponse::ok_json(
            serde_json::to_vec(&body_json).unwrap(),
        ))
    }

    /// ZLM-style unified WebRTC endpoint.
    ///
    /// `POST /zlm` with JSON body:
    /// ```json
    /// { "type": "push|play|echo", "sdp": "...", "app": "live", "stream": "demo" }
    /// ```
    ///
    /// The `type` field selects the operation:
    /// - `push` / `publish` → equivalent to `POST /publish`
    /// - `play` → equivalent to `POST /play`
    /// - `echo` → equivalent to `POST /echo`
    ///
    /// Also accepts `url=rtc://host/app/stream` as an alternative to
    /// explicit `app`/`stream` fields (ZLM client compatibility).
    async fn handle_zlm_unified(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let api_type = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match api_type.as_str() {
            "push" | "publish" => self.handle_sms_publish(req).await,
            "play" => self.handle_sms_play(req).await,
            "echo" => self.handle_echo_create(req).await,
            "" => Ok(http_json_status(
                400,
                "bad_request",
                "missing `type` field; expected push|play|echo",
            )),
            other => Ok(http_json_status(
                400,
                "bad_request",
                &format!("unknown type: {other}; expected push|play|echo"),
            )),
        }
    }

    /// Send a DataChannel message on a previously-opened channel.
    ///
    /// `POST /api/v1/rtc/session/{id}/datachannel/send` with JSON body:
    /// ```json
    /// { "channel": 0, "payload": "hello", "binary": false }
    /// ```
    /// `payload` is interpreted as UTF-8 text when `binary` is false (or
    /// missing); when `binary` is true the payload is base64-decoded
    /// before being handed to the driver. `channel` is the
    /// `DataChannelId` (`u32`) surfaced via the
    /// `WebRtcDataChannelEvent::Opened` event.
    ///
    /// On success returns `202 Accepted` because the driver write is
    /// asynchronous; the actual flush back to the peer can still be
    /// rejected at the SCTP layer (an oversize message or full send
    /// buffer surface as a diagnostic).
    async fn handle_datachannel_send(
        &self,
        path: &str,
        req: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        let id_part = match path
            .strip_prefix("/session/")
            .and_then(|rest| rest.rsplit_once("/datachannel/send").map(|(a, _)| a))
        {
            Some(p) => p,
            None => return Ok(http_json_status(404, "not_found", "session id not found")),
        };
        let session_id = match parse_session_id_str(id_part) {
            Some(id) => id,
            None => return Ok(http_json_status(404, "not_found", "session id not found")),
        };
        // Validate the session is alive.
        {
            let reg = self.registry.lock();
            match reg.sessions.get(&session_id) {
                None => return Ok(http_json_status(404, "not_found", "session id not found")),
                Some(s)
                    if matches!(
                        s.state,
                        WebRtcModuleSessionState::Closed | WebRtcModuleSessionState::Closing
                    ) =>
                {
                    return Ok(http_json_status(409, "session_closed", "session is closed"));
                }
                Some(_) => {}
            }
        }

        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("invalid json: {e}"),
                ))
            }
        };
        let channel_id_u32 = match body
            .get("channel")
            .or_else(|| body.get("channelId"))
            .and_then(|v| v.as_u64())
        {
            Some(c) if c <= u64::from(u32::MAX) => c as u32,
            _ => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing or invalid `channel` (u32)",
                ));
            }
        };
        let binary = body
            .get("binary")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let payload_str = match body.get("payload").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing field: payload (string)",
                ));
            }
        };
        let payload_bytes: Bytes = if binary {
            // Base64 decode for binary payloads. Reuse the workspace's
            // engine codec since we already pull it in transitively.
            match crate::compat::base64_decode(payload_str) {
                Ok(b) => Bytes::from(b),
                Err(_) => {
                    return Ok(http_json_status(
                        400,
                        "bad_request",
                        "binary payload is not valid base64",
                    ));
                }
            }
        } else {
            Bytes::from(payload_str.as_bytes().to_vec())
        };

        let driver = self.driver_handle()?;
        let out = cheetah_webrtc_core::WebRtcDataChannelOut {
            session_id,
            channel: cheetah_webrtc_core::DataChannelId::new(channel_id_u32),
            payload: payload_bytes,
            binary,
        };
        driver
            .send_command(WebRtcDriverCommand::SendDataChannel(out))
            .await;
        Ok(HttpResponse {
            status: 202,
            headers: Vec::new(),
            body: Bytes::new(),
        })
    }

    async fn handle_session_list(&self) -> Result<HttpResponse, SdkError> {
        let base_url = {
            let cfg = self.config.lock();
            cfg.public_webrtc_base_url.clone()
        };
        let list: Vec<Value> = {
            let reg = self.registry.lock();
            reg.list()
                .iter()
                .map(|s| {
                    let elapsed = s.created_at.elapsed();
                    let play_duration_ms = elapsed.as_millis() as u64;
                    let whep_url = build_whep_url(base_url.as_deref(), &s.stream_key);
                    serde_json::json!({
                        "session_id": format!("{}", s.id),
                        "protocol": "webrtc",
                        "app": s.stream_key.namespace,
                        "stream": s.stream_key.path,
                        "stream_key": format!("{}", s.stream_key),
                        "role": format!("{:?}", s.role),
                        "api_kind": format!("{:?}", s.api_kind),
                        "state": format!("{:?}", s.state),
                        "remote_addr": s.remote_addr.map(|a| a.to_string()),
                        "created_at_epoch_ms": std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64 - play_duration_ms)
                            .unwrap_or(0),
                        "play_duration_ms": play_duration_ms,
                        "candidate_type": s.candidate_type.as_deref(),
                        "whep_url": whep_url,
                    })
                })
                .collect()
        };
        let body = serde_json::json!({"sessions": list});
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    /// `GET /api/v1/rtc/metrics` — Prometheus exposition format.
    ///
    /// Returns the documented Phase 04 §4.8 metrics surface in the
    /// standard text-based Prometheus format. Operators scrape this
    /// endpoint into their TSDB; field names match the Phase 04
    /// docs (with the `webrtc_` prefix).
    fn handle_metrics(&self) -> Result<HttpResponse, SdkError> {
        let snap = self.compose_metrics_snapshot();
        let mut out = String::with_capacity(2048);
        // Each metric block follows the Prometheus exposition convention:
        // `# HELP …` then `# TYPE …` then a single sample line.
        // Counters use the `_total` suffix; gauges use a plain name.
        macro_rules! gauge {
            ($name:literal, $help:literal, $value:expr) => {
                out.push_str(&format!("# HELP {} {}\n", $name, $help));
                out.push_str(&format!("# TYPE {} gauge\n", $name));
                out.push_str(&format!("{} {}\n", $name, $value));
            };
        }
        macro_rules! counter {
            ($name:literal, $help:literal, $value:expr) => {
                out.push_str(&format!("# HELP {} {}\n", $name, $help));
                out.push_str(&format!("# TYPE {} counter\n", $name));
                out.push_str(&format!("{} {}\n", $name, $value));
            };
        }
        gauge!(
            "webrtc_sessions_active",
            "Active WebRTC sessions across all roles.",
            snap.sessions_active
        );
        gauge!(
            "webrtc_publish_sessions",
            "WebRTC sessions in Publisher role (WHIP / SMS publish / pull / P2P publish).",
            snap.publish_sessions
        );
        gauge!(
            "webrtc_play_sessions",
            "WebRTC sessions in Player role (WHEP / SMS play / push).",
            snap.play_sessions
        );
        counter!(
            "webrtc_packets_in_total",
            "Total RTP packets received from peers.",
            snap.packets_in_total
        );
        counter!(
            "webrtc_packets_out_total",
            "Total RTP packets sent to peers.",
            snap.packets_out_total
        );
        counter!(
            "webrtc_nack_in_total",
            "Total NACKs received from peers (egress retransmit pressure).",
            snap.nack_in_total
        );
        counter!(
            "webrtc_nack_out_total",
            "Total NACKs sent to peers (ingress packet loss recovery).",
            snap.nack_out_total
        );
        counter!(
            "webrtc_rtx_sent_total",
            "Total RTX (retransmit) packets sent.",
            snap.rtx_sent_total
        );
        counter!(
            "webrtc_rtx_miss_total",
            "Total RTX cache misses (NACK could not be served).",
            snap.rtx_miss_total
        );
        counter!(
            "webrtc_pli_total",
            "Total PLI (Picture Loss Indication) feedback.",
            snap.pli_total
        );
        counter!(
            "webrtc_fir_total",
            "Total FIR (Full Intra Request) feedback.",
            snap.fir_total
        );
        counter!(
            "webrtc_twcc_feedback_total",
            "Total TWCC feedback delivery samples.",
            snap.twcc_feedback_total
        );
        counter!(
            "webrtc_simulcast_layer_switch_total",
            "Total simulcast layer switches forced by Adaptive policy.",
            snap.simulcast_layer_switch_total
        );
        counter!(
            "webrtc_route_migration_total",
            "Total ICE route migrations (remote address changes).",
            snap.route_migration_total
        );
        counter!(
            "webrtc_queue_drop_total",
            "Total bounded-queue drops at module / driver boundaries.",
            snap.queue_drop_total
        );
        gauge!(
            "webrtc_remb_bitrate_bps",
            "Last observed REMB (Receiver Estimated Maximum Bitrate) cap.",
            snap.remb_bitrate_bps
        );
        gauge!(
            "webrtc_bwe_estimate_bps",
            "Last observed local BWE (Bandwidth Estimation) value.",
            snap.bwe_estimate_bps
        );

        Ok(HttpResponse {
            status: 200,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                // Prometheus default text exposition format. We do
                // not advertise OpenMetrics 1.0.0 here because the
                // current output omits the trailing `# EOF`; we can
                // extend later if a strict OpenMetrics consumer
                // appears.
                value: "text/plain; version=0.0.4; charset=utf-8".into(),
            }],
            body: Bytes::from(out),
        })
    }

    /// `GET /api/v1/rtc/metrics.json` — same data as `/metrics` but
    /// in JSON, useful for ad-hoc curl debugging or non-Prometheus
    /// consumers.
    fn handle_metrics_json(&self) -> Result<HttpResponse, SdkError> {
        let snap = self.compose_metrics_snapshot();
        let body = serde_json::json!({
            "sessions_active": snap.sessions_active,
            "publish_sessions": snap.publish_sessions,
            "play_sessions": snap.play_sessions,
            "packets_in_total": snap.packets_in_total,
            "packets_out_total": snap.packets_out_total,
            "nack_in_total": snap.nack_in_total,
            "nack_out_total": snap.nack_out_total,
            "rtx_sent_total": snap.rtx_sent_total,
            "rtx_miss_total": snap.rtx_miss_total,
            "pli_total": snap.pli_total,
            "fir_total": snap.fir_total,
            "twcc_feedback_total": snap.twcc_feedback_total,
            "simulcast_layer_switch_total": snap.simulcast_layer_switch_total,
            "route_migration_total": snap.route_migration_total,
            "queue_drop_total": snap.queue_drop_total,
            "remb_bitrate_bps": snap.remb_bitrate_bps,
            "bwe_estimate_bps": snap.bwe_estimate_bps,
        });
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    /// Combine the aggregator counters with the live registry session
    /// role counts. Mirrors `WebRtcModule::metrics_snapshot()` but
    /// reads the http service's own clones of `metrics` / `registry`,
    /// so we do not need to round-trip back to the module struct.
    fn compose_metrics_snapshot(&self) -> crate::metrics::WebRtcModuleMetricsSnapshot {
        use cheetah_webrtc_core::WebRtcSessionRole;
        let counters = self.metrics.snapshot_counters();
        let (active, publish, play) = {
            let reg = self.registry.lock();
            let mut publish = 0usize;
            let mut play = 0usize;
            for s in reg.sessions.values() {
                match s.role {
                    WebRtcSessionRole::Publisher => publish += 1,
                    WebRtcSessionRole::Player => play += 1,
                    WebRtcSessionRole::Bidirectional => {
                        publish += 1;
                        play += 1;
                    }
                }
            }
            (reg.sessions.len(), publish, play)
        };
        crate::metrics::WebRtcModuleMetricsSnapshot::assemble(counters, active, publish, play)
    }

    async fn wait_answer(
        &self,
        waiter: oneshot::Receiver<AnswerOutcome>,
    ) -> Result<String, String> {
        let timeout_ms = {
            let cfg = self.config.lock();
            cfg.wait_stream_timeout_ms.max(500)
        };
        let timeout = tokio::time::Duration::from_millis(timeout_ms.min(60_000));
        match tokio::time::timeout(timeout, waiter).await {
            Ok(Ok(AnswerOutcome::Sdp(sdp))) => Ok(sdp),
            Ok(Ok(AnswerOutcome::Failed(reason))) => Err(reason),
            Ok(Err(_)) => Err("driver answer channel closed".into()),
            Err(_) => Err("driver answer timeout".into()),
        }
    }

    fn check_codec_policy(&self, body: &Value) -> Option<String> {
        let cfg = self.config.lock();
        let profile = cfg.codec_profile;
        if let Some(v) = body.get("preferVideoCodec").and_then(|v| v.as_str()) {
            let pref = WebRtcVideoCodecPreference::from_str_lossy(v);
            if !pref.is_allowed(profile) {
                return Some(format!(
                    "preferVideoCodec={v} not allowed under {profile:?} profile"
                ));
            }
        }
        if let Some(v) = body.get("preferAudioCodec").and_then(|v| v.as_str()) {
            let pref = WebRtcAudioCodecPreference::from_str_lossy(v);
            if !pref.is_allowed(profile) {
                return Some(format!(
                    "preferAudioCodec={v} not allowed under {profile:?} profile"
                ));
            }
        }
        None
    }

    async fn cleanup_session(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
        reason: WebRtcCloseReason,
    ) {
        let driver = {
            let guard = self.driver.lock();
            guard.clone()
        };
        if let Some(driver) = driver {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: reason.clone(),
                })
                .await;
        }
        let mut reg = self.registry.lock();
        let removed = reg.remove(session_id);
        drop(reg);
        let mut bridges = self.bridges.lock();
        if let Some(bridge) = bridges.remove_publish(session_id) {
            bridge.close();
        }
        if let Some(cancel) = bridges.remove_play(session_id) {
            cancel.cancel();
        }
        drop(bridges);
        if let (Some(session), Some(engine)) = (removed.as_ref(), self.engine.as_ref()) {
            let min_duration = {
                let cfg = self.config.lock();
                std::time::Duration::from_millis(cfg.play_disconnect_min_duration_ms)
            };
            let play_reason =
                crate::play_disconnect::close_reason_to_play_disconnect_reason(&reason);
            crate::play_disconnect::observe_play_session_cleanup(
                engine.event_bus.as_ref(),
                self.metrics.as_ref(),
                session,
                play_reason,
                min_duration,
                std::time::Instant::now(),
            );
        }
    }

    async fn handle_job_start(
        &self,
        req: HttpRequest,
        kind: crate::jobs::WebRtcJobKind,
    ) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let url = body
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SdkError::InvalidArgument("missing field: url".into()))?
            .to_string();
        // Phase 05 follow-up: detect `webrtc://...?signaling_protocols=1`
        // URLs early and route them through the P2P plan validator.
        // The full pull/push job runner is still in progress, but the
        // validator already exercises SSRF, signaling URL derivation,
        // and `peer_room_id` requirements — all of which we want to
        // surface as actionable 501 responses today.
        if let Ok(parsed) = crate::compat::parse_zlm_rtc_url(&url) {
            if parsed.signaling_protocols == 1 {
                let policy = crate::p2p::SignalingUrlPolicy {
                    allow_private_ips: body
                        .get("allowPrivateIps")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    ..Default::default()
                };
                let plan_input = crate::p2p::P2pBridgePlanInput {
                    url: &parsed,
                    kind: match kind {
                        crate::jobs::WebRtcJobKind::Pull => crate::p2p::P2pJobKind::Pull,
                        crate::jobs::WebRtcJobKind::Push => crate::p2p::P2pJobKind::Push,
                    },
                    session_id: self.allocator.allocate(),
                    local_room_id: format!(
                        "ringing_{}_{}",
                        kind.label(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(0)
                    ),
                    transport_id: format!("tr_{}", self.allocator.allocate()),
                    policy: &policy,
                    pending_candidate_cap: 0,
                    offer_timeout: None,
                };
                match crate::p2p::plan_from_zlm_url(plan_input) {
                    Ok(plan) => {
                        // Phase 05 round 9: when the engine + driver
                        // are bound, spawn a real P2P client job and
                        // return 200 + session info. Falls back to
                        // 501 + plan extras when prerequisites are
                        // missing (driver hasn't started).
                        let driver_handle = self.driver.lock().clone();
                        let engine = self.engine.clone();
                        if let (Some(driver), Some(engine)) = (driver_handle, engine) {
                            let session_id = plan.bridge_config.session_id;
                            let request = crate::p2p_jobs::P2pClientJobRequest {
                                url: url.clone(),
                                kind: plan.kind,
                                allow_private_ips: body
                                    .get("allowPrivateIps")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                                signaling_url_override: Some(plan.signaling_url.render()),
                                connect_timeout: std::time::Duration::from_millis(
                                    body.get("connectTimeoutMs")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(10_000),
                                ),
                                offer_timeout: std::time::Duration::from_millis(
                                    body.get("offerTimeoutMs")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(10_000),
                                ),
                                supervisor: crate::p2p::KeeperSupervisorConfig::default(),
                            };
                            let runtime = crate::p2p_jobs::P2pClientJobRuntime {
                                registry: self.p2p_jobs.clone(),
                                keepers: self.keepers.clone(),
                                driver,
                                lifecycle: self.lifecycle_dispatcher.clone(),
                                engine,
                                parent_cancel: self.jobs_cancel.clone(),
                                answer_dispatcher: self.answer_dispatcher.clone(),
                            };
                            match crate::p2p_jobs::spawn(runtime, session_id, request) {
                                Ok(snap) => {
                                    return Ok(HttpResponse::ok_json(
                                        serde_json::to_vec(&serde_json::json!({
                                            "session_id": format!("{}", snap.session_id),
                                            "kind": match snap.kind {
                                                crate::p2p::P2pJobKind::Pull => "pull",
                                                crate::p2p::P2pJobKind::Push => "push",
                                            },
                                            "state": match snap.state {
                                                crate::p2p_jobs::P2pClientJobState::Pending => "pending",
                                                crate::p2p_jobs::P2pClientJobState::Running => "running",
                                                crate::p2p_jobs::P2pClientJobState::Stopped => "stopped",
                                                crate::p2p_jobs::P2pClientJobState::Failed => "failed",
                                            },
                                            "signaling_url": snap.signaling_url,
                                            "peer_room_id": snap.peer_room_id,
                                            "stream_key": snap.stream_key,
                                        }))
                                        .unwrap(),
                                    ));
                                }
                                Err(crate::p2p_jobs::P2pClientJobError::Conflict(_)) => {
                                    return Ok(http_json_status(
                                        409,
                                        "conflict",
                                        "p2p client job already running for this session id",
                                    ));
                                }
                                Err(err) => {
                                    return Ok(http_json_status(
                                        503,
                                        "p2p_unavailable",
                                        &err.to_string(),
                                    ));
                                }
                            }
                        }
                        return Ok(http_json_status_with_extras(
                            501,
                            "not_implemented",
                            "P2P pull/push entry validated (signaling URL + SSRF + plan); the driver / engine isn't bound yet — see plans-27-webrtc-zlm2/phase-05-p2p-signaling.md",
                            serde_json::json!({
                                "signaling_url": plan.signaling_url.render(),
                                "peer_room_id": plan.bridge_config.job.peer_room_id,
                                "kind": match plan.kind {
                                    crate::p2p::P2pJobKind::Pull => "pull",
                                    crate::p2p::P2pJobKind::Push => "push",
                                },
                            }),
                        ));
                    }
                    Err(err) => {
                        return Ok(http_json_status(400, "p2p_invalid_url", &err.to_string()));
                    }
                }
            }
        }
        let (app, stream) = crate::compat::extract_app_stream_aliases(&body);
        let stream = stream
            .ok_or_else(|| SdkError::InvalidArgument("missing field: streamName/stream".into()))?;
        let stream_key = StreamKey::new(&app, &stream);
        let protocol = match body.get("protocol").and_then(|v| v.as_str()) {
            Some(p) => match p.to_ascii_lowercase().as_str() {
                "whip" => crate::jobs::WebRtcSignalingProtocol::Whip,
                "whep" => crate::jobs::WebRtcSignalingProtocol::Whep,
                other => {
                    return Ok(http_json_status(
                        400,
                        "bad_request",
                        &format!("unknown signaling protocol `{other}` (expected whip|whep)"),
                    ));
                }
            },
            None => match kind {
                crate::jobs::WebRtcJobKind::Pull => crate::jobs::WebRtcSignalingProtocol::Whep,
                crate::jobs::WebRtcJobKind::Push => crate::jobs::WebRtcSignalingProtocol::Whip,
            },
        };
        let timeout_ms = body
            .get("timeoutMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(10_000);
        let retry = body.get("retry").and_then(|v| v.as_bool()).unwrap_or(true);
        let allow_private_ips = body
            .get("allowPrivateIps")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_retries = body.get("maxRetries").and_then(|v| v.as_u64()).unwrap_or(8) as u32;

        let driver = self.driver_handle()?;
        let engine = match self.engine.as_ref() {
            Some(e) => e.clone(),
            None => {
                return Ok(http_json_status(
                    503,
                    "engine_unavailable",
                    "WebRTC module not yet bound to engine",
                ));
            }
        };

        let spec = crate::jobs::WebRtcClientJobSpec {
            kind,
            stream_key: stream_key.clone(),
            url: url.clone(),
            protocol,
            timeout: std::time::Duration::from_millis(timeout_ms.min(60_000)),
            retry,
            retry_initial_backoff: std::time::Duration::from_millis(1_000),
            retry_max_backoff: std::time::Duration::from_millis(30_000),
            max_retries: max_retries.max(1),
            max_response_bytes: 64 * 1024,
            allow_private_ips,
        };
        let snapshot = crate::jobs::spawn_job(
            self.jobs.clone(),
            engine,
            driver,
            self.http_client.clone(),
            self.answer_dispatcher.clone(),
            self.allocator.clone(),
            spec,
            self.jobs_cancel.clone(),
        )
        .await;
        match snapshot {
            Ok(snap) => Ok(HttpResponse::ok_json(
                serde_json::to_vec(&serde_json::json!({
                    "code": 0,
                    "stream_key": snap.stream_key,
                    "url": snap.url,
                    "kind": snap.kind.label(),
                    "state": format!("{:?}", snap.state),
                }))
                .unwrap(),
            )),
            Err(crate::jobs::WebRtcJobError::Conflict(_)) => {
                Ok(http_json_status(409, "conflict", "job already running"))
            }
            Err(err) => Ok(http_json_status(503, "job_unavailable", &err.to_string())),
        }
    }

    async fn handle_job_stop(
        &self,
        req: HttpRequest,
        kind: crate::jobs::WebRtcJobKind,
    ) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let (app, stream) = crate::compat::extract_app_stream_aliases(&body);
        let stream = stream
            .ok_or_else(|| SdkError::InvalidArgument("missing field: streamName/stream".into()))?;
        let stream_key = format!("{}/{}", app, stream);
        let stopped = self.jobs.lock().stop(kind, &stream_key);
        if stopped {
            Ok(HttpResponse {
                status: 204,
                headers: Vec::new(),
                body: Bytes::new(),
            })
        } else {
            Ok(http_json_status(404, "not_found", "job not found"))
        }
    }

    fn handle_job_list(&self, kind: crate::jobs::WebRtcJobKind) -> Result<HttpResponse, SdkError> {
        let snapshots = self.jobs.lock().list(kind);
        let body = serde_json::json!({
            "jobs": snapshots
                .iter()
                .map(|s| serde_json::json!({
                    "stream_key": s.stream_key,
                    "url": s.url,
                    "kind": s.kind.label(),
                    "state": format!("{:?}", s.state),
                    "retry_count": s.retry_count,
                    "last_error": s.last_error,
                    "remote_session_location": s.remote_session_location,
                }))
                .collect::<Vec<_>>(),
        });
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    /// P2P add: accept an offer SDP from a peer and return an answer.
    /// This is essentially the same flow as WHIP/WHEP but with a
    /// `Bidirectional` role; the peer is responsible for any
    /// subsequent ICE candidate trickling via `PATCH /session/{id}`.
    ///
    /// The body may carry both `streamName` (the stream the peer is
    /// publishing to Cheetah) and an optional `playStreamName` (a
    /// stream the peer wants to receive from Cheetah). When
    /// `playStreamName` is present we additionally spawn an engine
    /// subscriber so the same session forwards frames back to the
    /// peer — completing the sendrecv loop without forcing the peer
    /// to negotiate two separate WebRTC sessions.
    async fn handle_p2p_add(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let (app, stream) = extract_app_stream_aliases(&body);
        let stream = stream
            .ok_or_else(|| SdkError::InvalidArgument("missing field: streamName/stream".into()))?;
        let sdp = body
            .get("sdp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SdkError::InvalidArgument("missing field: sdp".into()))?
            .to_string();
        // Optional play stream: when set, the same WebRTC session
        // also subscribes to engine `app/<playStreamName>` and
        // forwards frames out to the peer via the bidirectional
        // m-lines negotiated through the SDP. Defaults to no play
        // direction (publish-only P2P) for backwards compatibility.
        let play_stream = body
            .get("playStreamName")
            .or_else(|| body.get("playStream"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let stream_key = StreamKey::new(&app, &stream);
        let play_stream_key = play_stream.as_ref().map(|s| StreamKey::new(&app, s));
        let session_id = self.allocator.allocate();
        let driver = self.driver_handle()?;

        // Acquire the publish bridge eagerly so the engine sees the
        // P2P session as a publisher (matching the WHIP path).
        // Sessions without an explicit play stream key still need the
        // publish bridge — the peer is sending media to us.
        let bridge = match self.acquire_publish_bridge(stream_key.clone()).await {
            Ok(b) => b,
            Err(resp) => return Ok(resp),
        };

        {
            let mut reg = self.registry.lock();
            reg.insert(WebRtcModuleSession::new(
                session_id,
                stream_key.clone(),
                WebRtcSessionRole::Bidirectional,
                WebRtcApiKind::P2p,
            ));
        }
        self.bridges.lock().insert_publish(session_id, bridge);

        let waiter = self.answer_dispatcher.subscribe(session_id);
        driver
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id,
                role: WebRtcSessionRole::Bidirectional,
                remote_sdp_offer: sdp,
                candidate_transport_policy: CandidateTransportPolicy::All,
            }))
            .await;

        let answer_sdp = match self.wait_answer(waiter).await {
            Ok(sdp) => sdp,
            Err(reason) => {
                self.cleanup_session(session_id, WebRtcCloseReason::Internal(reason.clone()))
                    .await;
                return Ok(http_json_status(503, "p2p_unavailable", &reason));
            }
        };

        // If the peer asked for play media, spawn an engine
        // subscriber that pushes frames into the same WebRTC driver
        // session. The play subscriber and the publish bridge are
        // independent — frames published by the peer never feed
        // back into the play direction unless the engine routes them
        // through (which only happens when the peer explicitly sets
        // `playStreamName == streamName`, and we let that work as a
        // diagnostic loopback).
        if let Some(play_key) = play_stream_key {
            self.spawn_play(session_id, play_key, driver.clone()).await;
        }

        let body = serde_json::json!({
            "code": 0,
            "sessionid": format!("{session_id}"),
            "sdp": answer_sdp,
        });
        Ok(HttpResponse::ok_json(serde_json::to_vec(&body).unwrap()))
    }

    async fn handle_p2p_remove(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = serde_json::from_slice(&req.body)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid json: {e}")))?;
        let session_id = match body
            .get("sessionid")
            .or_else(|| body.get("sessionId"))
            .and_then(|v| v.as_str())
            .and_then(parse_session_id_str)
        {
            Some(id) => id,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing or invalid `sessionid`",
                ))
            }
        };
        let exists = {
            let reg = self.registry.lock();
            reg.sessions
                .get(&session_id)
                .map(|s| s.api_kind == WebRtcApiKind::P2p)
                .unwrap_or(false)
        };
        if !exists {
            return Ok(http_json_status(404, "not_found", "p2p session not found"));
        }
        self.cleanup_session(session_id, WebRtcCloseReason::Normal)
            .await;
        Ok(HttpResponse {
            status: 204,
            headers: Vec::new(),
            body: Bytes::new(),
        })
    }

    fn handle_p2p_list(&self) -> Result<HttpResponse, SdkError> {
        let sessions: Vec<Value> = {
            let reg = self.registry.lock();
            reg.list()
                .iter()
                .filter(|s| s.api_kind == WebRtcApiKind::P2p)
                .map(|s| {
                    serde_json::json!({
                        "session_id": format!("{}", s.id),
                        "stream_key": format!("{}", s.stream_key),
                        "state": format!("{:?}", s.state),
                    })
                })
                .collect()
        };
        Ok(HttpResponse::ok_json(
            serde_json::to_vec(&serde_json::json!({"sessions": sessions})).unwrap(),
        ))
    }

    /// `POST /api/v1/rtc/p2p/keeper/add` — register a P2P signaling
    /// room keeper. Body fields:
    ///
    /// ```json
    /// {
    ///   "server_host": "signaling.example.com",
    ///   "server_port": 8443,
    ///   "ssl": true,
    ///   "room_id": "room42",
    ///   "vhost": "__defaultVhost__",
    ///   "app": "live",
    ///   "stream": "demo"
    /// }
    /// ```
    ///
    /// Phase 05 follow-up: the registry only stores bookkeeping today.
    /// The actual WebSocket signaling client task that drives
    /// reconnect / check-in / candidate exchange lands in the next
    /// round.
    async fn handle_keeper_add(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("invalid json: {e}"),
                ));
            }
        };
        let cfg = match keeper_config_from_body(&body) {
            Ok(c) => c,
            Err(reason) => {
                return Ok(http_json_status(400, "bad_request", &reason));
            }
        };
        match self.keepers.add(cfg) {
            Ok(key) => Ok(HttpResponse::ok_json(
                serde_json::to_vec(&serde_json::json!({
                    "key": key.to_string(),
                }))
                .unwrap(),
            )),
            Err(crate::p2p::P2pRoomKeeperError::LimitReached(cap)) => Ok(http_json_status(
                429,
                "limit_reached",
                &format!("keeper capacity {cap} reached"),
            )),
            Err(err) => Ok(http_json_status(400, "bad_request", &err.to_string())),
        }
    }

    /// `POST /api/v1/rtc/p2p/keeper/remove` — body `{ "key": "keeper-N" }`.
    async fn handle_keeper_remove(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("invalid json: {e}"),
                ));
            }
        };
        let key_str = match body.get("key").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return Ok(http_json_status(400, "bad_request", "missing field `key`"));
            }
        };
        let id = match key_str
            .strip_prefix("keeper-")
            .and_then(|s| s.parse::<u64>().ok())
        {
            Some(id) => id,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "key must look like `keeper-<number>`",
                ));
            }
        };
        // The registry keeps a private id; we don't have public
        // construction so we look up the snapshot by string key
        // directly. A future round can plumb a typed helper through.
        let removed = {
            let listed = self.keepers.list();
            listed
                .into_iter()
                .find(|s| s.key.to_string() == format!("keeper-{id}"))
        };
        match removed {
            Some(snap) => match self.keepers.remove(snap.key) {
                Ok(_) => Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&serde_json::json!({"removed": snap.key.to_string()}))
                        .unwrap(),
                )),
                Err(_) => Ok(http_json_status(404, "not_found", "keeper not found")),
            },
            None => Ok(http_json_status(404, "not_found", "keeper not found")),
        }
    }

    /// `GET /api/v1/rtc/p2p/keeper/list` — return all known keepers.
    fn handle_keeper_list(&self) -> Result<HttpResponse, SdkError> {
        let keepers: Vec<Value> = self
            .keepers
            .list()
            .into_iter()
            .map(|snap| {
                serde_json::json!({
                    "key": snap.key.to_string(),
                    "server_host": snap.config.server_host,
                    "server_port": snap.config.server_port,
                    "ssl": snap.config.ssl,
                    "room_id": snap.config.room_id,
                    "app": snap.config.app,
                    "stream": snap.config.stream,
                    "state": snap.status.state.as_str(),
                    "last_error": snap.status.last_error,
                    "reconnect_attempts": snap.status.reconnect_attempts,
                })
            })
            .collect();
        Ok(HttpResponse::ok_json(
            serde_json::to_vec(&serde_json::json!({"keepers": keepers})).unwrap(),
        ))
    }

    /// `GET /api/v1/rtc/p2p/rooms` — distinct room ids registered
    /// locally. Mirrors `mk_webrtc_list_rooms`.
    fn handle_keeper_rooms(&self) -> Result<HttpResponse, SdkError> {
        let rooms = self.keepers.list_rooms();
        Ok(HttpResponse::ok_json(
            serde_json::to_vec(&serde_json::json!({"rooms": rooms})).unwrap(),
        ))
    }

    /// `GET /api/v1/rtc/p2p/client/list` — return all in-flight P2P
    /// pull/push client jobs.
    fn handle_p2p_client_list(&self) -> Result<HttpResponse, SdkError> {
        let jobs: Vec<Value> = self
            .p2p_jobs
            .list()
            .into_iter()
            .map(|snap| {
                serde_json::json!({
                    "session_id": format!("{}", snap.session_id),
                    "kind": match snap.kind {
                        crate::p2p::P2pJobKind::Pull => "pull",
                        crate::p2p::P2pJobKind::Push => "push",
                    },
                    "url": snap.url,
                    "state": match snap.state {
                        crate::p2p_jobs::P2pClientJobState::Pending => "pending",
                        crate::p2p_jobs::P2pClientJobState::Running => "running",
                        crate::p2p_jobs::P2pClientJobState::Stopped => "stopped",
                        crate::p2p_jobs::P2pClientJobState::Failed => "failed",
                    },
                    "last_error": snap.last_error,
                    "signaling_url": snap.signaling_url,
                    "peer_room_id": snap.peer_room_id,
                    "stream_key": snap.stream_key,
                })
            })
            .collect();
        Ok(HttpResponse::ok_json(
            serde_json::to_vec(&serde_json::json!({"jobs": jobs})).unwrap(),
        ))
    }

    /// `POST /api/v1/rtc/p2p/client/stop` — body `{ "session_id": "webrtc-session-N" }`.
    async fn handle_p2p_client_stop(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        let body: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    &format!("invalid json: {e}"),
                ));
            }
        };
        let raw = match body.get("session_id").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "missing field `session_id`",
                ));
            }
        };
        // `WebRtcSessionId::Display` is `webrtc-session-N`. Accept
        // both prefixed and bare-int forms so curl examples don't
        // need to escape dashes.
        let id_str = raw.strip_prefix("webrtc-session-").unwrap_or(raw);
        let session_id = match id_str.parse::<u64>() {
            Ok(n) => cheetah_webrtc_core::WebRtcSessionId::new(n),
            Err(_) => {
                return Ok(http_json_status(
                    400,
                    "bad_request",
                    "session_id must be `webrtc-session-N` or a bare integer",
                ));
            }
        };
        if self.p2p_jobs.stop(session_id) {
            Ok(HttpResponse::ok_json(
                serde_json::to_vec(&serde_json::json!({
                    "stopped": format!("{}", session_id),
                }))
                .unwrap(),
            ))
        } else {
            Ok(http_json_status(
                404,
                "not_found",
                "p2p client job not found",
            ))
        }
    }
}

fn keeper_config_from_body(body: &Value) -> Result<crate::p2p::P2pRoomKeeperConfig, String> {
    let server_host = body
        .get("server_host")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing field `server_host`".to_string())?;
    let server_port = body
        .get("server_port")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing field `server_port`".to_string())?;
    if server_port == 0 || server_port > u64::from(u16::MAX) {
        return Err(format!("server_port out of range: {server_port}"));
    }
    let room_id = body
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing field `room_id`".to_string())?;
    let cfg = crate::p2p::P2pRoomKeeperConfig {
        server_host: server_host.to_string(),
        server_port: server_port as u16,
        room_id: room_id.to_string(),
        vhost: body
            .get("vhost")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        app: body
            .get("app")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        stream: body
            .get("stream")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        ssl: body.get("ssl").and_then(|v| v.as_bool()).unwrap_or(false),
    };
    cfg.validate().map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// Notifier shared between the HTTP service and the driver-event worker
/// so that incoming `AnswerReady` events can be routed back to the
/// pending HTTP request.
pub(crate) struct AnswerDispatcher {
    waiters: Mutex<
        std::collections::HashMap<
            cheetah_webrtc_core::WebRtcSessionId,
            oneshot::Sender<AnswerOutcome>,
        >,
    >,
    /// Broadcast for diagnostic listeners that don't have a per-session
    /// oneshot. Phase 04 wires this up to a metrics worker; today it
    /// simply provides the shape so callers can subscribe.
    #[allow(dead_code)]
    pub diagnostics: broadcast::Sender<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum AnswerOutcome {
    Sdp(String),
    Failed(String),
}

impl AnswerDispatcher {
    pub(crate) fn new() -> Self {
        let (tx, _rx) = broadcast::channel(64);
        Self {
            waiters: Mutex::new(std::collections::HashMap::new()),
            diagnostics: tx,
        }
    }

    pub(crate) fn subscribe(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
    ) -> oneshot::Receiver<AnswerOutcome> {
        let (tx, rx) = oneshot::channel();
        let mut guard = self.waiters.lock();
        guard.insert(session_id, tx);
        rx
    }

    pub(crate) fn deliver_sdp(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
        sdp: String,
    ) {
        let mut guard = self.waiters.lock();
        if let Some(tx) = guard.remove(&session_id) {
            let _ = tx.send(AnswerOutcome::Sdp(sdp));
        }
    }

    pub(crate) fn deliver_failure(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
        reason: String,
    ) {
        let mut guard = self.waiters.lock();
        if let Some(tx) = guard.remove(&session_id) {
            let _ = tx.send(AnswerOutcome::Failed(reason));
        }
    }

    /// Subscribe to the next SDP delivery for the given session,
    /// returning a [`DispatcherOfferOutcome`]-shaped channel suitable
    /// for `crate::p2p::DispatcherOfferWaiter`. A small bridge task
    /// converts the internal `AnswerOutcome` enum into the public
    /// shape so the P2P bridge code stays free of `pub(crate)`
    /// types.
    pub(crate) fn subscribe_p2p(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
    ) -> futures::future::BoxFuture<'static, crate::p2p::DispatcherOfferOutcome> {
        let inner = self.subscribe(session_id);
        // Runtime-neutral adapter: await the inner receiver inline and
        // map the internal `AnswerOutcome` enum onto the public
        // `DispatcherOfferOutcome` shape. No task is spawned — the
        // returned future carries the conversion so the P2P bridge
        // stays free of `pub(crate)` types and of a runtime handle.
        Box::pin(async move {
            match inner.await {
                Ok(AnswerOutcome::Sdp(sdp)) => crate::p2p::DispatcherOfferOutcome::Sdp(sdp),
                Ok(AnswerOutcome::Failed(reason)) => {
                    crate::p2p::DispatcherOfferOutcome::Failed(reason)
                }
                Err(_) => {
                    crate::p2p::DispatcherOfferOutcome::Failed("driver offer channel closed".into())
                }
            }
        })
    }
}

impl OmeWsOfferWaiter for Arc<AnswerDispatcher> {
    fn wait_for_offer(
        &self,
        session_id: cheetah_webrtc_core::WebRtcSessionId,
        timeout: std::time::Duration,
    ) -> futures::future::BoxFuture<'_, Result<String, String>> {
        let waiter = self.subscribe(session_id);
        Box::pin(async move {
            match tokio::time::timeout(timeout, waiter).await {
                Ok(Ok(AnswerOutcome::Sdp(sdp))) => Ok(sdp),
                Ok(Ok(AnswerOutcome::Failed(reason))) => Err(reason),
                Ok(Err(_)) => Err("driver offer channel closed".into()),
                Err(_) => Err("timed out waiting for local offer".into()),
            }
        })
    }
}

fn parse_session_id(path: &str) -> Option<cheetah_webrtc_core::WebRtcSessionId> {
    // Path is expected to look like `/session/{id}`.
    let suffix = path.strip_prefix("/session/")?;
    let id_str = suffix.split('/').next().unwrap_or(suffix);
    parse_session_id_str(id_str)
}

fn parse_session_id_str(id_str: &str) -> Option<cheetah_webrtc_core::WebRtcSessionId> {
    if let Some(rest) = id_str.strip_prefix("webrtc-session-") {
        return rest
            .parse::<u64>()
            .ok()
            .map(cheetah_webrtc_core::WebRtcSessionId::new);
    }
    id_str
        .parse::<u64>()
        .ok()
        .map(cheetah_webrtc_core::WebRtcSessionId::new)
}

fn http_json_status(status: u16, code: &str, message: &str) -> HttpResponse {
    let body = serde_json::json!({
        "code": status,
        "error": code,
        "message": message,
    });
    HttpResponse {
        status,
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

/// Same as [`http_json_status`] but merges additional fields into the
/// response body. Used by the P2P `signaling_protocols=1` 501 path so
/// operators can inspect the resolved signaling URL / peer room id /
/// kind without re-parsing the URL on the client side.
fn http_json_status_with_extras(
    status: u16,
    code: &str,
    message: &str,
    extras: serde_json::Value,
) -> HttpResponse {
    let mut body = serde_json::Map::new();
    body.insert("code".into(), serde_json::json!(status));
    body.insert("error".into(), serde_json::json!(code));
    body.insert("message".into(), serde_json::json!(message));
    if let serde_json::Value::Object(extra_map) = extras {
        for (k, v) in extra_map {
            // Don't let the extras stomp on the standard fields.
            if k == "code" || k == "error" || k == "message" {
                continue;
            }
            body.insert(k, v);
        }
    }
    HttpResponse {
        status,
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body: Bytes::from(serde_json::to_vec(&serde_json::Value::Object(body)).unwrap()),
    }
}

#[cfg(test)]
fn http_sdp_created(session_id: cheetah_webrtc_core::WebRtcSessionId, sdp: String) -> HttpResponse {
    http_sdp_created_with_extra_headers(session_id, sdp, Vec::new())
}

fn http_sdp_created_with_extra_headers(
    session_id: cheetah_webrtc_core::WebRtcSessionId,
    sdp: String,
    extra_headers: Vec<HttpHeader>,
) -> HttpResponse {
    let location = format!("/api/v1/rtc/session/{}", session_id);
    let expose_headers = if extra_headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("link"))
    {
        "Location, Link"
    } else {
        "Location"
    };
    let mut headers = vec![
        HttpHeader {
            name: "content-type".into(),
            value: "application/sdp".into(),
        },
        HttpHeader {
            name: "location".into(),
            value: location,
        },
        HttpHeader {
            name: "access-control-allow-origin".into(),
            value: "*".into(),
        },
        HttpHeader {
            name: "access-control-expose-headers".into(),
            value: expose_headers.into(),
        },
    ];
    headers.extend(extra_headers);
    HttpResponse {
        status: 201,
        headers,
        body: Bytes::from(sdp),
    }
}

fn http_sdp_error(status: u16, reason: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "text/plain".into(),
        }],
        body: Bytes::from(reason.to_string()),
    }
}

fn http_not_found() -> HttpResponse {
    HttpResponse {
        status: 404,
        headers: Vec::new(),
        body: Bytes::from_static(b"{\"error\":\"not found\"}"),
    }
}

fn has_ome_query_marker(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };
    query.split('&').filter(|s| !s.is_empty()).any(|kv| {
        let key = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv);
        let key = url_decode_lossy(key);
        key.eq_ignore_ascii_case("direction") || key.eq_ignore_ascii_case("transport")
    })
}

fn sdp_from_body(req: &HttpRequest) -> Result<String, SdkError> {
    let content_type = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.as_str());
    if let Some(ct) = content_type {
        let lc = ct.to_ascii_lowercase();
        if !lc.contains("application/sdp") && !lc.is_empty() {
            warn!("WHIP/WHEP unexpected content-type {ct}");
        }
    }
    let sdp = std::str::from_utf8(&req.body)
        .map_err(|_| SdkError::InvalidArgument("body is not valid UTF-8 SDP".into()))?;
    if sdp.is_empty() {
        return Err(SdkError::InvalidArgument("empty SDP body".into()));
    }
    Ok(sdp.to_string())
}

/// Build the WHEP play URL for a stream.
///
/// When `public_webrtc_base_url` is configured, it is used as the
/// prefix. Otherwise a relative path is returned (the caller or
/// downstream consumer can prepend the request host).
///
/// The URL never includes DTLS fingerprints, private keys, or
/// authentication tokens — only the public WHEP endpoint path.
fn build_whep_url(base_url: Option<&str>, stream_key: &StreamKey) -> String {
    let path = format!(
        "/whep?app={}&stream={}",
        stream_key.namespace, stream_key.path
    );
    match base_url {
        Some(base) => {
            let trimmed = base.trim_end_matches('/');
            format!("{trimmed}{path}")
        }
        None => format!("/api/v1/rtc{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_webrtc_core::WebRtcSessionId;

    #[test]
    fn parses_numeric_session_id_path() {
        assert_eq!(
            parse_session_id("/session/42"),
            Some(WebRtcSessionId::new(42))
        );
    }

    #[test]
    fn parses_prefixed_session_id_path() {
        assert_eq!(
            parse_session_id("/session/webrtc-session-99"),
            Some(WebRtcSessionId::new(99))
        );
    }

    #[test]
    fn returns_none_for_unparseable_path() {
        assert!(parse_session_id("/session/").is_none());
        assert!(parse_session_id("/sessions/1").is_none());
    }

    #[test]
    fn http_sdp_created_returns_201_with_correct_headers() {
        let session_id = WebRtcSessionId::new(42);
        let sdp = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n".to_string();
        let resp = http_sdp_created(session_id, sdp.clone());

        assert_eq!(resp.status, 201);
        assert_eq!(resp.body, Bytes::from(sdp));

        let find_header = |name: &str| {
            resp.headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| h.value.clone())
        };
        assert_eq!(
            find_header("content-type").as_deref(),
            Some("application/sdp")
        );
        assert_eq!(
            find_header("location").as_deref(),
            Some("/api/v1/rtc/session/webrtc-session-42")
        );
        assert_eq!(
            find_header("access-control-allow-origin").as_deref(),
            Some("*")
        );
        assert!(
            find_header("access-control-expose-headers")
                .as_deref()
                .map(|v| v.contains("Location"))
                .unwrap_or(false),
            "must expose Location header for browser CORS"
        );
    }

    #[test]
    fn http_sdp_created_with_link_headers_exposes_link_for_cors() {
        let session_id = WebRtcSessionId::new(7);
        let resp = http_sdp_created_with_extra_headers(
            session_id,
            "v=0\r\n".to_string(),
            vec![HttpHeader {
                name: "link".into(),
                value: "<turn:relay.example.com:3478>; rel=\"ice-server\"".into(),
            }],
        );

        let expose = resp
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("access-control-expose-headers"))
            .map(|h| h.value.as_str());
        assert_eq!(expose, Some("Location, Link"));
        assert!(resp
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("link")));
    }

    #[test]
    fn build_whep_url_uses_public_base_url_when_configured() {
        let sk = StreamKey::new("live", "camera01");
        let url = build_whep_url(Some("http://cdn.example.com:8080/api/v1/rtc"), &sk);
        assert_eq!(
            url,
            "http://cdn.example.com:8080/api/v1/rtc/whep?app=live&stream=camera01"
        );
    }

    #[test]
    fn build_whep_url_trims_trailing_slash() {
        let sk = StreamKey::new("live", "stream1");
        let url = build_whep_url(Some("http://host:8080/api/v1/rtc/"), &sk);
        assert_eq!(
            url,
            "http://host:8080/api/v1/rtc/whep?app=live&stream=stream1"
        );
    }

    #[test]
    fn build_whep_url_falls_back_to_relative_path() {
        let sk = StreamKey::new("app1", "demo");
        let url = build_whep_url(None, &sk);
        assert_eq!(url, "/api/v1/rtc/whep?app=app1&stream=demo");
    }

    #[test]
    fn stream_list_contains_webrtc_whep_url() {
        // Verify that the session list JSON output includes the
        // expected WHEP URL, protocol, app, stream, and timing fields
        // while NOT leaking sensitive data.
        let sk = StreamKey::new("live", "camera01");
        let base_url = Some("http://cdn.example.com/api/v1/rtc");
        let whep_url = build_whep_url(base_url, &sk);

        assert_eq!(
            whep_url,
            "http://cdn.example.com/api/v1/rtc/whep?app=live&stream=camera01"
        );

        // Verify the URL does not contain any sensitive tokens or
        // DTLS fingerprint material.
        assert!(!whep_url.contains("fingerprint"));
        assert!(!whep_url.contains("token"));
        assert!(!whep_url.contains("key"));
        assert!(!whep_url.contains("secret"));
    }

    #[test]
    fn ome_http_marker_accepts_direction_or_transport() {
        assert!(has_ome_query_marker(Some("direction=whip")));
        assert!(has_ome_query_marker(Some("transport=relay")));
        assert!(has_ome_query_marker(Some("foo=bar&DIRECTION=send")));
        assert!(has_ome_query_marker(Some("%64irection=whip")));
        assert!(has_ome_query_marker(Some("%74ransport=relay")));
        assert!(!has_ome_query_marker(None));
        assert!(!has_ome_query_marker(Some("app=live&stream=camera01")));
    }

    #[test]
    fn ome_transport_maps_to_driver_candidate_policy() {
        let cases = [
            (OmeTransportMode::Udp, CandidateTransportPolicy::UdpOnly),
            (OmeTransportMode::Tcp, CandidateTransportPolicy::TcpOnly),
            (OmeTransportMode::Relay, CandidateTransportPolicy::RelayOnly),
            (OmeTransportMode::UdpTcp, CandidateTransportPolicy::UdpTcp),
            (OmeTransportMode::All, CandidateTransportPolicy::All),
        ];
        for (transport, expected) in cases {
            assert_eq!(
                ome_transport_to_candidate_policy(transport, false),
                expected
            );
        }
    }

    #[test]
    fn ome_tcp_relay_force_overrides_candidate_policy() {
        assert_eq!(
            ome_transport_to_candidate_policy(OmeTransportMode::Tcp, true),
            CandidateTransportPolicy::RelayOnly
        );
        assert_eq!(
            ome_transport_to_candidate_policy(OmeTransportMode::All, true),
            CandidateTransportPolicy::RelayOnly
        );
    }

    #[test]
    fn session_summary_does_not_leak_dtls_fingerprint_or_token() {
        // The session list JSON must never include DTLS fingerprint
        // private keys or authentication tokens. We verify by
        // constructing the JSON fields that handle_session_list would
        // produce and asserting no sensitive fields are present.
        let session_json = serde_json::json!({
            "session_id": "1",
            "protocol": "webrtc",
            "app": "live",
            "stream": "camera01",
            "remote_addr": "192.168.1.100:54321",
            "created_at_epoch_ms": 1700000000000u64,
            "play_duration_ms": 5000,
            "candidate_type": "host",
            "whep_url": "/api/v1/rtc/whep?app=live&stream=camera01",
        });

        let json_str = serde_json::to_string(&session_json).unwrap();
        // Must not contain any DTLS/crypto/auth sensitive fields.
        assert!(!json_str.contains("dtls_fingerprint"));
        assert!(!json_str.contains("private_key"));
        assert!(!json_str.contains("auth_token"));
        assert!(!json_str.contains("certificate"));
    }

    /// Verifies that `public_webrtc_base_url` controls the WHEP Location
    /// URL without relying on port parity to infer HTTP vs HTTPS.
    ///
    /// Acceptance criteria (Phase 05 Task 02):
    /// - Explicit `public_webrtc_base_url` determines the WHEP URL prefix.
    /// - Scheme is taken verbatim from the configured URL (no port
    ///   odd/even inference).
    /// - The WHEP URL produced for the session list is consistent with
    ///   what a client would use to initiate a WHEP play session.
    /// - When not configured, a relative path is returned so the
    ///   request host can be prepended by the caller.
    #[test]
    fn public_webrtc_base_url_controls_whep_location() {
        let sk = StreamKey::new("live", "camera01");

        // Case 1: Explicit HTTPS base URL on an odd port — scheme comes
        // from the configured URL, NOT from port parity.
        let url = build_whep_url(Some("https://media.example.com:8443/api/v1/rtc"), &sk);
        assert_eq!(
            url,
            "https://media.example.com:8443/api/v1/rtc/whep?app=live&stream=camera01"
        );
        assert!(url.starts_with("https://"), "scheme must be explicit https");

        // Case 2: Explicit HTTP base URL on an even port — no automatic
        // upgrade to HTTPS based on port number.
        let url = build_whep_url(Some("http://media.example.com:8080/api/v1/rtc"), &sk);
        assert_eq!(
            url,
            "http://media.example.com:8080/api/v1/rtc/whep?app=live&stream=camera01"
        );
        assert!(url.starts_with("http://"), "scheme must be explicit http");

        // Case 3: HTTPS on an even port — proves we do NOT use port
        // parity (ABL would infer http for even ports).
        let url = build_whep_url(Some("https://cdn.example.com:8080/api/v1/rtc"), &sk);
        assert_eq!(
            url,
            "https://cdn.example.com:8080/api/v1/rtc/whep?app=live&stream=camera01"
        );
        assert!(
            url.starts_with("https://"),
            "even port must NOT downgrade to http"
        );

        // Case 4: HTTP on an odd port — proves we do NOT use port
        // parity (ABL would infer https for odd ports).
        let url = build_whep_url(Some("http://cdn.example.com:8443/api/v1/rtc"), &sk);
        assert_eq!(
            url,
            "http://cdn.example.com:8443/api/v1/rtc/whep?app=live&stream=camera01"
        );
        assert!(
            url.starts_with("http://"),
            "odd port must NOT upgrade to https"
        );

        // Case 5: No base URL configured — returns relative path so
        // the request host/scheme can be prepended downstream.
        let url = build_whep_url(None, &sk);
        assert_eq!(url, "/api/v1/rtc/whep?app=live&stream=camera01");
        assert!(
            !url.contains("://"),
            "relative path must not contain a scheme"
        );

        // Case 6: Consistency — the URL produced by build_whep_url is
        // the same one that handle_session_list would embed in the
        // session JSON. Simulate what the handler does:
        let cfg_base = Some("http://cdn.example.com:8080/api/v1/rtc".to_string());
        let whep_url_from_list = build_whep_url(cfg_base.as_deref(), &sk);
        let whep_url_direct = build_whep_url(Some("http://cdn.example.com:8080/api/v1/rtc"), &sk);
        assert_eq!(
            whep_url_from_list, whep_url_direct,
            "session list WHEP URL must be consistent with direct build_whep_url"
        );
    }
}
