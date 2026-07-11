//! `cheetah-fmp4-core`: Sans-I/O state machine for the HTTP/WebSocket fMP4 protocol.
//!
//! `cheetah-fmp4-core` 是 HTTP/WebSocket fMP4 协议的 Sans-I/O 状态机。
//! 负责请求路由、WebSocket 升级、CORS 和会话状态，不依赖任何运行时、套接字或引擎。

/// HTTP request parsing and WebSocket upgrade.
///
/// HTTP 请求解析与 WebSocket 升级。
pub mod request;
/// Sans-I/O fMP4 session state machine.
///
/// Sans-I/O fMP4 会话状态机。
pub mod session;

pub use request::{
    parse_fmp4_request_target, validate_websocket_upgrade, websocket_accept_key, Fmp4CoreError,
    Fmp4Transport, HttpMethod, HttpRequestHead, HttpResponseHead, ParsedFmp4Request,
    StreamKeyParts, WebSocketMessage,
};
pub use session::{
    CloseReason, Fmp4Core, Fmp4CoreCommand, Fmp4CoreEvent, Fmp4CoreInput, Fmp4CoreOutput,
};
