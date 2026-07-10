//! `cheetah-fmp4-driver-tokio`: TCP/TLS server, HTTP/WS framing, and pull client for fMP4.
//!
//! `cheetah-fmp4-driver-tokio`：fMP4 的 TCP/TLS 服务器、HTTP/WS 分帧与拉取客户端。

/// Pull client for HTTP(S)/WS(S) fMP4 sources.
///
/// HTTP(S)/WS(S) fMP4 源的拉取客户端。
pub mod pull;
/// TCP/TLS server and HTTP/WebSocket framing.
///
/// TCP/TLS 服务器与 HTTP/WebSocket 分帧。
pub mod server;
/// TLS configuration helper.
///
/// TLS 配置辅助。
pub mod tls;

pub use pull::{connect_pull, Fmp4PullClientConfig, Fmp4PullEvent};
pub use server::{
    start_server, Fmp4CommandSender, Fmp4ConnectionId, Fmp4DriverCommand, Fmp4DriverConfig,
    Fmp4DriverEvent, Fmp4DriverHandle, Fmp4TlsConfig,
};
