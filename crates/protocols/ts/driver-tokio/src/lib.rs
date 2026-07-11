//! `cheetah-ts-driver-tokio`: HTTP/WS server and pull client for TS protocol.
//!
//! `cheetah-ts-driver-tokio`：TS 协议的 HTTP/WS 服务器与拉流客户端。

/// HTTP(S)/WS(S) TS pull client.
///
/// HTTP(S)/WS(S) TS 拉流客户端。
pub mod pull;
/// HTTP/WS TS server.
///
/// HTTP/WS TS 服务器。
pub mod server;
/// TLS support for HTTPS/WSS.
///
/// HTTPS/WSS 的 TLS 支持。
pub mod tls;

pub use pull::{TsPullClient, TsPullClientConfig, TsPullEvent};
pub use server::{
    start_server, TsCommandSender, TsConnectionId, TsDriverCommand, TsDriverConfig, TsDriverEvent,
    TsServerHandle, TsTlsConfig,
};
