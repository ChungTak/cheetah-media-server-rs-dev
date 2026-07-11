//! `cheetah-hls-driver-tokio`: HTTP server and file writer for the HLS protocol.
//!
//! `cheetah-hls-driver-tokio`：HLS 协议的 HTTP 服务器与文件写入器。

/// HLS TS/fMP4 segment and playlist file writer.
///
/// HLS TS/fMP4 分片与播放列表文件写入器。
pub mod file_writer;
/// HLS HTTP server and connection handling.
///
/// HLS HTTP 服务器与连接处理。
pub mod server;
/// TLS support for HTTPS HLS server.
///
/// HTTPS HLS 服务器 TLS 支持。
pub mod tls;

pub use cheetah_hls_core::session::HttpMethod;
pub use cheetah_hls_core::{HlsCoreEvent, HlsCoreInput, HlsCoreOutput};

pub use file_writer::HlsFileWriter;
pub use server::{
    start_server, DriverSendError, HlsCommandSender, HlsConnectionId, HlsDriverCommand,
    HlsDriverConfig, HlsDriverEvent, HlsServerHandle,
};
pub use tls::{start_tls_server, HlsTlsConfig};
