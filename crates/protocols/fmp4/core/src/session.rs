//! Sans-I/O fMP4 session state machine.

use bytes::Bytes;

use crate::request::{
    parse_fmp4_request_target, validate_websocket_upgrade, Fmp4Transport, HttpMethod,
    HttpRequestHead, HttpResponseHead, StreamKeyParts, WebSocketMessage,
};

/// Input events to the fMP4 core state machine.
#[derive(Debug, Clone)]
pub enum Fmp4CoreInput {
    /// `RequestHead` variant.
    /// `RequestHead` 变体.
    RequestHead(HttpRequestHead),
    /// `WebSocketMessage` variant.
    /// `WebSocketMessage` 变体.
    WebSocketMessage(WebSocketMessage),
    /// `Command` variant.
    /// `Command` 变体.
    Command(Fmp4CoreCommand),
}

/// Commands from module/driver to the core.
#[derive(Debug, Clone)]
pub enum Fmp4CoreCommand {
    /// `SendFmp4Bytes` variant.
    /// `SendFmp4Bytes` 变体.
    SendFmp4Bytes(Bytes),
    /// `Close` variant.
    /// `Close` 变体.
    Close,
}

/// Output actions from the core state machine.
#[derive(Debug, Clone)]
pub enum Fmp4CoreOutput {
    /// `SendHttpResponse` variant.
    /// `SendHttpResponse` 变体.
    SendHttpResponse(HttpResponseHead),
    /// `SendBytes` variant.
    /// `SendBytes` 变体.
    SendBytes(Bytes),
    /// `SendWebSocketBinary` variant.
    /// `SendWebSocketBinary` 变体.
    SendWebSocketBinary(Bytes),
    /// `SendWebSocketPong` variant.
    /// `SendWebSocketPong` 变体.
    SendWebSocketPong(Bytes),
    /// `Event` variant.
    /// `Event` 变体.
    Event(Fmp4CoreEvent),
    /// `Close` variant.
    /// `Close` 变体.
    Close { reason: CloseReason },
}

/// Events emitted by the core for the module layer.
#[derive(Debug, Clone)]
pub enum Fmp4CoreEvent {
    /// `PlayRequested` variant.
    /// `PlayRequested` 变体.
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: Fmp4Transport,
    },
    /// `PeerClosed` variant.
    /// `PeerClosed` 变体.
    PeerClosed,
}

/// Reason for closing a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// `Normal` variant.
    /// `Normal` 变体.
    Normal,
    /// `InvalidRequest` variant.
    /// `InvalidRequest` 变体.
    InvalidRequest,
    /// `MethodNotAllowed` variant.
    /// `MethodNotAllowed` 变体.
    MethodNotAllowed,
    /// `WebSocketTextMessage` variant.
    /// `WebSocketTextMessage` 变体.
    WebSocketTextMessage,
    /// `CommandClose` variant.
    /// `CommandClose` 变体.
    CommandClose,
}

/// Sans-I/O fMP4 core state machine.
pub struct Fmp4Core {
    /// `transport` field.
    /// `transport` 字段.
    transport: Option<Fmp4Transport>,
}

impl Fmp4Core {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self { transport: None }
    }

    /// `process` function.
    /// `process` 函数.
    pub fn process(&mut self, input: Fmp4CoreInput) -> Vec<Fmp4CoreOutput> {
        match input {
            Fmp4CoreInput::RequestHead(head) => self.handle_request(head),
            Fmp4CoreInput::WebSocketMessage(msg) => self.handle_ws_message(msg),
            Fmp4CoreInput::Command(cmd) => self.handle_command(cmd),
        }
    }

    fn handle_request(&mut self, head: HttpRequestHead) -> Vec<Fmp4CoreOutput> {
        match head.method {
            HttpMethod::Options => {
                return vec![
                    Fmp4CoreOutput::SendHttpResponse(cors_preflight_response()),
                    Fmp4CoreOutput::Close {
                        reason: CloseReason::Normal,
                    },
                ];
            }
            HttpMethod::Head => {
                let parsed = match parse_fmp4_request_target(&head.target) {
                    Ok(p) => p,
                    Err(_) => {
                        return vec![
                            Fmp4CoreOutput::SendHttpResponse(error_response(400, "Bad Request")),
                            Fmp4CoreOutput::Close {
                                reason: CloseReason::InvalidRequest,
                            },
                        ];
                    }
                };
                let _ = parsed;
                return vec![
                    Fmp4CoreOutput::SendHttpResponse(head_response()),
                    Fmp4CoreOutput::Close {
                        reason: CloseReason::Normal,
                    },
                ];
            }
            HttpMethod::Get => {}
            HttpMethod::Other => {
                return vec![
                    Fmp4CoreOutput::SendHttpResponse(error_response(405, "Method Not Allowed")),
                    Fmp4CoreOutput::Close {
                        reason: CloseReason::MethodNotAllowed,
                    },
                ];
            }
        }

        let parsed = match parse_fmp4_request_target(&head.target) {
            Ok(p) => p,
            Err(_) => {
                return vec![
                    Fmp4CoreOutput::SendHttpResponse(error_response(400, "Bad Request")),
                    Fmp4CoreOutput::Close {
                        reason: CloseReason::InvalidRequest,
                    },
                ];
            }
        };

        // WebSocket upgrade?
        if head.is_websocket_upgrade() {
            match validate_websocket_upgrade(&head) {
                Ok(accept_key) => {
                    self.transport = Some(Fmp4Transport::WebSocket);
                    return vec![
                        Fmp4CoreOutput::SendHttpResponse(websocket_upgrade_response(&accept_key)),
                        Fmp4CoreOutput::Event(Fmp4CoreEvent::PlayRequested {
                            stream_key: parsed.stream_key,
                            transport: Fmp4Transport::WebSocket,
                        }),
                    ];
                }
                Err(_) => {
                    return vec![
                        Fmp4CoreOutput::SendHttpResponse(error_response(400, "Bad Request")),
                        Fmp4CoreOutput::Close {
                            reason: CloseReason::InvalidRequest,
                        },
                    ];
                }
            }
        }

        // HTTP chunked streaming
        self.transport = Some(Fmp4Transport::Http);
        vec![
            Fmp4CoreOutput::SendHttpResponse(fmp4_play_response()),
            Fmp4CoreOutput::Event(Fmp4CoreEvent::PlayRequested {
                stream_key: parsed.stream_key,
                transport: Fmp4Transport::Http,
            }),
        ]
    }

    fn handle_ws_message(&mut self, msg: WebSocketMessage) -> Vec<Fmp4CoreOutput> {
        match msg {
            WebSocketMessage::Ping(data) => vec![Fmp4CoreOutput::SendWebSocketPong(data)],
            WebSocketMessage::Close => vec![
                Fmp4CoreOutput::Event(Fmp4CoreEvent::PeerClosed),
                Fmp4CoreOutput::Close {
                    reason: CloseReason::Normal,
                },
            ],
            WebSocketMessage::Text(_) => vec![Fmp4CoreOutput::Close {
                reason: CloseReason::WebSocketTextMessage,
            }],
            _ => Vec::new(),
        }
    }

    fn handle_command(&mut self, cmd: Fmp4CoreCommand) -> Vec<Fmp4CoreOutput> {
        match cmd {
            Fmp4CoreCommand::SendFmp4Bytes(data) => match self.transport {
                Some(Fmp4Transport::WebSocket) => vec![Fmp4CoreOutput::SendWebSocketBinary(data)],
                Some(Fmp4Transport::Http) => vec![Fmp4CoreOutput::SendBytes(data)],
                None => Vec::new(),
            },
            Fmp4CoreCommand::Close => vec![Fmp4CoreOutput::Close {
                reason: CloseReason::CommandClose,
            }],
        }
    }
}

impl Default for Fmp4Core {
    fn default() -> Self {
        Self::new()
    }
}

fn cors_preflight_response() -> HttpResponseHead {
    HttpResponseHead {
        status_code: 204,
        reason: "No Content",
        headers: vec![
            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
            (
                "Access-Control-Allow-Methods".to_string(),
                "GET, HEAD, OPTIONS".to_string(),
            ),
            ("Access-Control-Allow-Headers".to_string(), "*".to_string()),
            ("Access-Control-Max-Age".to_string(), "86400".to_string()),
        ],
    }
}

fn head_response() -> HttpResponseHead {
    HttpResponseHead {
        status_code: 200,
        reason: "OK",
        headers: vec![
            ("Content-Type".to_string(), "video/mp4".to_string()),
            ("Cache-Control".to_string(), "no-cache".to_string()),
            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
        ],
    }
}

fn fmp4_play_response() -> HttpResponseHead {
    HttpResponseHead {
        status_code: 200,
        reason: "OK",
        headers: vec![
            ("Content-Type".to_string(), "video/mp4".to_string()),
            ("Connection".to_string(), "keep-alive".to_string()),
            ("Cache-Control".to_string(), "no-cache".to_string()),
            ("Transfer-Encoding".to_string(), "chunked".to_string()),
            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
        ],
    }
}

fn websocket_upgrade_response(accept_key: &str) -> HttpResponseHead {
    HttpResponseHead {
        status_code: 101,
        reason: "Switching Protocols",
        headers: vec![
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Connection".to_string(), "Upgrade".to_string()),
            ("Sec-WebSocket-Accept".to_string(), accept_key.to_string()),
        ],
    }
}

fn error_response(status: u16, reason: &'static str) -> HttpResponseHead {
    HttpResponseHead {
        status_code: status,
        reason,
        headers: vec![
            ("Content-Length".to_string(), "0".to_string()),
            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_request(target: &str) -> HttpRequestHead {
        HttpRequestHead {
            method: HttpMethod::Get,
            method_raw: "GET".to_string(),
            target: target.to_string(),
            headers: Vec::new(),
        }
    }

    fn ws_upgrade_request(target: &str) -> HttpRequestHead {
        HttpRequestHead {
            method: HttpMethod::Get,
            method_raw: "GET".to_string(),
            target: target.to_string(),
            headers: vec![
                ("Connection".to_string(), "Upgrade".to_string()),
                ("Upgrade".to_string(), "websocket".to_string()),
                ("Sec-WebSocket-Version".to_string(), "13".to_string()),
                (
                    "Sec-WebSocket-Key".to_string(),
                    "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
                ),
            ],
        }
    }

    #[test]
    fn http_get_emits_play_event() {
        let mut core = Fmp4Core::new();
        let outputs = core.process(Fmp4CoreInput::RequestHead(get_request("/live/test.mp4")));
        assert!(outputs.iter().any(|o| matches!(
            o,
            Fmp4CoreOutput::Event(Fmp4CoreEvent::PlayRequested {
                transport: Fmp4Transport::Http,
                ..
            })
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 200)));
    }

    #[test]
    fn websocket_upgrade_emits_play_event() {
        let mut core = Fmp4Core::new();
        let outputs = core.process(Fmp4CoreInput::RequestHead(ws_upgrade_request(
            "/live/test.mp4",
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            Fmp4CoreOutput::Event(Fmp4CoreEvent::PlayRequested {
                transport: Fmp4Transport::WebSocket,
                ..
            })
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 101)));
    }

    #[test]
    fn options_returns_cors() {
        let mut core = Fmp4Core::new();
        let head = HttpRequestHead {
            method: HttpMethod::Options,
            method_raw: "OPTIONS".to_string(),
            target: "/live/test.mp4".to_string(),
            headers: Vec::new(),
        };
        let outputs = core.process(Fmp4CoreInput::RequestHead(head));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 204)));
    }

    #[test]
    fn options_closes_after_preflight_response() {
        let mut core = Fmp4Core::new();
        let head = HttpRequestHead {
            method: HttpMethod::Options,
            method_raw: "OPTIONS".to_string(),
            target: "/live/test.mp4".to_string(),
            headers: Vec::new(),
        };
        let outputs = core.process(Fmp4CoreInput::RequestHead(head));
        assert!(outputs.iter().any(|o| matches!(
            o,
            Fmp4CoreOutput::Close {
                reason: CloseReason::Normal
            }
        )));
    }

    #[test]
    fn head_closes_after_header_response() {
        let mut core = Fmp4Core::new();
        let head = HttpRequestHead {
            method: HttpMethod::Head,
            method_raw: "HEAD".to_string(),
            target: "/live/test.mp4".to_string(),
            headers: Vec::new(),
        };
        let outputs = core.process(Fmp4CoreInput::RequestHead(head));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 200)));
        assert!(outputs.iter().any(|o| matches!(
            o,
            Fmp4CoreOutput::Close {
                reason: CloseReason::Normal
            }
        )));
    }

    #[test]
    fn invalid_path_returns_400() {
        let mut core = Fmp4Core::new();
        let outputs = core.process(Fmp4CoreInput::RequestHead(get_request("/live/test.flv")));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 400)));
    }

    #[test]
    fn post_returns_405() {
        let mut core = Fmp4Core::new();
        let head = HttpRequestHead {
            method: HttpMethod::Other,
            method_raw: "POST".to_string(),
            target: "/live/test.mp4".to_string(),
            headers: Vec::new(),
        };
        let outputs = core.process(Fmp4CoreInput::RequestHead(head));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendHttpResponse(r) if r.status_code == 405)));
    }

    #[test]
    fn command_send_bytes_http() {
        let mut core = Fmp4Core::new();
        core.process(Fmp4CoreInput::RequestHead(get_request("/live/test.mp4")));
        let outputs = core.process(Fmp4CoreInput::Command(Fmp4CoreCommand::SendFmp4Bytes(
            Bytes::from_static(b"data"),
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendBytes(_))));
    }

    #[test]
    fn command_send_bytes_ws() {
        let mut core = Fmp4Core::new();
        core.process(Fmp4CoreInput::RequestHead(ws_upgrade_request(
            "/live/test.mp4",
        )));
        let outputs = core.process(Fmp4CoreInput::Command(Fmp4CoreCommand::SendFmp4Bytes(
            Bytes::from_static(b"data"),
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendWebSocketBinary(_))));
    }

    #[test]
    fn ws_text_closes() {
        let mut core = Fmp4Core::new();
        core.process(Fmp4CoreInput::RequestHead(ws_upgrade_request(
            "/live/test.mp4",
        )));
        let outputs = core.process(Fmp4CoreInput::WebSocketMessage(WebSocketMessage::Text(
            "hi".to_string(),
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            Fmp4CoreOutput::Close {
                reason: CloseReason::WebSocketTextMessage
            }
        )));
    }

    #[test]
    fn ws_ping_pong() {
        let mut core = Fmp4Core::new();
        core.process(Fmp4CoreInput::RequestHead(ws_upgrade_request(
            "/live/test.mp4",
        )));
        let outputs = core.process(Fmp4CoreInput::WebSocketMessage(WebSocketMessage::Ping(
            Bytes::from_static(b"ping"),
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, Fmp4CoreOutput::SendWebSocketPong(_))));
    }
}
