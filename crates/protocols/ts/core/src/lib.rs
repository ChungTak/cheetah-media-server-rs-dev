//! `cheetah-ts-core`: Sans-I/O state machine for HTTP/WS TS protocol.
//!
//! Handles request routing, WebSocket upgrade, CORS, and session state
//! without depending on any runtime, socket, or engine.

/// Module for `request`.
/// `request` 相关模块。
pub mod request;
/// Module for `rtp_ts`.
/// `rtp_ts` 相关模块。
pub mod rtp_ts;
/// Module for `session`.
/// `session` 相关模块。
pub mod session;

pub use request::{
    parse_ts_request_target, validate_websocket_upgrade, websocket_accept_key, HttpMethod,
    HttpRequestHead, HttpResponseHead, ParsedTsRequest, StreamKeyParts, TsTransport,
    WebSocketMessage,
};
pub use rtp_ts::{
    probe_payload, PayloadProbe, RtpTsDiagnostic, RtpTsIngest, RtpTsIngestConfig, RtpTsIngestEvent,
    RtpTsPublishSession,
};
pub use session::{CloseReason, TsCore, TsCoreCommand, TsCoreEvent, TsCoreInput, TsCoreOutput};
