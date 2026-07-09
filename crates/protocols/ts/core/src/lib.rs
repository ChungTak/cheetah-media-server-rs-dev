//! `cheetah-ts-core`: Sans-I/O state machine for HTTP/WS TS protocol.
//!
//! Handles request routing, WebSocket upgrade, CORS, and session state
//! without depending on any runtime, socket, or engine.

pub mod request;
pub mod rtp_ts;
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
