#[cfg(test)]
use bytes::Bytes;
use std::collections::HashMap;
use std::net::SocketAddr;

use cheetah_codec::{
    probe_rtp_payload, MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig, PsDemuxEvent,
    PsDemuxer, PsDemuxerConfig, RtpHeader, RtpPacket, RtpPayloadMode,
};

use crate::error::RtpCoreDiagnostic;
use crate::rtcp::{RtcpCompoundPacket, RtcpPacket};
use crate::rtcp_report::{default_clock_rate_hz, RtcpReportState};
use crate::types::{
    RtcpSend, RtpConnectionType, RtpCoreCommand, RtpCoreEvent, RtpCoreInput, RtpCoreOutput,
    RtpDatagram, RtpSessionKey, RtpTcpChunk, RtpTcpSend, RtpTrackFilter, RtpTransportMode,
    RtpUdpSend,
};

enum SessionDemuxer {
    Pending,
    Ts(MpegTsDemuxer),
    Ps(Box<PsDemuxer>),
    Es, // Raw audio/video ES routing
}

struct RtpSession {
    _session_key: RtpSessionKey,
    ssrc: u32,
    /// Configured RTP payload type, if supplied by the caller.
    payload_type: Option<u8>,
    /// Payload mode used to initialize the ingress demuxer.
    payload_mode: RtpPayloadMode,
    /// Payload mode used when packetizing outbound `SendFrame` frames.
    egress_payload_mode: RtpPayloadMode,
    transport_mode: RtpTransportMode,
    /// Filter applied to demuxed frames before they leave the core.
    track_filter: RtpTrackFilter,
    /// Filter applied to frames fed to `SendFrame`.
    egress_track_filter: RtpTrackFilter,

    // Ingress state
    demuxer: SessionDemuxer,
    last_seq: Option<u16>,
    source_addr: Option<SocketAddr>,
    last_activity_ms: u64,

    // Egress state
    destination: Option<SocketAddr>,
    tcp_conn_id: Option<u64>,
    next_seq: u16,
    peer_ssrc: u32,

    // Statistics
    packets_received: u32,
    bytes_received: u32,
    packets_sent: u32,
    bytes_sent: u32,
    last_rtcp_report_ms: u64,
    /// Last time an RTCP RR was observed for this sender; used by RR-timeout sender shutdown.
    last_rr_received_ms: u64,
    /// When true, idle/RR timeout checks are skipped but the session keeps receiving.
    check_paused: bool,
    /// Largest RTP payload observed on this session in bytes. Mirrors ABL's `nMaxRtpLength`
    /// dynamic learner so the driver can right-size send buffers and the module can flag
    /// pathological streams. Always bounded by the core's `max_rtp_len_cap`.
    max_rtp_len_observed: usize,

    // Concurrency control
    /// Monotonic generation updated whenever a mutable parameter actually changes.
    /// Starts at 1 and is compared by `UpdateSession` for atomicity.
    generation: u64,
    /// Last time a mutable parameter changed, in milliseconds, from the most recent tick.
    updated_at_ms: u64,
    /// Optional human-readable reason for the last recorded failure.
    last_error: Option<String>,

    /// RTCP report state for this session.
    rtcp: RtcpReportState,
}

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
    sessions: HashMap<RtpSessionKey, RtpSession>,
    ssrc_to_session: HashMap<u32, RtpSessionKey>,
    tcp_conn_to_session: HashMap<u64, RtpSessionKey>,
    ehome_decoders: HashMap<u64, cheetah_codec::EhomeDecoder>,
    max_sessions: usize,
    session_idle_timeout_ms: u64,
    now_ms: u64,
    /// TCP framing mode applied when deframing inbound RTP-over-TCP traffic. Defaults to
    /// `AutoDetect`, matching ABLMediaServer's behaviour of accepting both 2-byte length-prefix
    /// (`enable_tcp`) and 4-byte interleaved (`$ + channel + length`) frames on the same socket.
    tcp_framing: cheetah_codec::RtpTcpFraming,
    /// Hard upper bound on the dynamic `max_rtp_len_observed` learner. Payloads larger than
    /// this are still routed (we don't drop them) but produce an `OversizedPayload` diagnostic
    /// so operators can spot pathological streams.
    max_rtp_len_cap: usize,
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
            max_sessions,
            session_idle_timeout_ms,
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

    /// Parse an incoming RTCP packet and refresh peer-feedback timers.
    ///
    /// Only RTCP sender reports (PT=200) and receiver reports (PT=201) are consumed. For RR
    /// the report block at offset 8 contains the SSRC of the source being reported on; this
    /// described SSRC is used to locate the local sender session and reset its
    /// `last_rr_received_ms`. RTCP feedback packets such as NACK or PLI are not parsed in
    /// this phase and are ignored.
    ///
    /// 解析入站 RTCP 包并刷新对端反馈计时器。
    ///
    /// 仅消费 RTCP 发送者报告（PT=200）和接收者报告（PT=201）。对于 RR，偏移 8 处的报告块
    /// 包含被报告的源 SSRC；该被描述 SSRC 用于定位本地发送会话并重置其
    /// `last_rr_received_ms`。NACK 或 PLI 等 RTCP 反馈包在此阶段不解析并被忽略。
    fn process_rtcp_packet(&mut self, datagram: RtpDatagram, _outputs: &mut Vec<RtpCoreOutput>) {
        let Ok(compound) = RtcpCompoundPacket::parse(datagram.data) else {
            return;
        };

        for packet in compound.packets {
            match packet {
                RtcpPacket::SenderReport(sr) => {
                    for session in self.sessions.values_mut() {
                        if session.peer_ssrc == sr.ssrc {
                            session.rtcp.on_sender_report(sr.ntp_timestamp, self.now_ms);
                            session.last_rr_received_ms = self.now_ms.max(1);
                        }
                    }
                }
                RtcpPacket::ReceiverReport(rr) => {
                    for block in rr.report_blocks {
                        if let Some(session_key) = self.ssrc_to_session.get(&block.ssrc) {
                            if let Some(session) = self.sessions.get_mut(session_key) {
                                session.last_rr_received_ms = self.now_ms.max(1);
                            }
                        }
                    }
                }
                RtcpPacket::SourceDescription(_)
                | RtcpPacket::Bye(_)
                | RtcpPacket::App(_)
                | RtcpPacket::Unknown { .. } => {}
            }
        }
    }

    /// Parse a UDP RTP datagram and feed it into the matching session.
    ///
    /// If the RTP header cannot be parsed, the version field is checked to emit a targeted
    /// `InvalidRtpVersion` diagnostic before falling back to `RtpHeaderError`.
    ///
    /// 解析 UDP RTP 数据报并送入匹配会话。
    ///
    /// 如果无法解析 RTP 头，先检查版本字段以发出 `InvalidRtpVersion` 诊断，再回退到
    /// `RtpHeaderError`。
    fn process_udp_packet(&mut self, datagram: RtpDatagram, outputs: &mut Vec<RtpCoreOutput>) {
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

    /// Process TCP bytes for a single connection, handling Ehome2 and RTP-over-TCP framing.
    ///
    /// Ehome2 streams are detected by the 256-byte prefix signature `[0x01, 0x00, 0x01/0x02]`
    /// and are decoded into handshake SSRC, codec info, and media payloads. Non-Ehome bytes
    /// are deframed with `parse_tcp_rtp_frame_with`; on a framing failure a bounded recovery
    /// scan searches up to 4 KiB for a known SSRC or a PS pack-start header.
    ///
    /// 处理单条连接的 TCP 字节，支持 Ehome2 与 RTP-over-TCP 分帧。
    ///
    /// Ehome2 流通过 256 字节前缀签名 `[0x01, 0x00, 0x01/0x02]` 识别，并解码为 SSRC 握手、
    /// 编解码器信息与媒体负载。非 Ehome 字节使用 `parse_tcp_rtp_frame_with` 解帧；
    /// 分帧失败时会在 4 KiB 内做有界恢复扫描，寻找已知 SSRC 或 PS 包起始头。
    fn process_tcp_bytes(&mut self, chunk: RtpTcpChunk, outputs: &mut Vec<RtpCoreOutput>) {
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
                                track_filter: RtpTrackFilter::All,
                                egress_track_filter: RtpTrackFilter::All,
                                check_paused: false,
                                demuxer: SessionDemuxer::Pending,
                                last_seq: None,
                                source_addr: None,
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
                                    session.demuxer = SessionDemuxer::Es;
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

    /// Ingest a single RTP packet into the session table, creating a session on demand.
    ///
    /// For an unmapped SSRC, a new `RecvOnly` session is auto-created with the payload mode
    /// probed from the first packet. For every packet, the core updates activity timers,
    /// tracks the largest payload size (`max_rtp_len_observed`), checks for source-address
    /// changes, detects sequence-number gaps, and lazily initializes the demuxer (PS/TS/ES).
    ///
    /// 将单个 RTP 包送入会话表，必要时按需创建会话。
    ///
    /// 对于未映射的 SSRC，会根据第一个包探测到的负载模式自动创建 `RecvOnly` 会话。
    /// 对每个包，core 更新活动时间、跟踪最大负载大小（`max_rtp_len_observed`）、
    /// 检查源地址变化、检测序列号跳变，并惰性初始化 demuxer（PS/TS/ES）。
    fn feed_rtp_packet(
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
            let probed = probe_rtp_payload(&rtp.payload);
            let mode = if probed == RtpPayloadMode::Unknown {
                RtpPayloadMode::Ps // default standard compat
            } else {
                probed
            };

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
                _ => {
                    session.demuxer = SessionDemuxer::Es;
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
            _ => {
                // ES modes or unrecognized modes can be bridged directly in later module stages
            }
        }
    }

    /// Advance time and run per-session housekeeping.
    ///
    /// Housekeeping includes idle-timeout for receivers, RR-timeout for senders, and periodic
    /// RTCP report generation. Receivers emit a Receiver Report (PT=201) while senders emit a
    /// Sender Report (PT=200). Jitter is not computed in this core phase and is reported as 0.
    ///
    /// 推进时间并运行每会话的清理工作。
    ///
    /// 清理包括接收者空闲超时、发送者 RR 超时以及周期性 RTCP 报告生成。
    /// 接收者发送接收者报告（PT=201），发送者发送发送者报告（PT=200）。
    /// 此 core 阶段不计算抖动，报告中抖动字段为 0。
    fn process_tick(&mut self, now_ms: u64, outputs: &mut Vec<RtpCoreOutput>) {
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
                    to_remove.push((key.clone(), "Idle timeout".to_string()));
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
                        to_remove.push((key.clone(), "RR timeout".to_string()));
                        continue;
                    }
                }
            }

            // Generate RTCP Sender/Receiver Report every 5 seconds
            if session.last_rtcp_report_ms == 0 {
                session.last_rtcp_report_ms = now_ms;
            }

            if now_ms.saturating_sub(session.last_rtcp_report_ms) >= 5000 {
                session.last_rtcp_report_ms = now_ms;

                let session_key = session._session_key.clone();
                let conn_id = session.tcp_conn_id;
                let Some(dest) = session.destination.or(session.source_addr) else {
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

    /// Atomically update mutable session parameters and the SSRC index.
    ///
    /// `expected_generation` must match the current generation. A conflicting SSRC (already
    /// used by another session) or an empty patch leaves all state unchanged.
    ///
    /// 原子更新会话可变参数与 SSRC 索引。
    fn handle_update_session(
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

    /// Cleans up the session, SSRC, TCP connection, and Ehome decoder maps. Always emits
    /// `CloseSession` and `SessionClosed` so the driver can release sockets and the module can
    /// tear down higher-level state.
    ///
    /// 移除会话并发出生命周期清理输出。
    ///
    /// 清理会话、SSRC、TCP 连接与 Ehome 解码器映射。总是发出 `CloseSession` 和
    /// `SessionClosed`，使 driver 释放套接字、module 释放高层状态。
    fn close_session(
        &mut self,
        key: RtpSessionKey,
        reason: String,
        outputs: &mut Vec<RtpCoreOutput>,
    ) {
        if let Some(session) = self.sessions.remove(&key) {
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

    /// Handle a control command from the module/driver.
    ///
    /// Commands create server/client sessions, packetize and send a frame, or stop an existing
    /// session. `SendFrame` selects payload type and clock rate from the session mode and frame
    /// codec, then packetizes with `packetize_payload` and emits either TCP or UDP outputs.
    ///
    /// 处理来自 module/driver 的控制命令。
    ///
    /// 命令用于创建服务端/客户端会话、将帧打包并发送，或停止已有会话。
    /// `SendFrame` 根据会话模式与帧编解码器选择负载类型和时钟频率，然后使用
    /// `packetize_payload` 分包并输出 TCP 或 UDP 数据。
    fn process_command(&mut self, cmd: RtpCoreCommand, outputs: &mut Vec<RtpCoreOutput>) {
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
                    track_filter,
                    egress_track_filter: spec.track_filter,
                    check_paused: false,
                    demuxer: SessionDemuxer::Pending,
                    last_seq: None,
                    source_addr: None,
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
                    track_filter,
                    egress_track_filter: spec.track_filter,
                    check_paused: false,
                    demuxer: SessionDemuxer::Pending,
                    last_seq: None,
                    source_addr: None,
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

fn rand_ssrc() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_be_bytes(b) & 0x7FFFFFFF
}

/// Derive an internal payload mode from an RTP payload type value.
///
/// Mirrors the heuristic used by the orchestrator so that `UpdateSession` can
/// switch the demuxer/packetizer when the payload type actually changes.
fn payload_mode_from_payload_type(payload_type: u8) -> RtpPayloadMode {
    match payload_type {
        0 | 8 => RtpPayloadMode::RawAudio,
        33 => RtpPayloadMode::Ts,
        96..=99 => RtpPayloadMode::Es,
        _ => RtpPayloadMode::Ps,
    }
}

/// Whether the configured track filter allows a given media kind through.
fn track_filter_allows_track(filter: RtpTrackFilter, kind: cheetah_codec::MediaKind) -> bool {
    match filter {
        RtpTrackFilter::All => true,
        RtpTrackFilter::OnlyAudio => matches!(kind, cheetah_codec::MediaKind::Audio),
        RtpTrackFilter::OnlyVideo => matches!(kind, cheetah_codec::MediaKind::Video),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        RtpClientSpec, RtpConnectionType, RtpDatagram, RtpSendFrame, RtpServerSpec,
    };
    use cheetah_codec::{
        AVFrame, CodecId, FrameFormat, MediaKind, RtpHeader, RtpPacket, Timebase, TrackId,
    };
    use std::net::SocketAddr;

    #[test]
    fn test_rtp_core_ssrc_routing_and_auto_create() {
        let mut core = RtpCore::new(10, 5000);
        let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();

        // 1. Send an RTP packet with unmapped SSRC = 9999
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 100,
                timestamp: 1000,
                ssrc: 9999,
                marker: false,
            },
            // starts with PS start code to test probed payload mode Ps
            payload: Bytes::from(vec![
                0x00, 0x00, 0x01, 0xBA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]),
        };

        let datagram = RtpDatagram {
            source: addr,
            data: packet.encode(),
            received_at_ms: 0,
        };

        let outputs = core.handle_input(RtpCoreInput::UdpPacket(datagram));

        // Should auto-create a session named "live/9999" and emit SessionCreated event
        assert!(!outputs.is_empty());
        let mut has_created = false;
        for output in outputs {
            if let RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                transport_mode,
            }) = output
            {
                assert_eq!(session_key, "live/9999");
                assert_eq!(ssrc, 9999);
                assert_eq!(payload_mode, RtpPayloadMode::Ps);
                assert_eq!(transport_mode, RtpTransportMode::RecvOnly);
                has_created = true;
            }
        }
        assert!(has_created);
    }

    #[test]
    fn test_rtp_core_session_timeout() {
        let mut core = RtpCore::new(10, 1000); // 1000ms timeout

        // Pre-create server session
        let spec = RtpServerSpec {
            session_key: "test_session".to_string(),
            ssrc: Some(12345),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));
        assert_eq!(outputs.len(), 1);

        // Tick at t = 0
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });
        assert!(outputs.is_empty());

        // Tick at t = 1500 (idle timeout triggered)
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 1500 });
        let mut has_closed = false;
        for output in outputs {
            if let RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { session_key, .. }) = output {
                assert_eq!(session_key, "test_session");
                has_closed = true;
            }
        }
        assert!(has_closed);
    }

    #[test]
    fn test_rtp_core_pause_check_delays_timeout() {
        let mut core = RtpCore::new(10, 1000); // 1000ms timeout

        let spec = RtpServerSpec {
            session_key: "paused_session".to_string(),
            ssrc: Some(12345),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        // Pause timeout checks while the session receives no traffic.
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::PauseCheck {
            session_key: "paused_session".to_string(),
            paused: true,
        }));

        // Tick well past the idle timeout while paused: session must stay alive.
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 5000 });
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

        // Resume checks; the next tick should baseline activity, not immediately close.
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::PauseCheck {
            session_key: "paused_session".to_string(),
            paused: false,
        }));
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 5500 });
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

        // Only after the timeout window passes again does the session close.
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 6600 });
        assert!(outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));
    }

    #[test]
    fn test_rtp_core_tcp_recovery_via_known_ssrc() {
        // Pre-register a known SSRC, then feed a TCP chunk that begins with a corrupt
        // length-prefix but contains a valid RTP frame (with the known SSRC) further in.
        // The recovery scan should still extract the valid frame.
        let mut core = RtpCore::new(10, 30_000);
        let ssrc = 0xABCDEF12u32;
        let spec = RtpServerSpec {
            session_key: "live/recovery".to_string(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        // Build a valid RTP-over-TCP frame for the known SSRC.
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA, 0xBB]),
        };
        let valid_frame = cheetah_codec::encode_tcp_rtp_frame(&rtp);

        // Prepend 16 bytes of garbage so the parser must scan forward to recover.
        let mut chunk = vec![0xFFu8; 16];
        chunk.extend_from_slice(&valid_frame);

        let outputs = core.handle_input(RtpCoreInput::TcpBytes(crate::types::RtpTcpChunk {
            conn_id: 1,
            data: Bytes::from(chunk),
            received_at_ms: 0,
        }));

        // We should observe a Diagnostic for sequence-gap and at least one further event
        // that proves the RTP packet was processed (e.g. session created already exists, and
        // the demuxer was poked). At minimum we should not hang or panic.
        assert!(outputs.iter().any(|o| matches!(
            o,
            RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SequenceGap { .. })
        )));
    }

    #[test]
    fn test_rtp_core_rr_timeout_shuts_down_sender() {
        // Senders should be torn down when no RR feedback arrives within the idle timeout.
        let mut core = RtpCore::new(10, 1000);
        let spec = RtpServerSpec {
            session_key: "send_session".to_string(),
            ssrc: Some(42),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::SendOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        // Baseline tick at t=100 establishes last_rr_received_ms=100.
        let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });

        // 500ms later: still within idle window, no shutdown.
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 600 });
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

        // 2000ms later: well past timeout, sender should be closed with reason "RR timeout".
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 2200 });
        let closed = outputs.iter().any(|o| {
            matches!(
                o,
                RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { reason, .. })
                    if reason == "RR timeout"
            )
        });
        assert!(
            closed,
            "sender should close on RR timeout: outputs={outputs:?}"
        );
    }

    #[test]
    fn test_rtp_core_rr_resets_sender_timeout() {
        // When an RR is observed, the sender's RR-timeout baseline must move forward.
        let mut core = RtpCore::new(10, 1000);
        let spec = RtpServerSpec {
            session_key: "send_session".to_string(),
            ssrc: Some(99),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::SendOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));
        // Baseline.
        let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });

        // Build an RR RTCP packet describing SSRC=99 (our sender SSRC).
        // RR header: V=2, RC=1, PT=201, length=7, sender SSRC then source SSRC blocks.
        let mut rr = Vec::new();
        rr.push(0x81); // V=2, RC=1
        rr.push(201); // RR
        rr.extend_from_slice(&7u16.to_be_bytes());
        rr.extend_from_slice(&0u32.to_be_bytes()); // reporter SSRC (peer)
        rr.extend_from_slice(&99u32.to_be_bytes()); // describes our SSRC
        rr.extend_from_slice(&[0u8; 20]); // remaining report-block bytes

        let dgram = crate::types::RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: Bytes::from(rr),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::RtcpPacket(dgram));

        // 1500ms later: would have been past the 1000ms timeout if RR had not refreshed it.
        // After RR at t=now_ms, baseline moves to current `now_ms` (which is still 100 after the
        // last tick). To make this verifiable, advance another tick before the RR window ends.
        let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 800 });
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));
    }

    #[test]
    fn test_rtp_core_tcp_interleaved_framing_dispatches() {
        // Auto-detect framing must accept RTSP-style 4-byte interleaved RTP frames over a TCP
        // chunk just as it accepts the 2-byte RFC 4571 form.
        let mut core = RtpCore::new(10, 30_000);
        let ssrc = 0x12345678u32;
        let spec = RtpServerSpec {
            session_key: "live/interleaved".to_string(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 11,
                timestamp: 0,
                ssrc,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA, 0xBB]),
        };
        let frame = cheetah_codec::encode_interleaved_rtp_frame(&rtp, 0);

        // Feeding the interleaved frame should not produce a header diagnostic, indicating that
        // the auto-detect path matched on the leading `$` byte.
        let outputs = core.handle_input(RtpCoreInput::TcpBytes(crate::types::RtpTcpChunk {
            conn_id: 1,
            data: frame,
            received_at_ms: 0,
        }));
        assert!(!outputs.iter().any(|o| matches!(
            o,
            RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::RtpHeaderError)
        )));
    }

    #[test]
    fn test_rtp_core_oversized_payload_diagnostic() {
        // ABL-style dynamic max-RTP-length learner: when a payload exceeds the configured cap,
        // we still process the packet but emit `OversizedPayload` so operators can spot the
        // pathological stream.
        let mut core = RtpCore::new(10, 30_000);
        core.set_max_rtp_len_cap(1500);
        let ssrc = 0x1u32;

        let huge_payload = vec![0u8; 4096];
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 0,
                ssrc,
                marker: false,
            },
            payload: Bytes::from(huge_payload),
        };
        let dgram = crate::types::RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };

        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(outputs.iter().any(|o| matches!(
            o,
            RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::OversizedPayload {
                ssrc: 1,
                len: 4096,
                cap: 1500,
            })
        )));
    }

    #[test]
    fn test_voice_talk_upgrades_session_and_sends_audio() {
        // An inbound session can be upgraded to VoiceTalk, reusing the same socket
        // (same session_key) to push audio back to the peer.
        let mut core = RtpCore::new(10, 30_000);
        let session_key = "recv/talk/cam".to_string();
        let ssrc = 7777u32;

        let server_spec = RtpServerSpec {
            session_key: session_key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
            server_spec,
        )));

        // The peer address would normally be learned from the first ingress frame.
        let peer = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

        // Upgrade the same session to VoiceTalk / SendRecv with audio-only egress.
        let client_spec = RtpClientSpec {
            session_key: session_key.clone(),
            destination: peer,
            ssrc,
            payload_mode: RtpPayloadMode::RawAudio,
            transport_mode: RtpTransportMode::SendRecv,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::VoiceTalk),
            track_filter: RtpTrackFilter::OnlyAudio,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
            client_spec,
        )));

        // Audio frame should be emitted as UDP with static PT 0 (G.711 u-law).
        let audio = AVFrame::new(
            TrackId(1),
            MediaKind::Audio,
            CodecId::G711U,
            FrameFormat::G711Packet,
            0,
            0,
            Timebase::new(1, 8000),
            Bytes::from(vec![0xD5; 160]),
        );
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
            RtpSendFrame {
                session_key: session_key.clone(),
                frame: audio,
            },
        )));

        let mut sent = false;
        for output in outputs {
            if let RtpCoreOutput::SendUdp(udp) = output {
                assert_eq!(udp.destination, peer);
                assert_eq!(udp.session_key, session_key);
                let parsed = RtpPacket::parse(&udp.data).unwrap();
                assert_eq!(parsed.header.ssrc, ssrc);
                assert_eq!(parsed.header.payload_type, 0);
                sent = true;
            }
        }
        assert!(sent, "expected SendUdp output for voice talk audio");

        // Video frame should be dropped by the OnlyAudio track filter.
        let video = AVFrame::new(
            TrackId(2),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        );
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
            RtpSendFrame {
                session_key,
                frame: video,
            },
        )));
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::SendUdp(_))));
    }

    #[test]
    fn test_update_session_advances_generation_and_ssrc_index() {
        let mut core = RtpCore::new(10, 30_000);
        let key = "recv/update".to_string();
        let spec = RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        // Update SSRC and pause with the correct expected generation.
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
            session_key: key.clone(),
            expected_generation: 1,
            ssrc: Some(2000),
            payload_type: Some(96),
            pause_check: Some(true),
        }));

        let mut updated = false;
        for output in outputs {
            if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
                session_key,
                generation,
                ssrc,
                payload_type,
                pause_check,
            }) = output
            {
                assert_eq!(session_key, key);
                assert_eq!(generation, 2);
                assert_eq!(ssrc, Some(2000));
                assert_eq!(payload_type, Some(96));
                assert_eq!(pause_check, Some(true));
                updated = true;
            }
        }
        assert!(updated);

        // The new SSRC must be routed to the same session.
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 0,
                ssrc: 2000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
    }

    #[test]
    fn test_update_session_rejects_wrong_generation_and_conflict() {
        let mut core = RtpCore::new(10, 30_000);
        let spec_a = RtpServerSpec {
            session_key: "a".to_string(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let spec_b = RtpServerSpec {
            session_key: "b".to_string(),
            ssrc: Some(2000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec_a)));
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec_b)));

        // Wrong expected generation.
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
            session_key: "a".to_string(),
            expected_generation: 99,
            ssrc: Some(3000),
            payload_type: None,
            pause_check: None,
        }));
        assert!(outputs.iter().any(|o| matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason,
            }) if session_key == "a" && reason == "generation mismatch"
        )));

        // Duplicate SSRC already used by session b.
        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
            session_key: "a".to_string(),
            expected_generation: 1,
            ssrc: Some(2000),
            payload_type: None,
            pause_check: None,
        }));
        assert!(outputs.iter().any(|o| matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason,
            }) if session_key == "a" && reason.contains("already in use")
        )));

        // Session a must keep its original SSRC.
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 0,
                ssrc: 1000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
    }

    #[test]
    fn test_update_session_payload_type_changes_mode_and_generation() {
        let mut core = RtpCore::new(10, 30_000);
        let key = "recv/pt".to_string();
        let spec = RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
            session_key: key.clone(),
            expected_generation: 1,
            ssrc: None,
            payload_type: Some(96),
            pause_check: None,
        }));

        let mut updated = false;
        for output in outputs {
            if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
                session_key,
                generation,
                ssrc,
                payload_type,
                pause_check,
            }) = output
            {
                assert_eq!(session_key, key);
                assert_eq!(generation, 2);
                assert_eq!(ssrc, None);
                assert_eq!(payload_type, Some(96));
                assert_eq!(pause_check, None);
                updated = true;
            }
        }
        assert!(updated, "expected SessionUpdated event");

        // A packet with the new PT should still be routed to the same session.
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 0,
                ssrc: 1000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(!outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
    }

    #[test]
    fn test_update_session_no_change_keeps_generation() {
        let mut core = RtpCore::new(10, 30_000);
        let key = "recv/noop".to_string();
        let spec = RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        };
        let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

        let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
            session_key: key.clone(),
            expected_generation: 1,
            ssrc: Some(1000),
            payload_type: None,
            pause_check: None,
        }));
        let updated = outputs.iter().find_map(|o| match o {
            RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated { generation, .. }) => {
                Some(*generation)
            }
            _ => None,
        });
        assert_eq!(updated, Some(1));
    }
}
