use cheetah_codec::{RtpHeader, RtpPayloadMode};

use crate::rtcp_report::{default_clock_rate_hz, RtcpReportState};
use crate::types::*;

use super::{state::*, RtpCore};

impl RtpCore {
    pub(super) fn handle_update_session(
        &mut self,
        session_key: RtpSessionKey,
        expected_generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        let Some(mut session) = self.sessions.remove(&session_key) else {
            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason: "session not found".to_string(),
            }));
            return;
        };

        if ssrc.is_none() && payload_type.is_none() && pause_check.is_none() {
            self.sessions.insert(session_key.clone(), session);
            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason: "empty patch".to_string(),
            }));
            return;
        }

        if session.generation != expected_generation {
            self.sessions.insert(session_key.clone(), session);
            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason: "generation mismatch".to_string(),
            }));
            return;
        }

        let mut changed = false;
        let mut result_ssrc: Option<u32> = None;
        if let Some(new_ssrc) = ssrc {
            if new_ssrc != session.ssrc {
                if let Some(existing_key) = self.ssrc_to_session.get(&new_ssrc) {
                    if existing_key != &session_key {
                        self.sessions.insert(session_key.clone(), session);
                        outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                            session_key,
                            reason: format!("ssrc {new_ssrc} already in use"),
                        }));
                        return;
                    }
                }
                self.ssrc_to_session.remove(&session.ssrc);
                self.ssrc_to_session.insert(new_ssrc, session_key.clone());
                session.ssrc = new_ssrc;
                session.peer_ssrc = new_ssrc;
                changed = true;
                result_ssrc = Some(new_ssrc);
            }
        }

        if let Some(new_payload_type) = payload_type {
            if session.payload_type != Some(new_payload_type) {
                session.payload_type = Some(new_payload_type);
                changed = true;
            }
            let new_mode = payload_mode_from_payload_type(new_payload_type);
            if session.payload_mode != new_mode {
                session.payload_mode = new_mode;
                session.egress_payload_mode = new_mode;
                session.demuxer = SessionDemuxer::Pending;
                changed = true;
            }
        }

        let mut result_pause: Option<bool> = None;
        if let Some(paused) = pause_check {
            if session.check_paused != paused {
                session.check_paused = paused;
                changed = true;
                result_pause = Some(paused);
                if !paused {
                    session.last_activity_ms = self.now_ms;
                    session.last_rr_received_ms = self.now_ms;
                }
            }
        }

        if changed {
            session.generation += 1;
            session.updated_at_ms = self.now_ms;
        }
        session.last_error = None;

        let current_generation = session.generation;
        self.sessions.insert(session_key.clone(), session);
        outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
            session_key,
            generation: current_generation,
            ssrc: result_ssrc,
            payload_type,
            pause_check: result_pause,
        }));
    }

    pub(super) fn close_session(
        &mut self,
        key: RtpSessionKey,
        reason: String,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        if let Some(mut session) = self.sessions.remove(&key) {
            // Mark terminal state before dropping; the public `SessionClosed` event is the
            // authoritative lifecycle signal, so we do not emit a redundant state change.
            let _ = session.transition_to(RtpSessionState::Closed);
            self.ssrc_to_session.remove(&session.ssrc);
            if let Some(conn_id) = session.tcp_conn_id {
                self.tcp_conn_to_session.remove(&conn_id);
                self.ehome_decoders.remove(&conn_id);
            }
            outputs.push(RtpCoreOutput::CloseSession(key.clone()));
            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionClosed {
                session_key: key,
                reason,
            }));
        }
    }

    pub(super) fn process_command(
        &mut self,
        cmd: RtpCoreCommand,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        match cmd {
            RtpCoreCommand::CreateServer(spec) => {
                if self.sessions.contains_key(&spec.session_key) {
                    return;
                }
                let ssrc = spec.ssrc.unwrap_or(rand_ssrc());
                let track_filter = spec.track_filter;
                let session = RtpSession {
                    _session_key: spec.session_key.clone(),
                    ssrc,
                    payload_type: None,
                    payload_mode: spec.payload_mode,
                    egress_payload_mode: spec.payload_mode,
                    transport_mode: spec.transport_mode,
                    state: RtpSessionState::Inactive,
                    track_filter,
                    egress_track_filter: spec.track_filter,
                    check_paused: false,
                    demuxer: SessionDemuxer::Pending,
                    last_seq: None,
                    source_addr: None,
                    rtcp_source_addr: None,
                    last_activity_ms: 0,
                    destination: None,
                    tcp_conn_id: None,
                    next_seq: 1,
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
                    rtcp: RtcpReportState::new(default_clock_rate_hz(spec.payload_mode)),
                };
                self.sessions.insert(spec.session_key.clone(), session);
                self.ssrc_to_session.insert(ssrc, spec.session_key.clone());
                outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
                    session_key: spec.session_key,
                    ssrc,
                    payload_mode: spec.payload_mode,
                    transport_mode: spec.transport_mode,
                }));
            }
            RtpCoreCommand::CreateClient(spec) => {
                if let Some(session) = self.sessions.get_mut(&spec.session_key) {
                    // Active TCP connect for an existing server/receiver session: attach the new
                    // TCP connection id and destination so ingress can flow on it.
                    if let Some(conn_id) = spec.tcp_conn_id {
                        if let Some(old) = session.tcp_conn_id {
                            self.tcp_conn_to_session.remove(&old);
                        }
                        session.tcp_conn_id = Some(conn_id);
                        self.tcp_conn_to_session
                            .insert(conn_id, spec.session_key.clone());
                    }
                    if session.destination.is_none() {
                        session.destination = Some(spec.destination);
                    }
                    // VoiceTalk upgrades an existing inbound session to SendRecv and locks
                    // egress to audio so the same socket can push talkback audio back.
                    if spec.connection_type == Some(RtpConnectionType::VoiceTalk) {
                        session.transport_mode = spec.transport_mode;
                        session.egress_track_filter = spec.track_filter;
                        session.egress_payload_mode = spec.payload_mode;
                        session.destination = Some(spec.destination);
                        if let Some(old) = session.transition_to(RtpSessionState::Talk) {
                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged {
                                session_key: spec.session_key.clone(),
                                old_state: old,
                                new_state: RtpSessionState::Talk,
                            }));
                        }
                    }
                    return;
                }
                let track_filter = spec.track_filter;
                let session = RtpSession {
                    _session_key: spec.session_key.clone(),
                    ssrc: spec.ssrc,
                    payload_type: None,
                    payload_mode: spec.payload_mode,
                    egress_payload_mode: spec.payload_mode,
                    transport_mode: spec.transport_mode,
                    state: RtpSessionState::Inactive,
                    track_filter,
                    egress_track_filter: spec.track_filter,
                    check_paused: false,
                    demuxer: SessionDemuxer::Pending,
                    last_seq: None,
                    source_addr: None,
                    rtcp_source_addr: None,
                    last_activity_ms: 0,
                    destination: Some(spec.destination),
                    tcp_conn_id: spec.tcp_conn_id,
                    next_seq: 1,
                    peer_ssrc: spec.ssrc,
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
                    rtcp: RtcpReportState::new(default_clock_rate_hz(spec.payload_mode)),
                };
                self.sessions.insert(spec.session_key.clone(), session);
                self.ssrc_to_session
                    .insert(spec.ssrc, spec.session_key.clone());
                if let Some(conn_id) = spec.tcp_conn_id {
                    self.tcp_conn_to_session
                        .insert(conn_id, spec.session_key.clone());
                }
                outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
                    session_key: spec.session_key,
                    ssrc: spec.ssrc,
                    payload_mode: spec.payload_mode,
                    transport_mode: spec.transport_mode,
                }));
            }
            RtpCoreCommand::SendFrame(send_frame) => {
                if let Some(session) = self.sessions.get_mut(&send_frame.session_key) {
                    if session.transport_mode == RtpTransportMode::RecvOnly {
                        return;
                    }
                    if !track_filter_allows_track(
                        session.egress_track_filter,
                        send_frame.frame.media_kind,
                    ) {
                        return;
                    }

                    // Mux frame data or directly packetize if it is raw payload bytes
                    let payload = &send_frame.frame.payload;
                    if payload.is_empty() {
                        return;
                    }

                    // RTP clock-rate selection. Video defaults to 90kHz; audio uses the
                    // frame timebase denominator when available, otherwise falls back to 8kHz.
                    let clock_rate: u32 = match send_frame.frame.media_kind {
                        cheetah_codec::MediaKind::Video => 90_000,
                        _ => {
                            let den = send_frame.frame.timebase.den;
                            if den > 0 {
                                den
                            } else {
                                8_000
                            }
                        }
                    };
                    let rtp_clock = cheetah_codec::RtpClock { rate: clock_rate };
                    let timestamp = rtp_clock.micros_to_ticks(send_frame.frame.pts_us);
                    session.rtcp.on_sent(timestamp);

                    let payload_type =
                        match (session.egress_payload_mode, send_frame.frame.media_kind) {
                            (RtpPayloadMode::Ps, _) => 96,
                            (RtpPayloadMode::Ts, _) => 33,
                            // Audio in raw / ES mode: prefer the canonical static PTs for codecs
                            // that have well-known assignments (RFC 3551). Falls back to a
                            // dynamic PT when the codec has no static assignment.
                            (_, cheetah_codec::MediaKind::Audio) => match send_frame.frame.codec {
                                cheetah_codec::CodecId::G711U => 0,
                                cheetah_codec::CodecId::G711A => 8,
                                cheetah_codec::CodecId::MP3 => 14,
                                _ => 97,
                            },
                            // Video in raw / ES mode uses dynamic PT 96.
                            (_, cheetah_codec::MediaKind::Video) => 96,
                            _ => 97,
                        };

                    let rtp_header = RtpHeader {
                        version: 2,
                        payload_type,
                        sequence_number: session.next_seq,
                        timestamp,
                        ssrc: session.ssrc,
                        marker: false,
                    };

                    let packets = cheetah_codec::packetize_payload(payload, 1400, rtp_header);
                    let packet_count = packets.len() as u16;
                    for pkt in packets {
                        let encoded = pkt.encode();
                        session.packets_sent += 1;
                        session.bytes_sent += pkt.payload.len() as u32;

                        if let Some(conn_id) = session.tcp_conn_id {
                            let frame_tcp = cheetah_codec::encode_tcp_rtp_frame(&pkt);
                            outputs.push(RtpCoreOutput::SendTcp(RtpTcpSend {
                                conn_id,
                                data: frame_tcp,
                            }));
                        } else if let Some(dest) = session.destination {
                            outputs.push(RtpCoreOutput::SendUdp(RtpUdpSend {
                                session_key: session._session_key.clone(),
                                destination: dest,
                                data: encoded,
                            }));
                        }
                    }

                    // Advance sequence by number of packets actually emitted.
                    session.next_seq = session.next_seq.wrapping_add(packet_count.max(1));

                    // Reflect the runtime state transition caused by egress.
                    let transport_mode = session.transport_mode;
                    let current_state = session.state;
                    let is_talk = current_state == RtpSessionState::Talk;
                    if let Some(new_state) =
                        state_after_egress(transport_mode, current_state, is_talk)
                    {
                        if let Some(old) = session.transition_to(new_state) {
                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged {
                                session_key: send_frame.session_key.clone(),
                                old_state: old,
                                new_state,
                            }));
                        }
                    }
                }
            }
            RtpCoreCommand::UpdateSession {
                session_key,
                expected_generation,
                ssrc,
                payload_type,
                pause_check,
            } => {
                self.handle_update_session(
                    session_key,
                    expected_generation,
                    ssrc,
                    payload_type,
                    pause_check,
                    outputs,
                );
            }
            RtpCoreCommand::StopSession(key) => {
                self.close_session(key, "Stopped by command".to_string(), outputs);
            }
            RtpCoreCommand::PauseCheck {
                session_key,
                paused,
            } => {
                if let Some(session) = self.sessions.get_mut(&session_key) {
                    session.check_paused = paused;
                    // Reset activity baseline on resume so the next tick does not immediately
                    // fire an idle timeout that accrued while checks were paused.
                    if !paused {
                        session.last_activity_ms = self.now_ms;
                        session.last_rr_received_ms = self.now_ms;
                    }
                }
            }
        }
    }
}
