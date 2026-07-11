//! Module-level session registry.
//!
//! The driver-level core owns ICE/DTLS/SRTP state. The module owns
//! "what is this session for in cheetah" state: stream key, role, HTTP
//! origin (WHIP/WHEP/SMS), publish lease, last-activity timestamps.
//!
//! # Session Lifecycle State Machine
//!
//! ```text
//!                    POST /whip | /whep | /publish | /play
//!                                  │
//!                                  ▼
//!                    ┌─────────────────────────────┐
//!                    │          Created            │
//!                    │  (signaling session exists  │
//!                    │   in registry; SDP exchange │
//!                    │   in progress)              │
//!                    └──────────┬──────────────────┘
//!                               │
//!              ┌────────────────┼────────────────────┐
//!              │ answer fails   │ ICE/DTLS/SRTP OK   │
//!              ▼                ▼                     │
//!   ┌──────────────┐   ┌──────────────────┐         │
//!   │ cleanup_session│   │    Connected     │         │
//!   │ (removes from │   │  (play session   │         │
//!   │  registry,    │   │   forwarding     │         │
//!   │  no residue)  │   │   media)         │         │
//!   └──────────────┘   └───────┬──────────┘         │
//!                               │                     │
//!              ┌────────────────┼─────────────────────┘
//!              │                │
//!              │  Triggers that enter unified cleanup:
//!              │  • DELETE /session/{id}
//!              │  • Driver close (ICE timeout / DTLS failure)
//!              │  • Session timeout (no activity)
//!              │  • Stream closed (publisher gone)
//!              │
//!              ▼
//!   ┌──────────────────────────────────────┐
//!   │         Unified Cleanup Path         │
//!   │  1. Send StopSession to driver       │
//!   │  2. Remove from session registry     │
//!   │  3. Close publish bridge (drop lease) │
//!   │  4. Cancel play subscriber           │
//!   └──────────────────────────────────────┘
//! ```
//!
//! ## Key Invariants
//!
//! - **HTTP request drop ≠ session close**: The HTTP connection that
//!   delivered the POST/WHIP/WHEP request is independent of the WebRTC
//!   session. Dropping the HTTP connection does NOT trigger session
//!   cleanup. ABL early versions had this bug; it was fixed in
//!   2025-06-13.
//!
//! - **Half-initialized failure leaves no residue**: If answer
//!   generation fails after the session is allocated in the registry,
//!   `cleanup_session` is called immediately, removing the session
//!   from the registry and releasing any partially-acquired bridges.
//!
//! - **Unified cleanup path**: Whether triggered by DELETE, driver
//!   close, timeout, or stream closure, all paths converge to the
//!   same cleanup logic: stop the driver session, remove from
//!   registry, close bridges.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use cheetah_sdk::{CancellationToken, PublishLease, StreamKey};
use cheetah_webrtc_core::{WebRtcSessionId, WebRtcSessionRole};

/// Origin of the WebRTC session: SMS, WHIP, WHEP, P2P, Echo, or OME WebSocket.
///
/// WebRTC 会话来源：SMS、WHIP、WHEP、P2P、Echo 或 OME WebSocket。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcApiKind {
    SmsPublish,
    SmsPlay,
    Whip,
    Whep,
    P2p,
    Echo,
    OmeWs,
}

/// Module-level lifecycle state for a session: Created, Connected, Closing, or Closed.
///
/// 模块级会话生命周期状态：Created、Connected、Closing 或 Closed。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcModuleSessionState {
    /// Created locally; SDP exchange ongoing.
    Created,
    /// Session is connected and forwarding media.
    Connected,
    /// Closing.
    Closing,
    /// Closed.
    Closed,
}

/// Per-session echo configuration for data channel and media.
///
/// 每会话的数据通道与媒体回声配置。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebRtcEchoConfig {
    pub data_channel: bool,
    pub media: bool,
}

/// Per-session telemetry accumulated from stats and BWE events.
///
/// 从统计与 BWE 事件累积的每会话遥测数据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebRtcSessionTelemetry {
    pub rtp_extensions: Vec<cheetah_webrtc_core::RtpExtensionMapping>,
    pub bwe_estimated_bps: Option<u64>,
    pub bwe_target_bps: Option<u64>,
    pub remb_bitrate_bps: Option<u64>,
    pub rtt_micros: Option<u64>,
    pub loss_fraction_x10000: Option<u32>,
    pub packets_in: u64,
    pub packets_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub nack_in: u64,
    pub nack_out: u64,
    pub pli_in: u64,
    pub pli_out: u64,
    pub fir_in: u64,
    pub fir_out: u64,
    pub rtcp_sr: u64,
    pub rtcp_rr: u64,
    pub rtcp_nack: u64,
    pub rtx_sent: u64,
    pub rtx_miss: u64,
    pub last_update_at: Option<Instant>,
}

impl WebRtcSessionTelemetry {
    /// Merge an incoming stats snapshot into the running telemetry.
    /// Ingress and egress stats carry partial data, so only non-zero fields are overwritten and RTT/loss are taken when present.
    ///
    /// 将入站统计快照合并到运行中的遥测。
    /// 入口与出口统计携带部分数据，因此仅覆盖非零字段，并在存在时更新 RTT/丢包。
    pub fn merge_stats(&mut self, snapshot: &cheetah_webrtc_core::WebRtcSessionStats) {
        if snapshot.packets_in != 0 {
            self.packets_in = snapshot.packets_in;
        }
        if snapshot.packets_out != 0 {
            self.packets_out = snapshot.packets_out;
        }
        if snapshot.bytes_in != 0 {
            self.bytes_in = snapshot.bytes_in;
        }
        if snapshot.bytes_out != 0 {
            self.bytes_out = snapshot.bytes_out;
        }
        if snapshot.nack_in != 0 {
            self.nack_in = snapshot.nack_in;
        }
        if snapshot.nack_out != 0 {
            self.nack_out = snapshot.nack_out;
        }
        if snapshot.pli_in != 0 {
            self.pli_in = snapshot.pli_in;
        }
        if snapshot.pli_out != 0 {
            self.pli_out = snapshot.pli_out;
        }
        if snapshot.fir_in != 0 {
            self.fir_in = snapshot.fir_in;
        }
        if snapshot.fir_out != 0 {
            self.fir_out = snapshot.fir_out;
        }
        if snapshot.rtx_sent != 0 {
            self.rtx_sent = snapshot.rtx_sent;
        }
        if snapshot.rtx_miss != 0 {
            self.rtx_miss = snapshot.rtx_miss;
        }
        if snapshot.rtt_us.is_some() {
            self.rtt_micros = snapshot.rtt_us;
        }
        if snapshot.loss_fraction_x10000.is_some() {
            self.loss_fraction_x10000 = snapshot.loss_fraction_x10000;
        }
        self.last_update_at = Some(Instant::now());
    }

    /// Merge a BWE snapshot into the running telemetry.
    /// None values do not clobber previous estimates, keeping BWE and REMB tracks independent.
    ///
    /// 将 BWE 快照合并到运行中的遥测。
    /// None 值不会覆盖之前的估计，使 BWE 与 REMB 保持独立。
    pub fn merge_bwe(&mut self, snapshot: &cheetah_webrtc_core::WebRtcBweStats) {
        if snapshot.estimated_bitrate_bps.is_some() {
            self.bwe_estimated_bps = snapshot.estimated_bitrate_bps;
        }
        if snapshot.target_bitrate_bps.is_some() {
            self.bwe_target_bps = snapshot.target_bitrate_bps;
        }
        self.last_update_at = Some(Instant::now());
    }

    /// Replace the negotiated RTP header extension mappings observed during SDP negotiation.
    ///
    /// 替换 SDP 协商期间观察到的 RTP 头扩展映射。
    pub fn record_rtp_extensions(
        &mut self,
        mappings: Vec<cheetah_webrtc_core::RtpExtensionMapping>,
    ) {
        self.rtp_extensions = mappings;
        self.last_update_at = Some(Instant::now());
    }

    /// Record a REMB feedback estimate from the remote receiver.
    ///
    /// 记录来自远端接收者的 REMB 反馈估计。
    pub fn record_remb(&mut self, bitrate_bps: u64) {
        self.remb_bitrate_bps = Some(bitrate_bps);
        self.last_update_at = Some(Instant::now());
    }

    /// Increment the RTCP sender report counter.
    ///
    /// 递增 RTCP sender report 计数器。
    pub fn inc_rtcp_sr(&mut self) {
        self.rtcp_sr = self.rtcp_sr.saturating_add(1);
        self.last_update_at = Some(Instant::now());
    }

    /// Increment the RTCP receiver report counter.
    ///
    /// 递增 RTCP receiver report 计数器。
    pub fn inc_rtcp_rr(&mut self) {
        self.rtcp_rr = self.rtcp_rr.saturating_add(1);
        self.last_update_at = Some(Instant::now());
    }

    /// Add RTCP NACK count to the running telemetry.
    ///
    /// 将 RTCP NACK 计数加入运行中的遥测。
    pub fn add_rtcp_nack(&mut self, count: u32) {
        self.rtcp_nack = self.rtcp_nack.saturating_add(count as u64);
        self.last_update_at = Some(Instant::now());
    }
}

/// Per-session module state captured by the registry.
/// Includes stream key, role, API origin, lifecycle state, publish lease, echo config, telemetry, and transport info.
///
/// 注册表捕获的每会话模块状态。
/// 包含 stream key、角色、API 来源、生命周期状态、发布租约、回声配置、遥测与传输信息。
pub struct WebRtcModuleSession {
    pub id: WebRtcSessionId,
    pub stream_key: StreamKey,
    pub role: WebRtcSessionRole,
    pub api_kind: WebRtcApiKind,
    pub state: WebRtcModuleSessionState,
    pub created_at: Instant,
    pub last_activity_at: Instant,
    pub publish_lease: Option<PublishLease>,
    pub subscriber_cancel: Option<CancellationToken>,
    pub echo: WebRtcEchoConfig,
    pub telemetry: WebRtcSessionTelemetry,
    /// Remote peer address observed from the selected ICE candidate
    /// pair. Populated when the driver reports a connected transport.
    pub remote_addr: Option<std::net::SocketAddr>,
    /// ICE candidate type of the selected pair (e.g. "host", "srflx",
    /// "relay"). Populated from the driver's candidate pair report.
    pub candidate_type: Option<String>,
}

impl WebRtcModuleSession {
    /// Create a new session in the Created state with the given id, stream key, role, and API origin.
    ///
    /// 使用给定 id、stream key、角色与 API 来源创建处于 Created 状态的新会话。
    pub fn new(
        id: WebRtcSessionId,
        stream_key: StreamKey,
        role: WebRtcSessionRole,
        api_kind: WebRtcApiKind,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            stream_key,
            role,
            api_kind,
            state: WebRtcModuleSessionState::Created,
            created_at: now,
            last_activity_at: now,
            publish_lease: None,
            subscriber_cancel: None,
            echo: WebRtcEchoConfig::default(),
            telemetry: WebRtcSessionTelemetry::default(),
            remote_addr: None,
            candidate_type: None,
        }
    }
}

/// Atomic allocator for monotonically increasing WebRTC session ids.
///
/// 单调递增 WebRTC 会话 id 的原子分配器。
pub struct WebRtcSessionIdAllocator {
    next: AtomicU64,
}

impl WebRtcSessionIdAllocator {
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    /// Allocate the next session id.
    ///
    /// 分配下一个会话 id。
    pub fn allocate(&self) -> WebRtcSessionId {
        WebRtcSessionId::new(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for WebRtcSessionIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry of module sessions keyed by WebRTC session id.
///
/// 按 WebRTC 会话 id 索引的模块会话注册表。
#[derive(Default)]
pub struct WebRtcSessionRegistry {
    pub sessions: HashMap<WebRtcSessionId, WebRtcModuleSession>,
}

impl WebRtcSessionRegistry {
    pub fn insert(&mut self, session: WebRtcModuleSession) {
        self.sessions.insert(session.id, session);
    }

    pub fn remove(&mut self, id: WebRtcSessionId) -> Option<WebRtcModuleSession> {
        self.sessions.remove(&id)
    }

    pub fn touch(&mut self, id: WebRtcSessionId) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.last_activity_at = Instant::now();
        }
    }

    pub fn mark_state(&mut self, id: WebRtcSessionId, state: WebRtcModuleSessionState) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.state = state;
        }
    }

    /// Update the remote address and selected candidate type for a session when the driver reports a connected candidate pair.
    ///
    /// 当驱动报告已连接的 candidate pair 时更新会话的远端地址与选中 candidate 类型。
    pub fn set_transport_info(
        &mut self,
        id: WebRtcSessionId,
        remote_addr: std::net::SocketAddr,
        candidate_type: Option<String>,
    ) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.remote_addr = Some(remote_addr);
            session.candidate_type = candidate_type;
        }
    }

    pub fn list(&self) -> Vec<&WebRtcModuleSession> {
        self.sessions.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_yields_unique_ids() {
        let alloc = WebRtcSessionIdAllocator::new();
        let a = alloc.allocate();
        let b = alloc.allocate();
        assert_ne!(a, b);
    }

    #[test]
    fn registry_round_trips() {
        let mut reg = WebRtcSessionRegistry::default();
        let id = WebRtcSessionId::new(42);
        let session = WebRtcModuleSession::new(
            id,
            StreamKey::new("live", "demo"),
            WebRtcSessionRole::Publisher,
            WebRtcApiKind::Whip,
        );
        reg.insert(session);
        assert!(reg.sessions.contains_key(&id));
        reg.mark_state(id, WebRtcModuleSessionState::Connected);
        assert_eq!(
            reg.sessions.get(&id).unwrap().state,
            WebRtcModuleSessionState::Connected
        );
        let removed = reg.remove(id);
        assert!(removed.is_some());
    }

    /// Telemetry merge keeps non-zero values across split ingress/egress
    /// `Stats` snapshots, since core emits each direction separately.
    #[test]
    fn telemetry_merges_split_ingress_egress_stats() {
        use cheetah_webrtc_core::WebRtcSessionStats;
        let mut t = WebRtcSessionTelemetry::default();

        let ingress = WebRtcSessionStats {
            packets_in: 100,
            bytes_in: 12_345,
            nack_out: 3,
            pli_out: 1,
            fir_out: 0,
            rtt_us: Some(20_000),
            loss_fraction_x10000: Some(120),
            ..Default::default()
        };
        t.merge_stats(&ingress);
        assert_eq!(t.packets_in, 100);
        assert_eq!(t.bytes_in, 12_345);
        assert_eq!(t.nack_out, 3);
        assert_eq!(t.pli_out, 1);
        assert_eq!(t.rtt_micros, Some(20_000));
        assert_eq!(t.loss_fraction_x10000, Some(120));

        let egress = WebRtcSessionStats {
            packets_out: 200,
            bytes_out: 24_000,
            nack_in: 5,
            pli_in: 2,
            rtt_us: Some(25_000),
            ..Default::default()
        };
        t.merge_stats(&egress);
        // Ingress fields preserved.
        assert_eq!(t.packets_in, 100);
        assert_eq!(t.nack_out, 3);
        // Egress fields recorded.
        assert_eq!(t.packets_out, 200);
        assert_eq!(t.nack_in, 5);
        assert_eq!(t.pli_in, 2);
        // RTT updated (last writer wins).
        assert_eq!(t.rtt_micros, Some(25_000));
    }

    #[test]
    fn telemetry_merges_bwe_snapshot() {
        use cheetah_webrtc_core::WebRtcBweStats;
        let mut t = WebRtcSessionTelemetry::default();
        t.merge_bwe(&WebRtcBweStats {
            estimated_bitrate_bps: Some(2_500_000),
            target_bitrate_bps: None,
        });
        assert_eq!(t.bwe_estimated_bps, Some(2_500_000));
        assert_eq!(t.bwe_target_bps, None);
        // None values must not clobber a previous Some.
        t.merge_bwe(&WebRtcBweStats {
            estimated_bitrate_bps: None,
            target_bitrate_bps: Some(2_000_000),
        });
        assert_eq!(t.bwe_estimated_bps, Some(2_500_000));
        assert_eq!(t.bwe_target_bps, Some(2_000_000));
    }

    #[test]
    fn telemetry_records_remb_separately_from_bwe() {
        let mut t = WebRtcSessionTelemetry::default();
        t.record_remb(1_800_000);
        assert_eq!(t.remb_bitrate_bps, Some(1_800_000));
        assert_eq!(t.bwe_estimated_bps, None);
    }

    #[test]
    fn telemetry_records_rtp_extension_mappings() {
        let mut t = WebRtcSessionTelemetry::default();
        t.record_rtp_extensions(vec![cheetah_webrtc_core::RtpExtensionMapping {
            id: 3,
            ext_type: cheetah_webrtc_core::RtpExtensionType::AbsSendTime,
            uri: "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time".into(),
            direction: Some("sendonly".into()),
        }]);

        assert_eq!(t.rtp_extensions.len(), 1);
        assert_eq!(
            t.rtp_extensions[0].ext_type,
            cheetah_webrtc_core::RtpExtensionType::AbsSendTime
        );
        assert!(t.last_update_at.is_some());
    }

    #[test]
    fn telemetry_tracks_rtcp_rr_sr_nack() {
        let mut t = WebRtcSessionTelemetry::default();
        t.inc_rtcp_sr();
        t.inc_rtcp_rr();
        t.add_rtcp_nack(7);
        assert_eq!(t.rtcp_sr, 1);
        assert_eq!(t.rtcp_rr, 1);
        assert_eq!(t.rtcp_nack, 7);
    }

    /// Phase 04 dual-track BWE/REMB simulation.
    ///
    /// When TWCC drives the BWE estimate and REMB arrives later from
    /// the remote receiver, both values must be observable on the
    /// telemetry surface. Operators rely on `bwe_estimated_bps` to
    /// follow TWCC and `remb_bitrate_bps` to follow REMB so they can
    /// detect divergence between local pacing decisions and remote
    /// receiver hints.
    #[test]
    fn telemetry_dual_track_bwe_and_remb_remain_independent() {
        use cheetah_webrtc_core::WebRtcBweStats;
        let mut t = WebRtcSessionTelemetry::default();

        // TWCC-driven BWE arrives first.
        t.merge_bwe(&WebRtcBweStats {
            estimated_bitrate_bps: Some(2_500_000),
            target_bitrate_bps: None,
        });
        // REMB from the remote receiver arrives next, suggesting a
        // lower cap.
        t.record_remb(1_500_000);

        // Both surfaces must be visible — REMB does not clobber the
        // local BWE estimate and vice versa.
        assert_eq!(t.bwe_estimated_bps, Some(2_500_000));
        assert_eq!(t.remb_bitrate_bps, Some(1_500_000));

        // A subsequent TWCC update raises the local estimate but
        // leaves the remote's REMB cap untouched until a fresh REMB
        // arrives.
        t.merge_bwe(&WebRtcBweStats {
            estimated_bitrate_bps: Some(3_000_000),
            target_bitrate_bps: None,
        });
        assert_eq!(t.bwe_estimated_bps, Some(3_000_000));
        assert_eq!(t.remb_bitrate_bps, Some(1_500_000));

        // When REMB is updated, only the REMB field changes.
        t.record_remb(1_200_000);
        assert_eq!(t.bwe_estimated_bps, Some(3_000_000));
        assert_eq!(t.remb_bitrate_bps, Some(1_200_000));
    }
}
