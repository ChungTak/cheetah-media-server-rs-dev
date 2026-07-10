use bytes::Bytes;
use std::net::SocketAddr;

use cheetah_codec::{AVFrame, RtpPayloadMode, TrackInfo};

use crate::error::RtpCoreDiagnostic;

pub type RtpSessionKey = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTransportMode {
    RecvOnly,
    SendOnly,
    SendRecv,
}

/// ZLMediaKit-style connection types. Mirrors `kTcpActive`/`kTcpPassive`/`kUdpActive`/`kUdpPassive`/`kVoiceTalk`
/// from `vendor-ref/ZLMediaKit/src/Rtp/RtpSender.cpp`.
///
/// - `*_Active` modes initiate the network connection towards the peer (push side).
/// - `*_Passive` modes wait for the peer to connect / send first.
/// - `VoiceTalk` reuses an existing inbound RTP session's socket to push audio back to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpConnectionType {
    UdpActive,
    UdpPassive,
    TcpActive,
    TcpPassive,
    VoiceTalk,
}

/// Track filter applied at session creation. Mirrors ZLM `OnlyTrack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpTrackFilter {
    #[default]
    All,
    OnlyAudio,
    OnlyVideo,
}

#[derive(Debug, Clone)]
pub struct RtpServerSpec {
    pub session_key: RtpSessionKey,
    pub ssrc: Option<u32>,
    pub payload_mode: RtpPayloadMode,
    pub transport_mode: RtpTransportMode,
    /// Optional connection-type hint. Defaults to `UdpPassive` when unset.
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Track filter to apply on ingress.
    pub track_filter: RtpTrackFilter,
}

#[derive(Debug, Clone)]
pub struct RtpClientSpec {
    pub session_key: RtpSessionKey,
    pub destination: SocketAddr,
    pub ssrc: u32,
    pub payload_mode: RtpPayloadMode,
    pub transport_mode: RtpTransportMode,
    pub tcp_conn_id: Option<u64>,
    /// Optional connection-type hint. Defaults to `UdpActive` when unset.
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Track filter to apply on egress.
    pub track_filter: RtpTrackFilter,
}

#[derive(Debug, Clone)]
pub struct RtpSendFrame {
    pub session_key: RtpSessionKey,
    pub frame: AVFrame,
}

#[derive(Debug, Clone)]
pub struct RtpDatagram {
    pub source: SocketAddr,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct RtpTcpChunk {
    pub conn_id: u64,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct RtpUdpSend {
    pub destination: SocketAddr,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct RtpTcpSend {
    pub conn_id: u64,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct RtcpSend {
    pub destination: SocketAddr,
    pub conn_id: Option<u64>,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreEvent {
    SessionCreated {
        session_key: RtpSessionKey,
        ssrc: u32,
        payload_mode: RtpPayloadMode,
        transport_mode: RtpTransportMode,
    },
    SessionClosed {
        session_key: RtpSessionKey,
        reason: String,
    },
    TrackFound {
        session_key: RtpSessionKey,
        tracks: Vec<TrackInfo>,
    },
    Frame {
        session_key: RtpSessionKey,
        frame: AVFrame,
    },
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreInput {
    UdpPacket(RtpDatagram),
    TcpBytes(RtpTcpChunk),
    /// Incoming RTCP datagram (non-RTP UDP arriving on the RTCP port). Used to update
    /// peer feedback statistics and reset the RR-timeout sender shutdown.
    RtcpPacket(RtpDatagram),
    Tick {
        now_ms: u64,
    },
    Command(RtpCoreCommand),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreCommand {
    CreateServer(RtpServerSpec),
    CreateClient(RtpClientSpec),
    SendFrame(RtpSendFrame),
    StopSession(RtpSessionKey),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreOutput {
    SendUdp(RtpUdpSend),
    SendTcp(RtpTcpSend),
    SendRtcp(RtcpSend),
    Event(RtpCoreEvent),
    Diagnostic(RtpCoreDiagnostic),
    CloseSession(RtpSessionKey),
}
