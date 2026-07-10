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

pub type RtspConnectionId = u64;

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

#[derive(Debug)]
pub enum DriverEvent {
    ConnectionOpened {
        connection_id: RtspConnectionId,
        peer: Option<SocketAddr>,
    },
    ConnectionClosed {
        connection_id: RtspConnectionId,
        reason: String,
    },
    Core {
        connection_id: RtspConnectionId,
        event: RtspEvent,
    },
}

pub struct RtspServerHandle {
    events_rx: mpsc::Receiver<DriverEvent>,
    cmd_tx: RtspCoreCommandSender,
    cancel: CancellationToken,
    join: Box<dyn JoinHandle>,
}

impl RtspServerHandle {
    pub async fn recv_event(&mut self) -> Option<DriverEvent> {
        self.events_rx.recv().await
    }

    pub async fn send_command(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.cmd_tx.send(command).await
    }

    pub fn command_sender(&self) -> RtspCoreCommandSender {
        self.cmd_tx.clone()
    }

    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}
