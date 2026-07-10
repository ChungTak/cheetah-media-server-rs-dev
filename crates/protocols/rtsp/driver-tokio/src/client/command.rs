use bytes::Bytes;
use cheetah_rtsp_core::RtspRequestMessage;
use tokio::sync::mpsc;

/// Command for `RTSP Client`.
/// `RTSP Client` 的命令。
#[derive(Debug, Clone)]
pub enum RtspClientCommand {
    SendRequest(RtspRequestMessage),
    SendInterleaved { channel: u8, payload: Bytes },
    Close,
}

/// `RtspClientCommandSender` data structure.
/// `RtspClientCommandSender` 数据结构。
#[derive(Debug, Clone)]
pub struct RtspClientCommandSender {
    tx: mpsc::Sender<RtspClientCommand>,
}

impl RtspClientCommandSender {
    pub(super) fn new(tx: mpsc::Sender<RtspClientCommand>) -> Self {
        Self { tx }
    }

    /// Sends data to the peer.
    /// 向对端发送数据。
    pub async fn send(&self, command: RtspClientCommand) -> Result<(), super::RtspClientSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| super::RtspClientSendError::ChannelClosed)
    }
}
