use std::net::SocketAddr;

use cheetah_rtsp_core::RtspEvent;
use cheetah_runtime_api::{CancellationToken, JoinHandle, TaskJoinError};
use tokio::sync::mpsc;

mod command;
mod connection;
mod http_tunnel;
mod listener;
#[cfg(test)]
mod tests;
mod tls;

pub use command::{DriverSendError, RtspCoreCommandSender, RtspDriverCommand};
pub use listener::start_server;
pub use tls::{start_tls_server, DriverTlsConfig};

/// Opaque numeric identifier for an active server-side RTSP connection.
///
/// 活动服务端 RTSP 连接的不透明数字标识符。
pub type RtspConnectionId = u64;

/// Configuration for the RTSP server driver.
///
/// Queue and buffer capacities are bounded. HTTP tunnel specific limits control the
/// pending pair registry, Base64 decode buffers, and pairing timeouts.
///
/// RTSP 服务器驱动配置。
///
/// 队列与缓冲区容量均有界。HTTP 隧道相关限制控制待配对注册表、Base64 解码缓冲区
/// 以及配对超时。
#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub write_queue_capacity: usize,
    pub command_queue_capacity: usize,
    pub event_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub http_tunnel_max_pending: usize,
    pub http_tunnel_pending_timeout_ms: u64,
    pub http_tunnel_max_decoded_chunk_bytes: usize,
    pub http_tunnel_max_base64_buffer_bytes: usize,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            write_queue_capacity: 256,
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
            read_buffer_size: 64 * 1024,
            http_tunnel_max_pending: 1024,
            http_tunnel_pending_timeout_ms: 15_000,
            http_tunnel_max_decoded_chunk_bytes: 64 * 1024,
            http_tunnel_max_base64_buffer_bytes: 256 * 1024,
        }
    }
}

/// Events emitted by the RTSP server driver to the application.
///
/// `ConnectionOpened`/`ConnectionClosed` bracket the lifecycle of each connection.
/// `Core` carries the Sans-I/O `RtspEvent` produced by `cheetah_rtsp_core` for the
/// connection, which is where request parsing and state machine events live.
///
/// RTSP 服务器驱动向应用层发出的事件。
///
/// `ConnectionOpened`/`ConnectionClosed` 标记每个连接的生命周期。`Core` 携带
/// `cheetah_rtsp_core` 为连接产生的 Sans-I/O `RtspEvent`，其中包含请求解析和
/// 状态机事件。
#[derive(Debug)]
pub enum DriverEvent {
    /// A new connection has been accepted and registered.
    ///
    /// 新连接已被接受并注册。
    ConnectionOpened {
        connection_id: RtspConnectionId,
        peer: Option<SocketAddr>,
    },

    /// A connection has shut down and been removed from the registry.
    ///
    /// 连接已关闭并从注册表中移除。
    ConnectionClosed {
        connection_id: RtspConnectionId,
        reason: String,
    },

    /// A protocol event from the Sans-I/O core for this connection.
    ///
    /// 来自该连接 Sans-I/O 核心的协议事件。
    Core {
        connection_id: RtspConnectionId,
        event: RtspEvent,
    },
}

/// Handle to a running RTSP server driver.
///
/// 运行中 RTSP 服务器驱动的句柄。
pub struct RtspServerHandle {
    events_rx: mpsc::Receiver<DriverEvent>,
    cmd_tx: RtspCoreCommandSender,
    cancel: CancellationToken,
    join: Box<dyn JoinHandle>,
}

impl RtspServerHandle {
    /// Receive the next event from the server driver.
    ///
    /// 从服务器驱动接收下一个事件。
    pub async fn recv_event(&mut self) -> Option<DriverEvent> {
        self.events_rx.recv().await
    }

    /// Send a command to the server driver.
    ///
    /// 向服务器驱动发送命令。
    pub async fn send_command(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.cmd_tx.send(command).await
    }

    /// Clone the command sender.
    ///
    /// 克隆命令发送器。
    pub fn command_sender(&self) -> RtspCoreCommandSender {
        self.cmd_tx.clone()
    }

    /// Request graceful shutdown of the server listener and all connections.
    ///
    /// 请求优雅关闭服务器监听器及所有连接。
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    /// Wait for the server driver task to complete.
    ///
    /// 等待服务器驱动任务完成。
    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}
