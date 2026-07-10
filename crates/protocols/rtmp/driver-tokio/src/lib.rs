/// Module for `client`.
/// `client` 相关模块。
pub mod client;
/// Module for `server`.
/// `server` 相关模块。
pub mod server;
/// Module for `tls`.
/// `tls` 相关模块。
pub mod tls;

pub use cheetah_rtmp_core::{RtmpCoreCommand, RtmpEvent, RtmpMediaType};

pub use client::{
    start_client, start_tls_client, ClientDriverEvent, ClientSendError, RtmpClientCommandSender,
    RtmpClientDriverCommand, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
};

pub use server::{
    start_server, start_tls_server, DriverConfig, DriverEvent, DriverSendError, RtmpConnectionId,
    RtmpCoreCommandSender, RtmpDriverCommand, RtmpServerHandle,
};

pub use tls::{RtmpTlsClientConfig, RtmpTlsConfig};
