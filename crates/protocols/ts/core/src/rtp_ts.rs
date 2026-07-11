//! RTP-TS ingest: Sans-I/O state machine for receiving TS over RTP.
//!
//! Handles RTP header validation, SSRC session routing, TS/PS payload probe,
//! and 188-byte aligned payload slicing into the TS demuxer.

use std::collections::HashMap;
use std::net::SocketAddr;

use cheetah_codec::{
    MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig, RtpPacket, TS_PACKET_SIZE,
};

/// Per-SSRC demuxer selection (TS or PS) before probing is complete.
///
/// 探测完成前，每个 SSRC 的解复用器选择（TS 或 PS）。
enum SessionDemuxer {
    Pending,
    Ts(MpegTsDemuxer),
    Ps(cheetah_codec::PsDemuxer),
}

/// `SessionDemuxer` flushing and state delegation.
///
/// `SessionDemuxer` 的刷新与状态委托。
impl SessionDemuxer {
    pub fn flush(&mut self) -> Vec<MpegTsDemuxEvent> {
        match self {
            SessionDemuxer::Pending => Vec::new(),
            SessionDemuxer::Ts(demuxer) => demuxer.flush(),
            SessionDemuxer::Ps(demuxer) => {
                let ps_events = demuxer.flush();
                let mut events = Vec::with_capacity(ps_events.len());
                for ev in ps_events {
                    match ev {
                        cheetah_codec::PsDemuxEvent::TrackInfo(tracks) => {
                            for track in tracks {
                                events.push(MpegTsDemuxEvent::TrackFound(track));
                            }
                        }
                        cheetah_codec::PsDemuxEvent::Frame(frame) => {
                            events.push(MpegTsDemuxEvent::Frame(*frame));
                        }
                        cheetah_codec::PsDemuxEvent::Diagnostic(_) => {}
                    }
                }
                events
            }
        }
    }
}

/// Configuration for the RTP-TS ingest.
///
/// RTP-TS 摄入配置。
#[derive(Debug, Clone)]
pub struct RtpTsIngestConfig {
    /// Maximum concurrent SSRC sessions.
    ///
    /// 最大并发 SSRC 会话数。
    pub max_sessions: usize,
    /// Session idle timeout in milliseconds.
    ///
    /// 会话空闲超时（毫秒）。
    pub session_idle_timeout_ms: u64,
    /// Allow non-188-aligned RTP payloads (use demux push for resync).
    ///
    /// 是否允许非 188 字节对齐的 RTP 负载（使用 demux push 重新同步）。
    pub allow_unaligned_payload: bool,
    /// TS demuxer config for each session.
    ///
    /// 每个会话的 TS 解复用器配置。
    pub demux_config: MpegTsDemuxerConfig,
    /// Maximum consecutive sync losses before resetting demuxer.
    ///
    /// 连续同步丢失阈值，超过后重置解复用器。
    pub max_sync_loss: usize,
}

impl Default for RtpTsIngestConfig {
    fn default() -> Self {
        Self {
            max_sessions: 1024,
            session_idle_timeout_ms: 30_000,
            allow_unaligned_payload: true,
            demux_config: MpegTsDemuxerConfig::default(),
            max_sync_loss: 10,
        }
    }
}

/// Diagnostic events from RTP-TS ingest.
///
/// RTP-TS 摄入诊断事件。
#[derive(Debug, Clone)]
pub enum RtpTsDiagnostic {
    /// RTP packet is not version 2.
    ///
    /// RTP 包版本不是 2。
    InvalidRtpVersion { version: u8 },
    /// RTP header parsing failed (too short, extension overflow, etc).
    ///
    /// RTP 头解析失败（过短、扩展溢出等）。
    RtpHeaderError,
    /// Empty RTP payload after header stripping.
    ///
    /// 去除 RTP 头后负载为空。
    EmptyPayload { ssrc: u32 },
    /// Payload detected as PS (not supported).
    ///
    /// 检测到 PS 负载（暂不支持）。
    UnsupportedPsPayload { ssrc: u32 },
    /// Payload is neither TS nor PS.
    ///
    /// 既不是 TS 也不是 PS 的未知负载。
    UnknownPayload { ssrc: u32 },
    /// RTP sequence gap detected.
    ///
    /// 检测到 RTP 序列号跳变。
    SequenceGap { ssrc: u32, expected: u16, got: u16 },
    /// Source address changed for existing SSRC.
    ///
    /// 已有 SSRC 的源地址发生变化。
    SourceAddressChanged {
        ssrc: u32,
        old: SocketAddr,
        new: SocketAddr,
    },
    /// Session limit reached, new SSRC rejected.
    ///
    /// 会话数达到上限，新 SSRC 被拒绝。
    SessionLimitReached { ssrc: u32 },
    /// Session idle timeout.
    ///
    /// 会话空闲超时。
    SessionTimeout { ssrc: u32 },
    /// Non-188-aligned payload (using compat path).
    ///
    /// 非 188 字节对齐负载（使用兼容路径）。
    UnalignedPayload { ssrc: u32, payload_len: usize },
    /// Consecutive sync losses exceeded threshold.
    ///
    /// 连续同步丢失超过阈值。
    SyncLossThreshold { ssrc: u32 },
}

/// Events emitted by the RTP-TS ingest.
///
/// RTP-TS 摄入发出的输出事件。
#[derive(Debug, Clone)]
pub enum RtpTsIngestEvent {
    /// A new SSRC session was created.
    ///
    /// 新 SSRC 会话已创建。
    SessionCreated { ssrc: u32 },
    /// A session was removed (timeout or error).
    ///
    /// 会话被移除（超时或错误）。
    SessionRemoved { ssrc: u32 },
    /// Demux event from a session's TS demuxer.
    ///
    /// 来自会话 TS 解复用器的解复用事件。
    Demux {
        ssrc: u32,
        event: Box<MpegTsDemuxEvent>,
    },
    /// Diagnostic (non-fatal).
    ///
    /// 诊断事件（非致命）。
    Diagnostic(RtpTsDiagnostic),
}

/// Detected payload type.
///
/// 探测到的负载类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadProbe {
    Ts,
    Ps,
    Es,
    Ehome,
    /// Hikvision / vendor private XHB container.
    ///
    /// 海康/厂商私有 XHB 容器。
    Xhb,
    /// JT/T 1078 vehicle terminal payload.
    ///
    /// JT/T 1078 车载终端负载。
    Jtt1078,
    Unknown,
}

/// Per-SSRC session state.
///
/// 每个 SSRC 的会话状态。
struct RtpTsSession {
    demuxer: SessionDemuxer,
    last_seq: Option<u16>,
    source_addr: Option<SocketAddr>,
    last_activity_ms: u64,
    sync_loss_count: usize,
    payload_probed: Option<PayloadProbe>,
}

/// Sans-I/O RTP-TS ingest router.
///
/// RTP-TS 摄入的 Sans-I/O 路由器。
pub struct RtpTsIngest {
    config: RtpTsIngestConfig,
    sessions: HashMap<u32, RtpTsSession>,
}

/// `RtpTsIngest` routing, session, and demuxing API.
///
/// `RtpTsIngest` 路由、会话与解复用 API。
impl RtpTsIngest {
    /// Create a new ingest router with the given configuration.
    ///
    /// 使用给定配置创建新的摄入路由器。
    pub fn new(config: RtpTsIngestConfig) -> Self {
        Self {
            config,
            sessions: HashMap::new(),
        }
    }

    /// Feed a raw UDP/TCP RTP packet and return events.
    ///
    /// 输入一个原始 UDP/TCP RTP 包并返回事件。
    /// `now_ms` 是当前的单调时间（毫秒），用于空闲跟踪。
    /// `source` 是远端地址，用于源地址变化检测。
    pub fn feed_packet(
        &mut self,
        raw: &[u8],
        source: SocketAddr,
        now_ms: u64,
    ) -> Vec<RtpTsIngestEvent> {
        let mut events = Vec::new();

        // Parse RTP header
        let Some(rtp) = RtpPacket::parse(raw) else {
            // Check if it's a version issue
            if !raw.is_empty() {
                let version = raw[0] >> 6;
                if version != 2 {
                    events.push(RtpTsIngestEvent::Diagnostic(
                        RtpTsDiagnostic::InvalidRtpVersion { version },
                    ));
                    return events;
                }
            }
            events.push(RtpTsIngestEvent::Diagnostic(
                RtpTsDiagnostic::RtpHeaderError,
            ));
            return events;
        };

        // Version check
        if rtp.header.version != 2 {
            events.push(RtpTsIngestEvent::Diagnostic(
                RtpTsDiagnostic::InvalidRtpVersion {
                    version: rtp.header.version,
                },
            ));
            return events;
        }

        // Empty payload check
        if rtp.payload.is_empty() {
            events.push(RtpTsIngestEvent::Diagnostic(
                RtpTsDiagnostic::EmptyPayload {
                    ssrc: rtp.header.ssrc,
                },
            ));
            return events;
        }

        let ssrc = rtp.header.ssrc;

        // Get or create session
        if !self.sessions.contains_key(&ssrc) {
            if self.sessions.len() >= self.config.max_sessions {
                events.push(RtpTsIngestEvent::Diagnostic(
                    RtpTsDiagnostic::SessionLimitReached { ssrc },
                ));
                return events;
            }
            self.sessions.insert(
                ssrc,
                RtpTsSession {
                    demuxer: SessionDemuxer::Pending,
                    last_seq: None,
                    source_addr: Some(source),
                    last_activity_ms: now_ms,
                    sync_loss_count: 0,
                    payload_probed: None,
                },
            );
            events.push(RtpTsIngestEvent::SessionCreated { ssrc });
        }

        let Some(session) = self.sessions.get_mut(&ssrc) else {
            return events;
        };
        session.last_activity_ms = now_ms;

        // Source address change detection
        if let Some(old_addr) = session.source_addr {
            if old_addr != source {
                events.push(RtpTsIngestEvent::Diagnostic(
                    RtpTsDiagnostic::SourceAddressChanged {
                        ssrc,
                        old: old_addr,
                        new: source,
                    },
                ));
                session.source_addr = Some(source);
            }
        } else {
            session.source_addr = Some(source);
        }

        // Sequence gap detection
        if let Some(last) = session.last_seq {
            let expected = last.wrapping_add(1);
            if rtp.header.sequence_number != expected {
                events.push(RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::SequenceGap {
                    ssrc,
                    expected,
                    got: rtp.header.sequence_number,
                }));
            }
        }
        session.last_seq = Some(rtp.header.sequence_number);

        // Probe payload type on first packet
        if session.payload_probed.is_none() || session.payload_probed == Some(PayloadProbe::Unknown)
        {
            let probe = probe_payload(&rtp.payload);
            if probe != PayloadProbe::Unknown {
                session.payload_probed = Some(probe);
            }
        }

        if session.payload_probed == Some(PayloadProbe::Unknown) {
            events.push(RtpTsIngestEvent::Diagnostic(
                RtpTsDiagnostic::UnknownPayload { ssrc },
            ));
            return events;
        }

        // Lazily initialize demuxer based on probed payload type
        if let SessionDemuxer::Pending = session.demuxer {
            match session.payload_probed {
                Some(PayloadProbe::Ts) => {
                    session.demuxer =
                        SessionDemuxer::Ts(MpegTsDemuxer::new(self.config.demux_config.clone()));
                }
                Some(PayloadProbe::Ps) => {
                    let ps_config = cheetah_codec::PsDemuxerConfig {
                        max_reassembly_bytes: self.config.demux_config.max_reassembly_bytes,
                        max_tracks: 32,
                    };
                    session.demuxer = SessionDemuxer::Ps(cheetah_codec::PsDemuxer::new(ps_config));
                }
                _ => {}
            }
        }

        // Feed payload to the correct demuxer
        let demux_events = match session.payload_probed {
            Some(PayloadProbe::Ts) => {
                Self::feed_ts_payload(session, &rtp.payload, self.config.allow_unaligned_payload)
            }
            Some(PayloadProbe::Ps) => Self::feed_ps_payload(session, &rtp.payload),
            _ => Vec::new(),
        };

        for ev in demux_events {
            events.push(RtpTsIngestEvent::Demux {
                ssrc,
                event: Box::new(ev),
            });
        }

        events
    }

    /// Check for idle sessions and remove them. Returns removal events.
    ///
    /// 检查空闲会话并移除，返回移除事件。
    pub fn check_idle(&mut self, now_ms: u64) -> Vec<RtpTsIngestEvent> {
        let mut events = Vec::new();
        let timeout = self.config.session_idle_timeout_ms;
        let expired: Vec<u32> = self
            .sessions
            .iter()
            .filter(|(_, s)| now_ms.saturating_sub(s.last_activity_ms) > timeout)
            .map(|(&ssrc, _)| ssrc)
            .collect();

        for ssrc in expired {
            if let Some(mut session) = self.sessions.remove(&ssrc) {
                // Flush demuxer
                for ev in session.demuxer.flush() {
                    events.push(RtpTsIngestEvent::Demux {
                        ssrc,
                        event: Box::new(ev),
                    });
                }
                events.push(RtpTsIngestEvent::Diagnostic(
                    RtpTsDiagnostic::SessionTimeout { ssrc },
                ));
                events.push(RtpTsIngestEvent::SessionRemoved { ssrc });
            }
        }
        events
    }

    /// Flush a specific session's demuxer and remove it.
    ///
    /// 刷新并移除指定会话的解复用器。
    pub fn remove_session(&mut self, ssrc: u32) -> Vec<RtpTsIngestEvent> {
        let mut events = Vec::new();
        if let Some(mut session) = self.sessions.remove(&ssrc) {
            for ev in session.demuxer.flush() {
                events.push(RtpTsIngestEvent::Demux {
                    ssrc,
                    event: Box::new(ev),
                });
            }
            events.push(RtpTsIngestEvent::SessionRemoved { ssrc });
        }
        events
    }

    /// Number of active sessions.
    ///
    /// 当前活动会话数。
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Feed a TS payload to the session demuxer, handling alignment and sync loss.
    ///
    /// 将 TS 负载送入会话解复用器，处理对齐与同步丢失。
    fn feed_ts_payload(
        session: &mut RtpTsSession,
        payload: &[u8],
        allow_unaligned_payload: bool,
    ) -> Vec<MpegTsDemuxEvent> {
        // Fast path: 188-aligned payload
        let events =
            if payload.len() >= TS_PACKET_SIZE && payload.len().is_multiple_of(TS_PACKET_SIZE) {
                if let SessionDemuxer::Ts(ref mut demuxer) = session.demuxer {
                    demuxer.push(payload)
                } else {
                    Vec::new()
                }
            } else if allow_unaligned_payload {
                // Compat path: find TS sync byte and feed from there
                let start = payload.iter().position(|&b| b == 0x47).unwrap_or(0);
                if let SessionDemuxer::Ts(ref mut demuxer) = session.demuxer {
                    demuxer.push(&payload[start..])
                } else {
                    Vec::new()
                }
            } else {
                return Vec::new();
            };

        // Track sync losses
        let sync_losses = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    MpegTsDemuxEvent::Diagnostic(cheetah_codec::MpegTsDemuxDiagnostic::SyncLoss)
                )
            })
            .count();
        if sync_losses > 0 {
            session.sync_loss_count += sync_losses;
        } else if !events.is_empty() {
            session.sync_loss_count = 0;
        }

        events
    }

    /// Feed a PS payload to the session demuxer and normalize its events.
    ///
    /// 将 PS 负载送入会话解复用器并归一化其事件。
    fn feed_ps_payload(session: &mut RtpTsSession, payload: &[u8]) -> Vec<MpegTsDemuxEvent> {
        let mut events = Vec::new();
        if let SessionDemuxer::Ps(ref mut demuxer) = session.demuxer {
            let ps_events = demuxer.push(payload);
            for ev in ps_events {
                match ev {
                    cheetah_codec::PsDemuxEvent::TrackInfo(tracks) => {
                        for track in tracks {
                            events.push(MpegTsDemuxEvent::TrackFound(track));
                        }
                    }
                    cheetah_codec::PsDemuxEvent::Frame(frame) => {
                        events.push(MpegTsDemuxEvent::Frame(*frame));
                    }
                    cheetah_codec::PsDemuxEvent::Diagnostic(_) => {
                        // ignore/log diagnostics
                    }
                }
            }
        }
        events
    }
}

/// Probe RTP payload to determine the container/payload mode.
///
/// 探测 RTP 负载以确定容器/负载模式。
pub fn probe_payload(payload: &[u8]) -> PayloadProbe {
    match cheetah_codec::probe_rtp_payload(payload) {
        cheetah_codec::RtpPayloadMode::Ts => PayloadProbe::Ts,
        cheetah_codec::RtpPayloadMode::Ps => PayloadProbe::Ps,
        cheetah_codec::RtpPayloadMode::Ehome => PayloadProbe::Ehome,
        cheetah_codec::RtpPayloadMode::Xhb => PayloadProbe::Xhb,
        cheetah_codec::RtpPayloadMode::Jtt1078 => PayloadProbe::Jtt1078,
        cheetah_codec::RtpPayloadMode::Es
        | cheetah_codec::RtpPayloadMode::RawAudio
        | cheetah_codec::RtpPayloadMode::RawVideo => PayloadProbe::Es,
        cheetah_codec::RtpPayloadMode::Unknown => PayloadProbe::Unknown,
    }
}

/// Per-session publish state tracker for module-level integration.
///
/// 模块级集成的每个会话发布状态跟踪器。
/// 跟踪单个 RTP-TS 会话发现的轨道与帧率估算。
pub struct RtpTsPublishSession {
    pub ssrc: u32,
    tracks: Vec<cheetah_codec::TrackInfo>,
    frame_rate_estimator: cheetah_codec::FrameRateEstimator,
    tracks_dirty: bool,
}

/// `RtpTsPublishSession` track and frame-rate tracking API.
///
/// `RtpTsPublishSession` 轨道与帧率跟踪 API。
impl RtpTsPublishSession {
    /// Create a new publish session tracker for the given SSRC.
    ///
    /// 为指定 SSRC 创建新的发布会话跟踪器。
    pub fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            tracks: Vec::new(),
            frame_rate_estimator: cheetah_codec::FrameRateEstimator::with_abl_defaults(250),
            tracks_dirty: false,
        }
    }

    /// Process a demux event and update tracks / frame-rate state.
    ///
    /// 处理解复用事件并更新轨道/帧率状态，返回轨道是否更新。
    pub fn on_demux_event(&mut self, event: &MpegTsDemuxEvent) -> bool {
        match event {
            MpegTsDemuxEvent::TrackFound(info) => {
                if !self.tracks.iter().any(|t| t.track_id == info.track_id) {
                    self.tracks.push(info.clone());
                    self.tracks_dirty = true;
                    return true;
                }
            }
            MpegTsDemuxEvent::Frame(frame)
                if frame.media_kind == cheetah_codec::MediaKind::Video =>
            {
                self.frame_rate_estimator.on_frame(frame.pts_us);
            }
            _ => {}
        }
        false
    }

    /// Take the accumulated tracks if dirty (for update_tracks call).
    ///
    /// 如果轨道列表有变化，取出并清空脏标记。
    pub fn take_tracks_if_dirty(&mut self) -> Option<&[cheetah_codec::TrackInfo]> {
        if self.tracks_dirty {
            self.tracks_dirty = false;
            Some(&self.tracks)
        } else {
            None
        }
    }

    /// Current estimated video frame rate.
    ///
    /// 当前估算的视频帧率。
    pub fn estimated_fps(&self) -> Option<f64> {
        self.frame_rate_estimator.estimated_fps()
    }

    /// Number of discovered tracks.
    ///
    /// 已发现轨道数量。
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::RtpHeader;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), port)
    }

    fn make_rtp_ts_packet(ssrc: u32, seq: u16, ts_payload: &[u8]) -> Vec<u8> {
        let header = RtpHeader {
            version: 2,
            payload_type: 33, // MP2T
            sequence_number: seq,
            timestamp: 0,
            ssrc,
            marker: false,
        };
        let mut pkt = Vec::from(&header.encode()[..]);
        pkt.extend_from_slice(ts_payload);
        pkt
    }

    #[test]
    fn rtp_ts_version_check() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());
        // Version 0 packet
        let mut pkt = vec![0u8; 20];
        pkt[0] = 0x00; // version 0
        let events = ingest.feed_packet(&pkt, addr(1234), 0);
        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::InvalidRtpVersion { version: 0 })
        )));
    }

    #[test]
    fn rtp_ts_header_too_short() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());
        let events = ingest.feed_packet(&[0x80, 0x21], addr(1234), 0);
        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::RtpHeaderError)
        )));
    }

    #[test]
    fn rtp_ts_empty_payload() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());
        let header = RtpHeader {
            version: 2,
            payload_type: 33,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 100,
            marker: false,
        };
        let pkt = header.encode();
        let events = ingest.feed_packet(&pkt, addr(1234), 0);
        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::EmptyPayload { ssrc: 100 })
        )));
    }

    #[test]
    fn rtp_ts_session_creation_and_routing() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Create a minimal TS packet (PAT-like)
        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;
        ts_pkt[1] = 0x40;
        ts_pkt[2] = 0x00;
        ts_pkt[3] = 0x10;

        let rtp = make_rtp_ts_packet(1000, 1, &ts_pkt);
        let events = ingest.feed_packet(&rtp, addr(5000), 100);

        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { ssrc: 1000 })));
        assert_eq!(ingest.session_count(), 1);

        // Second SSRC
        let rtp2 = make_rtp_ts_packet(2000, 1, &ts_pkt);
        let events2 = ingest.feed_packet(&rtp2, addr(5001), 200);
        assert!(events2
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { ssrc: 2000 })));
        assert_eq!(ingest.session_count(), 2);
    }

    #[test]
    fn rtp_ts_session_limit() {
        let config = RtpTsIngestConfig {
            max_sessions: 2,
            ..Default::default()
        };
        let mut ingest = RtpTsIngest::new(config);

        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;

        let _ = ingest.feed_packet(&make_rtp_ts_packet(1, 1, &ts_pkt), addr(1), 0);
        let _ = ingest.feed_packet(&make_rtp_ts_packet(2, 1, &ts_pkt), addr(2), 0);
        let events = ingest.feed_packet(&make_rtp_ts_packet(3, 1, &ts_pkt), addr(3), 0);

        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::SessionLimitReached { ssrc: 3 })
        )));
        assert_eq!(ingest.session_count(), 2);
    }

    #[test]
    fn rtp_ts_idle_timeout() {
        let config = RtpTsIngestConfig {
            session_idle_timeout_ms: 1000,
            ..Default::default()
        };
        let mut ingest = RtpTsIngest::new(config);

        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;

        let _ = ingest.feed_packet(&make_rtp_ts_packet(100, 1, &ts_pkt), addr(1), 0);
        assert_eq!(ingest.session_count(), 1);

        // Check at 500ms - should not timeout
        let events = ingest.check_idle(500);
        assert!(events.is_empty());
        assert_eq!(ingest.session_count(), 1);

        // Check at 1500ms - should timeout
        let events = ingest.check_idle(1500);
        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::SessionTimeout { ssrc: 100 })
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionRemoved { ssrc: 100 })));
        assert_eq!(ingest.session_count(), 0);
    }

    #[test]
    fn rtp_ts_sequence_gap_detection() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;

        let _ = ingest.feed_packet(&make_rtp_ts_packet(1, 100, &ts_pkt), addr(1), 0);
        let _ = ingest.feed_packet(&make_rtp_ts_packet(1, 101, &ts_pkt), addr(1), 0);
        // Skip seq 102
        let events = ingest.feed_packet(&make_rtp_ts_packet(1, 103, &ts_pkt), addr(1), 0);

        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::SequenceGap {
                ssrc: 1,
                expected: 102,
                got: 103,
            })
        )));
    }

    #[test]
    fn rtp_ts_source_address_change() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;

        let _ = ingest.feed_packet(&make_rtp_ts_packet(1, 1, &ts_pkt), addr(1000), 0);
        let events = ingest.feed_packet(&make_rtp_ts_packet(1, 2, &ts_pkt), addr(2000), 0);

        assert!(events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::SourceAddressChanged { ssrc: 1, .. })
        )));
    }

    #[test]
    fn rtp_ts_ps_payload_accepted() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Raw PS byte slice starting with 0x000001BA pack header (14 bytes minimum)
        let mut ps_payload = vec![0u8; 14];
        ps_payload[0..4].copy_from_slice(&[0x00, 0x00, 0x01, 0xBA]);

        let rtp = make_rtp_ts_packet(1, 1, &ps_payload);
        let events = ingest.feed_packet(&rtp, addr(1), 0);

        // Should successfully accept the PS payload and create session
        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { ssrc: 1 })));
        assert_eq!(ingest.session_count(), 1);

        // Check that it does NOT complain about unsupported PS
        assert!(!events.iter().any(|e| matches!(
            e,
            RtpTsIngestEvent::Diagnostic(RtpTsDiagnostic::UnsupportedPsPayload { .. })
        )));
    }

    #[test]
    fn rtp_ts_aligned_payload_demux() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Build valid PAT+PMT TS data
        use cheetah_codec::{
            CodecId, MediaKind, MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig, TrackId, TrackInfo,
        };
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Feed as single RTP packet (376 bytes = 2 TS packets, 188-aligned)
        let rtp = make_rtp_ts_packet(1, 1, &ts_data);
        let events = ingest.feed_packet(&rtp, addr(1), 0);

        // Should create session and find track
        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { .. })));
        let has_track = events.iter().any(|e| {
            if let RtpTsIngestEvent::Demux { event, .. } = e {
                matches!(**event, MpegTsDemuxEvent::TrackFound(_))
            } else {
                false
            }
        });
        assert!(has_track, "should find track from PAT+PMT");
    }

    #[test]
    fn rtp_ts_unaligned_payload_compat() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Build valid PAT+PMT with 5 bytes of vendor prefix
        use cheetah_codec::{
            CodecId, MediaKind, MpegTsMuxEvent, MpegTsMuxer, MpegTsMuxerConfig, TrackId, TrackInfo,
        };
        let tracks = vec![TrackInfo::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            90_000,
        )];
        let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);
        let mut ts_data = Vec::new();
        for ev in muxer.write_tables() {
            if let MpegTsMuxEvent::Packet(d) = ev {
                ts_data.extend_from_slice(&d);
            }
        }

        // Add vendor prefix before TS data
        let mut payload = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        payload.extend_from_slice(&ts_data);

        let rtp = make_rtp_ts_packet(1, 1, &payload);
        let events = ingest.feed_packet(&rtp, addr(1), 0);

        // Should still find track via resync
        let has_track = events.iter().any(|e| {
            if let RtpTsIngestEvent::Demux { event, .. } = e {
                matches!(**event, MpegTsDemuxEvent::TrackFound(_))
            } else {
                false
            }
        });
        assert!(has_track, "should find track despite vendor prefix");
    }

    #[test]
    fn probe_payload_ts() {
        let mut ts = [0u8; 376];
        ts[0] = 0x47;
        ts[188] = 0x47;
        assert_eq!(probe_payload(&ts), PayloadProbe::Ts);
    }

    #[test]
    fn probe_payload_ps() {
        let ps = [0x00, 0x00, 0x01, 0xBA, 0x00, 0x00];
        assert_eq!(probe_payload(&ps), PayloadProbe::Ps);
    }

    #[test]
    fn probe_payload_unknown() {
        let data = [0xAA, 0xBB, 0xCC, 0xDD];
        assert_eq!(probe_payload(&data), PayloadProbe::Unknown);
    }

    #[test]
    fn rtp_ts_with_csrc_and_extension() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Build RTP with 2 CSRCs and an extension
        let mut pkt = Vec::new();
        // Byte 0: V=2, P=0, X=1, CC=2
        pkt.push(0x80 | 0x10 | 0x02);
        // Byte 1: M=0, PT=33
        pkt.push(33);
        // Seq
        pkt.extend_from_slice(&1u16.to_be_bytes());
        // Timestamp
        pkt.extend_from_slice(&0u32.to_be_bytes());
        // SSRC
        pkt.extend_from_slice(&42u32.to_be_bytes());
        // 2 CSRCs
        pkt.extend_from_slice(&100u32.to_be_bytes());
        pkt.extend_from_slice(&200u32.to_be_bytes());
        // Extension header: profile=0xBEDE, length=1 word
        pkt.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // 1 word of extension data

        // TS payload (single packet)
        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;
        ts_pkt[1] = 0x40;
        ts_pkt[2] = 0x00;
        ts_pkt[3] = 0x10;
        pkt.extend_from_slice(&ts_pkt);

        let events = ingest.feed_packet(&pkt, addr(1), 0);
        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { ssrc: 42 })));
    }

    #[test]
    fn rtp_ts_with_padding() {
        let mut ingest = RtpTsIngest::new(RtpTsIngestConfig::default());

        // Build RTP with padding
        let mut pkt = Vec::new();
        // Byte 0: V=2, P=1, X=0, CC=0
        pkt.push(0x80 | 0x20);
        // Byte 1: M=0, PT=33
        pkt.push(33);
        pkt.extend_from_slice(&1u16.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes());
        pkt.extend_from_slice(&55u32.to_be_bytes());

        // TS payload
        let mut ts_pkt = [0xFF_u8; 188];
        ts_pkt[0] = 0x47;
        ts_pkt[1] = 0x40;
        ts_pkt[2] = 0x00;
        ts_pkt[3] = 0x10;
        pkt.extend_from_slice(&ts_pkt);

        // Padding: 4 bytes (last byte = padding count)
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x04]);

        let events = ingest.feed_packet(&pkt, addr(1), 0);
        assert!(events
            .iter()
            .any(|e| matches!(e, RtpTsIngestEvent::SessionCreated { ssrc: 55 })));
    }
}
