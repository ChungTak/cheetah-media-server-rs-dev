use bytes::Bytes;
use cheetah_rtsp_core::RtspRequestMessage;
use tokio::sync::mpsc;

/// `RtspClientCommand` enumeration.
/// `RtspClientCommand` 枚举.
#[derive(Debug, Clone)]
pub enum RtspClientCommand {
    /// `SendRequest` variant.
    /// `SendRequest` 变体.
    SendRequest(RtspRequestMessage),
    /// `SendInterleaved` variant.
    /// `SendInterleaved` 变体.
    SendInterleaved { channel: u8, payload: Bytes },
    /// `Close` variant.
    /// `Close` 变体.
    Close,
}

/// `RtspClientCommandSender` data structure.
/// `RtspClientCommandSender` 数据结构.
#[derive(Debug, Clone)]
pub struct RtspClientCommandSender {
    /// `tx` field.
    /// `tx` 字段.
    tx: mpsc::Sender<RtspClientCommand>,
}

impl RtspClientCommandSender {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub(super) fn new(tx: mpsc::Sender<RtspClientCommand>) -> Self {
        Self { tx }
    }

    /// `send` function.
    /// `send` 函数.
    pub async fn send(&self, command: RtspClientCommand) -> Result<(), super::RtspClientSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| super::RtspClientSendError::ChannelClosed)
    }
}
