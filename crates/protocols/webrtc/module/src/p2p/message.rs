//! ZLM-style P2P signaling wire schema.
//!
//! All messages share a small JSON envelope:
//!
//! ```json
//! {
//!   "type": "check_in",
//!   "room_id": "room42",
//!   "peer_id": "peer1",
//!   ...
//! }
//! ```
//!
//! Per the architecture document we explicitly enumerate accepted
//! message types and reject anything else with [`P2pMessage::Error`].
//! Unknown types decode into [`P2pMessage::Unknown`] so the receive
//! loop can log them once and respond with `error` rather than panic.
//!
//! Every string field has a hard length cap. SDP and ICE candidate
//! fields have separate higher caps because they legitimately carry
//! more bytes. Caps are evaluated *after* JSON parsing so a malicious
//! peer can't bypass them by smuggling a long string outside a
//! recognised field.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hard cap on small identifier-like fields. Mirrors the bound from
/// the architecture document (1..128). Names, room ids, peer ids,
/// transport ids all share this cap.
pub const P2P_MAX_FIELD_BYTES: usize = 128;
/// Hard cap on full WebSocket messages we accept. Aligns with ZLM's
/// 4 MB default but stays well below the SCTP DataChannel cap.
pub const P2P_DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Hard cap on SDP fields. Real SDPs sit well below this; the bound
/// just protects the receive loop from amplification.
pub const P2P_DEFAULT_MAX_SDP_BYTES: usize = 64 * 1024;
/// Hard cap on ICE candidate strings.
pub const P2P_DEFAULT_MAX_CANDIDATE_BYTES: usize = 1024;

/// Direction of a P2P session as advertised on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum P2pDirection {
    /// Cheetah pulls media from the remote peer (we're the receiver).
    Pull,
    /// Cheetah pushes media to the remote peer (we're the sender).
    Push,
    /// Bidirectional P2P (e.g. video chat).
    P2p,
}

/// Stream tuple `{ vhost, app, stream }`. All three fields are
/// required; the wire format is identical to ZLM's signaling JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct P2pStreamTuple {
    #[serde(default)]
    pub vhost: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub stream: String,
}

/// Common fields shared by most P2P messages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct P2pMessageHeader {
    pub room_id: Option<String>,
    pub peer_id: Option<String>,
    pub transport_id: Option<String>,
}

/// Decoded wire message. The shape is tagged on the top-level `type`
/// field, matching ZLM's existing protocol so the implementation can
/// be exchanged interchangeably with `WebRtcSignalingPeer`.
#[derive(Debug, Clone, PartialEq)]
pub enum P2pMessage {
    /// `check_in` — register a peer/transport into a room.
    CheckIn {
        header: P2pMessageHeader,
        direction: P2pDirection,
        stream: P2pStreamTuple,
        sdp: Option<String>,
    },
    /// `check_in_ok` — server-side acknowledgement carrying the local
    /// SDP answer when applicable.
    CheckInOk {
        header: P2pMessageHeader,
        sdp: Option<String>,
    },
    /// `offer` — explicit SDP offer (used when offer doesn't piggyback
    /// on `check_in`).
    Offer {
        header: P2pMessageHeader,
        sdp: String,
    },
    /// `answer` — SDP answer.
    Answer {
        header: P2pMessageHeader,
        sdp: String,
    },
    /// `candidate` — trickle ICE candidate.
    Candidate {
        header: P2pMessageHeader,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    },
    /// `bye` — graceful shutdown signal.
    Bye {
        header: P2pMessageHeader,
        reason: Option<String>,
    },
    /// `error` — server-reported error.
    Error {
        header: P2pMessageHeader,
        code: i32,
        message: String,
    },
    /// `ping` / `pong` — keepalive pair.
    Ping {
        header: P2pMessageHeader,
    },
    Pong {
        header: P2pMessageHeader,
    },
    /// `room_list` — list of rooms reported by the peer.
    RoomList {
        header: P2pMessageHeader,
        rooms: Vec<String>,
    },
    /// Unrecognised message type. We keep the raw `type` so the
    /// receive loop can log it and reply with `error`.
    Unknown {
        ty: String,
    },
}

impl P2pMessage {
    /// Wire `type` discriminant.
    pub fn type_name(&self) -> &str {
        match self {
            P2pMessage::CheckIn { .. } => "check_in",
            P2pMessage::CheckInOk { .. } => "check_in_ok",
            P2pMessage::Offer { .. } => "offer",
            P2pMessage::Answer { .. } => "answer",
            P2pMessage::Candidate { .. } => "candidate",
            P2pMessage::Bye { .. } => "bye",
            P2pMessage::Error { .. } => "error",
            P2pMessage::Ping { .. } => "ping",
            P2pMessage::Pong { .. } => "pong",
            P2pMessage::RoomList { .. } => "room_list",
            P2pMessage::Unknown { ty } => ty.as_str(),
        }
    }

    /// True for messages that identify an unknown wire type. Used by
    /// receivers to short-circuit into an `error` reply without
    /// touching session state.
    pub fn is_unknown(&self) -> bool {
        matches!(self, P2pMessage::Unknown { .. })
    }
}

/// Decode failures. The receive loop converts these into
/// [`P2pMessage::Error`] replies.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum P2pMessageError {
    #[error("payload exceeds {limit} bytes")]
    PayloadTooLarge { limit: usize },
    #[error("invalid json: {0}")]
    InvalidJson(String),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("field `{field}` exceeds {limit} bytes (was {actual})")]
    FieldTooLarge {
        field: &'static str,
        limit: usize,
        actual: usize,
    },
    #[error("invalid value for `{field}`: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

/// Decoder configuration. Defaults match the architecture document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pDecoderConfig {
    pub max_message_bytes: usize,
    pub max_sdp_bytes: usize,
    pub max_candidate_bytes: usize,
}

impl Default for P2pDecoderConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: P2P_DEFAULT_MAX_MESSAGE_BYTES,
            max_sdp_bytes: P2P_DEFAULT_MAX_SDP_BYTES,
            max_candidate_bytes: P2P_DEFAULT_MAX_CANDIDATE_BYTES,
        }
    }
}

/// Parse a raw WebSocket text frame into a [`P2pMessage`]. The caller
/// is expected to enforce the websocket message-size limit upstream
/// (we double-check here so validation stays close to the parser).
pub fn parse(raw: &str, config: P2pDecoderConfig) -> Result<P2pMessage, P2pMessageError> {
    if raw.len() > config.max_message_bytes {
        return Err(P2pMessageError::PayloadTooLarge {
            limit: config.max_message_bytes,
        });
    }
    let envelope: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| P2pMessageError::InvalidJson(e.to_string()))?;
    let obj = envelope
        .as_object()
        .ok_or_else(|| P2pMessageError::InvalidJson("expected json object".into()))?;
    let ty = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(P2pMessageError::MissingField("type"))?
        .to_string();

    let header = parse_header(obj)?;
    match ty.as_str() {
        "check_in" => {
            let direction = parse_direction(obj)?;
            let stream = parse_stream(obj)?;
            let sdp = parse_optional_string(obj, "sdp", config.max_sdp_bytes)?;
            Ok(P2pMessage::CheckIn {
                header,
                direction,
                stream,
                sdp,
            })
        }
        "check_in_ok" => {
            let sdp = parse_optional_string(obj, "sdp", config.max_sdp_bytes)?;
            Ok(P2pMessage::CheckInOk { header, sdp })
        }
        "offer" => {
            let sdp = parse_required_string(obj, "sdp", config.max_sdp_bytes)?;
            Ok(P2pMessage::Offer { header, sdp })
        }
        "answer" => {
            let sdp = parse_required_string(obj, "sdp", config.max_sdp_bytes)?;
            Ok(P2pMessage::Answer { header, sdp })
        }
        "candidate" => {
            let candidate = parse_required_string(obj, "candidate", config.max_candidate_bytes)?;
            let sdp_mid = parse_optional_string(obj, "sdpMid", P2P_MAX_FIELD_BYTES)?;
            let sdp_mline_index = obj
                .get("sdpMLineIndex")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32);
            Ok(P2pMessage::Candidate {
                header,
                candidate,
                sdp_mid,
                sdp_mline_index,
            })
        }
        "bye" => {
            let reason = parse_optional_string(obj, "reason", P2P_MAX_FIELD_BYTES)?;
            Ok(P2pMessage::Bye { header, reason })
        }
        "error" => {
            let code = obj.get("code").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let message =
                parse_optional_string(obj, "message", P2P_MAX_FIELD_BYTES)?.unwrap_or_default();
            Ok(P2pMessage::Error {
                header,
                code,
                message,
            })
        }
        "ping" => Ok(P2pMessage::Ping { header }),
        "pong" => Ok(P2pMessage::Pong { header }),
        "room_list" => {
            let rooms: Vec<String> = obj
                .get("rooms")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            // Cap each room id length at P2P_MAX_FIELD_BYTES.
            for room in rooms.iter() {
                if room.len() > P2P_MAX_FIELD_BYTES {
                    return Err(P2pMessageError::FieldTooLarge {
                        field: "rooms[]",
                        limit: P2P_MAX_FIELD_BYTES,
                        actual: room.len(),
                    });
                }
            }
            Ok(P2pMessage::RoomList { header, rooms })
        }
        other => Ok(P2pMessage::Unknown { ty: other.into() }),
    }
}

/// Render a [`P2pMessage`] into a JSON string suitable for sending
/// over WebSocket text frames. Returns the encoded string and the
/// total byte count (so the caller can enforce a peer-side limit
/// without re-walking the buffer).
pub fn render(message: &P2pMessage) -> Result<String, P2pMessageError> {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "type".into(),
        serde_json::Value::String(message.type_name().to_string()),
    );
    let header = match message {
        P2pMessage::CheckIn { header, .. }
        | P2pMessage::CheckInOk { header, .. }
        | P2pMessage::Offer { header, .. }
        | P2pMessage::Answer { header, .. }
        | P2pMessage::Candidate { header, .. }
        | P2pMessage::Bye { header, .. }
        | P2pMessage::Error { header, .. }
        | P2pMessage::Ping { header }
        | P2pMessage::Pong { header }
        | P2pMessage::RoomList { header, .. } => Some(header),
        P2pMessage::Unknown { .. } => None,
    };
    if let Some(header) = header {
        if let Some(room_id) = &header.room_id {
            obj.insert("room_id".into(), serde_json::Value::String(room_id.clone()));
        }
        if let Some(peer_id) = &header.peer_id {
            obj.insert("peer_id".into(), serde_json::Value::String(peer_id.clone()));
        }
        if let Some(transport_id) = &header.transport_id {
            obj.insert(
                "transport_id".into(),
                serde_json::Value::String(transport_id.clone()),
            );
        }
    }
    match message {
        P2pMessage::CheckIn {
            direction,
            stream,
            sdp,
            ..
        } => {
            obj.insert(
                "direction".into(),
                serde_json::to_value(direction)
                    .map_err(|e| P2pMessageError::InvalidJson(e.to_string()))?,
            );
            obj.insert(
                "stream".into(),
                serde_json::to_value(stream)
                    .map_err(|e| P2pMessageError::InvalidJson(e.to_string()))?,
            );
            if let Some(sdp) = sdp {
                obj.insert("sdp".into(), serde_json::Value::String(sdp.clone()));
            }
        }
        P2pMessage::CheckInOk { sdp, .. } => {
            if let Some(sdp) = sdp {
                obj.insert("sdp".into(), serde_json::Value::String(sdp.clone()));
            }
        }
        P2pMessage::Offer { sdp, .. } | P2pMessage::Answer { sdp, .. } => {
            obj.insert("sdp".into(), serde_json::Value::String(sdp.clone()));
        }
        P2pMessage::Candidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            obj.insert(
                "candidate".into(),
                serde_json::Value::String(candidate.clone()),
            );
            if let Some(sdp_mid) = sdp_mid {
                obj.insert("sdpMid".into(), serde_json::Value::String(sdp_mid.clone()));
            }
            if let Some(sdp_mline_index) = sdp_mline_index {
                obj.insert(
                    "sdpMLineIndex".into(),
                    serde_json::Value::Number((*sdp_mline_index).into()),
                );
            }
        }
        P2pMessage::Bye { reason, .. } => {
            if let Some(reason) = reason {
                obj.insert("reason".into(), serde_json::Value::String(reason.clone()));
            }
        }
        P2pMessage::Error { code, message, .. } => {
            obj.insert("code".into(), serde_json::Value::Number((*code).into()));
            obj.insert("message".into(), serde_json::Value::String(message.clone()));
        }
        P2pMessage::Ping { .. } | P2pMessage::Pong { .. } => {}
        P2pMessage::RoomList { rooms, .. } => {
            obj.insert(
                "rooms".into(),
                serde_json::Value::Array(
                    rooms
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        P2pMessage::Unknown { .. } => {
            // Re-rendering an unknown message would echo it back to
            // the wire. We refuse so a client can't smuggle an
            // arbitrary type through us.
            return Err(P2pMessageError::InvalidField {
                field: "type",
                reason: "cannot render an unknown message".into(),
            });
        }
    }
    serde_json::to_string(&serde_json::Value::Object(obj))
        .map_err(|e| P2pMessageError::InvalidJson(e.to_string()))
}

fn parse_header(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<P2pMessageHeader, P2pMessageError> {
    Ok(P2pMessageHeader {
        room_id: parse_optional_string(obj, "room_id", P2P_MAX_FIELD_BYTES)?,
        peer_id: parse_optional_string(obj, "peer_id", P2P_MAX_FIELD_BYTES)?,
        transport_id: parse_optional_string(obj, "transport_id", P2P_MAX_FIELD_BYTES)?,
    })
}

fn parse_direction(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<P2pDirection, P2pMessageError> {
    let raw = obj
        .get("direction")
        .and_then(|v| v.as_str())
        .ok_or(P2pMessageError::MissingField("direction"))?;
    match raw {
        "pull" => Ok(P2pDirection::Pull),
        "push" => Ok(P2pDirection::Push),
        "p2p" => Ok(P2pDirection::P2p),
        other => Err(P2pMessageError::InvalidField {
            field: "direction",
            reason: format!("unexpected value `{other}`"),
        }),
    }
}

fn parse_stream(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<P2pStreamTuple, P2pMessageError> {
    let stream_obj = obj
        .get("stream")
        .and_then(|v| v.as_object())
        .ok_or(P2pMessageError::MissingField("stream"))?;
    let vhost = parse_optional_string(stream_obj, "vhost", P2P_MAX_FIELD_BYTES)?
        .unwrap_or_else(|| "__defaultVhost__".to_string());
    let app = parse_required_string(stream_obj, "app", P2P_MAX_FIELD_BYTES)?;
    let stream = parse_required_string(stream_obj, "stream", P2P_MAX_FIELD_BYTES)?;
    Ok(P2pStreamTuple { vhost, app, stream })
}

fn parse_required_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    limit: usize,
) -> Result<String, P2pMessageError> {
    let raw = obj
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or(P2pMessageError::MissingField(field))?;
    if raw.len() > limit {
        return Err(P2pMessageError::FieldTooLarge {
            field,
            limit,
            actual: raw.len(),
        });
    }
    Ok(raw.to_string())
}

fn parse_optional_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    limit: usize,
) -> Result<Option<String>, P2pMessageError> {
    match obj.get(field) {
        Some(serde_json::Value::String(s)) => {
            if s.len() > limit {
                return Err(P2pMessageError::FieldTooLarge {
                    field,
                    limit,
                    actual: s.len(),
                });
            }
            Ok(Some(s.clone()))
        }
        Some(serde_json::Value::Null) | None => Ok(None),
        Some(_) => Err(P2pMessageError::InvalidField {
            field,
            reason: "expected string".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> P2pDecoderConfig {
        P2pDecoderConfig::default()
    }

    #[test]
    fn parses_check_in_message() {
        let raw = r#"{
            "type": "check_in",
            "room_id": "room42",
            "peer_id": "peer1",
            "transport_id": "tr1",
            "direction": "pull",
            "stream": {"vhost": "__defaultVhost__", "app": "live", "stream": "demo"},
            "sdp": "v=0\r\n"
        }"#;
        let msg = parse(raw, cfg()).unwrap();
        match msg {
            P2pMessage::CheckIn {
                header,
                direction,
                stream,
                sdp,
            } => {
                assert_eq!(header.room_id.as_deref(), Some("room42"));
                assert_eq!(header.peer_id.as_deref(), Some("peer1"));
                assert_eq!(header.transport_id.as_deref(), Some("tr1"));
                assert_eq!(direction, P2pDirection::Pull);
                assert_eq!(stream.app, "live");
                assert_eq!(stream.stream, "demo");
                assert_eq!(sdp.as_deref(), Some("v=0\r\n"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn render_round_trips_check_in() {
        let original = P2pMessage::CheckIn {
            header: P2pMessageHeader {
                room_id: Some("r".into()),
                peer_id: Some("p".into()),
                transport_id: None,
            },
            direction: P2pDirection::Push,
            stream: P2pStreamTuple {
                vhost: "v".into(),
                app: "a".into(),
                stream: "s".into(),
            },
            sdp: Some("v=0\r\n".into()),
        };
        let rendered = render(&original).unwrap();
        let decoded = parse(&rendered, cfg()).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn unknown_type_decodes_into_unknown_variant() {
        let msg = parse(r#"{"type":"whatever"}"#, cfg()).unwrap();
        assert!(msg.is_unknown());
        match msg {
            P2pMessage::Unknown { ty } => assert_eq!(ty, "whatever"),
            other => panic!("expected unknown, got {other:?}"),
        }
    }

    #[test]
    fn rejects_message_above_size_cap() {
        let big = "x".repeat(P2P_DEFAULT_MAX_MESSAGE_BYTES + 1);
        let raw = format!(r#"{{"type":"ping","note":"{big}"}}"#);
        let cfg = P2pDecoderConfig {
            max_message_bytes: 64,
            ..Default::default()
        };
        let err = parse(&raw, cfg).unwrap_err();
        assert!(matches!(err, P2pMessageError::PayloadTooLarge { .. }));
    }

    #[test]
    fn rejects_field_above_cap() {
        let big = "x".repeat(P2P_MAX_FIELD_BYTES + 1);
        let raw = format!(r#"{{"type":"ping","room_id":"{big}"}}"#);
        let err = parse(&raw, cfg()).unwrap_err();
        assert!(matches!(err, P2pMessageError::FieldTooLarge { .. }));
    }

    #[test]
    fn rejects_oversize_sdp() {
        let big = "v".repeat(P2P_DEFAULT_MAX_SDP_BYTES + 1);
        let raw = format!(r#"{{"type":"offer","sdp":"{big}"}}"#);
        let err = parse(&raw, cfg()).unwrap_err();
        assert!(matches!(
            err,
            P2pMessageError::FieldTooLarge { field: "sdp", .. }
        ));
    }

    #[test]
    fn rejects_oversize_candidate() {
        let big = "c".repeat(P2P_DEFAULT_MAX_CANDIDATE_BYTES + 1);
        let raw = format!(r#"{{"type":"candidate","candidate":"{big}"}}"#);
        let err = parse(&raw, cfg()).unwrap_err();
        assert!(matches!(
            err,
            P2pMessageError::FieldTooLarge {
                field: "candidate",
                ..
            }
        ));
    }

    #[test]
    fn rejects_invalid_direction() {
        let raw = r#"{
            "type": "check_in",
            "direction": "sideways",
            "stream": {"vhost":"v","app":"a","stream":"s"}
        }"#;
        let err = parse(raw, cfg()).unwrap_err();
        assert!(matches!(
            err,
            P2pMessageError::InvalidField {
                field: "direction",
                ..
            }
        ));
    }

    #[test]
    fn rejects_missing_stream() {
        let raw = r#"{"type":"check_in","direction":"pull"}"#;
        let err = parse(raw, cfg()).unwrap_err();
        assert!(matches!(err, P2pMessageError::MissingField("stream")));
    }

    #[test]
    fn render_refuses_unknown() {
        let err = render(&P2pMessage::Unknown { ty: "x".into() }).unwrap_err();
        assert!(matches!(err, P2pMessageError::InvalidField { .. }));
    }

    #[test]
    fn parses_candidate_with_sdpmid() {
        let raw = r#"{
            "type":"candidate",
            "candidate":"candidate:1 1 udp 2122260223 192.168.1.1 12345 typ host",
            "sdpMid":"0",
            "sdpMLineIndex":0
        }"#;
        let msg = parse(raw, cfg()).unwrap();
        match msg {
            P2pMessage::Candidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                ..
            } => {
                assert!(candidate.starts_with("candidate:1"));
                assert_eq!(sdp_mid.as_deref(), Some("0"));
                assert_eq!(sdp_mline_index, Some(0));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
