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

/// `RtspConnectionId` type alias.
/// `RtspConnectionId` 类型别名.
pub type RtspConnectionId = u64;

/// `DriverConfig` data structure.
/// `DriverConfig` 数据结构.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    /// `write_queue_capacity` field of type `usize`.
    /// `write_queue_capacity` 字段，类型为 `usize`.
    pub write_queue_capacity: usize,
    /// `command_queue_capacity` field of type `usize`.
    /// `command_queue_capacity` 字段，类型为 `usize`.
    pub command_queue_capacity: usize,
    /// `event_queue_capacity` field of type `usize`.
    /// `event_queue_capacity` 字段，类型为 `usize`.
    pub event_queue_capacity: usize,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `http_tunnel_max_pending` field of type `usize`.
    /// `http_tunnel_max_pending` 字段，类型为 `usize`.
    pub http_tunnel_max_pending: usize,
    /// `http_tunnel_pending_timeout_ms` field of type `u64`.
    /// `http_tunnel_pending_timeout_ms` 字段，类型为 `u64`.
    pub http_tunnel_pending_timeout_ms: u64,
    /// `http_tunnel_max_decoded_chunk_bytes` field of type `usize`.
    /// `http_tunnel_max_decoded_chunk_bytes` 字段，类型为 `usize`.
    pub http_tunnel_max_decoded_chunk_bytes: usize,
    /// `http_tunnel_max_base64_buffer_bytes` field of type `usize`.
    /// `http_tunnel_max_base64_buffer_bytes` 字段，类型为 `usize`.
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

/// `DriverEvent` enumeration.
/// `DriverEvent` 枚举.
#[derive(Debug)]
pub enum DriverEvent {
    /// `ConnectionOpened` variant.
    /// `ConnectionOpened` 变体.
    ConnectionOpened {
        connection_id: RtspConnectionId,
        peer: Option<SocketAddr>,
    },
    /// `ConnectionClosed` variant.
    /// `ConnectionClosed` 变体.
    ConnectionClosed {
        connection_id: RtspConnectionId,
        reason: String,
    },
    /// `Core` variant.
    /// `Core` 变体.
    Core {
        connection_id: RtspConnectionId,
        event: RtspEvent,
    },
}

/// `RtspServerHandle` data structure.
/// `RtspServerHandle` 数据结构.
pub struct RtspServerHandle {
    /// `events_rx` field.
    /// `events_rx` 字段.
    events_rx: mpsc::Receiver<DriverEvent>,
    /// `cmd_tx` field of type `RtspCoreCommandSender`.
    /// `cmd_tx` 字段，类型为 `RtspCoreCommandSender`.
    cmd_tx: RtspCoreCommandSender,
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    cancel: CancellationToken,
    /// `join` field.
    /// `join` 字段.
    join: Box<dyn JoinHandle>,
}

impl RtspServerHandle {
    /// `recv_event` function.
    /// `recv_event` 函数.
    pub async fn recv_event(&mut self) -> Option<DriverEvent> {
        self.events_rx.recv().await
    }

    /// `send_command` function.
    /// `send_command` 函数.
    pub async fn send_command(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.cmd_tx.send(command).await
    }

    /// `command_sender` function.
    /// `command_sender` 函数.
    pub fn command_sender(&self) -> RtspCoreCommandSender {
        self.cmd_tx.clone()
    }

    /// `shutdown` function.
    /// `shutdown` 函数.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    /// `wait` function.
    /// `wait` 函数.
    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}
