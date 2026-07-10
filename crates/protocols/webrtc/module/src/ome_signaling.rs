//! OvenMediaEngine-compatible WebRTC signaling response helpers.

use std::time::Duration;

use async_trait::async_trait;
use cheetah_sdk::HttpHeader;
use cheetah_sdk::StreamKey;
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcSessionId, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{CandidateTransportPolicy, WebRtcDriverCommand};
use futures::future::BoxFuture;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::compat::{OmeDirection, OmeTransportMode, OmeWebRtcRequest};
use crate::config::WebRtcIceServerConfig;
use crate::session::WebRtcApiKind;

/// `OME_WS_DEFAULT_MAX_MESSAGE_BYTES` constant.
/// `OME_WS_DEFAULT_MAX_MESSAGE_BYTES` 常量。
pub const OME_WS_DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// `OME_WS_DEFAULT_MAX_SDP_BYTES` constant.
/// `OME_WS_DEFAULT_MAX_SDP_BYTES` 常量。
pub const OME_WS_DEFAULT_MAX_SDP_BYTES: usize = 64 * 1024;
/// `OME_WS_DEFAULT_MAX_CANDIDATE_BYTES` constant.
/// `OME_WS_DEFAULT_MAX_CANDIDATE_BYTES` 常量。
pub const OME_WS_DEFAULT_MAX_CANDIDATE_BYTES: usize = 1024;
/// `OME_WS_DEFAULT_MAX_FIELD_BYTES` constant.
/// `OME_WS_DEFAULT_MAX_FIELD_BYTES` 常量。
pub const OME_WS_DEFAULT_MAX_FIELD_BYTES: usize = 128;

/// `OmeIceServersJson` data structure.
/// `OmeIceServersJson` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmeIceServersJson {
    pub standard: Value,
    pub legacy: Value,
}

/// Configuration for `Ome Ws Decoder`.
/// `Ome Ws Decoder` 的配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OmeWsDecoderConfig {
    pub max_message_bytes: usize,
    pub max_sdp_bytes: usize,
    pub max_candidate_bytes: usize,
    pub max_field_bytes: usize,
}

impl Default for OmeWsDecoderConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: OME_WS_DEFAULT_MAX_MESSAGE_BYTES,
            max_sdp_bytes: OME_WS_DEFAULT_MAX_SDP_BYTES,
            max_candidate_bytes: OME_WS_DEFAULT_MAX_CANDIDATE_BYTES,
            max_field_bytes: OME_WS_DEFAULT_MAX_FIELD_BYTES,
        }
    }
}

/// Error returned by `Ome Ws Message` operations.
/// `Ome Ws Message` 操作返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum OmeWsMessageError {
    #[error("payload exceeds {limit} bytes")]
    PayloadTooLarge { limit: usize },
    #[error("invalid json: {0}")]
    InvalidJson(String),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("unsupported command `{0}`")]
    UnsupportedCommand(String),
    #[error("field `{field}` exceeds {limit} bytes (was {actual})")]
    FieldTooLarge {
        field: &'static str,
        limit: usize,
        actual: usize,
    },
    #[error("invalid field `{field}`: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("invalid signaling session id {actual}, expected {expected}")]
    InvalidSessionId { expected: u64, actual: u64 },
}

/// `OmeWsCandidate` data structure.
/// `OmeWsCandidate` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmeWsCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u32>,
    pub username_fragment: Option<String>,
}

/// Message used by `Ome Ws`.
/// `Ome Ws` 使用的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmeWsMessage {
    RequestOffer {
        id: Option<u64>,
        peer_id: Option<u64>,
    },
    Answer {
        id: Option<u64>,
        peer_id: Option<u64>,
        sdp: String,
    },
    Candidate {
        id: Option<u64>,
        peer_id: Option<u64>,
        candidates: Vec<OmeWsCandidate>,
    },
    Stop {
        id: Option<u64>,
        peer_id: Option<u64>,
    },
}

/// `OmeWsAction` enumeration.
/// `OmeWsAction` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmeWsAction {
    RequestOffer,
    ApplyAnswer {
        session_id: Option<cheetah_webrtc_core::WebRtcSessionId>,
        sdp: String,
    },
    AddRemoteCandidates {
        session_id: Option<cheetah_webrtc_core::WebRtcSessionId>,
        candidates: Vec<String>,
    },
    Stop {
        session_id: Option<cheetah_webrtc_core::WebRtcSessionId>,
    },
}

impl OmeWsMessage {
    /// `id` function of `OmeWsMessage`.
    /// `OmeWsMessage` 的 `id` 函数。
    pub fn id(&self) -> Option<u64> {
        match self {
            OmeWsMessage::RequestOffer { id, .. }
            | OmeWsMessage::Answer { id, .. }
            | OmeWsMessage::Candidate { id, .. }
            | OmeWsMessage::Stop { id, .. } => *id,
        }
    }

    /// Converts to `action` representation.
    /// 转换为 `action` 表示。
    pub fn to_action(
        &self,
        session_id: Option<cheetah_webrtc_core::WebRtcSessionId>,
    ) -> OmeWsAction {
        match self {
            OmeWsMessage::RequestOffer { .. } => OmeWsAction::RequestOffer,
            OmeWsMessage::Answer { sdp, .. } => OmeWsAction::ApplyAnswer {
                session_id,
                sdp: sdp.clone(),
            },
            OmeWsMessage::Candidate { candidates, .. } => OmeWsAction::AddRemoteCandidates {
                session_id,
                candidates: candidates
                    .iter()
                    .map(|candidate| candidate.candidate.clone())
                    .collect(),
            },
            OmeWsMessage::Stop { .. } => OmeWsAction::Stop { session_id },
        }
    }
}

/// Response for `Ome Ws Offer`.
/// `Ome Ws Offer` 的响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmeWsOfferResponse {
    pub id: u64,
    pub peer_id: u64,
    pub sdp: String,
    pub candidates: Vec<OmeWsCandidate>,
    pub ice_servers: Option<OmeIceServersJson>,
}

/// `OmeWsRequestOfferPlan` data structure.
/// `OmeWsRequestOfferPlan` 数据结构。
#[derive(Debug, Clone)]
pub struct OmeWsRequestOfferPlan {
    pub session_id: WebRtcSessionId,
    pub stream_key: StreamKey,
    pub role: WebRtcSessionRole,
    pub api_kind: WebRtcApiKind,
    pub offer_spec: WebRtcOfferSpec,
    pub candidate_transport_policy: CandidateTransportPolicy,
    pub ice_servers: Option<OmeIceServersJson>,
}

/// `OmeWsRequestOfferInput` data structure.
/// `OmeWsRequestOfferInput` 数据结构。
pub struct OmeWsRequestOfferInput<'a> {
    pub target: &'a OmeWebRtcRequest,
    pub session_id: WebRtcSessionId,
    pub request_id: Option<u64>,
    pub peer_id: Option<u64>,
    pub tcp_relay_force: bool,
    pub ice_server_configs: &'a [WebRtcIceServerConfig],
    pub offer_timeout: Duration,
}

/// `OmeWsRequestOfferOutcome` data structure.
/// `OmeWsRequestOfferOutcome` 数据结构。
pub struct OmeWsRequestOfferOutcome {
    pub session: crate::session::WebRtcModuleSession,
    pub response_json: String,
}

/// `OmeWsEstablishedOutcome` data structure.
/// `OmeWsEstablishedOutcome` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OmeWsEstablishedOutcome {
    pub closed: bool,
}

/// Error returned by `Ome Ws Session` operations.
/// `Ome Ws Session` 操作返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum OmeWsSessionError {
    #[error("driver offer failed: {0}")]
    OfferFailed(String),
    #[error(transparent)]
    Message(#[from] OmeWsMessageError),
}

/// Sink that consumes `Ome Ws Driver`.
/// 消费 `Ome Ws Driver` 的 Sink。
#[async_trait]
pub trait OmeWsDriverSink: Send + Sync {
    async fn send_command(&self, command: WebRtcDriverCommand);
}

#[async_trait]
impl OmeWsDriverSink for std::sync::Arc<cheetah_webrtc_driver_tokio::WebRtcDriverHandle> {
    async fn send_command(&self, command: WebRtcDriverCommand) {
        cheetah_webrtc_driver_tokio::WebRtcDriverHandle::send_command(self, command).await;
    }
}

/// `OmeWsOfferWaiter` trait.
/// `OmeWsOfferWaiter` trait。
pub trait OmeWsOfferWaiter: Send + Sync {
    fn wait_for_offer(
        &self,
        session_id: WebRtcSessionId,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<String, String>>;
}

impl OmeWsRequestOfferPlan {
    /// Creates the `offer command`.
    /// 创建 `offer command`。
    pub fn create_offer_command(&self) -> WebRtcDriverCommand {
        WebRtcDriverCommand::CreateOffer {
            session_id: self.session_id,
            role: self.role,
            spec: self.offer_spec.clone(),
            candidate_transport_policy: self.candidate_transport_policy,
        }
    }

    /// `registry_session` function of `OmeWsRequestOfferPlan`.
    /// `OmeWsRequestOfferPlan` 的 `registry_session` 函数。
    pub fn registry_session(&self) -> crate::session::WebRtcModuleSession {
        crate::session::WebRtcModuleSession::new(
            self.session_id,
            self.stream_key.clone(),
            self.role,
            self.api_kind,
        )
    }
}

/// Handles the `request offer` event.
/// 处理 `request offer` 事件。
pub async fn handle_request_offer<D, W>(
    input: OmeWsRequestOfferInput<'_>,
    driver: &D,
    waiter: &W,
) -> Result<OmeWsRequestOfferOutcome, OmeWsSessionError>
where
    D: OmeWsDriverSink,
    W: OmeWsOfferWaiter,
{
    let plan = plan_request_offer(
        input.target,
        input.session_id,
        input.tcp_relay_force,
        input.ice_server_configs,
    );
    // Subscribe before sending CreateOffer. The driver may generate
    // OfferReady synchronously on a fast path; subscribing afterwards
    // would let AnswerDispatcher drop the SDP and make OME WS time out.
    let offer_waiter = waiter.wait_for_offer(plan.session_id, input.offer_timeout);
    driver.send_command(plan.create_offer_command()).await;
    let sdp = offer_waiter.await.map_err(OmeWsSessionError::OfferFailed)?;
    let candidates = extract_ome_candidates_from_sdp(&sdp);
    let response_json = render_offer_response(&OmeWsOfferResponse {
        id: input.request_id.unwrap_or(plan.session_id.value()),
        peer_id: input.peer_id.unwrap_or(0),
        sdp,
        candidates,
        ice_servers: plan.ice_servers.clone(),
    })?;
    Ok(OmeWsRequestOfferOutcome {
        session: plan.registry_session(),
        response_json,
    })
}

/// Handles the `established message` event.
/// 处理 `established message` 事件。
pub async fn handle_established_message<D>(
    session_id: WebRtcSessionId,
    expected_signaling_id: u64,
    message: OmeWsMessage,
    driver: &D,
) -> Result<OmeWsEstablishedOutcome, OmeWsSessionError>
where
    D: OmeWsDriverSink,
{
    if !matches!(message, OmeWsMessage::RequestOffer { .. }) {
        match message.id() {
            Some(actual) if actual == expected_signaling_id => {}
            Some(actual) => {
                return Err(OmeWsMessageError::InvalidSessionId {
                    expected: expected_signaling_id,
                    actual,
                }
                .into());
            }
            None => return Err(OmeWsMessageError::MissingField("id").into()),
        }
    }

    match message {
        OmeWsMessage::Answer { sdp, .. } => {
            driver
                .send_command(WebRtcDriverCommand::ApplyRemoteAnswer {
                    session_id,
                    remote_sdp: sdp,
                })
                .await;
            Ok(OmeWsEstablishedOutcome { closed: false })
        }
        OmeWsMessage::Candidate { candidates, .. } => {
            for candidate in candidates {
                driver
                    .send_command(WebRtcDriverCommand::AddRemoteCandidate {
                        session_id,
                        candidate: candidate.candidate,
                    })
                    .await;
            }
            Ok(OmeWsEstablishedOutcome { closed: false })
        }
        OmeWsMessage::Stop { .. } => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::PeerClosed,
                })
                .await;
            Ok(OmeWsEstablishedOutcome { closed: true })
        }
        OmeWsMessage::RequestOffer { .. } => Ok(OmeWsEstablishedOutcome { closed: false }),
    }
}

/// `should_include_ice_servers` function.
/// `should_include_ice_servers` 函数。
pub fn should_include_ice_servers(transport: OmeTransportMode, tcp_relay_force: bool) -> bool {
    tcp_relay_force || matches!(transport, OmeTransportMode::Relay | OmeTransportMode::All)
}

/// `ome_transport_to_candidate_policy` function.
/// `ome_transport_to_candidate_policy` 函数。
pub fn ome_transport_to_candidate_policy(
    transport: OmeTransportMode,
    tcp_relay_force: bool,
) -> CandidateTransportPolicy {
    if tcp_relay_force {
        return CandidateTransportPolicy::RelayOnly;
    }
    match transport {
        OmeTransportMode::Udp => CandidateTransportPolicy::UdpOnly,
        OmeTransportMode::Tcp => CandidateTransportPolicy::TcpOnly,
        OmeTransportMode::Relay => CandidateTransportPolicy::RelayOnly,
        OmeTransportMode::UdpTcp => CandidateTransportPolicy::UdpTcp,
        OmeTransportMode::All => CandidateTransportPolicy::All,
    }
}

/// `plan_request_offer` function.
/// `plan_request_offer` 函数。
pub fn plan_request_offer(
    target: &OmeWebRtcRequest,
    session_id: WebRtcSessionId,
    tcp_relay_force: bool,
    ice_server_configs: &[WebRtcIceServerConfig],
) -> OmeWsRequestOfferPlan {
    let (role, direction) = match target.direction {
        OmeDirection::Send | OmeDirection::Whip => {
            (WebRtcSessionRole::Publisher, WebRtcOfferDirection::RecvOnly)
        }
        OmeDirection::Play => (WebRtcSessionRole::Player, WebRtcOfferDirection::SendOnly),
    };
    OmeWsRequestOfferPlan {
        session_id,
        stream_key: StreamKey::new(&target.app, &target.stream),
        role,
        api_kind: WebRtcApiKind::OmeWs,
        offer_spec: WebRtcOfferSpec {
            video_direction: Some(direction),
            audio_direction: Some(direction),
            data_channel: false,
        },
        candidate_transport_policy: ome_transport_to_candidate_policy(
            target.transport,
            tcp_relay_force,
        ),
        ice_servers: render_ome_ice_servers_json(
            ice_server_configs,
            target.transport,
            tcp_relay_force,
        ),
    }
}

/// Parses `ome ws message` from input.
/// 从输入解析 `ome ws message`。
pub fn parse_ome_ws_message(
    raw: &str,
    config: OmeWsDecoderConfig,
) -> Result<OmeWsMessage, OmeWsMessageError> {
    if raw.len() > config.max_message_bytes {
        return Err(OmeWsMessageError::PayloadTooLarge {
            limit: config.max_message_bytes,
        });
    }
    let value: Value =
        serde_json::from_str(raw).map_err(|err| OmeWsMessageError::InvalidJson(err.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| OmeWsMessageError::InvalidJson("expected json object".into()))?;
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .ok_or(OmeWsMessageError::MissingField("command"))?;
    let id = object.get("id").and_then(Value::as_u64);
    let peer_id = object.get("peer_id").and_then(Value::as_u64);

    match normalize_command(command).as_str() {
        "request_offer" => Ok(OmeWsMessage::RequestOffer { id, peer_id }),
        "answer" => Ok(OmeWsMessage::Answer {
            id,
            peer_id,
            sdp: parse_sdp_object(object, "answer", config.max_sdp_bytes)?,
        }),
        "candidate" => Ok(OmeWsMessage::Candidate {
            id,
            peer_id,
            candidates: parse_candidates(object, config)?,
        }),
        "stop" => Ok(OmeWsMessage::Stop { id, peer_id }),
        other => Err(OmeWsMessageError::UnsupportedCommand(other.to_string())),
    }
}

/// `render_offer_response` function.
/// `render_offer_response` 函数。
pub fn render_offer_response(response: &OmeWsOfferResponse) -> Result<String, OmeWsMessageError> {
    let mut object = Map::new();
    object.insert("command".into(), Value::String("offer".into()));
    object.insert("id".into(), Value::Number(response.id.into()));
    object.insert("peer_id".into(), Value::Number(response.peer_id.into()));
    object.insert("code".into(), Value::Number(200.into()));

    let mut sdp = Map::new();
    sdp.insert("type".into(), Value::String("offer".into()));
    sdp.insert("sdp".into(), Value::String(response.sdp.clone()));
    object.insert("sdp".into(), Value::Object(sdp));

    object.insert(
        "candidates".into(),
        Value::Array(
            response
                .candidates
                .iter()
                .map(candidate_json)
                .collect::<Vec<_>>(),
        ),
    );

    if let Some(ice_servers) = &response.ice_servers {
        object.insert("iceServers".into(), ice_servers.standard.clone());
        object.insert("ice_servers".into(), ice_servers.legacy.clone());
    }

    serde_json::to_string(&Value::Object(object))
        .map_err(|err| OmeWsMessageError::InvalidJson(err.to_string()))
}

/// `render_error_response` function.
/// `render_error_response` 函数。
pub fn render_error_response(
    id: u64,
    peer_id: Option<u64>,
    reason: impl Into<String>,
) -> Result<String, OmeWsMessageError> {
    let mut object = Map::new();
    object.insert("command".into(), Value::String("error".into()));
    object.insert("id".into(), Value::Number(id.into()));
    object.insert("peer_id".into(), Value::Number(peer_id.unwrap_or(0).into()));
    object.insert("reason".into(), Value::String(reason.into()));

    serde_json::to_string(&Value::Object(object))
        .map_err(|err| OmeWsMessageError::InvalidJson(err.to_string()))
}

/// `render_ome_ice_servers_json` function.
/// `render_ome_ice_servers_json` 函数。
pub fn render_ome_ice_servers_json(
    servers: &[WebRtcIceServerConfig],
    transport: OmeTransportMode,
    tcp_relay_force: bool,
) -> Option<OmeIceServersJson> {
    if servers.is_empty() || !should_include_ice_servers(transport, tcp_relay_force) {
        return None;
    }

    let standard = servers
        .iter()
        .map(|server| ice_server_json(server, "username"))
        .collect::<Vec<_>>();
    let legacy = servers
        .iter()
        .map(|server| ice_server_json(server, "user_name"))
        .collect::<Vec<_>>();

    Some(OmeIceServersJson {
        standard: Value::Array(standard),
        legacy: Value::Array(legacy),
    })
}

/// `ice_server_link_headers` function.
/// `ice_server_link_headers` 函数。
pub fn ice_server_link_headers(
    servers: &[WebRtcIceServerConfig],
    transport: OmeTransportMode,
    tcp_relay_force: bool,
) -> Vec<HttpHeader> {
    if !should_include_ice_servers(transport, tcp_relay_force) {
        return Vec::new();
    }

    servers
        .iter()
        .flat_map(|server| {
            server.urls.iter().map(|url| HttpHeader {
                name: "link".into(),
                value: ice_server_link_value(url, server),
            })
        })
        .collect()
}

fn ice_server_json(server: &WebRtcIceServerConfig, username_key: &str) -> Value {
    let mut map = Map::new();
    map.insert("urls".into(), Value::Array(string_values(&server.urls)));
    if let Some(username) = server.username.as_deref() {
        map.insert(username_key.into(), Value::String(username.to_string()));
    }
    if let Some(credential) = server.credential.as_deref() {
        map.insert("credential".into(), Value::String(credential.to_string()));
    }
    Value::Object(map)
}

fn string_values(values: &[String]) -> Vec<Value> {
    values
        .iter()
        .map(|value| Value::String(value.clone()))
        .collect()
}

fn ice_server_link_value(url: &str, server: &WebRtcIceServerConfig) -> String {
    let mut value = format!("<{url}>; rel=\"ice-server\"");
    if let Some(username) = server.username.as_deref() {
        value.push_str(&format!("; username=\"{}\"", escape_header_value(username)));
    }
    if let Some(credential) = server.credential.as_deref() {
        value.push_str(&format!(
            "; credential=\"{}\"",
            escape_header_value(credential)
        ));
    }
    value
}

fn escape_header_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn normalize_command(command: &str) -> String {
    command.trim().to_ascii_lowercase().replace('-', "_")
}

fn parse_sdp_object(
    object: &Map<String, Value>,
    expected_type: &'static str,
    max_sdp_bytes: usize,
) -> Result<String, OmeWsMessageError> {
    let sdp = object
        .get("sdp")
        .and_then(Value::as_object)
        .ok_or(OmeWsMessageError::MissingField("sdp"))?;
    let sdp_type = sdp
        .get("type")
        .and_then(Value::as_str)
        .ok_or(OmeWsMessageError::MissingField("sdp.type"))?;
    if !sdp_type.eq_ignore_ascii_case(expected_type) {
        return Err(OmeWsMessageError::InvalidField {
            field: "sdp.type",
            reason: format!("expected {expected_type:?}"),
        });
    }
    bounded_required_string(sdp, "sdp", max_sdp_bytes)
}

fn parse_candidates(
    object: &Map<String, Value>,
    config: OmeWsDecoderConfig,
) -> Result<Vec<OmeWsCandidate>, OmeWsMessageError> {
    let candidates = object
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or(OmeWsMessageError::MissingField("candidates"))?;

    candidates
        .iter()
        .filter_map(|item| {
            let object = match item.as_object() {
                Some(object) => object,
                None => {
                    return Some(Err(OmeWsMessageError::InvalidField {
                        field: "candidates[]",
                        reason: "expected object".into(),
                    }));
                }
            };
            let candidate =
                match bounded_required_string(object, "candidate", config.max_candidate_bytes) {
                    Ok(candidate) if candidate.is_empty() => return None,
                    Ok(candidate) => normalize_ome_candidate(candidate),
                    Err(err) => return Some(Err(err)),
                };
            if candidate.is_empty() {
                return None;
            }
            let sdp_mid = match bounded_optional_string(object, "sdpMid", config.max_field_bytes) {
                Ok(value) => value,
                Err(err) => return Some(Err(err)),
            };
            let username_fragment =
                match bounded_optional_string(object, "usernameFragment", config.max_field_bytes) {
                    Ok(value) => value,
                    Err(err) => return Some(Err(err)),
                };
            let sdp_mline_index = match parse_optional_u32(object, "sdpMLineIndex") {
                Ok(value) => value,
                Err(err) => return Some(Err(err)),
            };

            Some(Ok(OmeWsCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                username_fragment,
            }))
        })
        .collect()
}

fn bounded_required_string(
    object: &Map<String, Value>,
    field: &'static str,
    limit: usize,
) -> Result<String, OmeWsMessageError> {
    bounded_optional_string(object, field, limit)?.ok_or(OmeWsMessageError::MissingField(field))
}

fn bounded_optional_string(
    object: &Map<String, Value>,
    field: &'static str,
    limit: usize,
) -> Result<Option<String>, OmeWsMessageError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(OmeWsMessageError::InvalidField {
            field,
            reason: "expected string".into(),
        });
    };
    if value.len() > limit {
        return Err(OmeWsMessageError::FieldTooLarge {
            field,
            limit,
            actual: value.len(),
        });
    }
    Ok(Some(value.to_string()))
}

fn parse_optional_u32(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<u32>, OmeWsMessageError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(OmeWsMessageError::InvalidField {
            field,
            reason: "expected unsigned integer".into(),
        });
    };
    if number > u64::from(u32::MAX) {
        return Err(OmeWsMessageError::InvalidField {
            field,
            reason: format!("value {number} exceeds u32::MAX"),
        });
    }
    Ok(Some(number as u32))
}

fn normalize_ome_candidate(candidate: String) -> String {
    let trimmed = candidate.trim();
    if trimmed
        .get(..12)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("a=candidate:"))
    {
        format!("candidate:{}", &trimmed[12..])
    } else {
        trimmed.to_string()
    }
}

fn candidate_json(candidate: &OmeWsCandidate) -> Value {
    let mut object = Map::new();
    object.insert(
        "candidate".into(),
        Value::String(candidate.candidate.clone()),
    );
    if let Some(index) = candidate.sdp_mline_index {
        object.insert("sdpMLineIndex".into(), Value::Number(index.into()));
    }
    if let Some(mid) = candidate.sdp_mid.as_deref() {
        object.insert("sdpMid".into(), Value::String(mid.to_string()));
    }
    if let Some(ufrag) = candidate.username_fragment.as_deref() {
        object.insert("usernameFragment".into(), Value::String(ufrag.to_string()));
    }
    Value::Object(object)
}

/// `extract_ome_candidates_from_sdp` function.
/// `extract_ome_candidates_from_sdp` 函数。
pub fn extract_ome_candidates_from_sdp(sdp: &str) -> Vec<OmeWsCandidate> {
    let mut current_mid: Option<String> = None;
    let mut current_mline_index: Option<u32> = None;
    let mut candidates = Vec::new();
    for line in sdp.lines() {
        if line.starts_with("m=") {
            current_mline_index = Some(current_mline_index.map_or(0, |idx| idx + 1));
            current_mid = None;
            continue;
        }
        if let Some(mid) = line.strip_prefix("a=mid:") {
            current_mid = Some(mid.trim().to_string());
            continue;
        }
        if let Some(candidate) = line.strip_prefix("a=candidate:") {
            candidates.push(OmeWsCandidate {
                candidate: format!("candidate:{}", candidate.trim()),
                sdp_mid: current_mid.clone(),
                sdp_mline_index: current_mline_index,
                username_fragment: None,
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use crate::compat::OmeTransportMode;
    use crate::config::WebRtcIceServerConfig;

    use super::*;

    #[derive(Default)]
    struct RecordingOmeDriver {
        commands: Mutex<Vec<WebRtcDriverCommand>>,
    }

    #[async_trait]
    impl OmeWsDriverSink for RecordingOmeDriver {
        async fn send_command(&self, command: WebRtcDriverCommand) {
            self.commands.lock().await.push(command);
        }
    }

    struct StaticOmeOfferWaiter {
        sdp: String,
    }

    impl OmeWsOfferWaiter for StaticOmeOfferWaiter {
        fn wait_for_offer(
            &self,
            _session_id: WebRtcSessionId,
            _timeout: Duration,
        ) -> BoxFuture<'_, Result<String, String>> {
            let sdp = self.sdp.clone();
            Box::pin(async move { Ok(sdp) })
        }
    }

    struct SubscriptionOrderDriver {
        subscribed: Arc<AtomicBool>,
        sent_before_subscribe: Arc<AtomicBool>,
    }

    #[async_trait]
    impl OmeWsDriverSink for SubscriptionOrderDriver {
        async fn send_command(&self, _command: WebRtcDriverCommand) {
            if !self.subscribed.load(Ordering::Acquire) {
                self.sent_before_subscribe.store(true, Ordering::Release);
            }
        }
    }

    struct SubscriptionOrderWaiter {
        subscribed: Arc<AtomicBool>,
        sdp: String,
    }

    impl OmeWsOfferWaiter for SubscriptionOrderWaiter {
        fn wait_for_offer(
            &self,
            _session_id: WebRtcSessionId,
            _timeout: Duration,
        ) -> BoxFuture<'_, Result<String, String>> {
            self.subscribed.store(true, Ordering::Release);
            let sdp = self.sdp.clone();
            Box::pin(async move { Ok(sdp) })
        }
    }

    fn turn_server() -> WebRtcIceServerConfig {
        WebRtcIceServerConfig {
            urls: vec!["turn:192.168.0.200:3478?transport=tcp".into()],
            username: Some("ome".into()),
            credential: Some("airen".into()),
        }
    }

    #[test]
    fn render_json_includes_standard_and_legacy_ome_ice_server_fields() {
        let rendered =
            render_ome_ice_servers_json(&[turn_server()], OmeTransportMode::Relay, false)
                .expect("relay transport must advertise configured ice servers");

        assert_eq!(
            rendered.standard,
            json!([{
                "urls": ["turn:192.168.0.200:3478?transport=tcp"],
                "username": "ome",
                "credential": "airen"
            }])
        );
        assert_eq!(
            rendered.legacy,
            json!([{
                "urls": ["turn:192.168.0.200:3478?transport=tcp"],
                "user_name": "ome",
                "credential": "airen"
            }])
        );
    }

    #[test]
    fn render_json_omits_ice_servers_for_udptcp_unless_tcp_relay_force_is_enabled() {
        assert!(
            render_ome_ice_servers_json(&[turn_server()], OmeTransportMode::UdpTcp, false)
                .is_none()
        );
        assert!(
            render_ome_ice_servers_json(&[turn_server()], OmeTransportMode::UdpTcp, true).is_some()
        );
    }

    #[test]
    fn render_json_includes_ice_servers_for_all_transport() {
        assert!(
            render_ome_ice_servers_json(&[turn_server()], OmeTransportMode::All, false).is_some()
        );
    }

    #[test]
    fn link_headers_escape_quoted_values_and_expose_each_url() {
        let server = WebRtcIceServerConfig {
            urls: vec![
                "turn:relay.example.com:3478?transport=tcp".into(),
                "turns:relay.example.com:5349".into(),
            ],
            username: Some("om\"e".into()),
            credential: Some("sec\\ret".into()),
        };

        let headers = ice_server_link_headers(&[server], OmeTransportMode::Relay, false);

        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].name, "link");
        assert_eq!(
            headers[0].value,
            "<turn:relay.example.com:3478?transport=tcp>; rel=\"ice-server\"; username=\"om\\\"e\"; credential=\"sec\\\\ret\""
        );
    }

    #[test]
    fn parse_request_offer_accepts_ome_underscore_and_hyphen_aliases() {
        for command in ["request_offer", "request-offer"] {
            let parsed = parse_ome_ws_message(
                &json!({
                    "command": command,
                    "id": 7,
                    "peer_id": 0
                })
                .to_string(),
                OmeWsDecoderConfig::default(),
            )
            .expect("request offer must parse");

            assert_eq!(
                parsed,
                OmeWsMessage::RequestOffer {
                    id: Some(7),
                    peer_id: Some(0)
                }
            );
        }
    }

    #[test]
    fn parse_answer_extracts_nested_sdp() {
        let parsed = parse_ome_ws_message(
            &json!({
                "command": "answer",
                "id": 7,
                "sdp": {
                    "type": "answer",
                    "sdp": "v=0\r\n"
                }
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("answer must parse");

        assert_eq!(
            parsed,
            OmeWsMessage::Answer {
                id: Some(7),
                peer_id: None,
                sdp: "v=0\r\n".into()
            }
        );
    }

    #[test]
    fn parse_answer_accepts_sdp_type_case_insensitively() {
        let parsed = parse_ome_ws_message(
            &json!({
                "command": "answer",
                "id": 7,
                "sdp": {
                    "type": "ANSWER",
                    "sdp": "v=0\r\n"
                }
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("answer type should be case-insensitive");

        assert_eq!(
            parsed,
            OmeWsMessage::Answer {
                id: Some(7),
                peer_id: None,
                sdp: "v=0\r\n".into()
            }
        );
    }

    #[test]
    fn parse_candidate_extracts_candidates_and_ignores_empty_candidate_strings() {
        let parsed = parse_ome_ws_message(
            &json!({
                "command": "candidate",
                "id": 7,
                "candidates": [
                    {
                        "candidate": "candidate:0 1 UDP 50 192.0.2.10 10000 typ host",
                        "sdpMLineIndex": 0,
                        "sdpMid": "video",
                        "usernameFragment": "uf"
                    },
                    {
                        "candidate": "",
                        "sdpMLineIndex": 1
                    }
                ]
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("candidate must parse");

        assert_eq!(
            parsed,
            OmeWsMessage::Candidate {
                id: Some(7),
                peer_id: None,
                candidates: vec![OmeWsCandidate {
                    candidate: "candidate:0 1 UDP 50 192.0.2.10 10000 typ host".into(),
                    sdp_mid: Some("video".into()),
                    sdp_mline_index: Some(0),
                    username_fragment: Some("uf".into()),
                }]
            }
        );
    }

    #[test]
    fn parse_candidate_rejects_overflowing_mline_index() {
        let err = parse_ome_ws_message(
            &json!({
                "command": "candidate",
                "id": 7,
                "candidates": [{
                    "candidate": "candidate:0 1 UDP 50 192.0.2.10 10000 typ host",
                    "sdpMLineIndex": u64::from(u32::MAX) + 1
                }]
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect_err("overflowing m-line index must be rejected");

        assert!(matches!(
            err,
            OmeWsMessageError::InvalidField {
                field: "sdpMLineIndex",
                ..
            }
        ));
    }

    #[test]
    fn parse_candidate_rejects_non_integer_mline_index() {
        let err = parse_ome_ws_message(
            &json!({
                "command": "candidate",
                "id": 7,
                "candidates": [{
                    "candidate": "candidate:0 1 UDP 50 192.0.2.10 10000 typ host",
                    "sdpMLineIndex": "0"
                }]
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect_err("typed m-line index must not be silently ignored");

        assert!(matches!(
            err,
            OmeWsMessageError::InvalidField {
                field: "sdpMLineIndex",
                ..
            }
        ));
    }

    #[test]
    fn parse_candidate_normalizes_sdp_line_prefix() {
        let parsed = parse_ome_ws_message(
            &json!({
                "command": "candidate",
                "id": 7,
                "candidates": [{
                    "candidate": "  a=candidate:0 1 UDP 50 192.0.2.10 10000 typ host  "
                }]
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("candidate with SDP line prefix must parse");

        match parsed {
            OmeWsMessage::Candidate { candidates, .. } => {
                assert_eq!(
                    candidates[0].candidate,
                    "candidate:0 1 UDP 50 192.0.2.10 10000 typ host"
                );
            }
            other => panic!("expected candidate message, got {other:?}"),
        }
    }

    #[test]
    fn parse_candidate_normalizes_sdp_line_prefix_case_insensitively() {
        let parsed = parse_ome_ws_message(
            &json!({
                "command": "candidate",
                "id": 7,
                "candidates": [{
                    "candidate": "A=CANDIDATE:0 1 UDP 50 192.0.2.10 10000 typ host"
                }]
            })
            .to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("candidate prefix should be case-insensitive");

        match parsed {
            OmeWsMessage::Candidate { candidates, .. } => {
                assert_eq!(
                    candidates[0].candidate,
                    "candidate:0 1 UDP 50 192.0.2.10 10000 typ host"
                );
            }
            other => panic!("expected candidate message, got {other:?}"),
        }
    }

    #[test]
    fn parse_stop_and_action_mapping_close_session() {
        let parsed = parse_ome_ws_message(
            &json!({"command": "stop", "id": 7}).to_string(),
            OmeWsDecoderConfig::default(),
        )
        .expect("stop must parse");

        assert_eq!(
            parsed.to_action(Some(cheetah_webrtc_core::WebRtcSessionId::new(77))),
            OmeWsAction::Stop {
                session_id: Some(cheetah_webrtc_core::WebRtcSessionId::new(77))
            }
        );
    }

    #[test]
    fn render_offer_response_includes_candidates_and_both_ice_server_fields() {
        let rendered = render_offer_response(&OmeWsOfferResponse {
            id: 7,
            peer_id: 0,
            sdp: "v=0\r\n".into(),
            candidates: vec![OmeWsCandidate {
                candidate: "candidate:0 1 TCP 50 192.0.2.10 3478 typ relay".into(),
                sdp_mid: Some("video".into()),
                sdp_mline_index: Some(0),
                username_fragment: None,
            }],
            ice_servers: render_ome_ice_servers_json(
                &[turn_server()],
                OmeTransportMode::Relay,
                false,
            ),
        })
        .expect("offer response must render");
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["command"], "offer");
        assert_eq!(value["code"], 200);
        assert_eq!(value["sdp"]["type"], "offer");
        assert_eq!(value["candidates"][0]["sdpMid"], "video");
        assert_eq!(value["iceServers"][0]["username"], "ome");
        assert_eq!(value["ice_servers"][0]["user_name"], "ome");
    }

    #[test]
    fn render_error_response_uses_signaling_id() {
        let rendered = render_error_response(7, Some(3), "bad id").expect("error response");
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["command"], "error");
        assert_eq!(value["id"], 7);
        assert_eq!(value["peer_id"], 3);
        assert_eq!(value["reason"], "bad id");
    }

    #[test]
    fn request_offer_plan_for_send_uses_publisher_recvonly_and_relay_policy() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: None,
            direction: crate::compat::OmeDirection::Send,
            transport: OmeTransportMode::Relay,
        };

        let plan = plan_request_offer(
            &target,
            cheetah_webrtc_core::WebRtcSessionId::new(9),
            false,
            &[turn_server()],
        );

        assert_eq!(
            plan.stream_key,
            cheetah_sdk::StreamKey::new("live", "camera01")
        );
        assert_eq!(plan.role, cheetah_webrtc_core::WebRtcSessionRole::Publisher);
        assert_eq!(plan.api_kind, crate::session::WebRtcApiKind::OmeWs);
        assert_eq!(
            plan.offer_spec.video_direction,
            Some(cheetah_webrtc_core::WebRtcOfferDirection::RecvOnly)
        );
        assert_eq!(
            plan.candidate_transport_policy,
            cheetah_webrtc_driver_tokio::CandidateTransportPolicy::RelayOnly
        );
        assert!(plan.ice_servers.is_some());
    }

    #[test]
    fn request_offer_plan_for_play_uses_player_sendonly_without_default_ice_servers() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: Some("abr".into()),
            direction: crate::compat::OmeDirection::Play,
            transport: OmeTransportMode::UdpTcp,
        };

        let plan = plan_request_offer(
            &target,
            cheetah_webrtc_core::WebRtcSessionId::new(10),
            false,
            &[turn_server()],
        );

        assert_eq!(plan.role, cheetah_webrtc_core::WebRtcSessionRole::Player);
        assert_eq!(
            plan.offer_spec.video_direction,
            Some(cheetah_webrtc_core::WebRtcOfferDirection::SendOnly)
        );
        assert_eq!(
            plan.candidate_transport_policy,
            cheetah_webrtc_driver_tokio::CandidateTransportPolicy::UdpTcp
        );
        assert!(plan.ice_servers.is_none());
    }

    #[test]
    fn request_offer_plan_tcp_relay_force_overrides_transport_and_includes_ice_servers() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: None,
            direction: crate::compat::OmeDirection::Play,
            transport: OmeTransportMode::Udp,
        };

        let plan = plan_request_offer(
            &target,
            cheetah_webrtc_core::WebRtcSessionId::new(11),
            true,
            &[turn_server()],
        );

        assert_eq!(
            plan.candidate_transport_policy,
            cheetah_webrtc_driver_tokio::CandidateTransportPolicy::RelayOnly
        );
        assert!(plan.ice_servers.is_some());
    }

    #[test]
    fn request_offer_plan_builds_driver_command_and_registry_session() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: None,
            direction: crate::compat::OmeDirection::Play,
            transport: OmeTransportMode::Tcp,
        };
        let plan = plan_request_offer(
            &target,
            cheetah_webrtc_core::WebRtcSessionId::new(12),
            false,
            &[],
        );

        let command = plan.create_offer_command();
        match command {
            cheetah_webrtc_driver_tokio::WebRtcDriverCommand::CreateOffer {
                session_id,
                role,
                spec,
                candidate_transport_policy,
            } => {
                assert_eq!(session_id, plan.session_id);
                assert_eq!(role, cheetah_webrtc_core::WebRtcSessionRole::Player);
                assert_eq!(
                    spec.video_direction,
                    Some(cheetah_webrtc_core::WebRtcOfferDirection::SendOnly)
                );
                assert_eq!(
                    candidate_transport_policy,
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::TcpOnly
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let session = plan.registry_session();
        assert_eq!(session.id, plan.session_id);
        assert_eq!(session.stream_key, plan.stream_key);
        assert_eq!(session.role, cheetah_webrtc_core::WebRtcSessionRole::Player);
        assert_eq!(session.api_kind, crate::session::WebRtcApiKind::OmeWs);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_offer_handler_sends_create_offer_and_renders_ome_offer_response() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: None,
            direction: crate::compat::OmeDirection::Play,
            transport: OmeTransportMode::Relay,
        };
        let driver = RecordingOmeDriver::default();
        let waiter = StaticOmeOfferWaiter {
            sdp: concat!(
                "v=0\r\n",
                "a=candidate:0 1 TCP 50 192.0.2.10 3478 typ relay\r\n"
            )
            .into(),
        };

        let outcome = handle_request_offer(
            OmeWsRequestOfferInput {
                target: &target,
                session_id: cheetah_webrtc_core::WebRtcSessionId::new(13),
                request_id: Some(7),
                peer_id: Some(0),
                tcp_relay_force: false,
                ice_server_configs: &[turn_server()],
                offer_timeout: std::time::Duration::from_secs(1),
            },
            &driver,
            &waiter,
        )
        .await
        .expect("request_offer handler must succeed");

        let commands = driver.commands.lock().await;
        assert!(matches!(
            commands.as_slice(),
            [cheetah_webrtc_driver_tokio::WebRtcDriverCommand::CreateOffer { .. }]
        ));
        drop(commands);
        assert_eq!(
            outcome.session.api_kind,
            crate::session::WebRtcApiKind::OmeWs
        );

        let response: serde_json::Value = serde_json::from_str(&outcome.response_json).unwrap();
        assert_eq!(response["command"], "offer");
        assert_eq!(response["id"], 7);
        assert_eq!(response["peer_id"], 0);
        assert_eq!(response["sdp"]["sdp"], waiter.sdp);
        assert_eq!(
            response["candidates"][0]["candidate"],
            "candidate:0 1 TCP 50 192.0.2.10 3478 typ relay"
        );
        assert_eq!(response["iceServers"][0]["username"], "ome");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_offer_handler_subscribes_before_create_offer() {
        let target = crate::compat::OmeWebRtcRequest {
            app: "live".into(),
            stream: "camera01".into(),
            playlist: None,
            direction: crate::compat::OmeDirection::Play,
            transport: OmeTransportMode::UdpTcp,
        };
        let subscribed = Arc::new(AtomicBool::new(false));
        let sent_before_subscribe = Arc::new(AtomicBool::new(false));
        let driver = SubscriptionOrderDriver {
            subscribed: subscribed.clone(),
            sent_before_subscribe: sent_before_subscribe.clone(),
        };
        let waiter = SubscriptionOrderWaiter {
            subscribed,
            sdp: "v=0\r\n".into(),
        };

        handle_request_offer(
            OmeWsRequestOfferInput {
                target: &target,
                session_id: cheetah_webrtc_core::WebRtcSessionId::new(15),
                request_id: Some(7),
                peer_id: Some(0),
                tcp_relay_force: false,
                ice_server_configs: &[],
                offer_timeout: std::time::Duration::from_secs(1),
            },
            &driver,
            &waiter,
        )
        .await
        .expect("request_offer handler must succeed");

        assert!(
            !sent_before_subscribe.load(Ordering::Acquire),
            "CreateOffer was sent before the OME WS offer waiter subscribed"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn established_handler_routes_answer_candidate_and_stop_to_driver() {
        let driver = RecordingOmeDriver::default();
        let session_id = cheetah_webrtc_core::WebRtcSessionId::new(14);

        handle_established_message(
            session_id,
            session_id.value(),
            OmeWsMessage::Answer {
                id: Some(session_id.value()),
                peer_id: Some(0),
                sdp: "v=0\r\nanswer".into(),
            },
            &driver,
        )
        .await
        .expect("valid signaling id must be accepted");
        handle_established_message(
            session_id,
            session_id.value(),
            OmeWsMessage::Candidate {
                id: Some(session_id.value()),
                peer_id: Some(0),
                candidates: vec![OmeWsCandidate {
                    candidate: "candidate:0 1 UDP 50 192.0.2.20 10000 typ host".into(),
                    sdp_mid: Some("video".into()),
                    sdp_mline_index: Some(0),
                    username_fragment: None,
                }],
            },
            &driver,
        )
        .await
        .expect("valid signaling id must be accepted");
        let outcome = handle_established_message(
            session_id,
            session_id.value(),
            OmeWsMessage::Stop {
                id: Some(session_id.value()),
                peer_id: Some(0),
            },
            &driver,
        )
        .await
        .expect("valid signaling id must be accepted");

        assert!(outcome.closed);
        let commands = driver.commands.lock().await;
        assert!(matches!(
            commands.as_slice(),
            [
                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::ApplyRemoteAnswer { .. },
                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::AddRemoteCandidate { .. },
                cheetah_webrtc_driver_tokio::WebRtcDriverCommand::StopSession { .. },
            ]
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn established_handler_rejects_mismatched_signaling_id() {
        let driver = RecordingOmeDriver::default();
        let session_id = cheetah_webrtc_core::WebRtcSessionId::new(14);

        let err = handle_established_message(
            session_id,
            session_id.value(),
            OmeWsMessage::Answer {
                id: Some(999),
                peer_id: Some(0),
                sdp: "v=0\r\nanswer".into(),
            },
            &driver,
        )
        .await
        .expect_err("mismatched OME signaling id must be rejected");

        assert_eq!(
            err,
            OmeWsSessionError::Message(OmeWsMessageError::InvalidSessionId {
                expected: session_id.value(),
                actual: 999
            })
        );
        assert!(driver.commands.lock().await.is_empty());
    }
}
