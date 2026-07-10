use std::collections::HashMap;
use std::sync::Arc;

use cheetah_rtsp_core::RtspCommand;
use cheetah_runtime_api::CancellationToken;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

use super::RtspConnectionId;

/// Command for `RTSP Driver`.
/// `RTSP Driver` 的命令。
#[derive(Debug, Clone)]
pub enum RtspDriverCommand {
    Core {
        connection_id: RtspConnectionId,
        command: RtspCommand,
    },
    CloseConnection {
        connection_id: RtspConnectionId,
    },
    Shutdown,
}

/// `RtspCoreCommandSender` data structure.
/// `RtspCoreCommandSender` 数据结构。
#[derive(Clone)]
pub struct RtspCoreCommandSender {
    tx: mpsc::Sender<RtspDriverCommand>,
}

/// Error returned by `Driver Send` operations.
/// `Driver Send` 操作返回的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverSendError {
    ChannelClosed,
}

impl RtspCoreCommandSender {
    pub(super) fn new(tx: mpsc::Sender<RtspDriverCommand>) -> Self {
        Self { tx }
    }

    /// Sends data to the peer.
    /// 向对端发送数据。
    pub async fn send(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }

    /// Sends `core` to the peer.
    /// 向对端发送 `core`。
    pub async fn send_core(
        &self,
        connection_id: RtspConnectionId,
        command: RtspCommand,
    ) -> Result<(), DriverSendError> {
        self.send(RtspDriverCommand::Core {
            connection_id,
            command,
        })
        .await
    }

    /// Closes the `connection`.
    /// 关闭 `connection`。
    pub async fn close_connection(
        &self,
        connection_id: RtspConnectionId,
    ) -> Result<(), DriverSendError> {
        self.send(RtspDriverCommand::CloseConnection { connection_id })
            .await
    }
}

#[derive(Debug)]
pub(super) enum ConnectionCommand {
    Core(RtspCommand),
    Close,
}

#[derive(Clone)]
pub(super) struct ConnectionHandle {
    pub(super) tx: mpsc::Sender<ConnectionCommand>,
    pub(super) cancel: CancellationToken,
}

pub(super) type ConnectionMap = Arc<Mutex<HashMap<RtspConnectionId, ConnectionHandle>>>;

pub(super) async fn handle_driver_command(
    cmd: RtspDriverCommand,
    conn_map: &ConnectionMap,
    cancel: &CancellationToken,
) -> bool {
    match cmd {
        RtspDriverCommand::Core {
            connection_id,
            command,
        } => {
            send_connection_command(connection_id, ConnectionCommand::Core(command), conn_map);
            false
        }
        RtspDriverCommand::CloseConnection { connection_id } => {
            request_close_connection(connection_id, conn_map);
            false
        }
        RtspDriverCommand::Shutdown => {
            cancel.cancel();
            true
        }
    }
}

pub(super) fn send_connection_command(
    connection_id: RtspConnectionId,
    command: ConnectionCommand,
    conn_map: &ConnectionMap,
) {
    let handle = conn_map.lock().get(&connection_id).cloned();
    let Some(handle) = handle else {
        warn!(connection_id, "rtsp command target connection not found");
        return;
    };

    match handle.tx.try_send(command) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            conn_map.lock().remove(&connection_id);
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            warn!(
                connection_id,
                "connection command queue full, force closing to avoid driver stall"
            );
            force_close_connection(connection_id, conn_map);
        }
    }
}

pub(super) fn request_close_connection(connection_id: RtspConnectionId, conn_map: &ConnectionMap) {
    let handle = conn_map.lock().get(&connection_id).cloned();
    let Some(handle) = handle else {
        return;
    };

    match handle.tx.try_send(ConnectionCommand::Close) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            conn_map.lock().remove(&connection_id);
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            force_close_connection(connection_id, conn_map);
        }
    }
}

pub(super) fn force_close_connection(connection_id: RtspConnectionId, conn_map: &ConnectionMap) {
    let handle = conn_map.lock().remove(&connection_id);
    let Some(handle) = handle else {
        return;
    };

    handle.cancel.cancel();
    match handle.tx.try_send(ConnectionCommand::Close) {
        Ok(())
        | Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
        | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
    }
}
