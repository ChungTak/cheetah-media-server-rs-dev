use bytes::Bytes;
use cheetah_rtsp_core::RtspRequestMessage;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum RtspClientCommand {
    SendRequest(RtspRequestMessage),
    SendInterleaved { channel: u8, payload: Bytes },
    Close,
}

#[derive(Debug, Clone)]
pub struct RtspClientCommandSender {
    tx: mpsc::Sender<RtspClientCommand>,
}

impl RtspClientCommandSender {
    pub(super) fn new(tx: mpsc::Sender<RtspClientCommand>) -> Self {
        Self { tx }
    }

    pub async fn send(&self, command: RtspClientCommand) -> Result<(), super::RtspClientSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| super::RtspClientSendError::ChannelClosed)
    }
}
