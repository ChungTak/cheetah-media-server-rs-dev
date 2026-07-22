use cheetah_codec::RtpPacket;

use crate::error::RtpCoreDiagnostic;
use crate::rtcp::{RtcpCompoundPacket, RtcpPacket};
use crate::types::*;

use super::RtpCore;

impl RtpCore {
    pub(super) fn process_rtcp_packet(
        &mut self,
        datagram: RtpDatagram,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        let rtcp_source = datagram.source;
        let Ok(compound) = RtcpCompoundPacket::parse(datagram.data) else {
            return;
        };

        let mut bye_keys: Vec<RtpSessionKey> = Vec::new();

        for packet in compound.packets {
            match packet {
                RtcpPacket::SenderReport(sr) => {
                    for session in self.sessions.values_mut() {
                        if session.peer_ssrc == sr.ssrc {
                            session.rtcp_source_addr = Some(rtcp_source);
                            session.rtcp.on_sender_report(
                                sr.ntp_timestamp,
                                sr.rtp_timestamp,
                                self.now_ms,
                            );
                            session.last_rr_received_ms = self.now_ms.max(1);
                        }
                    }
                }
                RtcpPacket::ReceiverReport(rr) => {
                    for block in rr.report_blocks {
                        if let Some(session_key) = self.ssrc_to_session.get(&block.ssrc) {
                            if let Some(session) = self.sessions.get_mut(session_key) {
                                session.rtcp_source_addr = Some(rtcp_source);
                                session.last_rr_received_ms = self.now_ms.max(1);
                            }
                        }
                    }
                }
                RtcpPacket::Bye(bye) => {
                    for bye_ssrc in bye.ssrcs {
                        for (key, session) in &self.sessions {
                            if session.peer_ssrc == bye_ssrc || session.ssrc == bye_ssrc {
                                bye_keys.push(key.clone());
                            }
                        }
                    }
                }
                RtcpPacket::SourceDescription(_)
                | RtcpPacket::App(_)
                | RtcpPacket::Unknown { .. } => {}
            }
        }

        for key in bye_keys {
            if let Some(session) = self.sessions.get_mut(&key) {
                session.rtcp_source_addr = Some(rtcp_source);
            }
            self.close_session(key, RtpSessionCloseReason::Bye, outputs);
        }
    }

    pub(super) fn process_udp_packet(
        &mut self,
        datagram: RtpDatagram,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        let Some(rtp) = RtpPacket::parse(&datagram.data) else {
            if !datagram.data.is_empty() {
                let version = datagram.data[0] >> 6;
                if version != 2 {
                    outputs.push(RtpCoreOutput::Diagnostic(
                        RtpCoreDiagnostic::InvalidRtpVersion { version },
                    ));
                    return;
                }
            }
            outputs.push(RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::RtpHeaderError));
            return;
        };

        self.feed_rtp_packet(
            rtp,
            Some(datagram.source),
            None,
            datagram.received_at_ms,
            outputs,
        );
    }
}
