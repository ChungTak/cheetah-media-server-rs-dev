use std::collections::HashMap;
use std::sync::Arc;

use cheetah_rtsp_core::RtspCommand;
use cheetah_runtime_api::CancellationToken;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

use super::RtspConnectionId;

/// `RtspDriverCommand` enumeration.
/// `RtspDriverCommand` 枚举.
#[derive(Debug, Clone)]
pub enum RtspDriverCommand {
    /// `Core` variant.
    /// `Core` 变体.
    Core {
        connection_id: RtspConnectionId,
        command: RtspCommand,
    },
    /// `CloseConnection` variant.
    /// `CloseConnection` 变体.
    CloseConnection { connection_id: RtspConnectionId },
    /// `Shutdown` variant.
    /// `Shutdown` 变体.
    Shutdown,
}

/// `RtspCoreCommandSender` data structure.
/// `RtspCoreCommandSender` 数据结构.
#[derive(Clone)]
pub struct RtspCoreCommandSender {
    /// `tx` field.
    /// `tx` 字段.
    tx: mpsc::Sender<RtspDriverCommand>,
}

/// `DriverSendError` enumeration.
/// `DriverSendError` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverSendError {
    /// `ChannelClosed` variant.
    /// `ChannelClosed` 变体.
    ChannelClosed,
}

impl RtspCoreCommandSender {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub(super) fn new(tx: mpsc::Sender<RtspDriverCommand>) -> Self {
        Self { tx }
    }

    /// `send` function.
    /// `send` 函数.
    pub async fn send(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }

    /// `send_core` function.
    /// `send_core` 函数.
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

    /// `close_connection` function.
    /// `close_connection` 函数.
    pub async fn close_connection(
        &self,
        connection_id: RtspConnectionId,
    ) -> Result<(), DriverSendError> {
        self.send(RtspDriverCommand::CloseConnection { connection_id })
            .await
    }
}

/// `ConnectionCommand` enumeration.
/// `ConnectionCommand` 枚举.
#[derive(Debug)]
pub(super) enum ConnectionCommand {
    /// `Core` variant.
    /// `Core` 变体.
    Core(RtspCommand),
    /// `Close` variant.
    /// `Close` 变体.
    Close,
}

/// `ConnectionHandle` data structure.
/// `ConnectionHandle` 数据结构.
#[derive(Clone)]
pub(super) struct ConnectionHandle {
    /// `tx` field.
    /// `tx` 字段.
    pub(super) tx: mpsc::Sender<ConnectionCommand>,
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub(super) cancel: CancellationToken,
}

/// `ConnectionMap` type alias.
/// `ConnectionMap` 类型别名.
pub(super) type ConnectionMap = Arc<Mutex<HashMap<RtspConnectionId, ConnectionHandle>>>;

/// `handle_driver_command` function.
/// `handle_driver_command` 函数.
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

/// `send_connection_command` function.
/// `send_connection_command` 函数.
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

/// `request_close_connection` function.
/// `request_close_connection` 函数.
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

/// `force_close_connection` function.
/// `force_close_connection` 函数.
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
