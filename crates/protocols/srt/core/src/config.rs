use serde::{Deserialize, Serialize};

/// `SrtRole` enumeration.
/// `SrtRole` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtRole {
    /// `Listener` variant.
    /// `Listener` 变体.
    Listener,
    /// `Caller` variant.
    /// `Caller` 变体.
    Caller,
}

/// `SrtStreamMode` enumeration.
/// `SrtStreamMode` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtStreamMode {
    /// `Publish` variant.
    /// `Publish` 变体.
    Publish,
    /// `Request` variant.
    /// `Request` 变体.
    Request,
    /// `Play` variant.
    /// `Play` 变体.
    Play,
}

/// `SrtPayloadKind` enumeration.
/// `SrtPayloadKind` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrtPayloadKind {
    /// `MpegTs` variant.
    /// `MpegTs` 变体.
    MpegTs,
}

/// `SrtKeyLength` enumeration.
/// `SrtKeyLength` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SrtKeyLength {
    /// `Aes128` variant.
    /// `Aes128` 变体.
    Aes128,
    /// `Aes256` variant.
    /// `Aes256` 变体.
    Aes256,
}

/// `SrtEncryptionOptions` data structure.
/// `SrtEncryptionOptions` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtEncryptionOptions {
    /// `enabled` field of type `bool`.
    /// `enabled` 字段，类型为 `bool`.
    pub enabled: bool,
    /// `passphrase` field of type `String`.
    /// `passphrase` 字段，类型为 `String`.
    pub passphrase: String,
    /// `key_length` field of type `SrtKeyLength`.
    /// `key_length` 字段，类型为 `SrtKeyLength`.
    pub key_length: SrtKeyLength,
}

/// `SrtSessionOptions` data structure.
/// `SrtSessionOptions` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtSessionOptions {
    /// `role` field of type `SrtRole`.
    /// `role` 字段，类型为 `SrtRole`.
    pub role: SrtRole,
    /// `mode` field of type `SrtStreamMode`.
    /// `mode` 字段，类型为 `SrtStreamMode`.
    pub mode: SrtStreamMode,
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `latency_ms` field of type `u64`.
    /// `latency_ms` 字段，类型为 `u64`.
    pub latency_ms: u64,
    /// `payload` field of type `SrtPayloadKind`.
    /// `payload` 字段，类型为 `SrtPayloadKind`.
    pub payload: SrtPayloadKind,
    /// `encryption` field of type `SrtEncryptionOptions`.
    /// `encryption` 字段，类型为 `SrtEncryptionOptions`.
    pub encryption: SrtEncryptionOptions,
}
