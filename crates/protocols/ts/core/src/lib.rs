//! `cheetah-ts-core`: Sans-I/O state machine for the HTTP/WS TS protocol.
//!
//! `cheetah-ts-core` 是 HTTP/WS TS 协议的 Sans-I/O 状态机。
//! 负责请求路由、WebSocket 升级、CORS 和会话状态，不依赖任何运行时、套接字或引擎。

/// HTTP/WS request parsing and WebSocket upgrade.
///
/// HTTP/WS 请求解析与 WebSocket 升级。
pub mod request;
/// RTP-over-TS ingestion state machine.
///
/// RTP-over-TS 摄入状态机。
pub mod rtp_ts;
/// TS session state machine.
///
/// TS 会话状态机。
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
