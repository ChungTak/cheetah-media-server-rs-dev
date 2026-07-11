/// RTSP client and server drivers for Tokio.
///
/// This crate bridges the Sans-I/O `cheetah-rtsp-core` protocol state machine
/// with actual network I/O. The `client` module exposes TCP, TLS, and HTTP
/// tunnel transports, plus UDP/RTP endpoint management. The `server` module
/// accepts RTSP, RTSPS, and HTTP tunnelled connections and routes core events
/// back to the engine.
///
/// 用于 Tokio 的 RTSP 客户端与服务器驱动。
///
/// 此 crate 将 Sans-I/O 的 `cheetah-rtsp-core` 协议状态机与实际网络 I/O 桥接。
/// `client` 模块提供 TCP、TLS 以及 HTTP 隧道传输，外加 UDP/RTP 端点管理。
/// `server` 模块接受 RTSP、RTSPS 以及 HTTP 隧道连接，并将核心事件路由回引擎。
pub mod client;
pub mod server;

pub use cheetah_rtsp_core::{RtspCommand, RtspEvent, RtspHeader, RtspMethod, RtspRequest};

pub use client::{
    allocate_udp_endpoint, authorization_header_from_response, configure_udp_remote_and_punch,
    spawn_udp_receive_tasks, start_http_tunnel_client, start_tcp_client, start_tls_client,
    RtspClientCommand, RtspClientCommandSender, RtspClientConfig, RtspClientCredentials,
    RtspClientEvent, RtspClientHandle, RtspClientPortRange, RtspClientSendError,
    RtspClientUdpEndpoint, RtspClientUdpRemote,
};
pub use server::{
    start_server, start_tls_server, DriverConfig, DriverEvent, DriverSendError, DriverTlsConfig,
    RtspConnectionId, RtspCoreCommandSender, RtspDriverCommand, RtspServerHandle,
};
