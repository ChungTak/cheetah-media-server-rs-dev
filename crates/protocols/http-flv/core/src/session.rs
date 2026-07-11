use bytes::Bytes;
use cheetah_codec::RtmpFlvPlayMode;
use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvTag};

use crate::request::{
    parse_play_request_target, validate_websocket_upgrade, HttpFlvTransport, HttpMethod,
    HttpRequestHead, HttpResponseHead, StreamKeyParts, WebSocketMessage,
};
use crate::HttpFlvCoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Commands from module/driver to the HTTP-FLV core.
///
/// 模块/驱动到 HTTP-FLV core 的命令。
pub enum HttpFlvCoreCommand {
    SendFlvBytes(Bytes),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Inputs delivered to the HTTP-FLV core state machine.
///
/// 递交给 HTTP-FLV core 状态机的输入。
pub enum HttpFlvCoreInput {
    RequestHead(HttpRequestHead),
    BodyBytes(Bytes),
    WebSocketMessage(WebSocketMessage),
    Command(HttpFlvCoreCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Reason for closing a connection.
///
/// 关闭连接的原因。
pub enum CloseReason {
    Normal,
    BadRequest,
    MethodNotAllowed,
    ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Events emitted by the core for the module layer.
///
/// core 向模块层发出的事件。
pub enum HttpFlvEvent {
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: HttpFlvTransport,
        play_mode: RtmpFlvPlayMode,
    },
    /// HTTP POST push: client is publishing FLV data.
    PublishRequested {
        stream_key: StreamKeyParts,
    },
    PullTag(FlvTag),
    PeerClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Output actions from the core state machine.
///
/// core 状态机的输出动作。
pub enum HttpFlvCoreOutput {
    SendHttpResponse(HttpResponseHead),
    SendBytes(Bytes),
    SendWebSocketBinary(Bytes),
    Event(HttpFlvEvent),
    Close { reason: CloseReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Internal HTTP-FLV session state.
///
/// 内部 HTTP-FLV 会话状态。
enum SessionState {
    Idle,
    HttpStreaming,
    HttpReceiving,
    WebSocketStreaming,
    Closed,
}

#[derive(Debug)]
/// Sans-I/O HTTP-FLV core state machine.
///
/// Sans-I/O HTTP-FLV core 状态机。
pub struct HttpFlvCore {
    state: SessionState,
    demuxer: FlvDemuxer,
}

/// `Default` delegates to `new()`.
///
/// `Default` 委托给 `new()`。
impl Default for HttpFlvCore {
    fn default() -> Self {
        Self::new()
    }
}

/// `HttpFlvCore` API: process inputs and handle request/WS/command paths.
///
/// `HttpFlvCore` API：处理输入并处理请求/WS/命令路径。
impl HttpFlvCore {
    /// Create a new HTTP-FLV core state machine.
    ///
    /// 创建新的 HTTP-FLV core 状态机。
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
            demuxer: FlvDemuxer::default(),
        }
    }

    /// Process one input and produce the corresponding outputs.
    ///
    /// 处理一个输入并产生对应的输出。
    pub fn handle_input(
        &mut self,
        input: HttpFlvCoreInput,
    ) -> Result<Vec<HttpFlvCoreOutput>, HttpFlvCoreError> {
        match input {
            HttpFlvCoreInput::RequestHead(head) => self.handle_request_head(head),
            HttpFlvCoreInput::BodyBytes(bytes) => self.handle_body_bytes(&bytes),
            HttpFlvCoreInput::WebSocketMessage(message) => self.handle_websocket_message(message),
            HttpFlvCoreInput::Command(command) => self.handle_command(command),
        }
    }

    /// Handle an HTTP request head, routing to play, publish, or WebSocket upgrade.
    ///
    /// 处理 HTTP 请求头，路由到播放、发布或 WebSocket 升级。
    fn handle_request_head(
        &mut self,
        head: HttpRequestHead,
    ) -> Result<Vec<HttpFlvCoreOutput>, HttpFlvCoreError> {
        if self.state != SessionState::Idle {
            return Ok(vec![HttpFlvCoreOutput::Close {
                reason: CloseReason::ProtocolError,
            }]);
        }

        match head.method {
            HttpMethod::Options => {
                self.state = SessionState::Closed;
                Ok(vec![
                    HttpFlvCoreOutput::SendHttpResponse(HttpResponseHead {
                        status_code: 204,
                        reason: "No Content",
                        headers: vec![
                            ("Allow".to_string(), "GET, OPTIONS".to_string()),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                    }),
                    HttpFlvCoreOutput::Close {
                        reason: CloseReason::Normal,
                    },
                ])
            }
            HttpMethod::Get => {
                let parsed = parse_play_request_target(&head.target)?;
                if head.is_websocket_upgrade() {
                    let accept = validate_websocket_upgrade(&head)?;
                    self.state = SessionState::WebSocketStreaming;
                    Ok(vec![
                        HttpFlvCoreOutput::SendHttpResponse(HttpResponseHead {
                            status_code: 101,
                            reason: "Switching Protocols",
                            headers: vec![
                                ("Upgrade".to_string(), "websocket".to_string()),
                                ("Connection".to_string(), "Upgrade".to_string()),
                                ("Sec-WebSocket-Accept".to_string(), accept),
                            ],
                        }),
                        HttpFlvCoreOutput::Event(HttpFlvEvent::PlayRequested {
                            stream_key: parsed.stream_key,
                            transport: HttpFlvTransport::WebSocket,
                            play_mode: parsed.mode.to_rtmp_play_mode(),
                        }),
                    ])
                } else {
                    self.state = SessionState::HttpStreaming;
                    Ok(vec![
                        HttpFlvCoreOutput::SendHttpResponse(HttpResponseHead {
                            status_code: 200,
                            reason: "OK",
                            headers: vec![
                                ("Content-Type".to_string(), "video/x-flv".to_string()),
                                ("Connection".to_string(), "keep-alive".to_string()),
                                ("Cache-Control".to_string(), "no-cache".to_string()),
                            ],
                        }),
                        HttpFlvCoreOutput::Event(HttpFlvEvent::PlayRequested {
                            stream_key: parsed.stream_key,
                            transport: HttpFlvTransport::Http,
                            play_mode: parsed.mode.to_rtmp_play_mode(),
                        }),
                    ])
                }
            }
            HttpMethod::Post => {
                let parsed = parse_play_request_target(&head.target)?;
                self.state = SessionState::HttpReceiving;
                Ok(vec![
                    HttpFlvCoreOutput::SendHttpResponse(HttpResponseHead {
                        status_code: 200,
                        reason: "OK",
                        headers: vec![
                            ("Connection".to_string(), "keep-alive".to_string()),
                            ("Content-Length".to_string(), "0".to_string()),
                        ],
                    }),
                    HttpFlvCoreOutput::Event(HttpFlvEvent::PublishRequested {
                        stream_key: parsed.stream_key,
                    }),
                ])
            }
            HttpMethod::Other => Ok(vec![
                HttpFlvCoreOutput::SendHttpResponse(HttpResponseHead {
                    status_code: 405,
                    reason: "Method Not Allowed",
                    headers: vec![
                        ("Allow".to_string(), "GET, OPTIONS".to_string()),
                        ("Content-Length".to_string(), "0".to_string()),
                    ],
                }),
                HttpFlvCoreOutput::Close {
                    reason: CloseReason::MethodNotAllowed,
                },
            ]),
        }
    }

    /// Demux FLV body bytes and emit `PullTag` events for the module layer.
    ///
    /// 解封装 FLV 体字节，并向模块层发出 `PullTag` 事件。
    fn handle_body_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<HttpFlvCoreOutput>, HttpFlvCoreError> {
        let events = self
            .demuxer
            .push(bytes)
            .map_err(|err| HttpFlvCoreError::FlvDemux(err.to_string()))?;
        let mut outputs = Vec::with_capacity(events.len());
        for event in events {
            match event {
                FlvDemuxEvent::Tag(tag) => {
                    outputs.push(HttpFlvCoreOutput::Event(HttpFlvEvent::PullTag(tag)))
                }
                FlvDemuxEvent::Header(_) | FlvDemuxEvent::PreviousTagSizeMismatch(_) => {}
            }
        }
        Ok(outputs)
    }

    /// Handle a WebSocket message after the upgrade is complete.
    ///
    /// 升级完成后处理 WebSocket 消息。
    fn handle_websocket_message(
        &mut self,
        message: WebSocketMessage,
    ) -> Result<Vec<HttpFlvCoreOutput>, HttpFlvCoreError> {
        match message {
            WebSocketMessage::Binary(payload) => self.handle_body_bytes(&payload),
            WebSocketMessage::Close => {
                self.state = SessionState::Closed;
                Ok(vec![HttpFlvCoreOutput::Event(HttpFlvEvent::PeerClosed)])
            }
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) | WebSocketMessage::Text(_) => {
                Ok(Vec::new())
            }
        }
    }

    /// Handle a driver command, sending FLV bytes over the active transport.
    ///
    /// 处理驱动命令，通过活动传输发送 FLV 字节。
    fn handle_command(
        &mut self,
        command: HttpFlvCoreCommand,
    ) -> Result<Vec<HttpFlvCoreOutput>, HttpFlvCoreError> {
        match command {
            HttpFlvCoreCommand::Close => {
                self.state = SessionState::Closed;
                Ok(vec![HttpFlvCoreOutput::Close {
                    reason: CloseReason::Normal,
                }])
            }
            HttpFlvCoreCommand::SendFlvBytes(bytes) => match self.state {
                SessionState::HttpStreaming => Ok(vec![HttpFlvCoreOutput::SendBytes(bytes)]),
                SessionState::WebSocketStreaming => {
                    Ok(vec![HttpFlvCoreOutput::SendWebSocketBinary(bytes)])
                }
                SessionState::Idle | SessionState::Closed | SessionState::HttpReceiving => {
                    Err(HttpFlvCoreError::NotHttpTransport)
                }
            },
        }
    }
}
