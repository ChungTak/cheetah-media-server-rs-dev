use cheetah_codec::{
    PsDemuxEvent, PsDemuxer, RtpPayloadMode, RtpReorderBuffer, RtpReorderSettings,
};

use crate::error::RtpCoreDiagnostic;
use crate::rtcp_report::{default_clock_rate_hz, RtcpReportState};
use crate::types::*;

use super::{state::*, RtpCore};

impl RtpCore {
    pub(super) fn process_tcp_bytes(
        &mut self,
        chunk: RtpTcpChunk,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        // Detect Ehome on this connection. We avoid the historical `0x00 0x00` heuristic which
        // false-positives on small RTP-over-TCP frames whose length high byte is zero. Ehome
        // is detected only via:
        //   - an existing Ehome decoder for this `conn_id` (sticky once latched), or
        //   - the Ehome2 256-byte prefix `0x01 0x00 0x01/0x02 ...` at the start of the chunk.
        let is_ehome = self.ehome_decoders.contains_key(&chunk.conn_id)
            || (chunk.data.len() >= 3
                && chunk.data[0] == 0x01
                && chunk.data[1] == 0x00
                && (chunk.data[2] == 0x01 || chunk.data[2] == 0x02));

        if is_ehome {
            let decoder = self.ehome_decoders.entry(chunk.conn_id).or_default();
            let mut bytes_mut = bytes::BytesMut::from(&chunk.data[..]);
            let ehome_outputs = decoder.decode(&mut bytes_mut);

            for out in ehome_outputs {
                match out {
                    cheetah_codec::EhomeOutput::HandshakeSsrc(ssrc_str) => {
                        let ssrc = ssrc_str.parse::<u32>().unwrap_or_else(|_| {
                            let mut h = 0u32;
                            for b in ssrc_str.bytes() {
                                h = h.wrapping_mul(31).wrapping_add(b as u32);
                            }
                            h & 0x7FFFFFFF
                        });

                        let session_key = format!("live/{ssrc_str}");
                        if !self.sessions.contains_key(&session_key) {
                            let session = RtpSession {
                                _session_key: session_key.clone(),
                                ssrc,
                                payload_type: None,
                                payload_mode: RtpPayloadMode::Ehome,
                                egress_payload_mode: RtpPayloadMode::Ehome,
                                transport_mode: RtpTransportMode::RecvOnly,
                                state: RtpSessionState::Inactive,
                                track_filter: RtpTrackFilter::All,
                                egress_track_filter: RtpTrackFilter::All,
                                check_paused: false,
                                demuxer: SessionDemuxer::Pending,
                                last_seq: None,
                                reorder: RtpReorderBuffer::new(RtpReorderSettings::default()),
                                source_addr: None,
                                source_policy: RtpSourcePolicy::Strict,
                                source_spoof_count: 0,
                                source_rebind_count: 0,
                                rtcp_source_addr: None,
                                last_activity_ms: 0,
                                destination: None,
                                tcp_conn_id: Some(chunk.conn_id),
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
                                rtcp: RtcpReportState::new(default_clock_rate_hz(
                                    RtpPayloadMode::Ehome,
                                )),
                            };
                            self.sessions.insert(session_key.clone(), session);
                            self.ssrc_to_session.insert(ssrc, session_key.clone());
                            self.tcp_conn_to_session
                                .insert(chunk.conn_id, session_key.clone());

                            outputs.push(RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
                                session_key: session_key.clone(),
                                ssrc,
                                payload_mode: RtpPayloadMode::Ehome,
                                transport_mode: RtpTransportMode::RecvOnly,
                            }));
                        }
                    }
                    cheetah_codec::EhomeOutput::HandshakeCodec(codec) => {
                        if let Some(session_key) =
                            self.tcp_conn_to_session.get(&chunk.conn_id).cloned()
                        {
                            if let Some(session) = self.sessions.get_mut(&session_key) {
                                // Configure PS Demuxer if the payload mode is ps
                                if codec.payload_type == "ps" {
                                    session.demuxer = SessionDemuxer::Ps(Box::new(PsDemuxer::new(
                                        cheetah_codec::PsDemuxerConfig::new(4 * 1024 * 1024, 8),
                                    )));
                                } else {
                                    // ES depacketization is implemented in a follow-up PR;
                                    // bridge the raw ES payload until then.
                                    session.demuxer = SessionDemuxer::Bypass;
                                }

                                // Construct Tracks
                                let mut tracks = Vec::new();
                                if let Some(video) = &codec.video_codec {
                                    let codec_id = if video == "h265" {
                                        cheetah_codec::CodecId::H265
                                    } else {
                                        cheetah_codec::CodecId::H264
                                    };
                                    let mut track = cheetah_codec::TrackInfo::new(
                                        cheetah_codec::TrackId(1),
                                        cheetah_codec::MediaKind::Video,
                                        codec_id,
                                        90000,
                                    );
                                    track.readiness = cheetah_codec::TrackReadiness::Ready;
                                    tracks.push(track);
                                }
                                if let Some(audio) = &codec.audio_codec {
                                    let codec_id = if audio == "g711u" {
                                        cheetah_codec::CodecId::G711U
                                    } else if audio == "aac" {
                                        cheetah_codec::CodecId::AAC
                                    } else {
                                        cheetah_codec::CodecId::G711A
                                    };
                                    let mut track = cheetah_codec::TrackInfo::new(
                                        cheetah_codec::TrackId(2),
                                        cheetah_codec::MediaKind::Audio,
                                        codec_id,
                                        codec.sample_rate,
                                    );
                                    track.channels = Some(codec.channels);
                                    track.sample_rate = Some(codec.sample_rate);
                                    track.readiness = cheetah_codec::TrackReadiness::Ready;
                                    tracks.push(track);
                                }

                                if !tracks.is_empty() {
                                    outputs.push(RtpCoreOutput::Event(RtpCoreEvent::TrackFound {
                                        session_key: session_key.clone(),
                                        tracks,
                                    }));
                                }
                            }
                        }
                    }
                    cheetah_codec::EhomeOutput::MediaPayload(media_payload) => {
                        if let Some(session_key) =
                            self.tcp_conn_to_session.get(&chunk.conn_id).cloned()
                        {
                            if let Some(session) = self.sessions.get_mut(&session_key) {
                                session.packets_received += 1;
                                session.bytes_received += media_payload.len() as u32;
                                let track_filter = session.track_filter;

                                match &mut session.demuxer {
                                    SessionDemuxer::Ps(demuxer) => {
                                        let demux_events = demuxer.push(&media_payload);
                                        for ev in demux_events {
                                            match ev {
                                                PsDemuxEvent::TrackInfo(tracks) => {
                                                    let filtered: Vec<_> = tracks
                                                        .into_iter()
                                                        .filter(|t| {
                                                            track_filter_allows_track(
                                                                track_filter,
                                                                t.media_kind,
                                                            )
                                                        })
                                                        .collect();
                                                    if !filtered.is_empty() {
                                                        outputs.push(RtpCoreOutput::Event(
                                                            RtpCoreEvent::TrackFound {
                                                                session_key: session_key.clone(),
                                                                tracks: filtered,
                                                            },
                                                        ));
                                                    }
                                                }
                                                PsDemuxEvent::Frame(frame)
                                                    if track_filter_allows_track(
                                                        track_filter,
                                                        frame.media_kind,
                                                    ) =>
                                                {
                                                    outputs.push(RtpCoreOutput::Event(
                                                        RtpCoreEvent::Frame {
                                                            session_key: session_key.clone(),
                                                            frame: *frame,
                                                            source_addr: session.source_addr,
                                                        },
                                                    ));
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    _ => {
                                        // Dynamic audio/video ES frame routing if codec has been negotiated
                                        if let Some(codec) = &self
                                            .ehome_decoders
                                            .get(&chunk.conn_id)
                                            .and_then(|d| d.codec_info())
                                        {
                                            let is_video = codec.video_codec.is_some()
                                                && (!media_payload.is_empty()
                                                    && (media_payload[0] & 0x1F == 5
                                                        || media_payload[0] & 0x1F == 1
                                                        || (media_payload[0] >> 1) & 0x3F == 19
                                                        || (media_payload[0] >> 1) & 0x3F == 1));
                                            let track_id = if is_video {
                                                cheetah_codec::TrackId(1)
                                            } else {
                                                cheetah_codec::TrackId(2)
                                            };
                                            let pts = (session.packets_received as i64) * 40000;
                                            let dts = pts;
                                            let media_kind = if is_video {
                                                cheetah_codec::MediaKind::Video
                                            } else {
                                                cheetah_codec::MediaKind::Audio
                                            };
                                            let codec_id = if is_video {
                                                if codec.video_codec.as_deref() == Some("h265") {
                                                    cheetah_codec::CodecId::H265
                                                } else {
                                                    cheetah_codec::CodecId::H264
                                                }
                                            } else {
                                                if codec.audio_codec.as_deref() == Some("g711u") {
                                                    cheetah_codec::CodecId::G711U
                                                } else if codec.audio_codec.as_deref()
                                                    == Some("aac")
                                                {
                                                    cheetah_codec::CodecId::AAC
                                                } else {
                                                    cheetah_codec::CodecId::G711A
                                                }
                                            };
                                            let format = if is_video {
                                                cheetah_codec::FrameFormat::CanonicalH26x
                                            } else {
                                                cheetah_codec::FrameFormat::G711Packet
                                            };
                                            let mut frame = cheetah_codec::AVFrame::new(
                                                track_id,
                                                media_kind,
                                                codec_id,
                                                format,
                                                pts,
                                                dts,
                                                cheetah_codec::Timebase::new(1, 1_000_000),
                                                media_payload,
                                            );
                                            if is_video
                                                && (!frame.payload.is_empty()
                                                    && (frame.payload[0] & 0x1F == 5
                                                        || (frame.payload[0] >> 1) & 0x3F == 19))
                                            {
                                                frame.flags.insert(cheetah_codec::FrameFlags::KEY);
                                            }
                                            outputs.push(RtpCoreOutput::Event(
                                                RtpCoreEvent::Frame {
                                                    session_key: session_key.clone(),
                                                    frame,
                                                    source_addr: session.source_addr,
                                                },
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            // Parse RTP over TCP using the configured framing mode (defaults to AutoDetect).
            //
            // ABL-style bounded recovery: when a frame fails to parse as RTP we try to recover
            // the stream context by scanning forward for either a known SSRC or a PS system
            // header (`00 00 01 BA`) within a bounded window. This mirrors the
            // `RtpSession::searchBySSRC` / `searchByPsHeaderFlag` pattern in ZLM and the
            // Hikvision-flavoured cache split logic in ABLMediaServer.
            let framing = self.tcp_framing;
            let mut remaining = &chunk.data[..];
            while !remaining.is_empty() {
                if let Some(parsed) = cheetah_codec::parse_tcp_rtp_frame_with(remaining, framing) {
                    self.feed_rtp_packet(
                        parsed.packet,
                        None,
                        Some(chunk.conn_id),
                        chunk.received_at_ms,
                        outputs,
                    );
                    remaining = &remaining[parsed.consumed..];
                } else {
                    if remaining.len() < 2 {
                        break;
                    }
                    // Bounded recovery: scan up to 4 KiB ahead for one of:
                    //   - a length prefix whose RTP body has a known SSRC
                    //   - a PS pack-start prefix (00 00 01 BA)
                    //   - an RTSP-style interleaved framing prefix (`$ + channel + len`)
                    let scan_window = remaining.len().min(4096);
                    let mut recovered = false;
                    for offset in 1..scan_window {
                        if offset + 2 >= scan_window {
                            break;
                        }
                        // Try as RTP frame using the configured framing.
                        if let Some(parsed) =
                            cheetah_codec::parse_tcp_rtp_frame_with(&remaining[offset..], framing)
                        {
                            if self
                                .ssrc_to_session
                                .contains_key(&parsed.packet.header.ssrc)
                            {
                                outputs.push(RtpCoreOutput::Diagnostic(
                                    RtpCoreDiagnostic::SequenceGap {
                                        ssrc: parsed.packet.header.ssrc,
                                        expected: 0,
                                        got: parsed.packet.header.sequence_number,
                                    },
                                ));
                                self.feed_rtp_packet(
                                    parsed.packet,
                                    None,
                                    Some(chunk.conn_id),
                                    chunk.received_at_ms,
                                    outputs,
                                );
                                remaining = &remaining[offset + parsed.consumed..];
                                recovered = true;
                                break;
                            }
                        }
                        // Try PS system header
                        if remaining[offset..].len() >= 4
                            && remaining[offset] == 0x00
                            && remaining[offset + 1] == 0x00
                            && remaining[offset + 2] == 0x01
                            && remaining[offset + 3] == 0xBA
                        {
                            // Skip past the bad bytes; remaining bytes will be reassembled when
                            // the next length prefix arrives.
                            remaining = &remaining[offset..];
                            recovered = true;
                            break;
                        }
                    }
                    if !recovered {
                        // Recovery window exhausted; defer to next chunk.
                        break;
                    }
                }
            }
        }
    }
}
