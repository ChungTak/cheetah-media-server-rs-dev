//! RTP module HTTP control API.
//!
//! RTP 模块 HTTP 控制 API。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_rtp_core::{RtpConnectionType, RtpPayloadMode, RtpTrackFilter, RtpTransportMode};
use cheetah_rtp_driver_tokio::RtpDriverHandle;
use cheetah_sdk::media_api::error::MediaError;
use cheetah_sdk::media_api::ids::MediaKey;
use cheetah_sdk::media_api::model::{RtpSessionState, RtpTcpMode};
use cheetah_sdk::media_api::rtp_session::SourceBindingPolicy;
use cheetah_sdk::{
    BackpressurePolicy, CancellationToken, EngineContext, HttpMethod, HttpRequest, HttpResponse,
    ModuleHttpService, SdkError, StreamKey, SubscriberOptions,
};
use parking_lot::Mutex;
use serde_json::Value;

use crate::egress::{run_egress_session, EgressCleanup};
use crate::orchestrator::RtpSessionOrchestrator;

/// HTTP control API for the RTP module.
///
/// RTP 模块的 HTTP 控制 API。
pub(crate) struct RtpHttpService {
    pub(crate) engine: EngineContext,
    /// Shared session orchestrator used by the `RtpApi` provider and HTTP routes.
    pub(crate) orchestrator: Arc<RtpSessionOrchestrator>,
    pub(crate) active_egress: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Maps logical session_key -> internal driver target session keys (1 entry for single target,
    /// `key#0`/`key#1`/... for multi-target senderInfos use cases).
    pub(crate) client_targets: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Module-scoped cancel token; egress sessions spawn children of this so that
    /// `RtpModule::stop()` cascades cancellation to them.
    pub(crate) module_cancel: CancellationToken,
}

/// `RtpHttpService` helpers.
///
/// `RtpHttpService` 辅助。
impl RtpHttpService {
    /// Retrieve the driver handle, returning `Unavailable` if not started.
    ///
    /// 获取驱动句柄；若未启动则返回 `Unavailable`。
    fn driver(&self) -> Result<Arc<RtpDriverHandle>, SdkError> {
        self.orchestrator
            .driver()
            .map_err(|e| SdkError::Unavailable(e.message.to_string()))
    }
}

/// `ModuleHttpService` implementation for RTP REST endpoints.
///
/// RTP REST 端点的 `ModuleHttpService` 实现。
#[async_trait]
impl ModuleHttpService for RtpHttpService {
    async fn handle(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match (req.method, req.path.as_str()) {
            (HttpMethod::Post, "/server/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                // SMS-compatible: `port` is OPTIONAL. When omitted the module reuses the
                // already-bound driver UDP socket; when provided the driver binds a dedicated
                // socket on the default interface and confirms the actual bound port.
                let port = body.get("port").and_then(|v| v.as_u64()).map(|v| v as u16);
                let bind_addr =
                    port.map(|p| SocketAddr::new(self.orchestrator.default_bind_addr().ip(), p));

                // Accept SMS `socketType` (string `tcp`/`udp`/`both` or numeric 1/2/3) but
                // record it for diagnostic purposes only — the active driver listens on whatever
                // sockets were configured at startup. ABL-style `enable_tcp`/`is_udp` flags are
                // also supported.
                let socket_type = body
                    .get("socketType")
                    .and_then(parse_socket_type)
                    .or_else(|| {
                        let enable_tcp = body.get("enable_tcp").and_then(|v| v.as_bool());
                        let is_udp = body.get("is_udp").and_then(|v| v.as_bool());
                        match (enable_tcp, is_udp) {
                            (Some(true), Some(true)) => Some("both".to_string()),
                            (Some(true), _) => Some("tcp".to_string()),
                            (_, Some(true)) => Some("udp".to_string()),
                            (Some(false), Some(false)) => None,
                            _ => None,
                        }
                    })
                    .unwrap_or_else(|| "udp".to_string());

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/recvStreamId/recv_stream/ssrc".to_string(),
                    )
                })?;

                let ssrc = body.get("ssrc").and_then(|v| v.as_u64()).map(|v| v as u32);
                let payload_mode = body
                    .get("payloadType")
                    .and_then(parse_payload_mode)
                    .unwrap_or(RtpPayloadMode::Ps);

                let transport_mode = body
                    .get("transportMode")
                    .and_then(parse_transport_mode)
                    .unwrap_or(RtpTransportMode::RecvOnly);

                let connection_type = body.get("conType").and_then(parse_connection_type);
                // ABL-style track filtering with `disableVideo` / `disableAudio`. Both flags
                // win over the simpler `onlyAudio` form when present.
                let track_filter = match (
                    body.get("disableVideo").and_then(|v| v.as_bool()),
                    body.get("disableAudio").and_then(|v| v.as_bool()),
                ) {
                    (Some(true), _) => RtpTrackFilter::OnlyAudio,
                    (_, Some(true)) => RtpTrackFilter::OnlyVideo,
                    _ => body
                        .get("onlyAudio")
                        .map(parse_only_audio_to_filter)
                        .unwrap_or(RtpTrackFilter::All),
                };

                let tcp_mode = match connection_type {
                    Some(RtpConnectionType::TcpPassive) => Some(RtpTcpMode::Passive),
                    Some(RtpConnectionType::TcpActive) => Some(RtpTcpMode::Active),
                    _ => None,
                };
                let media_key = MediaKey::with_default_vhost(&app_name, &stream_name, None)
                    .map_err(|e| SdkError::InvalidArgument(e.message.to_string()))?;
                let session_key = format!("{app_name}/{stream_name}");

                let session = self
                    .orchestrator
                    .create_server_session(
                        session_key.clone(),
                        media_key,
                        ssrc,
                        None,
                        payload_mode,
                        transport_mode,
                        connection_type,
                        track_filter,
                        tcp_mode,
                        bind_addr,
                        false,
                        RtpSessionState::Listening,
                        SourceBindingPolicy::default(),
                        None,
                    )
                    .await
                    .map_err(media_error_to_sdk_error)?;

                // ABL-style advisory egress flags. We don't mutate state in the RTP module for
                // these — other modules (HLS / MP4) own the actual egress lifecycle — but we
                // echo them in the response so callers know we accepted the values.
                let enable_hls = body
                    .get("enable_hls")
                    .or_else(|| body.get("enableHls"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let enable_mp4 = body
                    .get("enable_mp4")
                    .or_else(|| body.get("enableMp4"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "port": session.local_port.unwrap_or(0),
                        "socketType": socket_type,
                        "sessionKey": session_key,
                        "ssrc": ssrc.unwrap_or(0),
                        "enableHls": enable_hls,
                        "enableMp4": enable_mp4,
                    }
                });

                json_response(response)
            }
            (HttpMethod::Post, "/server/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/recvStream/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                self.orchestrator
                    .stop_session_by_key(&session_key)
                    .await
                    .map_err(media_error_to_sdk_error)?;

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                json_response(response)
            }
            (HttpMethod::Post, "/client/create") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let default_payload = body
                    .get("payloadType")
                    .and_then(parse_payload_mode)
                    .unwrap_or(RtpPayloadMode::Ps);

                let default_transport = body
                    .get("transportMode")
                    .and_then(parse_transport_mode)
                    .unwrap_or(RtpTransportMode::SendOnly);

                // Build the list of remote targets. Either `senderInfos` array (SMS multi-target)
                // or single peerIp/peerPort/ssrc (single target).
                let mut targets: Vec<(SocketAddr, u32, RtpPayloadMode, RtpTransportMode)> =
                    Vec::new();
                if let Some(arr) = body.get("senderInfos").and_then(|v| v.as_array()) {
                    for entry in arr {
                        let peer_ip =
                            entry
                                .get("peerIp")
                                .and_then(|v| v.as_str())
                                .ok_or_else(|| {
                                    SdkError::InvalidArgument(
                                        "senderInfos[]: missing peerIp".to_string(),
                                    )
                                })?;
                        let peer_port =
                            entry
                                .get("peerPort")
                                .and_then(|v| v.as_u64())
                                .ok_or_else(|| {
                                    SdkError::InvalidArgument(
                                        "senderInfos[]: missing peerPort".to_string(),
                                    )
                                })? as u16;
                        let ssrc = entry.get("ssrc").and_then(|v| v.as_u64()).ok_or_else(|| {
                            SdkError::InvalidArgument("senderInfos[]: missing ssrc".to_string())
                        })? as u32;
                        let payload = entry
                            .get("payloadType")
                            .and_then(parse_payload_mode)
                            .unwrap_or(default_payload);
                        let transport = entry
                            .get("transportMode")
                            .and_then(parse_transport_mode)
                            .unwrap_or(default_transport);
                        let addr = format!("{peer_ip}:{peer_port}")
                            .parse::<SocketAddr>()
                            .map_err(|e| {
                                SdkError::InvalidArgument(format!(
                                    "senderInfos[]: invalid peerIp/peerPort: {e}"
                                ))
                            })?;
                        targets.push((addr, ssrc, payload, transport));
                    }
                } else {
                    // Accept either ZLM `peerIp`/`peerPort` or ABL `dst_url`/`dst_port`.
                    let peer_ip = body
                        .get("peerIp")
                        .and_then(|v| v.as_str())
                        .or_else(|| body.get("dst_url").and_then(|v| v.as_str()))
                        .or_else(|| body.get("dstUrl").and_then(|v| v.as_str()))
                        .ok_or_else(|| {
                            SdkError::InvalidArgument("missing field: peerIp / dst_url".to_string())
                        })?
                        .to_string();
                    let peer_port = body
                        .get("peerPort")
                        .and_then(|v| v.as_u64())
                        .or_else(|| body.get("dst_port").and_then(|v| v.as_u64()))
                        .or_else(|| body.get("dstPort").and_then(|v| v.as_u64()))
                        .ok_or_else(|| {
                            SdkError::InvalidArgument(
                                "missing field: peerPort / dst_port".to_string(),
                            )
                        })? as u16;
                    let ssrc = body.get("ssrc").and_then(|v| v.as_u64()).ok_or_else(|| {
                        SdkError::InvalidArgument("missing field: ssrc".to_string())
                    })? as u32;
                    let dest_addr = format!("{peer_ip}:{peer_port}")
                        .parse::<SocketAddr>()
                        .map_err(|e| {
                            SdkError::InvalidArgument(format!("invalid peerIp/peerPort: {e}"))
                        })?;
                    targets.push((dest_addr, ssrc, default_payload, default_transport));
                }

                let media_key = MediaKey::with_default_vhost(&app_name, &stream_name, None)
                    .map_err(|e| SdkError::InvalidArgument(e.message.to_string()))?;
                let session_key = format!("{app_name}/{stream_name}");
                let mut session_keys = Vec::new();

                let connection_type = body.get("conType").and_then(parse_connection_type);
                // ABL-style `disableVideo`/`disableAudio` win over `onlyAudio`.
                let track_filter = match (
                    body.get("disableVideo").and_then(|v| v.as_bool()),
                    body.get("disableAudio").and_then(|v| v.as_bool()),
                ) {
                    (Some(true), _) => RtpTrackFilter::OnlyAudio,
                    (_, Some(true)) => RtpTrackFilter::OnlyVideo,
                    _ => body
                        .get("onlyAudio")
                        .map(parse_only_audio_to_filter)
                        .unwrap_or(RtpTrackFilter::All),
                };

                for (idx, (dest_addr, ssrc, payload_mode, transport_mode)) in
                    targets.iter().enumerate()
                {
                    let target_session = if targets.len() == 1 {
                        session_key.clone()
                    } else {
                        format!("{session_key}#{idx}")
                    };

                    self.orchestrator
                        .create_client_session(
                            target_session.clone(),
                            media_key.clone(),
                            *dest_addr,
                            dest_addr.to_string(),
                            Some(*ssrc),
                            None,
                            *payload_mode,
                            *transport_mode,
                            connection_type,
                            track_filter,
                            SourceBindingPolicy::default(),
                            None,
                        )
                        .await
                        .map_err(media_error_to_sdk_error)?;
                    session_keys.push(target_session);
                }

                self.client_targets
                    .lock()
                    .insert(session_key.clone(), session_keys.clone());

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                    "data": {
                        "sessionKey": session_key,
                        "targets": session_keys,
                    }
                });

                json_response(response)
            }
            (HttpMethod::Post, "/client/start") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                // Look up registered driver sessions for this stream. If none exist
                // (caller skipped /client/create), fall back to the canonical key.
                let driver_sessions = self
                    .client_targets
                    .lock()
                    .get(&session_key)
                    .cloned()
                    .unwrap_or_else(|| vec![session_key.clone()]);

                // Start egress streaming
                let mut map = self.active_egress.lock();
                if !map.contains_key(&session_key) {
                    // Child of the module cancel so `RtpModule::stop()` cascades to in-flight
                    // egress sessions.
                    let cancel_token = self.module_cancel.child_token();
                    let stream_key = StreamKey::new(&app_name, &stream_name);

                    let runtime_api = self.engine.runtime_api.clone();
                    let engine = self.engine.clone();
                    // Resolve the driver handle once at command time so the spawned task
                    // owns a concrete `Arc<RtpDriverHandle>`. The lookup may legitimately
                    // fail when callers race the module's start; fall through with an early
                    // return if so.
                    let driver_cmd_tx = self.driver()?;
                    let cancel_clone = cancel_token.clone();
                    let orchestrator = self.orchestrator.clone();
                    let cleanup =
                        EgressCleanup::new(self.active_egress.clone(), session_key.clone());
                    let subscriber_options = SubscriberOptions {
                        backpressure: BackpressurePolicy::DropDroppableFirst,
                        ..Default::default()
                    };

                    runtime_api.spawn(Box::pin(async move {
                        run_egress_session(
                            engine,
                            driver_cmd_tx,
                            driver_sessions,
                            stream_key,
                            cancel_clone,
                            Some(orchestrator),
                            Some(cleanup),
                            subscriber_options,
                            0,
                            None,
                            None,
                        )
                        .await;
                    }));

                    map.insert(session_key.clone(), cancel_token);
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                json_response(response)
            }
            (HttpMethod::Post, "/client/stop") => {
                let body: Value = serde_json::from_slice(&req.body)
                    .map_err(|e| SdkError::InvalidArgument(format!("invalid json body: {e}")))?;

                let (app_name, stream_name) = extract_app_stream_aliases(&body);
                let stream_name = stream_name.ok_or_else(|| {
                    SdkError::InvalidArgument(
                        "missing field: streamName/sendStream/ssrc".to_string(),
                    )
                })?;

                let session_key = format!("{app_name}/{stream_name}");

                if let Some(cancel) = self.active_egress.lock().remove(&session_key) {
                    cancel.cancel();
                }

                // Tear down every driver session created for this logical key.
                let driver_sessions = self
                    .client_targets
                    .lock()
                    .remove(&session_key)
                    .unwrap_or_else(|| vec![session_key.clone()]);
                for sk in driver_sessions {
                    self.orchestrator
                        .stop_session_by_key(&sk)
                        .await
                        .map_err(media_error_to_sdk_error)?;
                }

                let response = serde_json::json!({
                    "code": 200,
                    "msg": "success",
                });

                json_response(response)
            }
            _ => Ok(HttpResponse {
                status: 404,
                headers: Vec::new(),
                body: bytes::Bytes::from_static(b"{\"error\":\"not found\"}"),
            }),
        }
    }
}

/// Parse SMS/ZLM-style `socketType` field into a normalized string.
///
/// 将 SMS/ZLM 风格的 `socketType` 字段解析为规范字符串。
pub(crate) fn parse_socket_type(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    1 => Some("udp".to_string()),
                    2 => Some("tcp".to_string()),
                    3 => Some("both".to_string()),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse `transportMode` string or numeric value into `RtpTransportMode`.
///
/// 将 `transportMode` 字符串或数字值解析为 `RtpTransportMode`。
pub(crate) fn parse_transport_mode(val: &serde_json::Value) -> Option<RtpTransportMode> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "recv_only" | "recvonly" => Some(RtpTransportMode::RecvOnly),
            "send_only" | "sendonly" => Some(RtpTransportMode::SendOnly),
            "send_recv" | "sendrecv" => Some(RtpTransportMode::SendRecv),
            _ => None,
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    0 => Some(RtpTransportMode::RecvOnly),
                    1 => Some(RtpTransportMode::SendOnly),
                    2 => Some(RtpTransportMode::SendRecv),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse `payloadType` string or numeric value into `RtpPayloadMode`.
///
/// 将 `payloadType` 字符串或数字值解析为 `RtpPayloadMode`。
pub(crate) fn parse_payload_mode(val: &serde_json::Value) -> Option<RtpPayloadMode> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "ps" => Some(RtpPayloadMode::Ps),
            "ts" => Some(RtpPayloadMode::Ts),
            "es" => Some(RtpPayloadMode::Es),
            "ehome" => Some(RtpPayloadMode::Ehome),
            "xhb" | "hk" => Some(RtpPayloadMode::Xhb),
            "jtt1078" | "1078" => Some(RtpPayloadMode::Jtt1078),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve the canonical `(app, stream)` pair from an inbound REST body, accepting all the
/// alias spellings used by SMS / ZLM / ABL deployments. Returns `None` if no stream can be
/// identified at all (caller should produce an `InvalidArgument` error in that case).
///
/// 从 REST 请求体中解析规范 `(app, stream)` 对，兼容 SMS/ZLM/ABL 的多种字段别名。
pub(crate) fn extract_app_stream_aliases(body: &serde_json::Value) -> (String, Option<String>) {
    let app = body
        .get("appName")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("app").and_then(|v| v.as_str()))
        .or_else(|| body.get("recv_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvApp").and_then(|v| v.as_str()))
        .or_else(|| body.get("send_app").and_then(|v| v.as_str()))
        .or_else(|| body.get("sendApp").and_then(|v| v.as_str()))
        .unwrap_or("live")
        .to_string();
    let stream = body
        .get("streamName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            body.get("recvStreamId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("recv_stream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("recvStream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("send_stream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("sendStream")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("send_stream_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("sendStreamId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("ssrc")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
        });
    (app, stream)
}

/// Parse SMS / ZLM-style `conType` field.
///
/// Accepts string aliases (`tcp_active`, `tcp_passive`, `udp_active`, `udp_passive`,
/// `voice_talk`) and ZLM numeric values (0=tcp_active, 1=udp_active, 2=tcp_passive,
/// 3=udp_passive, 4=voice_talk).
///
/// 解析 SMS/ZLM 风格的 `conType` 字段。
pub(crate) fn parse_connection_type(val: &serde_json::Value) -> Option<RtpConnectionType> {
    match val {
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "tcp_active" | "tcpactive" => Some(RtpConnectionType::TcpActive),
            "tcp_passive" | "tcppassive" => Some(RtpConnectionType::TcpPassive),
            "udp_active" | "udpactive" => Some(RtpConnectionType::UdpActive),
            "udp_passive" | "udppassive" => Some(RtpConnectionType::UdpPassive),
            "voice_talk" | "voicetalk" => Some(RtpConnectionType::VoiceTalk),
            _ => None,
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                match i {
                    0 => Some(RtpConnectionType::TcpActive),
                    1 => Some(RtpConnectionType::UdpActive),
                    2 => Some(RtpConnectionType::TcpPassive),
                    3 => Some(RtpConnectionType::UdpPassive),
                    4 => Some(RtpConnectionType::VoiceTalk),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse `onlyAudio` JSON field into a `RtpTrackFilter`. Accepts:
/// - boolean: true => OnlyAudio, false => All
/// - integer 0/1: 1 => OnlyAudio, 0 => All
/// - string `audio`/`video`/`all`
///
/// 将 `onlyAudio` JSON 字段解析为 `RtpTrackFilter`。
pub(crate) fn parse_only_audio_to_filter(val: &serde_json::Value) -> RtpTrackFilter {
    match val {
        serde_json::Value::Bool(true) => RtpTrackFilter::OnlyAudio,
        serde_json::Value::Bool(false) => RtpTrackFilter::All,
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(1) => RtpTrackFilter::OnlyAudio,
            _ => RtpTrackFilter::All,
        },
        serde_json::Value::String(s) => match s.to_lowercase().as_str() {
            "audio" | "only_audio" | "onlyaudio" => RtpTrackFilter::OnlyAudio,
            "video" | "only_video" | "onlyvideo" => RtpTrackFilter::OnlyVideo,
            _ => RtpTrackFilter::All,
        },
        _ => RtpTrackFilter::All,
    }
}

/// Map a domain `MediaError` into the module-facing `SdkError` used by HTTP routes.
///
/// 将领域 `MediaError` 映射为 HTTP 路由使用的模块 `SdkError`。
pub(crate) fn media_error_to_sdk_error(err: MediaError) -> SdkError {
    use cheetah_sdk::media_api::error::MediaErrorCode;
    let msg = err.message.to_string();
    match err.code {
        MediaErrorCode::InvalidArgument => SdkError::InvalidArgument(msg),
        MediaErrorCode::NotFound => SdkError::NotFound(msg),
        MediaErrorCode::AlreadyExists => SdkError::AlreadyExists(msg),
        MediaErrorCode::Conflict => SdkError::Conflict(msg),
        MediaErrorCode::Unavailable => SdkError::Unavailable(msg),
        MediaErrorCode::RateLimited | MediaErrorCode::Busy => SdkError::Unavailable(msg),
        _ => SdkError::Internal(msg),
    }
}

/// Serialize a JSON response, returning an internal error if serialization fails.
///
/// 序列化 JSON 响应；序列化失败时返回内部错误。
fn json_response(value: serde_json::Value) -> Result<HttpResponse, SdkError> {
    let body = serde_json::to_vec(&value)
        .map_err(|e| SdkError::Internal(format!("failed to serialize response: {e}")))?;
    Ok(HttpResponse::ok_json(body))
}
