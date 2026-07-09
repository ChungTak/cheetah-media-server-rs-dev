//! HLS playback statistics.

use serde::{Deserialize, Serialize};

/// Statistics for a single HLS stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlsStreamStats {
    pub stream_key: String,
    pub active_sessions: usize,
    pub total_bytes_sent: u64,
}

/// Statistics for a single HLS session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlsSessionInfo {
    pub session_id: u64,
    pub bytes_sent: u64,
    pub last_request_us: u64,
}
