use std::collections::HashMap;
use std::sync::Arc;

use cheetah_rtsp_core::RtspCommand;
use cheetah_runtime_api::CancellationToken;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

use super::RtspConnectionId;

/// Commands sent from the application to the RTSP server driver.
///
/// `Core` forwards a protocol command to a specific connection's state machine.
/// `CloseConnection` requests a graceful close. `Shutdown` stops the listener.
///
/// 从应用层发送到 RTSP 服务器驱动的命令。
///
/// `Core` 将协议命令转发给指定连接的状态机。`CloseConnection` 请求优雅关闭。
/// `Shutdown` 停止监听器。
#[derive(Debug, Clone)]
pub enum RtspDriverCommand {
    /// Forward a `RtspCommand` to the Sans-I/O core for a connection.
    ///
    /// 将 `RtspCommand` 转发给连接的 Sans-I/O 核心。
    Core {
        connection_id: RtspConnectionId,
        command: RtspCommand,
    },

    /// Request a connection to close.
    ///
    /// 请求关闭连接。
    CloseConnection { connection_id: RtspConnectionId },

    /// Stop the server listener and signal all connections to close.
    ///
    /// 停止服务器监听器并通知所有连接关闭。
    Shutdown,
}

/// Sender for `RtspDriverCommand`.
///
/// Holds the `mpsc` channel and provides typed helpers for common operations.
///
/// `RtspDriverCommand` 的发送器。
///
/// 持有 `mpsc` 通道并提供常见操作的类型化辅助方法。
#[derive(Clone)]
pub struct RtspCoreCommandSender {
    tx: mpsc::Sender<RtspDriverCommand>,
}

/// Error returned when a driver command cannot be sent.
///
/// 驱动命令发送失败时返回的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverSendError {
    /// The command channel has closed.
    ///
    /// 命令通道已关闭。
    ChannelClosed,
}

impl RtspCoreCommandSender {
    pub(super) fn new(tx: mpsc::Sender<RtspDriverCommand>) -> Self {
        Self { tx }
    }

    /// Send a generic driver command.
    ///
    /// 发送通用驱动命令。
    pub async fn send(&self, command: RtspDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }

    /// Send a protocol command to a specific connection.
    ///
    /// 向指定连接发送协议命令。
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

    /// Request a connection to close.
    ///
    /// 请求关闭连接。
    pub async fn close_connection(
        &self,
        connection_id: RtspConnectionId,
    ) -> Result<(), DriverSendError> {
        self.send(RtspDriverCommand::CloseConnection { connection_id })
            .await
    }
}

/// Internal per-connection command.
///
/// 内部单连接命令。
#[derive(Debug)]
pub(super) enum ConnectionCommand {
    /// Forward a `RtspCommand` to the connection's core.
    ///
    /// 将 `RtspCommand` 转发给连接的核心。
    Core(RtspCommand),

    /// Close the connection after flushing.
    ///
    /// 在刷新后关闭连接。
    Close,
}

/// Handle to a registered connection in the server.
///
/// 服务器中已注册连接的句柄。
#[derive(Clone)]
pub(super) struct ConnectionHandle {
    pub(super) tx: mpsc::Sender<ConnectionCommand>,
    pub(super) cancel: CancellationToken,
}

/// Shared map of active connection IDs to their handles.
///
/// 活动连接 ID 到其句柄的共享映射。
pub(super) type ConnectionMap = Arc<Mutex<HashMap<RtspConnectionId, ConnectionHandle>>>;

/// Dispatch a top-level driver command and return whether the listener should stop.
///
/// `Shutdown` cancels the listener token and returns `true`. `Core` and `CloseConnection`
/// are routed to the appropriate connection.
///
/// 分发顶层驱动命令，并返回监听器是否应停止。
///
/// `Shutdown` 取消监听器令牌并返回 `true`。`Core` 与 `CloseConnection` 被路由到
/// 对应连接。
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

/// Send a command to a connection by ID.
///
/// If the command queue is full, the connection is force-closed to prevent the driver
/// from stalling. If the channel is closed, the stale entry is removed from the map.
///
/// 按 ID 向连接发送命令。
///
/// 若命令队列已满，为避免驱动卡死，强制关闭该连接。若通道已关闭，从映射中移除
/// 陈旧条目。
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

/// Request a connection to close gracefully.
///
/// 请求连接优雅关闭。
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

/// Force close a connection by removing it and cancelling its token.
///
/// 通过移除连接并取消其令牌来强制关闭连接。
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
