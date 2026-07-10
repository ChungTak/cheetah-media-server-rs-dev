use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtRole {
    Listener,
    Caller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtStreamMode {
    Publish,
    Request,
    Play,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtPayloadKind {
    MpegTs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SrtKeyLength {
    Aes128,
    Aes256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtEncryptionOptions {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: SrtKeyLength,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtSessionOptions {
    pub role: SrtRole,
    pub mode: SrtStreamMode,
    pub stream_key: String,
    pub latency_ms: u64,
    pub payload: SrtPayloadKind,
    pub encryption: SrtEncryptionOptions,
}
