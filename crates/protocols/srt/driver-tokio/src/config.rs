use std::net::SocketAddr;

use cheetah_srt_core::SrtKeyLength;

/// `SrtDriverConfig` data structure.
/// `SrtDriverConfig` 数据结构.
#[derive(Debug, Clone)]
pub struct SrtDriverConfig {
    /// `listen` field of type `SocketAddr`.
    /// `listen` 字段，类型为 `SocketAddr`.
    pub listen: SocketAddr,
    /// `max_connections` field of type `usize`.
    /// `max_connections` 字段，类型为 `usize`.
    pub max_connections: usize,
    /// `idle_timeout_ms` field of type `u64`.
    /// `idle_timeout_ms` 字段，类型为 `u64`.
    pub idle_timeout_ms: u64,
    /// `connect_timeout_ms` field of type `u64`.
    /// `connect_timeout_ms` 字段，类型为 `u64`.
    pub connect_timeout_ms: u64,
    /// `latency_ms` field of type `u64`.
    /// `latency_ms` 字段，类型为 `u64`.
    pub latency_ms: u64,
    /// `stats_interval_ms` field of type `u64`.
    /// `stats_interval_ms` 字段，类型为 `u64`.
    pub stats_interval_ms: u64,
    /// `recv_buffer_packets` field of type `usize`.
    /// `recv_buffer_packets` 字段，类型为 `usize`.
    pub recv_buffer_packets: usize,
    /// `send_queue_capacity` field of type `usize`.
    /// `send_queue_capacity` 字段，类型为 `usize`.
    pub send_queue_capacity: usize,
    /// `encryption` field of type `SrtDriverEncryption`.
    /// `encryption` 字段，类型为 `SrtDriverEncryption`.
    pub encryption: SrtDriverEncryption,
}

/// `SrtDriverEncryption` data structure.
/// `SrtDriverEncryption` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtDriverEncryption {
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

impl Default for SrtDriverEncryption {
    fn default() -> Self {
        Self {
            enabled: false,
            passphrase: String::new(),
            key_length: SrtKeyLength::Aes128,
        }
    }
}
