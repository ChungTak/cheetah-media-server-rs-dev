use std::net::SocketAddr;

use cheetah_codec::{MpegTsDemuxer, PsDemuxer, RtpPayloadMode, RtpPayloadProfile};

use crate::rtcp_report::RtcpReportState;
use crate::types::{RtpSessionKey, RtpTrackFilter, RtpTransportMode};

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
    /// Filter applied to demuxed frames before they leave the core.
    pub(super) track_filter: RtpTrackFilter,
    /// Filter applied to frames fed to `SendFrame`.
    pub(super) egress_track_filter: RtpTrackFilter,

    // Ingress state
    pub(super) demuxer: SessionDemuxer,
    pub(super) last_seq: Option<u16>,
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
    /// Last observed RTCP source address for this peer. Separate from `source_addr` because
    /// RTCP may travel on its own UDP port or even a different address.
    pub(super) rtcp_source_addr: Option<SocketAddr>,
    pub(super) last_activity_ms: u64,

    // Egress state
    pub(super) destination: Option<SocketAddr>,
    pub(super) tcp_conn_id: Option<u64>,
    pub(super) next_seq: u16,
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
