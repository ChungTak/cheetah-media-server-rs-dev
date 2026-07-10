use bytes::Bytes;
use cheetah_rtsp_core::RtspRequestMessage;
use tokio::sync::mpsc;

/// Commands sent from the application to the RTSP client driver.
///
/// All commands are asynchronous and are queued for the connection task. Sending a
/// `SendRequest` does not wait for the response; the caller receives the corresponding
/// `RtspClientEvent::Response` through the event channel.
///
/// 从应用层发送到 RTSP 客户端驱动的命令。
///
/// 所有命令均为异步并进入连接任务队列。发送 `SendRequest` 不会等待响应，调用者通过
/// 事件通道接收对应的 `RtspClientEvent::Response`。
#[derive(Debug, Clone)]
pub enum RtspClientCommand {
    /// Send an RTSP request to the peer.
    ///
    /// 向对端发送 RTSP 请求。
    SendRequest(RtspRequestMessage),

    /// Send an interleaved RTP/RTCP frame over the TCP or HTTP tunnel transport.
    ///
    /// 通过 TCP 或 HTTP 隧道传输发送交错的 RTP/RTCP 帧。
    SendInterleaved { channel: u8, payload: Bytes },

    /// Gracefully close the client connection after flushing pending writes.
    ///
    /// 在刷新完待写数据后优雅地关闭客户端连接。
    Close,
}

/// Sender handle for `RtspClientCommand`.
///
/// Clones can be held by multiple producers; the command channel is bounded by the
/// capacity configured in `RtspClientConfig`.
///
/// `RtspClientCommand` 的发送句柄。
///
/// 可克隆并由多个生产者持有；命令通道大小受 `RtspClientConfig` 配置限制。
#[derive(Debug, Clone)]
pub struct RtspClientCommandSender {
    tx: mpsc::Sender<RtspClientCommand>,
}

impl RtspClientCommandSender {
    pub(super) fn new(tx: mpsc::Sender<RtspClientCommand>) -> Self {
        Self { tx }
    }

    /// Send a command to the client connection task.
    ///
    /// Returns `ChannelClosed` if the driver task has shut down.
    ///
    /// 向客户端连接任务发送命令。
    ///
    /// 若驱动任务已关闭，返回 `ChannelClosed`。
    pub async fn send(&self, command: RtspClientCommand) -> Result<(), super::RtspClientSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| super::RtspClientSendError::ChannelClosed)
    }
}
