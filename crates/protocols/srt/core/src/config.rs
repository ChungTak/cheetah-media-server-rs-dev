use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Role of an SRT endpoint in a connection.
///
/// SRT 端点在连接中的角色。
pub enum SrtRole {
    Listener,
    Caller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Direction of an SRT stream.
///
/// SRT 流的方向。
pub enum SrtStreamMode {
    Publish,
    Request,
    Play,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Payload encapsulation used inside the SRT session.
///
/// SRT 会话中使用的负载封装。
pub enum SrtPayloadKind {
    MpegTs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// AES key length for SRT encryption.
///
/// SRT 加密的 AES 密钥长度。
pub enum SrtKeyLength {
    Aes128,
    Aes256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Encryption configuration for an SRT session.
///
/// SRT 会话的加密配置。
pub struct SrtEncryptionOptions {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: SrtKeyLength,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Full set of options for an SRT session.
///
/// SRT 会话的完整选项。
pub struct SrtSessionOptions {
    pub role: SrtRole,
    pub mode: SrtStreamMode,
    pub stream_key: String,
    pub latency_ms: u64,
    pub payload: SrtPayloadKind,
    pub encryption: SrtEncryptionOptions,
}
