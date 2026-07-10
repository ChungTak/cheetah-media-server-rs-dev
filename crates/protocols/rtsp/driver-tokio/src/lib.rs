/// `client` module.
/// `client` 模块.
pub mod client;
/// `server` module.
/// `server` 模块.
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
