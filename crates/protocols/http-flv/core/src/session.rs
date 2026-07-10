use bytes::Bytes;
use cheetah_codec::RtmpFlvPlayMode;
use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvTag};

use crate::request::{
    parse_play_request_target, validate_websocket_upgrade, HttpFlvTransport, HttpMethod,
    HttpRequestHead, HttpResponseHead, StreamKeyParts, WebSocketMessage,
};
use crate::HttpFlvCoreError;

/// `HttpFlvCoreCommand` enumeration.
/// `HttpFlvCoreCommand` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFlvCoreCommand {
    /// `SendFlvBytes` variant.
    /// `SendFlvBytes` 变体.
    SendFlvBytes(Bytes),
    /// `Close` variant.
    /// `Close` 变体.
    Close,
}

/// `HttpFlvCoreInput` enumeration.
/// `HttpFlvCoreInput` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFlvCoreInput {
    /// `RequestHead` variant.
    /// `RequestHead` 变体.
    RequestHead(HttpRequestHead),
    /// `BodyBytes` variant.
    /// `BodyBytes` 变体.
    BodyBytes(Bytes),
    /// `WebSocketMessage` variant.
    /// `WebSocketMessage` 变体.
    WebSocketMessage(WebSocketMessage),
    /// `Command` variant.
    /// `Command` 变体.
    Command(HttpFlvCoreCommand),
}

/// `CloseReason` enumeration.
/// `CloseReason` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// `Normal` variant.
    /// `Normal` 变体.
    Normal,
    /// `BadRequest` variant.
    /// `BadRequest` 变体.
    BadRequest,
    /// `MethodNotAllowed` variant.
    /// `MethodNotAllowed` 变体.
    MethodNotAllowed,
    /// `ProtocolError` variant.
    /// `ProtocolError` 变体.
    ProtocolError,
}

/// `HttpFlvEvent` enumeration.
/// `HttpFlvEvent` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFlvEvent {
    /// `PlayRequested` variant.
    /// `PlayRequested` 变体.
    PlayRequested {
        stream_key: StreamKeyParts,
        transport: HttpFlvTransport,
        play_mode: RtmpFlvPlayMode,
    },
    /// HTTP POST push: client is publishing FLV data.
    PublishRequested { stream_key: StreamKeyParts },
    /// `PullTag` variant.
    /// `PullTag` 变体.
    PullTag(FlvTag),
    /// `PeerClosed` variant.
    /// `PeerClosed` 变体.
    PeerClosed,
}

/// `HttpFlvCoreOutput` enumeration.
/// `HttpFlvCoreOutput` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFlvCoreOutput {
    /// `SendHttpResponse` variant.
    /// `SendHttpResponse` 变体.
    SendHttpResponse(HttpResponseHead),
    /// `SendBytes` variant.
    /// `SendBytes` 变体.
    SendBytes(Bytes),
    /// `SendWebSocketBinary` variant.
    /// `SendWebSocketBinary` 变体.
    SendWebSocketBinary(Bytes),
    /// `Event` variant.
    /// `Event` 变体.
    Event(HttpFlvEvent),
    /// `Close` variant.
    /// `Close` 变体.
    Close { reason: CloseReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    HttpStreaming,
    HttpReceiving,
    WebSocketStreaming,
    Closed,
}

/// `HttpFlvCore` data structure.
/// `HttpFlvCore` 数据结构.
#[derive(Debug)]
pub struct HttpFlvCore {
    /// `state` field of type `SessionState`.
    /// `state` 字段，类型为 `SessionState`.
    state: SessionState,
    /// `demuxer` field of type `FlvDemuxer`.
    /// `demuxer` 字段，类型为 `FlvDemuxer`.
    demuxer: FlvDemuxer,
}

impl Default for HttpFlvCore {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFlvCore {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
            demuxer: FlvDemuxer::default(),
        }
    }

    /// `handle_input` function.
    /// `handle_input` 函数.
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
