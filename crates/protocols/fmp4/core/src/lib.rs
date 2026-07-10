//! `cheetah-fmp4-core`: Sans-I/O state machine for HTTP/WS fMP4 protocol.
//!
//! Handles request routing, WebSocket upgrade, CORS, and session state
//! without depending on any runtime, socket, or engine.

/// `request` module.
/// `request` 模块.
pub mod request;
/// `session` module.
/// `session` 模块.
pub mod session;

pub use request::{
    parse_fmp4_request_target, validate_websocket_upgrade, websocket_accept_key, Fmp4CoreError,
    Fmp4Transport, HttpMethod, HttpRequestHead, HttpResponseHead, ParsedFmp4Request,
    StreamKeyParts, WebSocketMessage,
};
pub use session::{
    CloseReason, Fmp4Core, Fmp4CoreCommand, Fmp4CoreEvent, Fmp4CoreInput, Fmp4CoreOutput,
};
