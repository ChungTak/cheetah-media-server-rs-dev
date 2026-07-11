//! HLS playback statistics.
//!
//! Holds the public data structures used by the control plane to report stream
//! reachability and per-session bandwidth.
//!
//! HLS 播放统计。
//!
//! 保存控制面用于报告流可达性与单会话带宽的公共数据结构。
//!

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Statistics for a single HLS stream.
///
/// Aggregated from the session map and reflects the total bytes delivered to
/// all player sessions currently consuming the stream.
///
/// 单个 HLS 流的统计信息。
///
/// 从会话映射聚合而来，反映当前所有播放器会话接收到的总字节数。
pub struct HlsStreamStats {
    pub stream_key: String,
    pub active_sessions: usize,
    pub total_bytes_sent: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Statistics for a single HLS session.
///
/// Tracks byte consumption and the last request timestamp so the cleanup loop
/// can evict idle sessions.
///
/// 单个 HLS 会话的统计信息。
///
/// 记录字节消耗与最后请求时间戳，以便清理循环驱逐空闲会话。
pub struct HlsSessionInfo {
    pub session_id: u64,
    pub bytes_sent: u64,
    pub last_request_us: u64,
}
