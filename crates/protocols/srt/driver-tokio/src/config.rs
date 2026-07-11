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
    pub fec: SrtDriverFecConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// FEC configuration carried by the SRT Tokio driver.
///
/// As of `shiguredo_srt = "=2026.1.0-canary.1"`, the underlying library does not
/// expose a packet-filter / FEC API, so this struct is reserved for future
/// driver integration.
///
/// SRT Tokio 驱动携带的 FEC 配置。
/// 在 `shiguredo_srt = "=2026.1.0-canary.1"` 中底层库未暴露 packet-filter / FEC API，
/// 因此该结构体为未来的驱动集成预留。
pub struct SrtDriverFecConfig {
    pub enabled: bool,
    pub required: bool,
    pub cols: u32,
    pub rows: u32,
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
