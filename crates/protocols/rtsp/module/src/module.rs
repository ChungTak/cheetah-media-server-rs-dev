//! RTSP module factory, lifecycle, and event loop.
//!
//! `RtspModuleFactory` is registered by the engine. `RtspModule` implements the
//! `Module` trait: init, start, stop, and apply_config. The event loop runs the
//! RTSP server, dispatches driver events to request handlers, and supervises
//! background pull/push/relay jobs.
//!
//! RTSP 模块工厂、生命周期与事件循环。
//!
//! `RtspModuleFactory` 由引擎注册。`RtspModule` 实现 `Module` trait：初始化、
//! 启动、停止、应用配置。事件循环负责运行 RTSP 服务器、将驱动事件分发给请求
//! 处理器并监管后台拉流/推流/转发任务。

use async_trait::async_trait;
use bytes::Bytes;
use cheetah_codec::{RtpPacket, TrackId, TrackInfo};
use cheetah_rtsp_driver_tokio::{
    start_server, start_tls_server, DriverConfig, DriverEvent, DriverTlsConfig, RtspCommand,
    RtspConnectionId, RtspCoreCommandSender, RtspEvent, RtspMethod, RtspRequest, RtspServerHandle,
};
use cheetah_sdk::{
    BootstrapMode, BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext,
    ModuleManifest, ModuleSchemaRegistration, ModuleState, OneShotReceiver, PublisherOptions,
    RuntimeApi, SdkError, ServiceDescriptor, SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::warn;

use crate::config::RtspModuleConfig;
use crate::media::{
    build_frames_from_rtp, build_rtcp_bye, build_rtcp_receiver_report, build_rtcp_sdes_cname,
    build_rtcp_sender_report, build_vp8_frame_from_rtp, build_vp9_frame_from_rtp,
    packetize_frame_to_rtp_with_timestamp, parse_rtcp_sender_report, parse_setup_transport,
    parse_stream_key_from_uri, parse_track_control_from_uri, RtcpReceiverReportBlock,
    RtspSetupTransport,
};
use crate::sdp::{build_describe_sdp, normalize_control, parse_announce_sdp};
use crate::session::{
    PlaySession, PlayTrackState, PlayTransport, PublishSession, PublishUdpTrack,
    RtcpReceiverMetrics, RtspConnectionState, SessionMode,
};

const MODULE_ID: &str = "rtsp";
const RTSP_PUBLIC_METHODS: &str =
    "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, PAUSE, RECORD, TEARDOWN, GET_PARAMETER, SET_PARAMETER";
type RtspErrorResponse = (u16, &'static str, &'static [u8]);
type PauseResponse = (String, Option<String>);

struct PlayRequestMeta {
    cseq: Option<u32>,
    requested_range: Option<String>,
}

struct PauseRequestMeta {
    cseq: Option<u32>,
    requested_range: Option<String>,
}

/// Factory that creates and configures `RtspModule` instances.
///
/// `RtspModule` 实例的工厂。
pub struct RtspModuleFactory;

impl ModuleFactory for RtspModuleFactory {
    /// Returns the module manifest: id, name, HTTP prefix, and capabilities.
    ///
    /// 返回模块清单：id、名称、HTTP 前缀与能力。
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "RTSP Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "rtsp".to_string(),
            routes_prefix: "/rtsp".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    /// Creates a new `RtspModule` instance.
    ///
    /// 创建新的 `RtspModule` 实例。
    fn create(&self) -> Box<dyn Module> {
        Box::new(RtspModule::new())
    }

    /// Registers the JSON schema and validator for the `rtsp` config namespace.
    ///
    /// 注册 `rtsp` 配置命名空间的 JSON schema 与校验器。
    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "rtsp-module".to_string(),
            default_value: RtspModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                RtspModuleConfig::from_value(value.clone())
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })),
        })
    }
}

/// RTSP module instance that ties configuration, engine context, and runtime
/// tasks together.
///
/// RTSP 模块实例，将配置、引擎上下文与运行时任务绑定在一起。
pub struct RtspModule {
    info: ModuleInfo,
    state: ModuleState,
    engine: Option<EngineContext>,
    config: RtspModuleConfig,
    runtime_cancel: Option<CancellationToken>,
    event_loop: Option<OneShotReceiver>,
}

impl RtspModule {
    /// Constructs a new module in the `Created` state.
    ///
    /// 在 `Created` 状态下构造新模块。
    pub fn new() -> Self {
        Self {
            info: ModuleInfo {
                module_id: ModuleId::new(MODULE_ID),
                display_name: "RTSP Module".to_string(),
                state: ModuleState::Created,
            },
            state: ModuleState::Created,
            engine: None,
            config: RtspModuleConfig::default(),
            runtime_cancel: None,
            event_loop: None,
        }
    }
}

impl Default for RtspModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for RtspModule {
    /// Returns the static module information.
    ///
    /// 返回静态模块信息。
    fn info(&self) -> ModuleInfo {
        self.info.clone()
    }

    /// Returns the current module lifecycle state.
    ///
    /// 返回当前模块生命周期状态。
    fn state(&self) -> ModuleState {
        self.state
    }

    /// Initializes the module from `ModuleInitContext` and the parsed config.
    ///
    /// 从 `ModuleInitContext` 与解析后的配置初始化模块。
    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = RtspModuleConfig::from_value(ctx.initial_config)?;
        self.engine = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    /// Starts the RTSP server (and RTSPS server if enabled) and registers the
    /// service with the engine. Spawns the main event loop and background job
    /// supervisors.
    ///
    /// 启动 RTSP 服务器（如启用则含 RTSPS），向引擎注册服务，并启动主事件循环
    /// 与后台任务监管器。
    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        let Some(engine) = self.engine.clone() else {
            return Err(SdkError::Unavailable(
                "rtsp module is not initialized".to_string(),
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
            .map_err(|err| SdkError::InvalidArgument(format!("invalid rtsp.listen: {err}")))?;

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
        .map_err(|err| SdkError::Internal(format!("start rtsp driver failed: {err}")))?;

        if let Err(err) = engine.service_registry.register(ServiceDescriptor {
            name: MODULE_ID.to_string(),
            endpoint: format!("rtsp://{}", self.config.listen),
            metadata: Default::default(),
        }) {
            driver.shutdown();
            let _ = driver.wait().await;
            return Err(SdkError::Internal(format!(
                "register rtsp service failed: {err}"
            )));
        }

        // Start RTSPS (TLS) server if configured.
        if self.config.tls.enabled {
            let tls_listen: SocketAddr = self.config.tls.listen.parse().map_err(|err| {
                SdkError::InvalidArgument(format!("invalid rtsp.tls.listen: {err}"))
            })?;
            let server_config =
                load_rtsp_tls_config(&self.config.tls.cert_path, &self.config.tls.key_path)
                    .map_err(|err| SdkError::Internal(format!("load rtsps tls config: {err}")))?;
            let tls_driver = start_tls_server(
                engine.runtime_api.clone(),
                DriverTlsConfig {
                    listen: tls_listen,
                    server_config: std::sync::Arc::new(server_config),
                    handshake_timeout: std::time::Duration::from_millis(
                        self.config.tls.handshake_timeout_ms,
                    ),
                },
                DriverConfig {
                    write_queue_capacity: self.config.write_queue_capacity,
                    ..DriverConfig::default()
                },
                server_cancel.clone(),
            )
            .map_err(|err| SdkError::Internal(format!("start rtsps driver failed: {err}")))?;

            let _tls_task = spawn_runtime_task(
                engine.runtime_api.clone(),
                run_event_loop(
                    engine.clone(),
                    self.config.clone(),
                    tls_driver,
                    server_cancel.clone(),
                ),
            );
        }

        let event_task = spawn_runtime_task(
            engine.runtime_api.clone(),
            run_event_loop(engine, self.config.clone(), driver, server_cancel.clone()),
        );

        self.runtime_cancel = Some(server_cancel);
        self.event_loop = Some(event_task);
        self.state = ModuleState::Running;
        Ok(())
    }

    /// Stops the runtime task, awaits the event loop, and unregisters the
    /// service from the engine.
    ///
    /// 停止运行时任务、等待事件循环结束并从引擎注销服务。
    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.runtime_cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = self.event_loop.take() {
            let mut join = join;
            let _ = join.recv().await;
        }
        if let Some(engine) = self.engine.as_ref() {
            let _ = engine.service_registry.unregister(MODULE_ID);
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    /// Parses the new config and, if it differs, signals `ModuleRestartRequired`.
    ///
    /// 解析新配置；若发生变化，则返回 `ModuleRestartRequired` 信号。
    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let next = RtspModuleConfig::from_value(change.next)?;
        if next == self.config {
            return Ok(ConfigEffect::Immediate);
        }
        self.config = next;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}

/// Loads the TLS certificate and private key for the RTSPS listener.
///
/// 为 RTSPS 监听器加载 TLS 证书与私钥。
fn load_rtsp_tls_config(cert_path: &str, key_path: &str) -> std::io::Result<rustls::ServerConfig> {
    let cert_data = std::fs::read(cert_path)
        .map_err(|e| std::io::Error::other(format!("read cert {cert_path}: {e}")))?;
    let key_data = std::fs::read(key_path)
        .map_err(|e| std::io::Error::other(format!("read key {key_path}: {e}")))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_data.as_slice())
        .filter_map(|r| r.ok())
        .collect();
    if certs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no certificates found in cert file",
        ));
    }

    let key = rustls_pemfile::private_key(&mut key_data.as_slice())
        .map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse key: {e}"))
        })?
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "no private key found")
        })?;

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("tls config: {e}"))
        })
}

/// Spawns a future on the runtime and returns a one-shot receiver for completion.
///
/// 在运行时上生成一个 Future，并返回用于接收完成通知的 one-shot receiver。
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

/// Main event loop: accepts driver events, dispatches to request handlers,
/// and cleans up sessions and jobs on shutdown.
///
/// 主事件循环：接收驱动事件并分发给请求处理器，关闭时清理会话与任务。
async fn run_event_loop(
    engine: EngineContext,
    config: RtspModuleConfig,
    mut driver: RtspServerHandle,
    cancel: CancellationToken,
) {
    let command_tx = driver.command_sender();
    let sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let multicast = Arc::new(MulticastSenderRegistry::new(config.multicast.clone()));
    let mut pull_job_supervisors =
        spawn_pull_job_supervisors(&engine, &config, cancel.child_token());
    let mut push_job_supervisors =
        spawn_push_job_supervisors(&engine, &config, cancel.child_token());
    let mut relay_job_supervisors =
        spawn_relay_job_supervisors(&engine, &config, cancel.child_token());

    loop {
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
            if let Err(err) = driver.wait().await {
                warn!("rtsp driver listener exited with join error: {err}");
            }
            break;
        }

        let Some(event) = next_event else {
            break;
        };
        handle_driver_event(
            event,
            &engine,
            &config,
            &command_tx,
            sessions.clone(),
            multicast.clone(),
            cancel.clone(),
        )
        .await;
    }

    let session_ids: Vec<RtspConnectionId> = sessions.lock().keys().copied().collect();
    for connection_id in session_ids {
        cleanup_connection(connection_id, &engine, &sessions, &multicast).await;
    }
    wait_pull_job_supervisors(&mut pull_job_supervisors).await;
    wait_push_job_supervisors(&mut push_job_supervisors).await;
    wait_relay_job_supervisors(&mut relay_job_supervisors).await;
}

mod auth;
mod cleanup;
mod client_pull;
mod client_push;
mod client_relay;
mod multicast;
mod play;
mod publish;
mod request_dispatch;
mod response;
mod session_guard;
mod session_lifecycle;
mod udp_ports;

use auth::{build_www_authenticate_headers, check_request_auth, issue_digest_nonce, AuthError};
use cleanup::{cleanup_connection, cleanup_connection_with_config, send_play_rtcp_bye};
use client_pull::{spawn_pull_job_supervisors, wait_pull_job_supervisors};
use client_push::{spawn_push_job_supervisors, wait_push_job_supervisors};
use client_relay::{spawn_relay_job_supervisors, wait_relay_job_supervisors};
use multicast::{MulticastAcquireError, MulticastSenderRegistry};
use play::{handle_describe, handle_pause, handle_play, handle_setup};
use publish::{
    flush_publish_video_reorder, handle_announce, handle_interleaved_frame, handle_record,
    spawn_publish_rtcp_udp_task, spawn_publish_rtp_udp_task, PublishUdpRtpTaskContext,
};
use request_dispatch::handle_driver_event;
use response::{
    apply_pause_response_range, apply_play_response_range, build_pause_response_headers,
    build_play_response_headers, build_rtp_info_header, handle_get_parameter,
    send_basic_ok_with_session, send_response, session_header_value,
};
use session_guard::{
    default_payload_type, format_rtp_ssrc, header_value, interleaved_channels_in_use,
    next_play_interleaved_channels, next_publish_interleaved_channels,
    parse_request_range_scale_headers, play_interleaved_channels_conflict,
    publish_configured_track_count, publish_track_is_already_setup, runtime_unix_time_micros,
    wildcard_bind_addr,
};
#[cfg(test)]
use session_lifecycle::parse_session_token;
use session_lifecycle::{validate_pause_state, validate_record_state, validate_request_session};
use udp_ports::{
    bind_udp_socket_pair, resolve_udp_destination_ip, send_udp_hole_punch_probe,
    UdpSocketPairBindError, MAX_UDP_PORT_PAIR_BIND_ATTEMPTS,
};

#[cfg(test)]
use response::{
    build_get_parameter_response, resolve_pause_response_range, resolve_play_response_range,
};
#[cfg(test)]
use session_guard::{
    normalize_play_range_header, parse_npt_seconds, parse_play_range_header,
    validate_play_scale_header,
};

#[cfg(test)]
mod tests {
    use super::*;
    const TEST_SESSION_TIMEOUT_SECS: u32 = 60;

    fn make_request_with_session(session: Option<&str>) -> RtspRequest {
        RtspRequest {
            method: RtspMethod::Play,
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            version: "RTSP/1.0".to_string(),
            headers: Vec::new(),
            body: Bytes::new(),
            cseq: Some(1),
            session: session.map(str::to_string),
        }
    }

    fn make_request_with_headers(headers: Vec<(&str, &str)>) -> RtspRequest {
        RtspRequest {
            method: RtspMethod::Play,
            uri: "rtsp://127.0.0.1/live/test".to_string(),
            version: "RTSP/1.0".to_string(),
            headers: headers
                .into_iter()
                .map(|(name, value)| cheetah_rtsp_driver_tokio::RtspHeader {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
            body: Bytes::new(),
            cseq: Some(1),
            session: Some("rtsp-1".to_string()),
        }
    }

    #[test]
    fn parse_session_token_handles_timeout_param() {
        assert_eq!(parse_session_token("abc123;timeout=60"), "abc123");
        assert_eq!(parse_session_token("  abc123  "), "abc123");
    }

    #[test]
    fn session_header_value_roundtrips_with_parser() {
        let header = session_header_value("abc123", 75);
        assert_eq!(header, "abc123;timeout=75");
        assert_eq!(parse_session_token(&header), "abc123");
    }

    #[test]
    fn normalize_play_range_header_accepts_valid_npt_forms() {
        assert_eq!(
            normalize_play_range_header("npt=0.000-"),
            Some("npt=0.000-".to_string())
        );
        assert_eq!(
            normalize_play_range_header("NPT=now-"),
            Some("npt=now-".to_string())
        );
        assert_eq!(
            normalize_play_range_header("npt=00:01:02.500-"),
            Some("npt=00:01:02.500-".to_string())
        );
    }

    #[test]
    fn normalize_play_range_header_rejects_invalid_values() {
        assert!(normalize_play_range_header("").is_none());
        assert!(normalize_play_range_header("clock=0-1").is_none());
        assert!(normalize_play_range_header("npt=-").is_none());
        assert!(normalize_play_range_header("npt=abc-1").is_none());
        assert!(normalize_play_range_header("npt=10-5").is_none());
        assert!(normalize_play_range_header("npt=00:00:10-00:00:05").is_none());
        assert!(normalize_play_range_header("npt=5-now").is_none());
        assert!(normalize_play_range_header("npt=now-20.5").is_none());
    }

    #[test]
    fn parse_play_range_header_handles_absent_valid_and_invalid_input() {
        assert_eq!(parse_play_range_header(None).expect("none"), None);
        assert_eq!(
            parse_play_range_header(Some("npt=1.0-"))
                .expect("valid")
                .expect("present"),
            "npt=1.0-"
        );
        let err = parse_play_range_header(Some("clock=1-2")).expect_err("invalid");
        assert_eq!(err.0, 457);
        assert_eq!(err.1, "Invalid Range");
    }

    #[test]
    fn parse_request_range_scale_headers_validates_scale_and_range_together() {
        let req = make_request_with_headers(vec![("Scale", "1.0"), ("Range", "npt=5-10")]);
        let parsed = parse_request_range_scale_headers(&req).expect("valid headers");
        assert_eq!(parsed.as_deref(), Some("npt=5-10"));

        let req = make_request_with_headers(vec![("Scale", "2.0")]);
        let err = parse_request_range_scale_headers(&req).expect_err("invalid scale");
        assert_eq!(err.0, 406);

        let req = make_request_with_headers(vec![("Range", "clock=1-2")]);
        let err = parse_request_range_scale_headers(&req).expect_err("invalid range");
        assert_eq!(err.0, 457);
    }

    #[test]
    fn validate_play_scale_header_accepts_default_and_one() {
        validate_play_scale_header(None).expect("missing scale");
        validate_play_scale_header(Some("1")).expect("integer one");
        validate_play_scale_header(Some("1.0000")).expect("float one");
    }

    #[test]
    fn validate_play_scale_header_rejects_invalid_or_unsupported_values() {
        let err = validate_play_scale_header(Some("")).expect_err("must fail");
        assert_eq!(err.0, 400);

        let err = validate_play_scale_header(Some("foo")).expect_err("must fail");
        assert_eq!(err.0, 400);

        let err = validate_play_scale_header(Some("2.0")).expect_err("must fail");
        assert_eq!(err.0, 406);
        assert_eq!(err.2, b"only Scale: 1.0 is supported");
    }

    #[test]
    fn parse_npt_seconds_supports_decimal_and_hhmmss() {
        let seconds = parse_npt_seconds("12.5").expect("decimal");
        assert!((seconds - 12.5f64).abs() < 1e-9f64);

        let seconds = parse_npt_seconds("01:02:03.25").expect("clock");
        assert!((seconds - 3723.25f64).abs() < 1e-9f64);

        assert!(parse_npt_seconds("now").is_none());
        assert!(parse_npt_seconds("01:02").is_none());
        assert!(parse_npt_seconds("01:70:00").is_none());
        assert!(parse_npt_seconds("01:02:70").is_none());
    }

    #[test]
    fn format_rtp_ssrc_uses_upper_hex_with_padding() {
        assert_eq!(format_rtp_ssrc(0x1A2B_0034), "1A2B0034");
        assert_eq!(format_rtp_ssrc(0x42), "00000042");
    }

    #[test]
    fn validate_request_session_requires_matching_header() {
        let sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
            Arc::new(Mutex::new(HashMap::new()));
        sessions.lock().insert(1, RtspConnectionState::new(1));

        let missing = make_request_with_session(None);
        let err = validate_request_session(1, &missing, &sessions, true).expect_err("should fail");
        assert_eq!(err.0, 454);

        let mismatch = make_request_with_session(Some("other"));
        let err = validate_request_session(1, &mismatch, &sessions, true).expect_err("should fail");
        assert_eq!(err.0, 454);

        let matched = make_request_with_session(Some("rtsp-1"));
        validate_request_session(1, &matched, &sessions, true).expect("should pass");

        let matched_with_timeout = make_request_with_session(Some(&session_header_value(
            "rtsp-1",
            TEST_SESSION_TIMEOUT_SECS,
        )));
        validate_request_session(1, &matched_with_timeout, &sessions, true).expect("should pass");
    }

    #[test]
    fn build_rtp_info_orders_by_channel() {
        let mut control_to_track = HashMap::new();
        control_to_track.insert("trackID=0".to_string(), TrackId(1));
        control_to_track.insert("trackID=1".to_string(), TrackId(2));

        let mut play_tracks = HashMap::new();
        play_tracks.insert(
            TrackId(1),
            PlayTrackState {
                transport: PlayTransport::TcpInterleaved {
                    rtp_channel: 2,
                    rtcp_channel: 3,
                },
                payload_type: 96,
                seq: 200,
                ssrc: 1,
                packets_sent: 0,
                octets_sent: 0,
                last_rtp_timestamp: 3456,
                timestamp_repair_count: 0,
                sdes_sent: false,
                first_raw_timestamp: None,
            },
        );
        play_tracks.insert(
            TrackId(2),
            PlayTrackState {
                transport: PlayTransport::TcpInterleaved {
                    rtp_channel: 0,
                    rtcp_channel: 1,
                },
                payload_type: 97,
                seq: 100,
                ssrc: 2,
                packets_sent: 0,
                octets_sent: 0,
                last_rtp_timestamp: 1234,
                timestamp_repair_count: 0,
                sdes_sent: false,
                first_raw_timestamp: None,
            },
        );

        let header = build_rtp_info_header(
            Some("rtsp://127.0.0.1/live/test"),
            &control_to_track,
            &play_tracks,
        )
        .expect("rtp-info");

        let expected = "url=rtsp://127.0.0.1/live/test/trackID=1;seq=100;rtptime=1234,url=rtsp://127.0.0.1/live/test/trackID=0;seq=200;rtptime=3456";
        assert_eq!(header, expected);
    }

    #[test]
    fn build_play_response_headers_contains_range_scale_and_optional_rtp_info() {
        let headers = build_play_response_headers(
            "sess-1".to_string(),
            "npt=10.000-".to_string(),
            Some("url=rtsp://x/trackID=0;seq=1;rtptime=0".to_string()),
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            headers,
            vec![
                (
                    "Session".to_string(),
                    session_header_value("sess-1", TEST_SESSION_TIMEOUT_SECS),
                ),
                ("Range".to_string(), "npt=10.000-".to_string()),
                ("Scale".to_string(), "1.0".to_string()),
                (
                    "RTP-Info".to_string(),
                    "url=rtsp://x/trackID=0;seq=1;rtptime=0".to_string()
                ),
            ]
        );

        let headers = build_play_response_headers(
            "sess-2".to_string(),
            "npt=0.000-".to_string(),
            None,
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            headers,
            vec![
                (
                    "Session".to_string(),
                    session_header_value("sess-2", TEST_SESSION_TIMEOUT_SECS),
                ),
                ("Range".to_string(), "npt=0.000-".to_string()),
                ("Scale".to_string(), "1.0".to_string()),
            ]
        );
    }

    #[test]
    fn build_pause_response_headers_matches_play_and_publish_modes() {
        let play_headers = build_pause_response_headers(
            "sess-play".to_string(),
            Some("npt=20.500-".to_string()),
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            play_headers,
            vec![
                (
                    "Session".to_string(),
                    session_header_value("sess-play", TEST_SESSION_TIMEOUT_SECS),
                ),
                ("Range".to_string(), "npt=20.500-".to_string()),
                ("Scale".to_string(), "1.0".to_string()),
            ]
        );

        let publish_headers =
            build_pause_response_headers("sess-pub".to_string(), None, TEST_SESSION_TIMEOUT_SECS);
        assert_eq!(
            publish_headers,
            vec![(
                "Session".to_string(),
                session_header_value("sess-pub", TEST_SESSION_TIMEOUT_SECS),
            )]
        );
    }

    #[test]
    fn build_get_parameter_response_keeps_session_and_echoes_body() {
        let (headers, body) = build_get_parameter_response(
            Some("sess-1".to_string()),
            Some("application/x-rtsp-params"),
            Bytes::from_static(b"position\r\n"),
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            headers,
            vec![
                (
                    "Session".to_string(),
                    session_header_value("sess-1", TEST_SESSION_TIMEOUT_SECS),
                ),
                (
                    "Content-Type".to_string(),
                    "application/x-rtsp-params".to_string()
                )
            ]
        );
        assert_eq!(body, Bytes::from_static(b"position\r\n"));
    }

    #[test]
    fn build_get_parameter_response_uses_default_content_type_for_body() {
        let (headers, body) = build_get_parameter_response(
            Some("sess-2".to_string()),
            Some("  "),
            Bytes::from_static(b"ping\r\n"),
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            headers,
            vec![
                (
                    "Session".to_string(),
                    session_header_value("sess-2", TEST_SESSION_TIMEOUT_SECS),
                ),
                ("Content-Type".to_string(), "text/parameters".to_string())
            ]
        );
        assert_eq!(body, Bytes::from_static(b"ping\r\n"));
    }

    #[test]
    fn build_get_parameter_response_without_body_does_not_add_content_type() {
        let (headers, body) = build_get_parameter_response(
            Some("sess-3".to_string()),
            Some("application/x-rtsp-params"),
            Bytes::new(),
            TEST_SESSION_TIMEOUT_SECS,
        );
        assert_eq!(
            headers,
            vec![(
                "Session".to_string(),
                session_header_value("sess-3", TEST_SESSION_TIMEOUT_SECS),
            )]
        );
        assert!(body.is_empty());
    }

    #[test]
    fn resolve_pause_response_range_prefers_request_then_last_then_default() {
        assert_eq!(
            resolve_pause_response_range(Some("npt=5-".to_string()), Some("npt=2-")),
            "npt=5-"
        );
        assert_eq!(resolve_pause_response_range(None, Some("npt=2-")), "npt=2-");
        assert_eq!(resolve_pause_response_range(None, None), "npt=0.000-");
    }

    #[test]
    fn resolve_play_response_range_uses_request_or_default() {
        assert_eq!(
            resolve_play_response_range(Some("npt=8-".to_string())),
            "npt=8-"
        );
        assert_eq!(resolve_play_response_range(None), "npt=0.000-");
    }

    #[test]
    fn play_pause_play_range_sequence_updates_session_state() {
        let mut state = RtspConnectionState::new(42);
        let first_play = apply_play_response_range(&mut state, Some("npt=10.000-".to_string()));
        assert_eq!(first_play, "npt=10.000-");
        assert_eq!(state.play_response_range.as_deref(), Some("npt=10.000-"));

        let pause = apply_pause_response_range(&mut state, None);
        assert_eq!(pause, "npt=10.000-");
        assert_eq!(state.play_response_range.as_deref(), Some("npt=10.000-"));

        let second_play = apply_play_response_range(&mut state, None);
        assert_eq!(second_play, "npt=0.000-");
        assert_eq!(state.play_response_range.as_deref(), Some("npt=0.000-"));
    }

    #[test]
    fn validate_record_state_requires_publish_mode() {
        let err = validate_record_state(Some(SessionMode::Play), true, 1).expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"RECORD requires ANNOUNCE/SETUP");
    }

    #[test]
    fn validate_record_state_requires_publish_session() {
        let err =
            validate_record_state(Some(SessionMode::Publish), false, 1).expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"missing ANNOUNCE before RECORD");
    }

    #[test]
    fn validate_record_state_requires_setup_tracks() {
        let err =
            validate_record_state(Some(SessionMode::Publish), true, 0).expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"RECORD requires SETUP");
    }

    #[test]
    fn validate_record_state_accepts_ready_publish() {
        validate_record_state(Some(SessionMode::Publish), true, 1).expect("should pass");
    }

    #[test]
    fn validate_pause_state_requires_play_task_for_play_mode() {
        let err = validate_pause_state(Some(SessionMode::Play), false, false, false)
            .expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"PAUSE requires PLAY");
    }

    #[test]
    fn validate_pause_state_requires_publish_session_for_publish_mode() {
        let err = validate_pause_state(Some(SessionMode::Publish), false, false, false)
            .expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"missing ANNOUNCE before PAUSE");
    }

    #[test]
    fn validate_pause_state_requires_record_for_publish_mode() {
        let err = validate_pause_state(Some(SessionMode::Publish), false, true, false)
            .expect_err("must fail");
        assert_eq!(err.0, 455);
        assert_eq!(err.2, b"PAUSE requires RECORD");
    }

    #[test]
    fn validate_pause_state_accepts_play_mode_and_publish_record_mode() {
        validate_pause_state(Some(SessionMode::Play), true, false, false).expect("play pause");
        validate_pause_state(Some(SessionMode::Publish), false, true, true).expect("publish pause");
    }

    #[test]
    fn interleaved_channels_in_use_detects_all_cross_collisions() {
        let mut track_channels = HashMap::new();
        let mut rtcp_channels = HashMap::new();
        track_channels.insert(0, TrackId(1));
        rtcp_channels.insert(1, TrackId(1));

        assert!(interleaved_channels_in_use(
            &track_channels,
            &rtcp_channels,
            0,
            3
        ));
        assert!(interleaved_channels_in_use(
            &track_channels,
            &rtcp_channels,
            3,
            1
        ));
        assert!(!interleaved_channels_in_use(
            &track_channels,
            &rtcp_channels,
            2,
            3
        ));
    }

    #[test]
    fn play_interleaved_channels_conflict_ignores_same_track_and_rejects_others() {
        let mut play_tracks = HashMap::new();
        play_tracks.insert(
            TrackId(1),
            PlayTrackState {
                transport: PlayTransport::TcpInterleaved {
                    rtp_channel: 0,
                    rtcp_channel: 1,
                },
                payload_type: 96,
                seq: 10,
                ssrc: 11,
                packets_sent: 0,
                octets_sent: 0,
                last_rtp_timestamp: 0,
                timestamp_repair_count: 0,
                sdes_sent: false,
                first_raw_timestamp: None,
            },
        );
        play_tracks.insert(
            TrackId(2),
            PlayTrackState {
                transport: PlayTransport::UdpUnicast {
                    rtp_socket: Arc::new(NullUdpSocket),
                    rtcp_socket: Arc::new(NullUdpSocket),
                    target_rtp: SocketAddr::from(([127, 0, 0, 1], 5000)),
                    target_rtcp: SocketAddr::from(([127, 0, 0, 1], 5001)),
                },
                payload_type: 97,
                seq: 20,
                ssrc: 22,
                packets_sent: 0,
                octets_sent: 0,
                last_rtp_timestamp: 0,
                timestamp_repair_count: 0,
                sdes_sent: false,
                first_raw_timestamp: None,
            },
        );

        assert!(!play_interleaved_channels_conflict(
            &play_tracks,
            TrackId(1),
            0,
            1
        ));
        assert!(play_interleaved_channels_conflict(
            &play_tracks,
            TrackId(3),
            0,
            5
        ));
        assert!(!play_interleaved_channels_conflict(
            &play_tracks,
            TrackId(3),
            2,
            3
        ));
    }

    struct NullUdpSocket;

    #[async_trait::async_trait]
    impl cheetah_sdk::AsyncUdpSocket for NullUdpSocket {
        async fn recv_from(&self, _buf: &mut [u8]) -> std::io::Result<cheetah_sdk::UdpRecvMeta> {
            Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "null"))
        }

        async fn send_to(&self, _buf: &[u8], _target: SocketAddr) -> std::io::Result<usize> {
            Ok(0)
        }

        fn local_addr(&self) -> std::io::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 0)))
        }
    }
}
