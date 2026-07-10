use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::Value;
use tracing::{debug, info, warn};

use cheetah_gb28181_core::{GbDevice, GbInviteSpec, GbTalkSpec};
use cheetah_gb28181_driver_tokio::{
    start_driver, Gb28181DriverConfig, Gb28181DriverHandle, GbDriverCommand,
};
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    HttpRouteDescriptor, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleHttpService, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, SdkError,
};

use crate::config::Gb28181ModuleConfig;

const MODULE_ID: &str = "gb28181";

/// `Gb28181ModuleFactory` data structure.
/// `Gb28181ModuleFactory` 数据结构.
pub struct Gb28181ModuleFactory;

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

/// `Gb28181Module` data structure.
/// `Gb28181Module` 数据结构.
pub struct Gb28181Module {
    /// `state` field of type `ModuleState`.
    /// `state` 字段，类型为 `ModuleState`.
    state: ModuleState,
    /// `config` field of type `Gb28181ModuleConfig`.
    /// `config` 字段，类型为 `Gb28181ModuleConfig`.
    config: Gb28181ModuleConfig,
    /// `ctx` field.
    /// `ctx` 字段.
    ctx: Option<EngineContext>,
    /// Shared with the HTTP service so the latter sees the driver as soon as `start` sets it.
    /// `update_http_mount` runs at init time — before `start` — so the module can't pass a
    /// concrete handle directly.
    driver_handle: Arc<Mutex<Option<Arc<Gb28181DriverHandle>>>>,
    /// `cancel_token` field.
    /// `cancel_token` 字段.
    cancel_token: Option<CancellationToken>,
    /// `devices` field.
    /// `devices` 字段.
    devices: Arc<Mutex<HashMap<String, GbDevice>>>,
    /// `active_sessions` field.
    /// `active_sessions` 字段.
    active_sessions: Arc<Mutex<HashMap<String, String>>>, // session_key -> device_id
}

impl Gb28181Module {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Gb28181ModuleConfig::default(),
            ctx: None,
            driver_handle: Arc::new(Mutex::new(None)),
            cancel_token: None,
            devices: Arc::new(Mutex::new(HashMap::new())),
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for Gb28181Module {
    fn default() -> Self {
        Self::new()
    }
}

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
        self.ctx = Some(ctx.engine);
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
                "Gb28181Module::start called before init (engine context missing)".to_string(),
            )
        })?;
        let config = self.config.clone();

        self.state = ModuleState::Running;
        self.cancel_token = Some(cancel.clone());

        let listen_udp = config
            .listen_udp
            .parse::<SocketAddr>()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen_udp: {e}")))?;

        let listen_tcp = config
            .listen_tcp
            .parse::<SocketAddr>()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen_tcp: {e}")))?;

        let driver_config = Gb28181DriverConfig {
            listen_udp,
            listen_tcp,
            read_buffer_size: config.read_buffer_size,
            tick_interval_ms: config.tick_interval_ms,
        };

        let handle = Arc::new(start_driver(
            driver_config,
            ctx.runtime_api.clone(),
            cancel.clone(),
        ));
        *self.driver_handle.lock() = Some(handle.clone());

        // Spawn events worker
        {
            let devices = self.devices.clone();
            let runtime_api = ctx.runtime_api.clone();
            let runtime_for_now = ctx.runtime_api.clone();
            let handle_clone = handle.clone();
            let cancel_clone = cancel.clone();
            runtime_api.spawn(Box::pin(async move {
                loop {
                    if cancel_clone.is_cancelled() {
                        break;
                    }
                    match handle_clone.recv_event().await {
                        Some(cheetah_gb28181_core::Gb28181Event::DeviceRegistered {
                            device_id,
                            contact_addr,
                        }) => {
                            info!("GB28181 device registered: {device_id} at {contact_addr}");
                            let now = runtime_for_now.now().as_micros() / 1000;
                            devices.lock().insert(
                                device_id.clone(),
                                GbDevice {
                                    id: device_id,
                                    contact_addr,
                                    expires_at_ms: now + 3600 * 1000,
                                    last_keepalive_ms: now,
                                },
                            );
                        }
                        Some(cheetah_gb28181_core::Gb28181Event::DeviceKeepalive { device_id }) => {
                            debug!("GB28181 device keepalive: {device_id}");
                            let now = runtime_for_now.now().as_micros() / 1000;
                            if let Some(dev) = devices.lock().get_mut(&device_id) {
                                dev.last_keepalive_ms = now;
                                dev.expires_at_ms = now + 3600 * 1000;
                            }
                        }
                        Some(cheetah_gb28181_core::Gb28181Event::DeviceOffline { device_id }) => {
                            info!("GB28181 device offline: {device_id}");
                            devices.lock().remove(&device_id);
                        }
                        Some(cheetah_gb28181_core::Gb28181Event::InviteSuccess {
                            session_key,
                            ssrc,
                        }) => {
                            info!("GB28181 invite success: session={session_key}, ssrc={ssrc}");
                        }
                        Some(cheetah_gb28181_core::Gb28181Event::InviteClosed { session_key }) => {
                            info!("GB28181 invite closed: session={session_key}");
                        }
                        None => break,
                    }
                }
            }));
        }

        // Spawn diagnostics worker
        {
            let runtime_api = ctx.runtime_api.clone();
            let handle_clone = handle.clone();
            let cancel_clone = cancel.clone();
            runtime_api.spawn(Box::pin(async move {
                loop {
                    if cancel_clone.is_cancelled() {
                        break;
                    }
                    match handle_clone.recv_diagnostic().await {
                        Some(d) => {
                            warn!("GB28181 diagnostic warning: {:?}", d);
                        }
                        None => break,
                    }
                }
            }));
        }

        cancel.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Drop the driver handle so any HTTP request that arrives while we're stopping (or
        // before a subsequent restart re-initialises) gets `Unavailable`.
        *self.driver_handle.lock() = None;
        // Clear device registry and active session map so the module can be restarted from
        // a clean state. The driver will reissue REGISTER state on the next start.
        self.devices.lock().clear();
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
                method: HttpMethod::Get,
                path: "/devices".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/invite".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/bye".to_string(),
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
        let engine = self.ctx.clone()?;
        let local_ip = if self.config.public_ip.is_empty() {
            self.config
                .listen_udp
                .parse::<SocketAddr>()
                .map(|addr| addr.ip().to_string())
                .unwrap_or_else(|_| "127.0.0.1".to_string())
        } else {
            self.config.public_ip.clone()
        };
        Some(Arc::new(GbHttpService {
            engine,
            // Shared handle storage: at init time this slot is empty; `start` populates it
            // before the HTTP service starts dispatching requests.
            driver_handle: self.driver_handle.clone(),
            devices: self.devices.clone(),
            active_sessions: self.active_sessions.clone(),
            local_ip,
            default_media_port: self.config.default_media_port,
        }))
    }
}

struct GbHttpService {
    engine: EngineContext,
    /// Shared with `Gb28181Module`. Populated by `start()` and read on every HTTP request;
    /// when the driver isn't yet started, returns `Unavailable`.
    driver_handle: Arc<Mutex<Option<Arc<Gb28181DriverHandle>>>>,
    devices: Arc<Mutex<HashMap<String, GbDevice>>>,
    active_sessions: Arc<Mutex<HashMap<String, String>>>,
    /// Local IP advertised in SIP INVITE/SDP for media reception.
    local_ip: String,
    /// Default local RTP port for media reception when REST request omits `port`.
    default_media_port: u16,
}

impl GbHttpService {
    fn driver(&self) -> Result<Arc<Gb28181DriverHandle>, SdkError> {
        self.driver_handle
            .lock()
            .clone()
            .ok_or_else(|| SdkError::Unavailable("GB28181 driver not yet started".to_string()))
    }
}

async fn call_rtp_service(
    engine: &EngineContext,
    method: HttpMethod,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let rtp_service = engine
        .module_manager_api
        .upgrade()
        .ok_or_else(|| "module manager is unavailable".to_string())?
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.to_string() == "rtp")
        .map(|m| m.service)
        .ok_or_else(|| "RTP module is not mounted or registered".to_string())?;

    let rtp_req = HttpRequest {
        method,
        path: path.to_string(),
        query: None,
        headers: vec![cheetah_sdk::HttpHeader {
            name: "content-type".to_string(),
            value: "application/json".to_string(),
        }],
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    };

    let resp = rtp_service
        .handle(rtp_req)
        .await
        .map_err(|e| format!("RTP service invocation failed: {e:?}"))?;

    if resp.status != 200 {
        return Err(format!("RTP service returned HTTP status {}", resp.status));
    }

    let resp_val: serde_json::Value = serde_json::from_slice(&resp.body)
        .map_err(|e| format!("failed to parse RTP response JSON: {e}"))?;

    Ok(resp_val)
}

#[async_trait]
impl ModuleHttpService for GbHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Get, "/devices") => {
                let devs = self.devices.lock();
                let list: Vec<serde_json::Value> = devs
                    .values()
                    .map(|d| {
                        serde_json::json!({
                            "deviceId": d.id,
                            "contactAddr": d.contact_addr.to_string(),
                            "expiresAtMs": d.expires_at_ms,
                            "lastKeepaliveMs": d.last_keepalive_ms
                        })
                    })
                    .collect();

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": list
                });
                Ok(HttpResponse::ok_json(
                    serde_json::to_vec(&response).unwrap(),
                ))
            }
            (HttpMethod::Post, "/recv/create") | (HttpMethod::Post, "/invite") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let active = body.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
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

                let port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(30000) as u16;

                if active {
                    let device_id = body
                        .get("deviceId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&stream)
                        .to_string();

                    let contact_addr = {
                        let devs = self.devices.lock();
                        devs.get(&device_id).map(|d| d.contact_addr)
                    };

                    let contact_addr = match contact_addr {
                        Some(addr) => addr,
                        None => {
                            if let Some(dest_str) = body.get("ip").and_then(|v| v.as_str()) {
                                let dest_port =
                                    body.get("port").and_then(|v| v.as_u64()).unwrap_or(5060)
                                        as u16;
                                format!("{dest_str}:{dest_port}")
                                    .parse::<SocketAddr>()
                                    .map_err(|e| {
                                        SdkError::InvalidArgument(format!(
                                            "invalid fallback destination: {e}"
                                        ))
                                    })?
                            } else {
                                return Err(SdkError::InvalidArgument(format!(
                                    "Device {device_id} is not registered"
                                )));
                            }
                        }
                    };

                    // Allocate RTP server port and session in-process
                    let rtp_resp = call_rtp_service(
                        &self.engine,
                        HttpMethod::Post,
                        "/server/create",
                        serde_json::json!({
                            "port": port,
                            "appName": app,
                            "streamName": stream,
                            "ssrc": ssrc,
                            "payloadType": "PS",
                            "transportMode": "RecvOnly"
                        }),
                    )
                    .await
                    .map_err(SdkError::Internal)?;

                    let session_key = format!("{app}/{stream}");
                    self.active_sessions
                        .lock()
                        .insert(session_key.clone(), device_id.clone());

                    // Start SIP INVITE
                    let local_port = rtp_resp
                        .get("data")
                        .and_then(|d| d.get("port"))
                        .and_then(|p| p.as_u64())
                        .map(|p| p as u16)
                        .unwrap_or(port);
                    let spec = GbInviteSpec {
                        session_key: session_key.clone(),
                        ssrc,
                        destination: contact_addr,
                        app_name: app.clone(),
                        stream_name: stream.clone(),
                        is_video: true,
                        local_ip: self.local_ip.clone(),
                        local_port,
                    };
                    self.driver()?
                        .send_command(GbDriverCommand::StartInvite(spec))
                        .await
                        .map_err(|e| SdkError::Internal(e.to_string()))?;

                    let response = serde_json::json!({
                        "code": 200,
                        "msg": "success",
                        "data": {
                            "port": rtp_resp.get("data").and_then(|d| d.get("port")).and_then(|p| p.as_u64()).unwrap_or(port as u64),
                            "ssrc": ssrc,
                            "sessionKey": session_key,
                            "deviceId": device_id,
                        }
                    });
                    Ok(HttpResponse::ok_json(
                        serde_json::to_vec(&response).unwrap(),
                    ))
                } else {
                    // Passive receive mode: Allocate RTP server and return
                    let rtp_resp = call_rtp_service(
                        &self.engine,
                        HttpMethod::Post,
                        "/server/create",
                        serde_json::json!({
                            "port": port,
                            "appName": app,
                            "streamName": stream,
                            "ssrc": ssrc,
                            "payloadType": "PS",
                            "transportMode": "RecvOnly"
                        }),
                    )
                    .await
                    .map_err(SdkError::Internal)?;

                    let session_key = format!("{app}/{stream}");

                    let response = serde_json::json!({
                        "code": 200,
                        "msg": "success",
                        "data": {
                            "port": rtp_resp.get("data").and_then(|d| d.get("port")).and_then(|p| p.as_u64()).unwrap_or(port as u64),
                            "ssrc": ssrc,
                            "sessionKey": session_key,
                        }
                    });
                    Ok(HttpResponse::ok_json(
                        serde_json::to_vec(&response).unwrap(),
                    ))
                }
            }
            (HttpMethod::Post, "/recv/stop") | (HttpMethod::Post, "/bye") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid JSON body: {e}")))?;

                let app = extract_app_alias(&body);
                let stream = extract_stream_alias(&body).ok_or_else(|| {
                    SdkError::InvalidArgument("missing field: stream".to_string())
                })?;

                let session_key = format!("{app}/{stream}");

                let had_active = self.active_sessions.lock().remove(&session_key).is_some();
                if had_active {
                    if let Ok(driver) = self.driver() {
                        driver
                            .send_command(GbDriverCommand::StopInvite {
                                session_key: session_key.clone(),
                            })
                            .await
                            .ok();
                    }
                }

                // Stop RTP server receiver via RTP module
                call_rtp_service(
                    &self.engine,
                    HttpMethod::Post,
                    "/server/stop",
                    serde_json::json!({
                        "appName": app,
                        "streamName": stream
                    }),
                )
                .await
                .ok();

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

                // Create RTP client egress via RTP service
                call_rtp_service(
                    &self.engine,
                    HttpMethod::Post,
                    "/client/create",
                    serde_json::json!({
                        "appName": app,
                        "streamName": stream,
                        "peerIp": ip,
                        "peerPort": port,
                        "ssrc": ssrc,
                        "payloadType": "PS",
                        "transportMode": "SendOnly"
                    }),
                )
                .await
                .map_err(SdkError::Internal)?;

                // Start client egress streaming
                call_rtp_service(
                    &self.engine,
                    HttpMethod::Post,
                    "/client/start",
                    serde_json::json!({
                        "appName": app,
                        "streamName": stream
                    }),
                )
                .await
                .map_err(SdkError::Internal)?;

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

                call_rtp_service(
                    &self.engine,
                    HttpMethod::Post,
                    "/client/stop",
                    serde_json::json!({
                        "appName": app,
                        "streamName": stream
                    }),
                )
                .await
                .ok();

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

                let session_key = format!("{app}/{stream}");

                let local_port = body
                    .get("localPort")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u16)
                    .unwrap_or(self.default_media_port);
                let talk_spec = GbTalkSpec {
                    session_key,
                    ssrc,
                    destination: dest_addr,
                    app_name: app,
                    stream_name: stream,
                    local_ip: self.local_ip.clone(),
                    local_port,
                };
                self.driver()?
                    .send_command(GbDriverCommand::StartTalk(talk_spec))
                    .await
                    .map_err(|e| SdkError::Internal(e.to_string()))?;

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success"
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

                if let Ok(driver) = self.driver() {
                    driver
                        .send_command(GbDriverCommand::StopTalk { session_key })
                        .await
                        .ok();
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
