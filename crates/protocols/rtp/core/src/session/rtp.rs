use std::net::SocketAddr;

use cheetah_codec::{
    probe_rtp_payload, MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig, PsDemuxEvent,
    PsDemuxer, PsDemuxerConfig, RtpPacket, RtpPayloadMode, RtpPayloadProfile,
};

use crate::error::RtpCoreDiagnostic;
use crate::rtcp_report::{default_clock_rate_hz, RtcpReportState};
use crate::types::*;

use super::{state::*, RtpCore};

impl RtpCore {
    pub(super) fn feed_rtp_packet(
        &mut self,
        rtp: RtpPacket,
        source_addr: Option<SocketAddr>,
        tcp_conn_id: Option<u64>,
        received_at_ms: u64,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        if rtp.header.version != 2 {
            outputs.push(RtpCoreOutput::Diagnostic(
                RtpCoreDiagnostic::InvalidRtpVersion {
                    version: rtp.header.version,
                },
            ));
            return;
        }

        if rtp.payload.is_empty() {
            outputs.push(RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::EmptyPayload {
                ssrc: rtp.header.ssrc,
            }));
            return;
        }

        let mut skip_demuxer = false;
        let ssrc = rtp.header.ssrc;

        // Find session by SSRC
        let mut created = false;
        let session_key = if let Some(key) = self.ssrc_to_session.get(&ssrc) {
            key.clone()
        } else {
            // Unmapped SSRC: Auto-create session
            if self.sessions.len() >= self.max_sessions {
                outputs.push(RtpCoreOutput::Diagnostic(
                    RtpCoreDiagnostic::UnknownPayload { ssrc },
                ));
                return;
            }

            let key = format!("live/{ssrc}");
            let mode = probe_rtp_payload(&rtp.payload);
            // If `probe_rtp_payload` cannot determine the mode, leave it `Unknown`; the
            // `pt_resolver` will attempt a static mapping and bounded sniff on the first packet.

            let session = RtpSession {
                _session_key: key.clone(),
                ssrc,
                payload_type: None,
                payload_mode: mode,
                egress_payload_mode: mode,
                transport_mode: RtpTransportMode::RecvOnly,
                track_filter: RtpTrackFilter::All,
                egress_track_filter: RtpTrackFilter::All,
                check_paused: false,
                demuxer: SessionDemuxer::Pending,
                last_seq: None,
                source_addr,
                rtcp_source_addr: None,
                last_activity_ms: 0, // Will be updated
                destination: None,
                tcp_conn_id,
                next_seq: 0,
                peer_ssrc: ssrc,
                packets_received: 0,
                bytes_received: 0,
                packets_sent: 0,
                bytes_sent: 0,
                last_rtcp_report_ms: 0,
                last_rr_received_ms: 0,
                max_rtp_len_observed: 0,
                generation: 1,
                updated_at_ms: 0,
                pt_probe_attempts: 0,
                pt_pending_profile: None,
                pt_pending_confirm_count: 0,
                pt_change_unknown_count: 0,
                pt_format_change_count: 0,
                last_error: None,
                rtcp: RtcpReportState::new(default_clock_rate_hz(mode)),
            };

            self.sessions.insert(key.clone(), session);
            self.ssrc_to_session.insert(ssrc, key.clone());
            if let Some(conn_id) = tcp_conn_id {
                self.tcp_conn_to_session.insert(conn_id, key.clone());
            }

            created = true;
            key
        };

        let Some(session) = self.sessions.get_mut(&session_key) else {
            return;
        };

        // Resolve the payload mode for sessions created without an explicit mode.
        // Order: external binding (set via spec/UpdateSession), static PT table,
        // then payload sniff. The sniff budget is per-session so one stream cannot
        // exhaust it for others.
        let mut format_change = None;
        let mut close_reason = None;
        let commit_profile = |session: &mut RtpSession, pt: u8, profile: RtpPayloadProfile| {
            session.payload_type = Some(pt);
            session.payload_mode = profile.mode;
            session.egress_payload_mode = profile.mode;
            session.demuxer = SessionDemuxer::Pending;
            session.pt_change_unknown_count = 0;
            session
                .rtcp
                .set_clock_rate_hz(default_clock_rate_hz(profile.mode));
        };

        if session.payload_mode == RtpPayloadMode::Unknown {
            if session.pt_probe_attempts < self.max_pt_probe_packets {
                session.pt_probe_attempts += 1;
                match self
                    .pt_resolver
                    .resolve_with_source(rtp.header.payload_type, &rtp.payload)
                {
                    cheetah_codec::RtpPtResolveSource::Binding(profile)
                    | cheetah_codec::RtpPtResolveSource::Static(profile)
                    | cheetah_codec::RtpPtResolveSource::Encapsulation(profile) => {
                        // Authoritative: external binding, static table, or container
                        // sync/pack header (PS/TS/JTT/Ehome). Treat as immediately confirmed.
                        session.pt_pending_profile = Some(profile);
                        session.pt_pending_confirm_count = self.pt_lock_confidence;
                        commit_profile(session, rtp.header.payload_type, profile);
                    }
                    cheetah_codec::RtpPtResolveSource::Weak(profile) => {
                        // Weak pattern-based sniff (Annex-B start code / AAC ADTS) requires
                        // `pt_lock_confidence` consecutive matching packets before committing,
                        // so a single false-positive inside a PS/TS fragment cannot mis-route
                        // the stream.
                        if session.pt_pending_profile == Some(profile) {
                            session.pt_pending_confirm_count += 1;
                        } else {
                            session.pt_pending_profile = Some(profile);
                            session.pt_pending_confirm_count = 1;
                        }
                        if session.pt_pending_confirm_count >= self.pt_lock_confidence {
                            commit_profile(session, rtp.header.payload_type, profile);
                        }
                    }
                    cheetah_codec::RtpPtResolveSource::Unknown => {
                        // No recognizable signal this packet; drop any pending weak candidate
                        // so confirmation only counts consecutive matches.
                        session.pt_pending_profile = None;
                        session.pt_pending_confirm_count = 0;
                    }
                }
            }
            // If the mode is still unknown after exhausting the per-session sniff budget,
            // fall back to PS for GB28181 compatibility rather than leaving the stream on
            // the no-op ES demuxer.
            if session.payload_mode == RtpPayloadMode::Unknown
                && session.pt_probe_attempts >= self.max_pt_probe_packets
            {
                session.payload_mode = RtpPayloadMode::Ps;
                session.egress_payload_mode = RtpPayloadMode::Ps;
                session.demuxer = SessionDemuxer::Pending;
                session
                    .rtcp
                    .set_clock_rate_hz(default_clock_rate_hz(RtpPayloadMode::Ps));
            }
        } else {
            // The payload mode is already locked. Accept the first observed PT if none was
            // recorded, and react to mid-stream PT changes. Unresolved transient PTs (e.g.
            // RFC 4733 telephone-event or FEC/RED sharing the same SSRC) are tolerated up to a
            // dedicated per-session budget (max_tolerated_unknown_pt_packets) consecutive
            // packets before the session is closed.
            let current_pt = session.payload_type.unwrap_or(rtp.header.payload_type);
            if session.payload_type.is_none() {
                session.payload_type = Some(rtp.header.payload_type);
            } else if rtp.header.payload_type == current_pt {
                session.pt_change_unknown_count = 0;
            } else {
                let new_pt = rtp.header.payload_type;
                match self.pt_resolver.resolve_with_source(new_pt, &rtp.payload) {
                    cheetah_codec::RtpPtResolveSource::Binding(profile)
                    | cheetah_codec::RtpPtResolveSource::Static(profile)
                    | cheetah_codec::RtpPtResolveSource::Encapsulation(profile) => {
                        session.pt_change_unknown_count = 0;
                        if profile.mode != session.payload_mode {
                            session.pt_format_change_count += 1;
                            if session.pt_format_change_count > self.max_pt_format_changes {
                                close_reason = Some(format!(
                                    "payload mode oscillated from {payload_mode:?} to {new_mode:?} more than {max} times",
                                    payload_mode = session.payload_mode,
                                    new_mode = profile.mode,
                                    max = self.max_pt_format_changes,
                                ));
                            } else {
                                let old_mode = session.payload_mode;
                                commit_profile(session, new_pt, profile);
                                format_change = Some(RtpCoreEvent::FormatChanged {
                                    session_key: session_key.clone(),
                                    payload_type: new_pt,
                                    old_payload_mode: old_mode,
                                    new_payload_mode: profile.mode,
                                });
                            }
                        } else {
                            session.payload_type = Some(new_pt);
                        }
                    }
                    cheetah_codec::RtpPtResolveSource::Weak(_)
                    | cheetah_codec::RtpPtResolveSource::Unknown => {
                        session.pt_change_unknown_count += 1;
                        // Tolerate a run of unresolved PT packets (e.g. RFC 4733
                        // telephone-event or FEC/RED interleaved on the same SSRC) up to a
                        // dedicated budget before treating the change as a persistent spoof.
                        if session.pt_change_unknown_count >= self.max_tolerated_unknown_pt_packets
                        {
                            close_reason = Some(format!(
                                "payload type changed from {current_pt} to {new_pt} and could not be resolved for {} packets",
                                session.pt_change_unknown_count
                            ));
                        } else {
                            // Treat as a transient interleaved auxiliary payload; do not feed
                            // the unresolved bytes into the demuxer, but still account for it in
                            // sequence/RTCP statistics.
                            skip_demuxer = true;
                        }
                    }
                }
            }
        }

        if let Some(ev) = format_change {
            outputs.push(RtpCoreOutput::Event(ev));
        }
        if let Some(reason) = close_reason {
            self.close_session(session_key, reason, outputs);
            return;
        }

        // Update stats and activity
        session.packets_received += 1;
        session.bytes_received += rtp.payload.len() as u32;
        session.last_activity_ms = received_at_ms;
        session.rtcp.on_packet(
            rtp.header.sequence_number,
            rtp.header.timestamp,
            received_at_ms,
        );

        // Dynamic max-RTP-length learner (ABL `nMaxRtpLength`). Track the largest payload
        // observed; if it exceeds the configured cap, emit a diagnostic but still process the
        // packet — dropping was the historical wrong choice and broke real Hikvision feeds.
        let payload_len = rtp.payload.len();
        if payload_len > session.max_rtp_len_observed {
            session.max_rtp_len_observed = payload_len.min(self.max_rtp_len_cap);
            if payload_len > self.max_rtp_len_cap {
                outputs.push(RtpCoreOutput::Diagnostic(
                    RtpCoreDiagnostic::OversizedPayload {
                        ssrc,
                        len: payload_len,
                        cap: self.max_rtp_len_cap,
                    },
                ));
            }
        }

        if created {
            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
                session_key: session_key.clone(),
                ssrc,
                payload_mode: session.payload_mode,
                transport_mode: session.transport_mode,
            }));
        }

        // Check address change
        if let Some(src) = source_addr {
            if let Some(old) = session.source_addr {
                if old != src {
                    outputs.push(RtpCoreOutput::Diagnostic(
                        RtpCoreDiagnostic::SourceAddressChanged {
                            ssrc,
                            old,
                            new: src,
                        },
                    ));
                    session.source_addr = Some(src);
                }
            } else {
                session.source_addr = Some(src);
            }
        }

        // Sequence check
        if let Some(last) = session.last_seq {
            let expected = last.wrapping_add(1);
            if rtp.header.sequence_number != expected {
                outputs.push(RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SequenceGap {
                    ssrc,
                    expected,
                    got: rtp.header.sequence_number,
                }));
            }
        }
        session.last_seq = Some(rtp.header.sequence_number);

        if skip_demuxer {
            return;
        }

        // Lazily build demuxer
        if let SessionDemuxer::Pending = session.demuxer {
            match session.payload_mode {
                RtpPayloadMode::Ts => {
                    session.demuxer = SessionDemuxer::Ts(MpegTsDemuxer::new(MpegTsDemuxerConfig {
                        max_reassembly_bytes: 4 * 1024 * 1024,
                        strict_crc: false,
                    }));
                }
                RtpPayloadMode::Ps => {
                    session.demuxer = SessionDemuxer::Ps(Box::new(PsDemuxer::new(
                        PsDemuxerConfig::new(4 * 1024 * 1024, 8),
                    )));
                }
                RtpPayloadMode::Es => {
                    // ES depacketization is implemented in a follow-up PR.
                    session.demuxer = SessionDemuxer::Bypass;
                }
                _ => {
                    session.demuxer = SessionDemuxer::Bypass;
                }
            }
        }

        // Feed to demuxers
        let track_filter = session.track_filter;
        let source_addr = session.source_addr;
        match &mut session.demuxer {
            SessionDemuxer::Ts(demuxer) => {
                let demux_events = demuxer.push(&rtp.payload);
                outputs.reserve(demux_events.len());
                for ev in demux_events {
                    match ev {
                        MpegTsDemuxEvent::TrackFound(track)
                            if track_filter_allows_track(track_filter, track.media_kind) =>
                        {
                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::TrackFound {
                                session_key: session_key.clone(),
                                tracks: vec![track],
                            }));
                        }
                        MpegTsDemuxEvent::Frame(frame)
                            if track_filter_allows_track(track_filter, frame.media_kind) =>
                        {
                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::Frame {
                                session_key: session_key.clone(),
                                frame,
                                source_addr,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            SessionDemuxer::Ps(demuxer) => {
                let demux_events = demuxer.push(&rtp.payload);
                outputs.reserve(demux_events.len());
                for ev in demux_events {
                    match ev {
                        PsDemuxEvent::TrackInfo(tracks) => {
                            let filtered: Vec<_> = tracks
                                .into_iter()
                                .filter(|t| track_filter_allows_track(track_filter, t.media_kind))
                                .collect();
                            if !filtered.is_empty() {
                                outputs.push(RtpCoreOutput::Event(RtpCoreEvent::TrackFound {
                                    session_key: session_key.clone(),
                                    tracks: filtered,
                                }));
                            }
                        }
                        PsDemuxEvent::Frame(frame)
                            if track_filter_allows_track(track_filter, frame.media_kind) =>
                        {
                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::Frame {
                                session_key: session_key.clone(),
                                frame: *frame,
                                source_addr,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            SessionDemuxer::Bypass | SessionDemuxer::Pending => {
                // Raw audio/video or unrecognized modes are bridged in later module stages.
            }
        }
    }
}
