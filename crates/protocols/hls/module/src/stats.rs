//! HLS playback statistics.

use serde::{Deserialize, Serialize};

/// Statistics for a single HLS stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlsStreamStats {
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `active_sessions` field of type `usize`.
    /// `active_sessions` 字段，类型为 `usize`.
    pub active_sessions: usize,
    /// `total_bytes_sent` field of type `u64`.
    /// `total_bytes_sent` 字段，类型为 `u64`.
    pub total_bytes_sent: u64,
}

/// Statistics for a single HLS session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlsSessionInfo {
    /// `session_id` field of type `u64`.
    /// `session_id` 字段，类型为 `u64`.
    pub session_id: u64,
    /// `bytes_sent` field of type `u64`.
    /// `bytes_sent` 字段，类型为 `u64`.
    pub bytes_sent: u64,
    /// `last_request_us` field of type `u64`.
    /// `last_request_us` 字段，类型为 `u64`.
    pub last_request_us: u64,
}
