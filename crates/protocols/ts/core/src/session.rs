//! Sans-I/O session state machine for TS protocol.

use bytes::Bytes;

use crate::request::{
    parse_ts_request_target, validate_websocket_upgrade, HttpMethod, HttpRequestHead,
    HttpResponseHead, StreamKeyParts, TsTransport, WebSocketMessage,
};

/// Command for `TS Core`.
/// `TS Core` 的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsCoreCommand {
    SendTsBytes(Bytes),
    Close,
}

/// `TsCoreInput` enumeration.
/// `TsCoreInput` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsCoreInput {
    RequestHead(HttpRequestHead),
    WebSocketMessage(WebSocketMessage),
    Command(TsCoreCommand),
}

/// `CloseReason` enumeration.
/// `CloseReason` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    Normal,
    BadRequest,
    MethodNotAllowed,
    ProtocolError,
}

/// Events produced by the `TS Core` subsystem.
/// `TS Core` 子系统产生的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsCoreEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: TsTransport,
    },
    PeerClosed,
}

/// `TsCoreOutput` enumeration.
/// `TsCoreOutput` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsCoreOutput {
    SendHttpResponse(HttpResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    SendWebSocketPong(Bytes),
    Event(TsCoreEvent),
    Close { reason: CloseReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    HttpStreaming,
    WebSocketStreaming,
    Closed,
}

/// Sans-I/O TS protocol session state machine.
#[derive(Debug)]
pub struct TsCore {
    state: SessionState,
}

impl Default for TsCore {
    fn default() -> Self {
        Self::new()
    }
}

impl TsCore {
    /// Creates a new `TsCore` instance.
    /// 创建新的 `TsCore` 实例。
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
        }
    }

    /// Handles the `input` event.
    /// 处理 `input` 事件。
    pub fn handle_input(&mut self, input: TsCoreInput) -> Vec<TsCoreOutput> {
        match input {
            TsCoreInput::RequestHead(head) => self.handle_request_head(head),
            TsCoreInput::WebSocketMessage(msg) => self.handle_websocket_message(msg),
            TsCoreInput::Command(cmd) => self.handle_command(cmd),
        }
    }

    fn handle_request_head(&mut self, head: HttpRequestHead) -> Vec<TsCoreOutput> {
        if self.state != SessionState::Idle {
            return vec![TsCoreOutput::Close {
                reason: CloseReason::ProtocolError,
            }];
        }

        match head.method {
            HttpMethod::Options => {
                self.state = SessionState::Closed;
                vec![
                    TsCoreOutput::SendHttpResponse(HttpResponseHead {
                        status_code: 204,
                        reason: "No Content",
                        headers: vec![
                            ("Allow".to_string(), "GET, HEAD, OPTIONS".to_string()),
                            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
                            (
                                "Access-Control-Allow-Methods".to_string(),
                                "GET, HEAD, OPTIONS".to_string(),
                            ),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                    }),
                    TsCoreOutput::Close {
                        reason: CloseReason::Normal,
                    },
                ]
            }
            HttpMethod::Head => {
                let _parsed = match parse_ts_request_target(&head.target) {
                    Ok(p) => p,
                    Err(_) => return self.bad_request(),
                };
                self.state = SessionState::Closed;
                vec![
                    TsCoreOutput::SendHttpResponse(ts_response_head()),
                    TsCoreOutput::Close {
                        reason: CloseReason::Normal,
                    },
                ]
            }
            HttpMethod::Get => {
                let parsed = match parse_ts_request_target(&head.target) {
                    Ok(p) => p,
                    Err(_) => return self.bad_request(),
                };

                if head.is_websocket_upgrade() {
                    let accept = match validate_websocket_upgrade(&head) {
                        Ok(a) => a,
                        Err(_) => return self.bad_request(),
                    };
                    self.state = SessionState::WebSocketStreaming;
                    vec![
                        TsCoreOutput::SendHttpResponse(HttpResponseHead {
                            status_code: 101,
                            reason: "Switching Protocols",
                            headers: vec![
                                ("Upgrade".to_string(), "websocket".to_string()),
                                ("Connection".to_string(), "Upgrade".to_string()),
                                ("Sec-WebSocket-Accept".to_string(), accept),
                            ],
                        }),
                        TsCoreOutput::Event(TsCoreEvent::PlayRequested {
                            stream_key: parsed.stream_key,
                            transport: TsTransport::WebSocket,
                        }),
                    ]
                } else {
                    self.state = SessionState::HttpStreaming;
                    vec![
                        TsCoreOutput::SendHttpResponse(ts_response_head()),
                        TsCoreOutput::Event(TsCoreEvent::PlayRequested {
                            stream_key: parsed.stream_key,
                            transport: TsTransport::Http,
                        }),
                    ]
                }
            }
            HttpMethod::Other => {
                self.state = SessionState::Closed;
                vec![
                    TsCoreOutput::SendHttpResponse(HttpResponseHead {
                        status_code: 405,
                        reason: "Method Not Allowed",
                        headers: vec![
                            ("Allow".to_string(), "GET, HEAD, OPTIONS".to_string()),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                    }),
                    TsCoreOutput::Close {
                        reason: CloseReason::MethodNotAllowed,
                    },
                ]
            }
        }
    }

    fn handle_websocket_message(&mut self, msg: WebSocketMessage) -> Vec<TsCoreOutput> {
        match msg {
            WebSocketMessage::Close => {
                self.state = SessionState::Closed;
                vec![TsCoreOutput::Event(TsCoreEvent::PeerClosed)]
            }
            WebSocketMessage::Text(_) => {
                // Text messages not supported — close
                self.state = SessionState::Closed;
                vec![TsCoreOutput::Close {
                    reason: CloseReason::ProtocolError,
                }]
            }
            WebSocketMessage::Ping(data) => {
                // Respond with pong carrying same payload
                vec![TsCoreOutput::SendWebSocketPong(data)]
            }
            WebSocketMessage::Pong(_) => Vec::new(),
            WebSocketMessage::Binary(_) => Vec::new(),
            WebSocketMessage::Unmasked => {
                // RFC 6455: client frames MUST be masked
                self.state = SessionState::Closed;
                vec![TsCoreOutput::Close {
                    reason: CloseReason::ProtocolError,
                }]
            }
        }
    }

    fn handle_command(&mut self, cmd: TsCoreCommand) -> Vec<TsCoreOutput> {
        match cmd {
            TsCoreCommand::Close => {
                self.state = SessionState::Closed;
                vec![TsCoreOutput::Close {
                    reason: CloseReason::Normal,
                }]
            }
            TsCoreCommand::SendTsBytes(bytes) => match self.state {
                SessionState::HttpStreaming => vec![TsCoreOutput::SendBytes(bytes)],
                SessionState::WebSocketStreaming => {
                    vec![TsCoreOutput::SendWebSocketBinary(bytes)]
                }
                _ => Vec::new(),
            },
        }
    }

    fn bad_request(&mut self) -> Vec<TsCoreOutput> {
        self.state = SessionState::Closed;
        vec![
            TsCoreOutput::SendHttpResponse(HttpResponseHead {
                status_code: 400,
                reason: "Bad Request",
                headers: vec![("Content-Length".to_string(), "0".to_string())],
            }),
            TsCoreOutput::Close {
                reason: CloseReason::BadRequest,
            },
        ]
    }
}

fn ts_response_head() -> HttpResponseHead {
    HttpResponseHead {
        status_code: 200,
        reason: "OK",
        headers: vec![
            ("Content-Type".to_string(), "video/mp2t".to_string()),
            ("Connection".to_string(), "keep-alive".to_string()),
            ("Cache-Control".to_string(), "no-cache".to_string()),
            ("Access-Control-Allow-Origin".to_string(), "*".to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_get(target: &str, headers: &[(&str, &str)]) -> HttpRequestHead {
        HttpRequestHead {
            method: HttpMethod::Get,
            method_raw: "GET".to_string(),
            target: target.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn http_get_emits_play_event() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(make_get("/live/stream.ts", &[])));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 200
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::Event(TsCoreEvent::PlayRequested {
                transport: TsTransport::Http,
                ..
            })
        )));
    }

    #[test]
    fn options_returns_cors() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(HttpRequestHead {
            method: HttpMethod::Options,
            method_raw: "OPTIONS".to_string(),
            target: "/live/stream.ts".to_string(),
            headers: Vec::new(),
        }));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 204
        )));
    }

    #[test]
    fn websocket_upgrade() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(make_get(
            "/live/stream.ts",
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 101
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::Event(TsCoreEvent::PlayRequested {
                transport: TsTransport::WebSocket,
                ..
            })
        )));
    }

    #[test]
    fn unknown_method_returns_405() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(HttpRequestHead {
            method: HttpMethod::Other,
            method_raw: "DELETE".to_string(),
            target: "/live/stream.ts".to_string(),
            headers: Vec::new(),
        }));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 405
        )));
    }

    #[test]
    fn invalid_path_returns_400() {
        let mut core = TsCore::new();
        let outputs =
            core.handle_input(TsCoreInput::RequestHead(make_get("/live/stream.flv", &[])));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 400
        )));
    }

    #[test]
    fn send_ts_bytes_in_http_mode() {
        let mut core = TsCore::new();
        core.handle_input(TsCoreInput::RequestHead(make_get("/live/s.ts", &[])));
        let outputs = core.handle_input(TsCoreInput::Command(TsCoreCommand::SendTsBytes(
            bytes::Bytes::from_static(b"\x47test"),
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, TsCoreOutput::SendBytes(_))));
    }

    #[test]
    fn ws_text_closes_connection() {
        let mut core = TsCore::new();
        core.handle_input(TsCoreInput::RequestHead(make_get(
            "/live/s.ts",
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )));
        let outputs = core.handle_input(TsCoreInput::WebSocketMessage(WebSocketMessage::Text(
            "hello".to_string(),
        )));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, TsCoreOutput::Close { .. })));
    }

    #[test]
    fn live_ts_path_emits_play_event() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(make_get(
            "/live/stream.live.ts",
            &[],
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 200
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::Event(TsCoreEvent::PlayRequested {
                transport: TsTransport::Http,
                ..
            })
        )));
    }

    #[test]
    fn head_returns_headers_no_play_event() {
        let mut core = TsCore::new();
        let outputs = core.handle_input(TsCoreInput::RequestHead(HttpRequestHead {
            method: HttpMethod::Head,
            method_raw: "HEAD".to_string(),
            target: "/live/stream.ts".to_string(),
            headers: Vec::new(),
        }));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendHttpResponse(h) if h.status_code == 200
        )));
        // HEAD must NOT emit PlayRequested
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, TsCoreOutput::Event(TsCoreEvent::PlayRequested { .. }))));
        // HEAD must close the connection
        assert!(outputs
            .iter()
            .any(|o| matches!(o, TsCoreOutput::Close { .. })));
    }

    #[test]
    fn ws_ping_responds_with_pong() {
        let mut core = TsCore::new();
        core.handle_input(TsCoreInput::RequestHead(make_get(
            "/live/s.ts",
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )));
        let outputs = core.handle_input(TsCoreInput::WebSocketMessage(WebSocketMessage::Ping(
            bytes::Bytes::from_static(b"hello"),
        )));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::SendWebSocketPong(data) if data == &bytes::Bytes::from_static(b"hello")
        )));
    }

    #[test]
    fn ws_unmasked_client_closes() {
        let mut core = TsCore::new();
        core.handle_input(TsCoreInput::RequestHead(make_get(
            "/live/s.ts",
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )));
        let outputs = core.handle_input(TsCoreInput::WebSocketMessage(WebSocketMessage::Unmasked));
        assert!(outputs.iter().any(|o| matches!(
            o,
            TsCoreOutput::Close {
                reason: CloseReason::ProtocolError
            }
        )));
    }
}
