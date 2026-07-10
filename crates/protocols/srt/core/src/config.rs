use serde::{Deserialize, Serialize};

/// `SrtRole` enumeration.
/// `SrtRole` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtRole {
    Listener,
    Caller,
}

/// Mode selecting `SRT Stream` behavior.
/// 选择 `SRT Stream` 行为的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtStreamMode {
    Publish,
    Request,
    Play,
}

/// Kind of `SRT Payload`.
/// `SRT Payload` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtPayloadKind {
    MpegTs,
}

/// `SrtKeyLength` enumeration.
/// `SrtKeyLength` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SrtKeyLength {
    Aes128,
    Aes256,
}

/// Options for `SRT Encryption`.
/// `SRT Encryption` 的选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtEncryptionOptions {
    pub enabled: bool,
    pub passphrase: String,
    pub key_length: SrtKeyLength,
}

/// Options for `SRT Session`.
/// `SRT Session` 的选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtSessionOptions {
    pub role: SrtRole,
    pub mode: SrtStreamMode,
    pub stream_key: String,
    pub latency_ms: u64,
    pub payload: SrtPayloadKind,
    pub encryption: SrtEncryptionOptions,
}
