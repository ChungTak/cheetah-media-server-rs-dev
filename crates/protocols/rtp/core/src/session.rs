use std::collections::HashMap;

#[cfg(not(test))]
use crate::types::{RtpCoreInput, RtpCoreOutput, RtpSessionKey};

#[cfg(test)]
pub(crate) use crate::types::*;
#[cfg(test)]
pub(crate) use cheetah_codec::RtpPayloadMode;

mod command;
mod ingress;
mod rtp;
mod state;
mod tcp;
mod tick;

#[cfg(test)]
mod tests;

pub(crate) use state::RtpSession;

/// Sans-I/O state machine for one or more RTP/RTCP sessions.
///
/// This core dispatches UDP/TCP/RTCP inputs, maintains per-session state, and emits
/// outputs for the driver to send. It never performs I/O or reads the system clock.
///
/// 一个或多个 RTP/RTCP 会话的 Sans-I/O 状态机。
///
/// 该 core 分发 UDP/TCP/RTCP 输入、维护每会话状态，并产生输出供 driver 发送。
/// 它从不执行 I/O 或读取系统时钟。
pub struct RtpCore {
    pub(super) sessions: HashMap<RtpSessionKey, RtpSession>,
    pub(super) ssrc_to_session: HashMap<u32, RtpSessionKey>,
    pub(super) tcp_conn_to_session: HashMap<u64, RtpSessionKey>,
    pub(super) ehome_decoders: HashMap<u64, cheetah_codec::EhomeDecoder>,
    /// Payload-type resolver: binding -> static table -> payload sniff.
    pub(super) pt_resolver: cheetah_codec::RtpPtResolver,
    pub(super) max_sessions: usize,
    pub(super) session_idle_timeout_ms: u64,
    /// Per-session budget for payload-mode sniff when the mode is `Unknown`.
    pub(super) max_pt_probe_packets: u8,
    /// Number of consecutive matching sniff results required before locking a dynamic PT.
    pub(super) pt_lock_confidence: u8,
    /// Maximum number of tolerated mid-stream payload-mode switches before treating the
    /// stream as oscillating/spoofed and closing it.
    pub(super) max_pt_format_changes: u8,
    /// Per-session budget for consecutive unresolved PT packets on a locked session.
    /// DTMF/FEC/RED bursts may be longer than the sniff budget, so this is decoupled
    /// from `max_pt_probe_packets` to avoid closing legitimate streams.
    pub(super) max_tolerated_unknown_pt_packets: u8,
    /// Minimum idle time (ms) before `AllowValidatedRebind` will consider a new source.
    pub(super) source_rebind_idle_window_ms: u64,
    /// Maximum number of validated source rebinds allowed per session.
    pub(super) max_source_rebinds: u32,
    /// Interval between RTCP sender/receiver reports in milliseconds.
    pub(super) rtcp_report_interval_ms: u64,
    /// Wall-clock offset in milliseconds added to monotonic `now_ms` when producing
    /// outbound Sender Report NTP timestamps.
    pub(super) wall_clock_offset_ms: u64,
    pub(super) now_ms: u64,
    /// TCP framing mode applied when deframing inbound RTP-over-TCP traffic. Defaults to
    /// `AutoDetect`, matching ABLMediaServer's behaviour of accepting both 2-byte length-prefix
    /// (`enable_tcp`) and 4-byte interleaved (`$ + channel + length`) frames on the same socket.
    pub(super) tcp_framing: cheetah_codec::RtpTcpFraming,
    /// Hard upper bound on the dynamic `max_rtp_len_observed` learner. Payloads larger than
    /// this are still routed (we don't drop them) but produce an `OversizedPayload` diagnostic
    /// so operators can spot pathological streams.
    pub(super) max_rtp_len_cap: usize,
}

impl RtpCore {
    /// Create a new `RtpCore` with the given session limits and idle timeout.
    ///
    /// `session_idle_timeout_ms` is used for both idle and RR-timeout checks.
    ///
    /// 使用指定的会话限制和空闲超时创建新的 `RtpCore`。
    ///
    /// `session_idle_timeout_ms` 同时用于空闲超时和 RR 超时检查。
    pub fn new(max_sessions: usize, session_idle_timeout_ms: u64) -> Self {
        Self {
            sessions: HashMap::new(),
            ssrc_to_session: HashMap::new(),
            tcp_conn_to_session: HashMap::new(),
            ehome_decoders: HashMap::new(),
            pt_resolver: cheetah_codec::RtpPtResolver::new(),
            max_sessions,
            session_idle_timeout_ms,
            max_pt_probe_packets: 8,
            pt_lock_confidence: 2,
            max_pt_format_changes: 3,
            max_tolerated_unknown_pt_packets: 255,
            source_rebind_idle_window_ms: 1_000,
            max_source_rebinds: 10,
            rtcp_report_interval_ms: 5_000,
            wall_clock_offset_ms: 0,
            now_ms: 0,
            tcp_framing: cheetah_codec::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
        }
    }

    /// Override the default TCP framing mode (defaults to `AutoDetect`).
    ///
    /// `AutoDetect` accepts both RFC 4571 2-byte length-prefix and RTSP-style
    /// interleaved (`$ + channel + length`) frames on the same connection.
    ///
    /// 覆盖默认 TCP 分帧模式（默认为 `AutoDetect`）。
    ///
    /// `AutoDetect` 允许同一条连接上同时接受 RFC 4571 2 字节长度前缀和 RTSP 风格
    /// 交错帧（`$ + channel + length`）。
    pub fn set_tcp_framing(&mut self, framing: cheetah_codec::RtpTcpFraming) {
        self.tcp_framing = framing;
    }

    /// Override the RTCP sender/receiver report interval (defaults to 5 seconds).
    pub fn set_rtcp_report_interval_ms(&mut self, ms: u64) {
        self.rtcp_report_interval_ms = ms.max(1);
    }

    /// Override the wall-clock offset used for outbound Sender Report NTP timestamps.
    /// Drivers inject this because core is Sans-I/O and cannot read the system clock.
    pub fn set_wall_clock_offset_ms(&mut self, offset_ms: u64) {
        self.wall_clock_offset_ms = offset_ms;
        for session in self.sessions.values_mut() {
            session.rtcp.set_wall_clock_offset_ms(offset_ms);
        }
    }

    /// Override the dynamic max-RTP-length cap (defaults to 65 536 bytes).
    ///
    /// The cap is clamped to at least 1500 bytes. Payloads larger than the cap still
    /// flow through but produce an `OversizedPayload` diagnostic.
    ///
    /// 覆盖动态最大 RTP 长度上限（默认 65536 字节）。
    ///
    /// 上限至少被限制为 1500 字节。超过上限的负载仍会继续流通，但会触发
    /// `OversizedPayload` 诊断。
    pub fn set_max_rtp_len_cap(&mut self, cap: usize) {
        self.max_rtp_len_cap = cap.max(1500);
    }

    /// Override the number of consecutive matching sniff results required before a dynamic
    /// PT is locked (defaults to 2).
    ///
    /// 覆盖动态 PT 锁定所需的连续匹配次数（默认 2）。
    pub fn set_pt_lock_confidence(&mut self, confidence: u8) {
        self.pt_lock_confidence = confidence.max(1);
    }

    /// Override the default budget for consecutive unresolved PT packets on a locked session.
    pub fn set_max_tolerated_unknown_pt_packets(&mut self, max: u8) {
        self.max_tolerated_unknown_pt_packets = max.max(1);
    }

    /// Override the minimum idle time (ms) before a validated source rebind is allowed.
    pub fn set_source_rebind_idle_window_ms(&mut self, ms: u64) {
        self.source_rebind_idle_window_ms = ms.max(1);
    }

    /// Override the maximum number of validated source rebinds per session.
    pub fn set_max_source_rebinds(&mut self, max: u32) {
        self.max_source_rebinds = max.max(1);
    }

    /// Main Sans-I/O entry point. Drive the state machine with one input and return the
    /// resulting outputs for the caller to execute.
    ///
    /// Routing:
    /// - `UdpPacket` / `TcpBytes` / `RtcpPacket` are parsed and dispatched to the
    ///   matching session.
    /// - `Tick` updates the internal clock and runs idle/RR-timeout plus RTCP report
    ///   generation.
    /// - `Command` creates, configures, or stops sessions.
    ///
    /// `RtpCore` 的主 Sans-I/O 入口。用单个输入驱动状态机并返回由调用方执行的输出。
    ///
    /// 路由规则：
    /// - `UdpPacket` / `TcpBytes` / `RtcpPacket` 被解析并分派到匹配会话。
    /// - `Tick` 更新内部时钟，运行空闲/RR 超时与 RTCP 报告生成。
    /// - `Command` 创建、配置或停止会话。
    pub fn handle_input(&mut self, input: RtpCoreInput) -> Vec<RtpCoreOutput> {
        let mut outputs = Vec::with_capacity(4);
        match input {
            RtpCoreInput::UdpPacket(datagram) => {
                self.process_udp_packet(datagram, &mut outputs);
            }
            RtpCoreInput::TcpBytes(chunk) => {
                self.process_tcp_bytes(chunk, &mut outputs);
            }
            RtpCoreInput::TcpConnectionClosed { conn_id, .. } => {
                self.process_tcp_connection_closed(conn_id, &mut outputs);
            }
            RtpCoreInput::RtcpPacket(datagram) => {
                self.process_rtcp_packet(datagram, &mut outputs);
            }
            RtpCoreInput::Tick { now_ms } => {
                self.process_tick(now_ms, &mut outputs);
            }
            RtpCoreInput::Command(cmd) => {
                self.process_command(cmd, &mut outputs);
            }
        }
        outputs
    }
}
