use std::net::SocketAddr;

use cheetah_srt_core::SrtKeyLength;

#[derive(Debug, Clone)]
/// UDP/network configuration for the SRT Tokio driver.
///
/// SRT Tokio 驱动的 UDP/网络配置。
pub struct SrtDriverConfig {
    pub listen: SocketAddr,
    pub max_connections: usize,
    pub idle_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub latency_ms: u64,
    pub stats_interval_ms: u64,
    pub recv_buffer_packets: usize,
    pub send_queue_capacity: usize,
    pub srt_version: u32,
    pub encryption: SrtDriverEncryption,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Encryption settings applied to the SRT driver listener and caller.
///
/// 应用于 SRT 驱动监听端与呼叫端的加密设置。
pub struct SrtDriverEncryption {
    pub enabled: bool,
    pub passphrase: String,
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
