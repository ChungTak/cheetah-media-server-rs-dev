//! Tokio-based HTTP-FLV/WS-FLV server driver.
//!
//! 基于 Tokio 的 HTTP-FLV/WS-FLV 服务器驱动。

/// TCP server, connection loop, and request parsing.
///
/// TCP 服务器、连接循环与请求解析。
pub mod server;
/// TLS helper for HTTPS-FLV/WSS-FLV.
///
/// HTTPS-FLV/WSS-FLV 的 TLS 辅助。
pub mod tls;

pub use cheetah_http_flv_core::{
    CloseReason, HttpFlvCoreCommand, HttpFlvCoreInput, HttpFlvCoreOutput, HttpFlvEvent, HttpMethod,
};

pub use server::{
    start_server, DriverSendError, HttpFlvConnectionId, HttpFlvCoreCommandSender,
    HttpFlvDriverCommand, HttpFlvDriverConfig, HttpFlvDriverEvent, HttpFlvServerHandle,
};

pub use tls::{start_tls_server, HttpFlvTlsDriverConfig};
