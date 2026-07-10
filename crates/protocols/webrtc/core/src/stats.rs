//! Stats records exposed at the boundary.
//!
//! Phase 01 only models the shape; the values are populated by later
//! phases as `str0m` stats are wired through.

use serde::{Deserialize, Serialize};

use crate::types::{WebRtcSessionId, WebRtcSessionState};

/// `WebRtcSessionStats` data structure.
/// `WebRtcSessionStats` 数据结构。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcSessionStats {
    pub packets_in: u64,
    pub packets_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub rtt_us: Option<u64>,
    pub loss_fraction_x10000: Option<u32>,
    pub nack_in: u64,
    pub nack_out: u64,
    pub pli_in: u64,
    pub pli_out: u64,
    pub fir_in: u64,
    pub fir_out: u64,
    pub rtx_sent: u64,
    pub rtx_miss: u64,
}

/// `WebRtcBweStats` data structure.
/// `WebRtcBweStats` 数据结构。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebRtcBweStats {
    pub estimated_bitrate_bps: Option<u64>,
    pub target_bitrate_bps: Option<u64>,
}

/// `WebRtcSessionSnapshot` data structure.
/// `WebRtcSessionSnapshot` 数据结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcSessionSnapshot {
    pub session_id: WebRtcSessionId,
    pub state: WebRtcSessionState,
    pub stats: WebRtcSessionStats,
    pub bwe: WebRtcBweStats,
}
