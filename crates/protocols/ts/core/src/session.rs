//! Sans-I/O session state machine for the TS protocol.
//!
//! TS 协议的 Sans-I/O 会话状态机。

use bytes::Bytes;

use crate::request::{
    parse_ts_request_target, validate_websocket_upgrade, HttpMethod, HttpRequestHead,
    HttpResponseHead, StreamKeyParts, TsTransport, WebSocketMessage,
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Command sent into the TS core from the driver.
///
/// 驱动层发送给 TS core 的命令。
pub enum TsCoreCommand {
    SendTsBytes(Bytes),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Input events fed into the TS core state machine.
///
/// 输入到 TS core 状态机的事件。
pub enum TsCoreInput {
    RequestHead(HttpRequestHead),
    WebSocketMessage(WebSocketMessage),
    Command(TsCoreCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Reason for closing the TS session.
///
/// 关闭 TS 会话的原因。
pub enum CloseReason {
    Normal,
    BadRequest,
    MethodNotAllowed,
    ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Outbound events from the TS core to the module.
///
/// TS core 向模块发送的出站事件。
pub enum TsCoreEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: TsTransport,
    },
    PeerClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Output actions produced by the TS core after processing input.
///
/// TS core 处理输入后产生的输出动作。
pub enum TsCoreOutput {
    SendHttpResponse(HttpResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    SendWebSocketPong(Bytes),
    Event(TsCoreEvent),
    Close { reason: CloseReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Internal session state for HTTP/WS TS playback.
///
/// HTTP/WS TS 播放的内部会话状态。
enum SessionState {
    Idle,
    HttpStreaming,
    WebSocketStreaming,
    Closed,
}

/// Sans-I/O TS protocol session state machine.
///
/// TS 协议的 Sans-I/O 会话状态机。
#[derive(Debug)]
pub struct TsCore {
    state: SessionState,
}

/// Default `TsCore` starts in the idle state.
///
/// 默认 `TsCore` 从空闲状态开始。
impl Default for TsCore {
    fn default() -> Self {
        Self::new()
    }
}

/// `TsCore` input handling and state transition API.
///
/// `TsCore` 输入处理与状态转换 API。
impl TsCore {
    /// Create a new idle TS core session.
    ///
    /// 创建一个新的空闲 TS core 会话。
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
        }
    }

    /// Dispatch an input to the current state handler and return outputs.
    ///
    /// 将输入分派到当前状态处理器并返回输出。
    pub fn handle_input(&mut self, input: TsCoreInput) -> Vec<TsCoreOutput> {
        match input {
            TsCoreInput::RequestHead(head) => self.handle_request_head(head),
            TsCoreInput::WebSocketMessage(msg) => self.handle_websocket_message(msg),
            TsCoreInput::Command(cmd) => self.handle_command(cmd),
        }
    }

    /// Process the HTTP request head and transition to streaming or closed.
    ///
    /// 处理 HTTP 请求头并切换到流式或关闭状态。
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

    /// Handle a WebSocket message while in WS streaming state.
    ///
    /// 在 WS 流式状态下处理 WebSocket 消息。
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

    /// Process a driver command and return the appropriate output action.
    ///
    /// 处理驱动命令并返回相应的输出动作。
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

    /// Return a 400 Bad Request response and close the session.
    ///
    /// 返回 400 Bad Request 响应并关闭会话。
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

/// Build the standard HTTP 200 response head for TS streams.
///
/// 构建 TS 流的标准 HTTP 200 响应头。
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
