use bytes::Bytes;
use std::net::SocketAddr;

use cheetah_codec::{AVFrame, RtpPayloadMode, TrackInfo};

use crate::error::RtpCoreDiagnostic;

/// `RtpSessionKey` type alias.
/// `RtpSessionKey` 类型别名.
pub type RtpSessionKey = String;

/// `RtpTransportMode` enumeration.
/// `RtpTransportMode` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTransportMode {
    /// `RecvOnly` variant.
    /// `RecvOnly` 变体.
    RecvOnly,
    /// `SendOnly` variant.
    /// `SendOnly` 变体.
    SendOnly,
    /// `SendRecv` variant.
    /// `SendRecv` 变体.
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
    /// `UdpActive` variant.
    /// `UdpActive` 变体.
    UdpActive,
    /// `UdpPassive` variant.
    /// `UdpPassive` 变体.
    UdpPassive,
    /// `TcpActive` variant.
    /// `TcpActive` 变体.
    TcpActive,
    /// `TcpPassive` variant.
    /// `TcpPassive` 变体.
    TcpPassive,
    /// `VoiceTalk` variant.
    /// `VoiceTalk` 变体.
    VoiceTalk,
}

/// Track filter applied at session creation. Mirrors ZLM `OnlyTrack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpTrackFilter {
    /// `All` variant.
    /// `All` 变体.
    #[default]
    All,
    /// `OnlyAudio` variant.
    /// `OnlyAudio` 变体.
    OnlyAudio,
    /// `OnlyVideo` variant.
    /// `OnlyVideo` 变体.
    OnlyVideo,
}

/// `RtpServerSpec` data structure.
/// `RtpServerSpec` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpServerSpec {
    /// `session_key` field of type `RtpSessionKey`.
    /// `session_key` 字段，类型为 `RtpSessionKey`.
    pub session_key: RtpSessionKey,
    /// `ssrc` field.
    /// `ssrc` 字段.
    pub ssrc: Option<u32>,
    /// `payload_mode` field of type `RtpPayloadMode`.
    /// `payload_mode` 字段，类型为 `RtpPayloadMode`.
    pub payload_mode: RtpPayloadMode,
    /// `transport_mode` field of type `RtpTransportMode`.
    /// `transport_mode` 字段，类型为 `RtpTransportMode`.
    pub transport_mode: RtpTransportMode,
    /// Optional connection-type hint. Defaults to `UdpPassive` when unset.
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Track filter to apply on ingress.
    pub track_filter: RtpTrackFilter,
}

/// `RtpClientSpec` data structure.
/// `RtpClientSpec` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpClientSpec {
    /// `session_key` field of type `RtpSessionKey`.
    /// `session_key` 字段，类型为 `RtpSessionKey`.
    pub session_key: RtpSessionKey,
    /// `destination` field of type `SocketAddr`.
    /// `destination` 字段，类型为 `SocketAddr`.
    pub destination: SocketAddr,
    /// `ssrc` field of type `u32`.
    /// `ssrc` 字段，类型为 `u32`.
    pub ssrc: u32,
    /// `payload_mode` field of type `RtpPayloadMode`.
    /// `payload_mode` 字段，类型为 `RtpPayloadMode`.
    pub payload_mode: RtpPayloadMode,
    /// `transport_mode` field of type `RtpTransportMode`.
    /// `transport_mode` 字段，类型为 `RtpTransportMode`.
    pub transport_mode: RtpTransportMode,
    /// `tcp_conn_id` field.
    /// `tcp_conn_id` 字段.
    pub tcp_conn_id: Option<u64>,
    /// Optional connection-type hint. Defaults to `UdpActive` when unset.
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Track filter to apply on egress.
    pub track_filter: RtpTrackFilter,
}

/// `RtpSendFrame` data structure.
/// `RtpSendFrame` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpSendFrame {
    /// `session_key` field of type `RtpSessionKey`.
    /// `session_key` 字段，类型为 `RtpSessionKey`.
    pub session_key: RtpSessionKey,
    /// `frame` field of type `AVFrame`.
    /// `frame` 字段，类型为 `AVFrame`.
    pub frame: AVFrame,
}

/// `RtpDatagram` data structure.
/// `RtpDatagram` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpDatagram {
    /// `source` field of type `SocketAddr`.
    /// `source` 字段，类型为 `SocketAddr`.
    pub source: SocketAddr,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
}

/// `RtpTcpChunk` data structure.
/// `RtpTcpChunk` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpTcpChunk {
    /// `conn_id` field of type `u64`.
    /// `conn_id` 字段，类型为 `u64`.
    pub conn_id: u64,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
}

/// `RtpUdpSend` data structure.
/// `RtpUdpSend` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpUdpSend {
    /// `destination` field of type `SocketAddr`.
    /// `destination` 字段，类型为 `SocketAddr`.
    pub destination: SocketAddr,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
}

/// `RtpTcpSend` data structure.
/// `RtpTcpSend` 数据结构.
#[derive(Debug, Clone)]
pub struct RtpTcpSend {
    /// `conn_id` field of type `u64`.
    /// `conn_id` 字段，类型为 `u64`.
    pub conn_id: u64,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
}

/// `RtcpSend` data structure.
/// `RtcpSend` 数据结构.
#[derive(Debug, Clone)]
pub struct RtcpSend {
    /// `destination` field of type `SocketAddr`.
    /// `destination` 字段，类型为 `SocketAddr`.
    pub destination: SocketAddr,
    /// `conn_id` field.
    /// `conn_id` 字段.
    pub conn_id: Option<u64>,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
}

/// `RtpCoreEvent` enumeration.
/// `RtpCoreEvent` 枚举.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreEvent {
    /// `SessionCreated` variant.
    /// `SessionCreated` 变体.
    SessionCreated {
        session_key: RtpSessionKey,
        ssrc: u32,
        payload_mode: RtpPayloadMode,
        transport_mode: RtpTransportMode,
    },
    /// `SessionClosed` variant.
    /// `SessionClosed` 变体.
    SessionClosed {
        session_key: RtpSessionKey,
        reason: String,
    },
    /// `TrackFound` variant.
    /// `TrackFound` 变体.
    TrackFound {
        session_key: RtpSessionKey,
        tracks: Vec<TrackInfo>,
    },
    /// `Frame` variant.
    /// `Frame` 变体.
    Frame {
        session_key: RtpSessionKey,
        frame: AVFrame,
    },
}

/// `RtpCoreInput` enumeration.
/// `RtpCoreInput` 枚举.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreInput {
    /// `UdpPacket` variant.
    /// `UdpPacket` 变体.
    UdpPacket(RtpDatagram),
    /// `TcpBytes` variant.
    /// `TcpBytes` 变体.
    TcpBytes(RtpTcpChunk),
    /// Incoming RTCP datagram (non-RTP UDP arriving on the RTCP port). Used to update
    /// peer feedback statistics and reset the RR-timeout sender shutdown.
    RtcpPacket(RtpDatagram),
    /// `Tick` variant.
    /// `Tick` 变体.
    Tick { now_ms: u64 },
    /// `Command` variant.
    /// `Command` 变体.
    Command(RtpCoreCommand),
}

/// `RtpCoreCommand` enumeration.
/// `RtpCoreCommand` 枚举.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreCommand {
    /// `CreateServer` variant.
    /// `CreateServer` 变体.
    CreateServer(RtpServerSpec),
    /// `CreateClient` variant.
    /// `CreateClient` 变体.
    CreateClient(RtpClientSpec),
    /// `SendFrame` variant.
    /// `SendFrame` 变体.
    SendFrame(RtpSendFrame),
    /// `StopSession` variant.
    /// `StopSession` 变体.
    StopSession(RtpSessionKey),
}

/// `RtpCoreOutput` enumeration.
/// `RtpCoreOutput` 枚举.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreOutput {
    /// `SendUdp` variant.
    /// `SendUdp` 变体.
    SendUdp(RtpUdpSend),
    /// `SendTcp` variant.
    /// `SendTcp` 变体.
    SendTcp(RtpTcpSend),
    /// `SendRtcp` variant.
    /// `SendRtcp` 变体.
    SendRtcp(RtcpSend),
    /// `Event` variant.
    /// `Event` 变体.
    Event(RtpCoreEvent),
    /// `Diagnostic` variant.
    /// `Diagnostic` 变体.
    Diagnostic(RtpCoreDiagnostic),
    /// `CloseSession` variant.
    /// `CloseSession` 变体.
    CloseSession(RtpSessionKey),
}
