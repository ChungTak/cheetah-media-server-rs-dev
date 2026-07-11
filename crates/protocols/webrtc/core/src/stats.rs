//! Stats records exposed at the boundary.
//!
//! Phase 01 models the shape and forwards the subset that `str0m` already
//! emits via `Event::PeerStats`, `MediaIngressStats`, `MediaEgressStats` and
//! `EgressBitrateEstimate`. Later phases will populate additional fields and
//! aggregate per-track/per-layer snapshots.
//!
//! 本模块包含边界处暴露的统计记录。
//!
//! 阶段 01 先定义结构，并转发 `str0m` 已通过 `Event::PeerStats`、
//! `MediaIngressStats`、`MediaEgressStats` 与 `EgressBitrateEstimate` 发出的
//! 子集。后续阶段将填充更多字段并聚合每 track/每层快照。

use serde::{Deserialize, Serialize};

use crate::types::{WebRtcSessionId, WebRtcSessionState};

/// Per-session media and transport counters.
///
/// Values are populated from `str0m` stats events and merged into the boundary
/// event stream. Fields that `str0m` does not yet expose remain at their
/// default zero values.
///
/// 每个会话的媒体与传输计数器。
///
/// 值从 `str0m` 统计事件中填充并合并到边界事件流。`str0m` 尚未暴露的字段
/// 保持默认零值。
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

/// Bandwidth estimation snapshot.
///
/// Carries the current TWCC/REMB estimate and the target rate when a rate
/// limiter is active. Both are in bits per second.
///
/// 带宽估计快照。
///
/// 携带当前 TWCC/REMB 估计值以及限速器激活时的目标码率。单位均为比特每秒。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebRtcBweStats {
    pub estimated_bitrate_bps: Option<u64>,
    pub target_bitrate_bps: Option<u64>,
}

/// Combined session state + stats snapshot.
///
/// Used when the module asks for a full boundary view of a session in a
/// single request. The core copies the latest state and fills the stats
/// from the most recent event.
///
/// 会话状态与统计的合并快照。
///
/// 当模块请求单个会话的完整边界视图时使用。核心复制最新状态，并从最近事件
/// 填充统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRtcSessionSnapshot {
    pub session_id: WebRtcSessionId,
    pub state: WebRtcSessionState,
    pub stats: WebRtcSessionStats,
    pub bwe: WebRtcBweStats,
}
