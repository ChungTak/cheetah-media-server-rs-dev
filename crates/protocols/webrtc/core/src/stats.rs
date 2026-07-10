//! Stats records exposed at the boundary.
//!
//! Phase 01 only models the shape; the values are populated by later
//! phases as `str0m` stats are wired through.

use serde::{Deserialize, Serialize};

use crate::types::{WebRtcSessionId, WebRtcSessionState};

/// `WebRtcSessionStats` data structure.
/// `WebRtcSessionStats` 数据结构.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcSessionStats {
    /// `packets_in` field of type `u64`.
    /// `packets_in` 字段，类型为 `u64`.
    pub packets_in: u64,
    /// `packets_out` field of type `u64`.
    /// `packets_out` 字段，类型为 `u64`.
    pub packets_out: u64,
    /// `bytes_in` field of type `u64`.
    /// `bytes_in` 字段，类型为 `u64`.
    pub bytes_in: u64,
    /// `bytes_out` field of type `u64`.
    /// `bytes_out` 字段，类型为 `u64`.
    pub bytes_out: u64,
    /// `rtt_us` field.
    /// `rtt_us` 字段.
    pub rtt_us: Option<u64>,
    /// `loss_fraction_x10000` field.
    /// `loss_fraction_x10000` 字段.
    pub loss_fraction_x10000: Option<u32>,
    /// `nack_in` field of type `u64`.
    /// `nack_in` 字段，类型为 `u64`.
    pub nack_in: u64,
    /// `nack_out` field of type `u64`.
    /// `nack_out` 字段，类型为 `u64`.
    pub nack_out: u64,
    /// `pli_in` field of type `u64`.
    /// `pli_in` 字段，类型为 `u64`.
    pub pli_in: u64,
    /// `pli_out` field of type `u64`.
    /// `pli_out` 字段，类型为 `u64`.
    pub pli_out: u64,
    /// `fir_in` field of type `u64`.
    /// `fir_in` 字段，类型为 `u64`.
    pub fir_in: u64,
    /// `fir_out` field of type `u64`.
    /// `fir_out` 字段，类型为 `u64`.
    pub fir_out: u64,
    /// `rtx_sent` field of type `u64`.
    /// `rtx_sent` 字段，类型为 `u64`.
    pub rtx_sent: u64,
    /// `rtx_miss` field of type `u64`.
    /// `rtx_miss` 字段，类型为 `u64`.
    pub rtx_miss: u64,
}

/// `WebRtcBweStats` data structure.
/// `WebRtcBweStats` 数据结构.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebRtcBweStats {
    /// `estimated_bitrate_bps` field.
    /// `estimated_bitrate_bps` 字段.
    pub estimated_bitrate_bps: Option<u64>,
    /// `target_bitrate_bps` field.
    /// `target_bitrate_bps` 字段.
    pub target_bitrate_bps: Option<u64>,
}

/// `WebRtcSessionSnapshot` data structure.
/// `WebRtcSessionSnapshot` 数据结构.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcSessionSnapshot {
    /// `session_id` field of type `WebRtcSessionId`.
    /// `session_id` 字段，类型为 `WebRtcSessionId`.
    pub session_id: WebRtcSessionId,
    /// `state` field of type `WebRtcSessionState`.
    /// `state` 字段，类型为 `WebRtcSessionState`.
    pub state: WebRtcSessionState,
    /// `stats` field of type `WebRtcSessionStats`.
    /// `stats` 字段，类型为 `WebRtcSessionStats`.
    pub stats: WebRtcSessionStats,
    /// `bwe` field of type `WebRtcBweStats`.
    /// `bwe` 字段，类型为 `WebRtcBweStats`.
    pub bwe: WebRtcBweStats,
}
