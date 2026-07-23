//! GB28181 module factory and implementation.
//!
//! GB28181 模块工厂与实现。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::ids::MediaKey;
use cheetah_sdk::media_api::port::MediaRequestContext;
use cheetah_sdk::media_api::rtp_session::{
    MediaContainer, OpenRtpReceiver, OpenRtpSender, OpenRtpTalk, RtpDirection, RtpPayloadBinding,
    RtpSessionApi, RtpSessionParamsBuilder, RtpSessionRef, RtpTransport, StopRtpSession,
};
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, SdkError,
};
use parking_lot::Mutex;
use serde_json::Value;

use crate::config::{ControlOwner, Gb28181ModuleConfig};

const MODULE_ID: &str = "gb28181";

/// Factory for creating GB28181 modules.
///
/// GB28181 模块工厂。
pub struct Gb28181ModuleFactory;

/// `Gb28181ModuleFactory` implementation.
///
/// `Gb28181ModuleFactory` 实现。
impl ModuleFactory for Gb28181ModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "GB28181 Module".to_string(),
            dependencies: vec![ModuleId::new("rtp")], // Depends on rtp module for media delivery
            config_namespace: "gb28181".to_string(),
            routes_prefix: "/api/v1/gb28181".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(Gb28181Module::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "gb28181-module".to_string(),
            default_value: Gb28181ModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let config = Gb28181ModuleConfig::from_value(value.clone())
                    .map_err(|err| err.to_string())?;
                config.validate()
            })),
        })
    }
}

/// GB28181 module runtime state.
///
/// GB28181 模块运行时状态。
pub struct Gb28181Module {
    state: ModuleState,
    config: Gb28181ModuleConfig,
    ctx: Option<EngineContext>,
    cancel_token: Option<CancellationToken>,
    /// session_key -> (device_id, rtp_session_ref)
    active_sessions: Arc<Mutex<HashMap<String, (String, RtpSessionRef)>>>,
}

/// `Gb28181Module` constructor.
///
/// `Gb28181Module` 构造器。
impl Gb28181Module {
    /// Create a new GB28181 module instance.
    ///
    /// 创建新的 GB28181 模块实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Gb28181ModuleConfig::default(),
            ctx: None,
            cancel_token: None,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// `Default` forward to `Gb28181Module::new`.
///
/// `Default` 转发到 `Gb28181Module::new`。
impl Default for Gb28181Module {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns true when the signaling control plane is enabled and in a rollout
/// mode that can drive mutations for GB resources.
///
/// 当信号控制面已启用且处于可驱动 GB 资源变更的灰度/生产阶段时返回 true。
fn signaling_controls_gb(signaling_cfg: &Value) -> bool {
    let enabled = signaling_cfg
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return false;
    }
    matches!(
        signaling_cfg.get("rollout").and_then(Value::as_str),
        Some("canary") | Some("production")
    )
}

/// `Module` lifecycle and HTTP API for GB28181.
///
/// GB28181 的 `Module` 生命周期与 HTTP API。
#[async_trait]
impl Module for Gb28181Module {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "GB28181 Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = Gb28181ModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        // A disabled module never binds the local listener, so there is no
        // dual-owner risk. Only enforce ownership when the module is enabled.
        if self.config.enabled {
            let signaling_cfg = ctx
                .engine
                .config_provider
                .module(&ModuleId::new("signaling_control_plane"));

            match self.config.control_owner {
                ControlOwner::Signaling => {
                    if !signaling_cfg
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        return Err(SdkError::InvalidArgument(
                            "gb28181.control_owner=signaling requires signaling_control_plane.enabled=true"
                                .to_string(),
                        ));
                    }
                }
                ControlOwner::Local => {
                    if signaling_controls_gb(&signaling_cfg) {
                        return Err(SdkError::InvalidArgument(
                            "gb28181.control_owner=local conflicts with signaling_control_plane canary/production rollout"
                                .to_string(),
                        ));
                    }
                }
            }
        }

        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        if !self.config.enabled {
            // Module is disabled; nothing to run.
            self.state = ModuleState::Running;
            cancel.cancelled().await;
            return Ok(());
        }

        self.ctx.clone().ok_or_else(|| {
            SdkError::InvalidArgument(
                "Gb28181Module::start called before init (engine context missing)".to_string(),
            )
        })?;

        self.state = ModuleState::Running;
        self.cancel_token = Some(cancel.clone());

        // The module only exposes the structured media REST API. SIP/SDP signaling is handled
        // by an external control plane, so no local listener or driver is started here.
        cancel.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Drop any tracked active sessions so the module restarts from a clean state.
        self.active_sessions.lock().clear();
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = Gb28181ModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_config != self.config {
            self.config = new_config;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        if self.config.control_owner == ControlOwner::Signaling {
            return Vec::new();
        }
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/recv/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/recv/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/send/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/send/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/talk/start".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/talk/stop".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        if self.config.control_owner == ControlOwner::Signaling {
            return None;
        }
        let engine = self.ctx.clone()?;
        Some(Arc::new(GbHttpService {
            engine,
            active_sessions: self.active_sessions.clone(),
            default_media_port: self.config.default_media_port,
        }))
    }
}

/// HTTP control API for the GB28181 module.
///
/// GB28181 模块的 HTTP 控制 API。
struct GbHttpService {
    engine: EngineContext,
    /// session_key -> (device_id, rtp_session_ref)
    active_sessions: Arc<Mutex<HashMap<String, (String, RtpSessionRef)>>>,
    /// Default local RTP port for media reception when REST request omits `port`.
    default_media_port: u16,
}

/// `GbHttpService` helpers.
///
/// `GbHttpService` 辅助。
impl GbHttpService {
    /// Return the typed RTP session provider.
    fn rtp_session_api(&self) -> Result<Arc<dyn RtpSessionApi>, SdkError> {
        self.engine.media_services.rtp_session().ok_or_else(|| {
            SdkError::Unavailable("RTP session provider is not available".to_string())
        })
    }

    /// Build the default PS payload binding used by GB28181 streams.
    fn ps_payload_binding(&self) -> RtpPayloadBinding {
        RtpPayloadBinding {
            payload_type: 96,
            codec: "PS".to_string(),
            clock_rate: 90000,
            channels: None,
        }
    }

    /// Construct a `MediaKey` from the GB app/stream aliases.
    fn media_key(&self, app: &str, stream: &str) -> Result<MediaKey, SdkError> {
        MediaKey::with_default_vhost(app, stream, None)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid media key: {e}")))
    }

    /// Open an RTP receiver for the given GB session and return the descriptor.
    async fn open_gb_receiver(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        local_port: u16,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let local_endpoint_hint = SocketAddr::new(
            "0.0.0.0"
                .parse::<std::net::IpAddr>()
                .map_err(|e| SdkError::Internal(e.to_string()))?,
            local_port,
        );
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Receive)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(ssrc)
            .payload_binding(self.ps_payload_binding())
            .local_endpoint_hint(local_endpoint_hint)
            .build();
        let request = OpenRtpReceiver {
            params,
            playback_range: None,
        };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_receiver(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Open an RTP sender for the given GB session and return the descriptor.
    async fn open_gb_sender(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        remote: SocketAddr,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::Send)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::Ps)
            .ssrc(ssrc)
            .payload_binding(self.ps_payload_binding())
            .remote_endpoint(remote)
            .build();
        let request = OpenRtpSender { params };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_sender(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Open a duplex voice-talk session and return the descriptor.
    async fn open_gb_talk(
        &self,
        app: &str,
        stream: &str,
        ssrc: u32,
        remote: SocketAddr,
        local_port: u16,
        payload_binding: RtpPayloadBinding,
    ) -> Result<cheetah_sdk::media_api::rtp_session::RtpSessionDescriptor, SdkError> {
        let media_key = self.media_key(app, stream)?;
        let local_endpoint_hint = SocketAddr::new(
            "0.0.0.0"
                .parse::<std::net::IpAddr>()
                .map_err(|e| SdkError::Internal(e.to_string()))?,
            local_port,
        );
        let params = RtpSessionParamsBuilder::new(media_key, RtpDirection::DuplexTalk)
            .transport(RtpTransport::Udp)
            .container(MediaContainer::ElementaryStream)
            .ssrc(ssrc)
            .payload_binding(payload_binding.clone())
            .remote_endpoint(remote)
            .local_endpoint_hint(local_endpoint_hint)
            .build();
        let request = OpenRtpTalk {
            params,
            talkback_binding: Some(payload_binding),
        };
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        api.open_talk(&ctx, request)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    /// Stop a previously tracked RTP session and return whether it was found.
    async fn stop_gb_session(&self, session_ref: RtpSessionRef) -> Result<bool, SdkError> {
        let ctx = MediaRequestContext::default();
        let api = self.rtp_session_api()?;
        match api
            .stop_session(
                &ctx,
                StopRtpSession {
                    session_ref,
                    release_lease: true,
                },
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.code == cheetah_sdk::media_api::error::MediaErrorCode::NotFound => {
                Ok(false)
            }
            Err(e) => Err(SdkError::Internal(e.to_string())),
        }
    }
}

/// `ModuleHttpService` implementation for GB28181 REST endpoints.
///
/// GB28181 REST 端点的 `ModuleHttpService` 实现。
#[async_trait]
impl ModuleHttpService for GbHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/recv/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let ssrc = body
                    .get("ssrc")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or_else(|| {
                        use std::hash::{Hash, Hasher};
                        let mut s = std::collections::hash_map::DefaultHasher::new();
                        stream.hash(&mut s);
                        (s.finish() % 1_000_000_000) as u32
                    });

                let port = body
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(self.default_media_port as u64) as u16;

                // Allocate RTP server port and session in-process.
                // SIP INVITE/SDP negotiation is performed by the external signaling system.
                let descriptor = self.open_gb_receiver(&app, &stream, ssrc, port).await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key.clone(), (String::new(), rtp_session_ref));

                let local_port = descriptor.endpoints.local.port();

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": local_port,
                        "ssrc": ssrc,
                        "sessionKey": session_key,
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/recv/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let session_key = format!("{app}/{stream}");

                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/send/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let ip = body
                    .get("ip")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SdkError::InvalidArgument("missing field: ip".to_string()))?
                    .to_string();

                let port =
                    body.get("port").and_then(|v| v.as_u64()).ok_or_else(|| {
                        SdkError::InvalidArgument("missing field: port".to_string())
                    })? as u16;

                let ssrc =
                    body.get("ssrc").and_then(|v| v.as_u64()).ok_or_else(|| {
                        SdkError::InvalidArgument("missing field: ssrc".to_string())
                    })? as u32;

                let remote = format!("{ip}:{port}")
                    .parse::<SocketAddr>()
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid destination: {e}")))?;

                // Create RTP sender and start egress in one typed call.
                let descriptor = self.open_gb_sender(&app, &stream, ssrc, remote).await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key, (String::new(), rtp_session_ref));

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "appName": app,
                        "streamName": stream,
                        "ssrc": ssrc,
                        "sessionKey": format!("{app}/{stream}")
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/send/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let session_key = format!("{app}/{stream}");
                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/talk/start") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let ssrc = body
                    .get("ssrc")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or(0);
                let ip = body
                    .get("ip")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1")
                    .to_string();
                let port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(30000) as u16;

                let dest_addr = format!("{ip}:{port}").parse::<SocketAddr>().map_err(|e| {
                    SdkError::InvalidArgument(format!("invalid destination address: {e}"))
                })?;

                let local_port = body
                    .get("localPort")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u16)
                    .unwrap_or(self.default_media_port);

                let payload_type = body.get("pt").and_then(|v| v.as_u64()).unwrap_or(8) as u8;
                let codec = body
                    .get("codec")
                    .and_then(|v| v.as_str())
                    .unwrap_or("PCMA")
                    .to_string();
                let clock_rate = body
                    .get("clockRate")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(8000) as u32;
                let channels = body
                    .get("channels")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u8);

                let payload_binding = RtpPayloadBinding {
                    payload_type,
                    codec,
                    clock_rate,
                    channels,
                };

                let descriptor = self
                    .open_gb_talk(&app, &stream, ssrc, dest_addr, local_port, payload_binding)
                    .await?;

                let session_key = format!("{app}/{stream}");
                let rtp_session_ref = RtpSessionRef {
                    session_id: descriptor.session_id.clone(),
                    expected_generation: descriptor.generation,
                };
                self.active_sessions
                    .lock()
                    .insert(session_key.clone(), (String::new(), rtp_session_ref));

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": descriptor.endpoints.local.port(),
                        "ssrc": ssrc,
                        "sessionKey": session_key,
                    }
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/talk/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let session_key = format!("{app}/{stream}");

                let session_ref = {
                    let mut sessions = self.active_sessions.lock();
                    sessions.remove(&session_key).map(|(_, r)| r)
                };
                if let Some(session_ref) = session_ref {
                    self.stop_gb_session(session_ref).await.ok();
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            _ => Err(SdkError::InvalidArgument("Not Found".to_string())),
        }
    }
}

/// Wall-clock fallback for `now_ms` (legacy/test helper).
///
/// `now_ms` 的墙上时钟回退（遗留/测试辅助）。
fn now_ms() -> u64 {
    // Wallclock fallback used in tests/legacy paths; the live module reads time
    // from the runtime API, not this helper. Kept for parity with prior code.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(dead_code)]
fn _now_ms_keepalive() {
    let _ = now_ms();
}

/// Pull the `stream` identifier out of an inbound REST body, accepting all the alias spellings
/// observed across SMS / ZLM / ABL deployments.
///
/// 从 REST 请求体中提取 `stream` 标识符，兼容 SMS/ZLM/ABL 的字段别名。
fn extract_stream_alias(body: &serde_json::Value) -> Option<String> {
    body.get("stream")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("streamName").and_then(|v| v.as_str()))
        .or_else(|| body.get("recv_stream").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvStream").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvStreamId").and_then(|v| v.as_str()))
        .or_else(|| body.get("send_stream").and_then(|v| v.as_str()))
        .or_else(|| body.get("sendStream").and_then(|v| v.as_str()))
        .or_else(|| body.get("send_stream_id").and_then(|v| v.as_str()))
        .or_else(|| body.get("sendStreamId").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

/// Pull the `app` identifier out of an inbound REST body, accepting alias spellings.
///
/// 从 REST 请求体中提取 `app` 标识符，兼容字段别名。
fn extract_app_alias(body: &serde_json::Value) -> String {
    body.get("app")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("appName").and_then(|v| v.as_str()))
        .or_else(|| body.get("recv_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvApp").and_then(|v| v.as_str()))
        .or_else(|| body.get("send_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("sendApp").and_then(|v| v.as_str()))
        .unwrap_or("live")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signaling_controls_gb_only_in_active_rollout() {
        assert!(!signaling_controls_gb(
            &serde_json::json!({"enabled": false})
        ));
        assert!(!signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "register_only"
        })));
        assert!(!signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "shadow_query"
        })));
        assert!(signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "canary"
        })));
        assert!(signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "production"
        })));
    }

    #[test]
    fn signaling_owner_disables_http_routes() {
        let mut module = Gb28181Module::new();
        module.config.control_owner = ControlOwner::Signaling;
        assert!(module.http_routes().is_empty());
        assert!(module.http_service().is_none());
    }

    #[test]
    fn local_owner_keeps_http_routes() {
        let module = Gb28181Module::new();
        assert_eq!(module.config.control_owner, ControlOwner::Local);
        assert_eq!(module.http_routes().len(), 6);
    }
}
