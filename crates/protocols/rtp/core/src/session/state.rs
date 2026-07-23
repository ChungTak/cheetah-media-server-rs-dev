use std::net::SocketAddr;

use cheetah_codec::{
    MpegTsDemuxer, PsDemuxer, RtpPacket, RtpPayloadMode, RtpPayloadProfile, RtpReorderBuffer,
};

use crate::rtcp_report::{default_clock_rate_hz, RtcpReportState};
use crate::types::{
    RtpSessionKey, RtpSessionState, RtpSourcePolicy, RtpTrackFilter, RtpTransportMode,
};

pub(super) enum SessionDemuxer {
    Pending,
    Ts(MpegTsDemuxer),
    Ps(Box<PsDemuxer>),
    Bypass,
}
pub(crate) struct RtpSession {
    pub(super) _session_key: RtpSessionKey,
    pub(super) ssrc: u32,
    /// Configured RTP payload type, if supplied by the caller.
    pub(super) payload_type: Option<u8>,
    /// Payload mode used to initialize the ingress demuxer.
    pub(super) payload_mode: RtpPayloadMode,
    /// Payload mode used when packetizing outbound `SendFrame` frames.
    pub(super) egress_payload_mode: RtpPayloadMode,
    pub(super) transport_mode: RtpTransportMode,
    /// Explicit runtime state for receiver/sender/talk transitions.
    pub(super) state: RtpSessionState,
    /// Filter applied to demuxed frames before they leave the core.
    pub(super) track_filter: RtpTrackFilter,
    /// Filter applied to frames fed to `SendFrame`.
    pub(super) egress_track_filter: RtpTrackFilter,

    // Ingress state
    pub(super) demuxer: SessionDemuxer,
    pub(super) last_seq: Option<u16>,
    /// Last sequence number observed on arrival, used by the rebind gate. This is
    /// updated on packet acceptance (before reorder buffering) so the continuity
    /// check operates on arrival order, not release order.
    pub(super) last_received_seq: Option<u16>,
    /// Bounded RTP reorder buffer for this session. Packets that arrive out of order
    /// are held until their predecessors arrive or a latency/packet budget is exceeded.
    /// The buffer stores `(packet, arrival_ms, source_addr)` tuples so per-packet
    /// timestamps and source addresses are preserved when buffered packets are released.
    pub(super) reorder: RtpReorderBuffer<(RtpPacket, u64, Option<SocketAddr>)>,
    /// Count of payload-mode sniff attempts for sessions created with `Unknown` mode.
    /// Scoped per session so unrelated streams do not share the budget.
    pub(super) pt_probe_attempts: u8,
    /// Candidate profile from the last sniff; committed only after repeated matches.
    pub(super) pt_pending_profile: Option<RtpPayloadProfile>,
    /// Consecutive sniff matches for the same candidate profile.
    pub(super) pt_pending_confirm_count: u8,
    /// Consecutive packets with an unresolved but different PT after the mode is locked.
    pub(super) pt_change_unknown_count: u8,
    /// Number of mid-stream payload-mode switches already performed on this session.
    pub(super) pt_format_change_count: u8,
    pub(super) source_addr: Option<SocketAddr>,
    /// Source-address binding policy for this session. Defaults to `Strict`.
    pub(super) source_policy: RtpSourcePolicy,
    /// Number of source-address packets rejected under `Strict` or a failed rebind attempt.
    pub(super) source_spoof_count: u32,
    /// Number of validated source-address rebinds performed for this session.
    pub(super) source_rebind_count: u32,
    /// Last observed RTCP source address for this peer. Separate from `source_addr` because
    /// RTCP may travel on its own UDP port or even a different address.
    pub(super) rtcp_source_addr: Option<SocketAddr>,
    pub(super) last_activity_ms: u64,

    // Egress state
    pub(super) destination: Option<SocketAddr>,
    pub(super) tcp_conn_id: Option<u64>,
    pub(super) next_seq: u16,
    /// Next RTP timestamp to use for the first packet of the next raw-audio frame.
    /// Keeps G.711 talkback timestamps continuous across frames.
    ///
    /// 下一帧原始音频第一个 RTP 包应使用的时间戳；保证 G.711 对讲跨帧连续。
    pub(super) next_timestamp: Option<u32>,
    /// Target audio packet duration in milliseconds when packetizing raw audio.
    ///
    /// 原始音频打包时的目标包时长（毫秒）。
    pub(super) packet_duration_ms: Option<u32>,
    pub(super) peer_ssrc: u32,

    // Statistics
    pub(super) packets_received: u32,
    pub(super) bytes_received: u32,
    pub(super) packets_sent: u32,
    pub(super) bytes_sent: u32,
    pub(super) last_rtcp_report_ms: u64,
    /// Last time an RTCP RR was observed for this sender; used by RR-timeout sender shutdown.
    pub(super) last_rr_received_ms: u64,
    /// When true, idle/RR timeout checks are skipped but the session keeps receiving.
    pub(super) check_paused: bool,
    /// Largest RTP payload observed on this session in bytes. Mirrors ABL's `nMaxRtpLength`
    /// dynamic learner so the driver can right-size send buffers and the module can flag
    /// pathological streams. Always bounded by the core's `max_rtp_len_cap`.
    pub(super) max_rtp_len_observed: usize,

    // Concurrency control
    /// Monotonic generation updated whenever a mutable parameter actually changes.
    /// Starts at 1 and is compared by `UpdateSession` for atomicity.
    pub(super) generation: u64,
    /// Last time a mutable parameter changed, in milliseconds, from the most recent tick.
    pub(super) updated_at_ms: u64,
    /// Optional human-readable reason for the last recorded failure.
    pub(super) last_error: Option<String>,

    /// RTCP report state for this session.
    pub(super) rtcp: RtcpReportState,
}

pub(super) fn rand_ssrc() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_be_bytes(b) & 0x7FFFFFFF
}

/// Derive an internal payload mode from an RTP payload type value.
///
/// Mirrors the heuristic used by the orchestrator so that `UpdateSession` can
/// switch the demuxer/packetizer when the payload type actually changes.
pub(super) fn payload_mode_from_payload_type(payload_type: u8) -> RtpPayloadMode {
    match payload_type {
        0 | 8 => RtpPayloadMode::RawAudio,
        33 => RtpPayloadMode::Ts,
        96..=99 => RtpPayloadMode::Es,
        _ => RtpPayloadMode::Ps,
    }
}

/// Whether the configured track filter allows a given media kind through.
pub(super) fn track_filter_allows_track(
    filter: RtpTrackFilter,
    kind: cheetah_codec::MediaKind,
) -> bool {
    match filter {
        RtpTrackFilter::All => true,
        RtpTrackFilter::OnlyAudio => matches!(kind, cheetah_codec::MediaKind::Audio),
        RtpTrackFilter::OnlyVideo => matches!(kind, cheetah_codec::MediaKind::Video),
    }
}

/// Compute the runtime state a session should move to after receiving an RTP packet,
/// based on its negotiated transport mode and current state.
pub(super) fn state_after_ingress(
    transport_mode: RtpTransportMode,
    current: RtpSessionState,
) -> Option<RtpSessionState> {
    match (transport_mode, current) {
        // Once voice talk has started, receiving more audio keeps it in Talk.
        (RtpTransportMode::SendRecv, RtpSessionState::Talk) => None,
        // A RecvOnly session that starts seeing packets becomes a receiver.
        (RtpTransportMode::RecvOnly, RtpSessionState::Inactive)
        | (RtpTransportMode::RecvOnly, RtpSessionState::Receiving) => {
            Some(RtpSessionState::Receiving)
        }
        // A SendRecv session moves to bidirectional state on first ingress.
        (RtpTransportMode::SendRecv, RtpSessionState::Inactive)
        | (RtpTransportMode::SendRecv, RtpSessionState::Receiving)
        | (RtpTransportMode::SendRecv, RtpSessionState::Sending) => Some(RtpSessionState::SendRecv),
        _ => None,
    }
}

/// Compute the runtime state a session should move to after a SendFrame, based on
/// its negotiated transport mode and current state.
pub(super) fn state_after_egress(
    transport_mode: RtpTransportMode,
    current: RtpSessionState,
    is_talk: bool,
) -> Option<RtpSessionState> {
    if is_talk {
        return if current == RtpSessionState::Talk {
            None
        } else {
            Some(RtpSessionState::Talk)
        };
    }
    match (transport_mode, current) {
        (RtpTransportMode::SendOnly, RtpSessionState::Inactive)
        | (RtpTransportMode::SendOnly, RtpSessionState::Sending) => Some(RtpSessionState::Sending),
        (RtpTransportMode::SendRecv, RtpSessionState::Inactive)
        | (RtpTransportMode::SendRecv, RtpSessionState::Receiving)
        | (RtpTransportMode::SendRecv, RtpSessionState::Sending) => Some(RtpSessionState::SendRecv),
        _ => None,
    }
}

/// Commit a resolved payload profile to a session, resetting the demuxer and PT state
/// so the next packet is parsed under the new mode.
pub(super) fn commit_payload_profile(session: &mut RtpSession, pt: u8, profile: RtpPayloadProfile) {
    session.payload_type = Some(pt);
    session.payload_mode = profile.mode;
    session.egress_payload_mode = profile.mode;
    session.demuxer = SessionDemuxer::Pending;
    session.pt_change_unknown_count = 0;
    session
        .rtcp
        .set_clock_rate_hz(default_clock_rate_hz(profile.mode));
}

impl RtpSession {
    /// Attempt to move to `new_state`. Returns the previous state when a real transition
    /// happened; returns `None` if the session is already in the target state or has
    /// already reached a terminal state.
    pub(super) fn transition_to(&mut self, new_state: RtpSessionState) -> Option<RtpSessionState> {
        if self.state == new_state || self.state == RtpSessionState::Closed {
            return None;
        }
        let old = self.state;
        self.state = new_state;
        Some(old)
    }
}
