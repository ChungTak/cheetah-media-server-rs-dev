use crate::rtcp::{RtcpCompoundPacket, RtcpPacket};
use crate::types::*;

use super::RtpCore;

impl RtpCore {
    pub(super) fn process_tick(&mut self, now_ms: u64, outputs: &mut Vec<RtpCoreOutput>) {
        self.now_ms = now_ms;
        let mut to_remove = Vec::with_capacity(1);

        for (key, session) in &mut self.sessions {
            // Pause check suspends idle/RR timeout monitoring without stopping packet processing.
            if !session.check_paused {
                // Idle timeout only applies to sessions that can receive traffic. Pure senders are
                // supervised by RR-timeout instead. This mirrors ZLM's `RtpProcess` vs `RtpSender`
                // lifecycle split.
                let is_receiver = matches!(
                    session.transport_mode,
                    RtpTransportMode::RecvOnly | RtpTransportMode::SendRecv
                );
                if is_receiver
                    && session.last_activity_ms != 0
                    && now_ms.saturating_sub(session.last_activity_ms)
                        > self.session_idle_timeout_ms
                {
                    to_remove.push((key.clone(), RtpSessionCloseReason::IdleTimeout));
                    continue;
                }

                // Baseline activity on the first non-paused tick so a freshly created or
                // resumed session is not immediately closed.
                if session.last_activity_ms == 0 {
                    session.last_activity_ms = now_ms;
                }

                // RR-timeout sender shutdown (ZLM-style):
                //   - Only senders care about RR feedback.
                //   - We baseline `last_rr_received_ms` to the first tick after creation, then
                //     consider the sender dead if no RR has arrived within `session_idle_timeout_ms`
                //     after that baseline.
                //   - Pure recv sessions are covered by the idle path above.
                let is_sender = matches!(
                    session.transport_mode,
                    RtpTransportMode::SendOnly | RtpTransportMode::SendRecv
                );
                if is_sender {
                    if session.last_rr_received_ms == 0 {
                        session.last_rr_received_ms = now_ms;
                    } else if now_ms.saturating_sub(session.last_rr_received_ms)
                        > self.session_idle_timeout_ms
                    {
                        to_remove.push((key.clone(), RtpSessionCloseReason::RrTimeout));
                        continue;
                    }
                }
            }

            // Generate RTCP Sender/Receiver Report every 5 seconds
            if session.last_rtcp_report_ms == 0 {
                session.last_rtcp_report_ms = now_ms;
            }

            if now_ms.saturating_sub(session.last_rtcp_report_ms) >= self.rtcp_report_interval_ms {
                session.last_rtcp_report_ms = now_ms;

                let session_key = session._session_key.clone();
                let conn_id = session.tcp_conn_id;

                // If we have already seen an RTCP packet from the peer, reply directly to
                // that address. Otherwise, fall back to the RTP destination/source and let
                // the driver derive the RTCP port when using a dedicated RTCP socket.
                let rtcp_dest = session.rtcp_source_addr;
                let Some(dest) = rtcp_dest.or(session.destination).or(session.source_addr) else {
                    continue;
                };

                let peer_ssrc = session.peer_ssrc;
                let ssrc = session.ssrc;
                let packets_sent = session.packets_sent;
                let bytes_sent = session.bytes_sent;
                let has_received = session.rtcp.packets_received() > 0;

                let report_packet = if packets_sent > 0 {
                    let block = if has_received {
                        session.rtcp.report_block(peer_ssrc, now_ms)
                    } else {
                        None
                    };
                    Some(RtcpPacket::SenderReport(session.rtcp.sender_report(
                        ssrc,
                        packets_sent,
                        bytes_sent,
                        now_ms,
                        block,
                    )))
                } else if has_received {
                    session.rtcp.report_block(peer_ssrc, now_ms).map(|block| {
                        RtcpPacket::ReceiverReport(session.rtcp.receiver_report(ssrc, block))
                    })
                } else {
                    None
                };

                if let Some(packet) = report_packet {
                    let compound = RtcpCompoundPacket {
                        packets: vec![packet],
                    };
                    if let Ok(data) = compound.encode() {
                        outputs.push(RtpCoreOutput::SendRtcp(RtcpSend {
                            session_key,
                            rtcp_destination: rtcp_dest,
                            destination: dest,
                            conn_id,
                            data,
                        }));
                    }
                }
            }
        }

        for (key, reason) in to_remove {
            self.close_session(key, reason, outputs);
        }
    }
}
