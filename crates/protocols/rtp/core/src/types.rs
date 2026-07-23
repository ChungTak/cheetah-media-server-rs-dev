use bytes::Bytes;
use std::net::SocketAddr;

use cheetah_codec::{AVFrame, RtpPayloadMode, TrackInfo};

use crate::error::RtpCoreDiagnostic;

/// Stable identifier for an RTP session.
///
/// For auto-created sessions this is `live/{ssrc}`; explicit server/client specs use the
/// supplied key.
///
/// RTP 会话的稳定标识。
///
/// 自动创建的会话为 `live/{ssrc}`；显式 server/client 配置使用给定的 key。
pub type RtpSessionKey = String;

/// Direction of media flow for an RTP session.
///
/// This matches the SDP `sendonly`/`recvonly`/`sendrecv` semantics and drives timeout
/// policy: `RecvOnly` sessions are supervised by idle timeout, while `SendOnly` sessions
/// are supervised by RR-timeout.
///
/// RTP 会话的媒体流向。
///
/// 对应 SDP `sendonly`/`recvonly`/`sendrecv` 语义，并决定超时策略：
/// `RecvOnly` 会话由空闲超时监管，`SendOnly` 会话由 RR 超时监管。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTransportMode {
    RecvOnly,
    SendOnly,
    SendRecv,
}

/// Runtime state of an RTP session.
///
/// The state machine makes receiver / sender / talkback transitions explicit and is
/// independent of the negotiated `RtpTransportMode`. A `SendRecv` session, for example,
/// starts in `Inactive` and moves to `SendRecv` once media flows in either direction.
/// `Talk` is a distinct state because voice talkback reuses an inbound socket for
/// outbound audio and has its own timeout / codec assumptions.
///
/// RTP 会话的运行时状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpSessionState {
    /// Session exists but no media has flowed yet.
    #[default]
    Inactive,
    /// Receiving RTP from the peer.
    Receiving,
    /// Sending RTP to the peer.
    Sending,
    /// Bidirectional media is active.
    SendRecv,
    /// Voice talkback is active (ingress + egress audio on the same socket).
    Talk,
    /// Terminal state: the session has been closed.
    Closed,
}

/// ZLMediaKit-style connection types. Mirrors `kTcpActive`/`kTcpPassive`/`kUdpActive`/
/// `kUdpPassive`/`kVoiceTalk` from `vendor-ref/ZLMediaKit/src/Rtp/RtpSender.cpp`.
///
/// - `*_Active` modes initiate the network connection towards the peer (push side).
/// - `*_Passive` modes wait for the peer to connect / send first.
/// - `VoiceTalk` reuses an existing inbound RTP session's socket to push audio back to
///   the device.
///
/// ZLMediaKit 风格的连接类型。对应 `vendor-ref/ZLMediaKit/src/Rtp/RtpSender.cpp` 中的
/// `kTcpActive`/`kTcpPassive`/`kUdpActive`/`kUdpPassive`/`kVoiceTalk`。
///
/// - `*_Active` 模式主动向对端发起网络连接（推流侧）。
/// - `*_Passive` 模式等待对端先连接/发送。
/// - `VoiceTalk` 复用已有入站 RTP 会话的套接字，向设备回推音频。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpConnectionType {
    UdpActive,
    UdpPassive,
    TcpActive,
    TcpPassive,
    VoiceTalk,
}

/// Track filter applied at session creation. Mirrors ZLM `OnlyTrack`.
///
/// 会话创建时应用的轨道过滤器。对应 ZLM `OnlyTrack`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpTrackFilter {
    /// All tracks are accepted.
    ///
    /// 接受所有轨道。
    #[default]
    All,
    /// Only audio tracks are accepted.
    ///
    /// 只接受音频轨道。
    OnlyAudio,
    /// Only video tracks are accepted.
    ///
    /// 只接受视频轨道。
    OnlyVideo,
}

/// Policy controlling whether an inbound RTP session may rebind to a new source address.
///
/// `Strict` locks the source on the first packet and drops traffic from any other endpoint.
/// `AllowValidatedRebind` permits a rebind when the old source has been idle long enough and
/// the new packet preserves SSRC / payload / sequence continuity.
///
/// 控制入站 RTP 会话是否允许重新绑定到新源地址的策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpSourcePolicy {
    /// Lock the source on the first packet and reject every other source.
    ///
    /// This is the default for explicitly created server/client sessions so
    /// that the effective policy matches the SDK's `SourceBindingPolicy::Strict` default.
    #[default]
    Strict,
    /// Allow a rebind if the change passes the configured idle/continuity/rate checks.
    ///
    /// Used for auto-created fallback sessions where the source cannot be
    /// pre-negotiated and NAT/port migrations must be tolerated.
    AllowValidatedRebind,
}

/// Reasons why an RTP session reached a terminal state.
///
/// This replaces free-form `String` reasons so drivers and modules can match on a
/// small, versioned set of lifecycle outcomes.
///
/// RTP 会话进入终态的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpSessionCloseReason {
    /// Explicit stop command from the control plane.
    Stopped,
    /// No ingress activity within the configured idle window.
    IdleTimeout,
    /// Sender received no RTCP receiver reports within the configured window.
    RrTimeout,
    /// The peer sent an RTCP BYE packet.
    Bye,
    /// The payload type changed and could not be resolved for the tolerated budget.
    UnresolvablePayloadType { current: u8, new: u8, count: u8 },
    /// The payload mode oscillated more than the allowed number of times.
    PayloadModeOscillation {
        from: RtpPayloadMode,
        to: RtpPayloadMode,
    },
    /// The underlying TCP connection was closed by the peer (half-close or error).
    ConnectionClosed,
}

impl std::fmt::Display for RtpSessionCloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped by command"),
            Self::IdleTimeout => write!(f, "idle timeout"),
            Self::RrTimeout => write!(f, "RR timeout"),
            Self::Bye => write!(f, "RTCP BYE"),
            Self::UnresolvablePayloadType { current, new, count } => write!(
                f,
                "payload type changed from {current} to {new} and could not be resolved for {count} packets"
            ),
            Self::PayloadModeOscillation { from, to } => write!(
                f,
                "payload mode oscillated from {from:?} to {to:?}"
            ),
            Self::ConnectionClosed => write!(f, "TCP connection closed by peer"),
        }
    }
}

/// Specification for an inbound (server) RTP session.
///
/// The session listens for packets on the local socket and auto-creates internal tracks
/// once the payload mode is discovered.
///
/// 入站（服务端）RTP 会话的规格。
///
/// 该会话在本地套接字监听，发现负载模式后自动创建内部轨道。
#[derive(Debug, Clone)]
pub struct RtpServerSpec {
    pub session_key: RtpSessionKey,
    /// Optional fixed SSRC. When omitted a random SSRC is generated.
    ///
    /// 可选的固定 SSRC。省略时生成随机 SSRC。
    pub ssrc: Option<u32>,
    pub payload_mode: RtpPayloadMode,
    pub transport_mode: RtpTransportMode,
    /// Optional connection-type hint. Defaults to `UdpPassive` when unset.
    ///
    /// 可选的连接类型提示。未设置时默认为 `UdpPassive`。
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Source-address binding policy. `None` uses the core default (`Strict`).
    ///
    /// 源地址绑定策略。`None` 使用 core 默认值（`Strict`）。
    pub source_policy: Option<RtpSourcePolicy>,
    /// Track filter to apply on ingress.
    ///
    /// 入站时应用的轨道过滤器。
    pub track_filter: RtpTrackFilter,
}

/// Specification for an outbound (client) RTP session.
///
/// The session originates packets to a fixed destination and advances an internal
/// sequence-number counter for each emitted RTP packet.
///
/// 出站（客户端）RTP 会话的规格。
///
/// 该会话向固定目的地址发送包，每发一个 RTP 包都会递增内部序列号。
#[derive(Debug, Clone)]
pub struct RtpClientSpec {
    pub session_key: RtpSessionKey,
    pub destination: SocketAddr,
    pub ssrc: u32,
    pub payload_mode: RtpPayloadMode,
    pub transport_mode: RtpTransportMode,
    /// Optional TCP connection ID for RTP-over-TCP egress.
    ///
    /// 可选的 RTP-over-TCP 出向 TCP 连接 ID。
    pub tcp_conn_id: Option<u64>,
    /// Optional connection-type hint. Defaults to `UdpActive` when unset.
    ///
    /// 可选的连接类型提示。未设置时默认为 `UdpActive`。
    #[allow(dead_code)]
    pub connection_type: Option<RtpConnectionType>,
    /// Source-address binding policy. `None` uses the core default (`Strict`).
    ///
    /// 源地址绑定策略。`None` 使用 core 默认值（`Strict`）。
    pub source_policy: Option<RtpSourcePolicy>,
    /// Track filter to apply on egress.
    ///
    /// 出站时应用的轨道过滤器。
    pub track_filter: RtpTrackFilter,
}

/// Frame to be packetized and sent by an outbound RTP session.
///
/// 待由出站 RTP 会话打包并发送的帧。
#[derive(Debug, Clone)]
pub struct RtpSendFrame {
    pub session_key: RtpSessionKey,
    pub frame: AVFrame,
}

/// A single UDP datagram received from the network.
///
/// `received_at_ms` is the driver-side receive timestamp in milliseconds. It is
/// in the same monotonic time domain as `RtpCoreInput::Tick`; only differences
/// are meaningful for jitter and idle/RR-timeout tracking. The driver may keep it
/// in a runtime-specific monotonic domain or a wall-clock domain, but it must be
/// consistent across all inputs to the same `RtpCore` instance.
///
/// 从网络收到的单个 UDP 数据报。
#[derive(Debug, Clone)]
pub struct RtpDatagram {
    pub source: SocketAddr,
    pub data: Bytes,
    pub received_at_ms: u64,
}

/// A chunk of TCP bytes received on a single connection.
///
/// `received_at_ms` is the driver-side receive timestamp in milliseconds. It must
/// be in the same domain as `RtpCoreInput::Tick`; only differences are used by
/// `core` for jitter and idle/RR-timeout tracking.
///
/// 在单个连接上收到的一小段 TCP 字节。
#[derive(Debug, Clone)]
pub struct RtpTcpChunk {
    pub conn_id: u64,
    pub data: Bytes,
    pub received_at_ms: u64,
}

/// Outbound UDP datagram.
///
/// 出站 UDP 数据报。
#[derive(Debug, Clone)]
pub struct RtpUdpSend {
    pub session_key: RtpSessionKey,
    pub destination: SocketAddr,
    pub data: Bytes,
}

/// Outbound TCP-framed RTP data.
///
/// 出站 TCP 分帧 RTP 数据。
#[derive(Debug, Clone)]
pub struct RtpTcpSend {
    pub conn_id: u64,
    pub session_key: RtpSessionKey,
    pub data: Bytes,
}

/// Outbound RTCP packet, optionally targeted at a TCP connection.
///
/// RTCP 反馈或报告可以封装在 UDP 中发送，也可以绑定到某个 TCP 连接。
#[derive(Debug, Clone)]
pub struct RtcpSend {
    pub session_key: RtpSessionKey,
    /// Preferred RTCP destination (e.g. observed RTCP source address).
    ///
    /// When set, the driver should use this address directly for UDP RTCP;
    /// otherwise it derives the RTCP port from `destination`.
    ///
    /// 首选 RTCP 目的地址（如观察到的 RTCP 源地址）。
    pub rtcp_destination: Option<SocketAddr>,
    /// Fallback destination used when `rtcp_destination` is `None`.
    ///
    /// 当 `rtcp_destination` 为空时使用的回退目的地址。
    pub destination: SocketAddr,
    /// Optional TCP connection ID for RTP-over-TCP RTCP transport.
    ///
    /// 可选的 RTP-over-TCP RTCP 传输连接 ID。
    pub conn_id: Option<u64>,
    pub data: Bytes,
}

/// Events emitted by `RtpCore` to the driver or module.
///
/// `RtpCore` 向 driver 或 module 发出的事件。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreEvent {
    /// A new session was accepted or created.
    ///
    /// 接受或创建了新会话。
    SessionCreated {
        session_key: RtpSessionKey,
        ssrc: u32,
        payload_mode: RtpPayloadMode,
        transport_mode: RtpTransportMode,
    },
    /// A session was updated and the generation advanced.
    ///
    /// 会话已更新，generation 已递增。
    SessionUpdated {
        session_key: RtpSessionKey,
        generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
        source_policy: Option<RtpSourcePolicy>,
    },
    /// The payload format changed mid-stream after it had been locked.
    ///
    /// 已锁定的 payload 格式在中途发生变化。
    FormatChanged {
        session_key: RtpSessionKey,
        payload_type: u8,
        old_payload_mode: RtpPayloadMode,
        new_payload_mode: RtpPayloadMode,
    },
    /// The bound source address was validated and rebinding to a new peer endpoint.
    ///
    /// 源地址经过验证后重新绑定到新的对端端点。
    SourceChanged {
        session_key: RtpSessionKey,
        old: SocketAddr,
        new: SocketAddr,
    },
    /// A session update was rejected; the session retains its previous values.
    ///
    /// 会话更新被拒绝；会话保留旧值。
    SessionUpdateFailed {
        session_key: RtpSessionKey,
        reason: String,
    },
    /// The runtime session state changed.
    ///
    /// 运行时会话状态已改变。
    SessionStateChanged {
        session_key: RtpSessionKey,
        old_state: RtpSessionState,
        new_state: RtpSessionState,
    },
    /// A session was closed (idle timeout, RR timeout, explicit stop, BYE, or format change).
    ///
    /// 会话被关闭（空闲超时、RR 超时、显式停止、BYE 或格式变更）。
    SessionClosed {
        session_key: RtpSessionKey,
        reason: RtpSessionCloseReason,
    },
    /// One or more tracks were discovered by the demuxer.
    ///
    /// demuxer 发现了一条或多条轨道。
    TrackFound {
        session_key: RtpSessionKey,
        tracks: Vec<TrackInfo>,
    },
    /// A normalized media frame was produced by the demuxer.
    ///
    /// demuxer 产生了一帧归一化媒体数据。
    Frame {
        session_key: RtpSessionKey,
        frame: AVFrame,
        source_addr: Option<SocketAddr>,
    },
}

/// Inputs that drive the `RtpCore` state machine.
///
/// Time is supplied externally; the core does not call `Instant::now()`.
///
/// 驱动 `RtpCore` 状态机的输入。
///
/// 时间由外部注入；core 不会调用 `Instant::now()`。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreInput {
    /// UDP RTP packet.
    ///
    /// UDP RTP 包。
    UdpPacket(RtpDatagram),
    /// TCP-framed RTP bytes.
    ///
    /// TCP 分帧 RTP 字节。
    TcpBytes(RtpTcpChunk),
    /// The peer closed the underlying TCP connection (half-close or read error).
    ///
    /// 对端关闭底层 TCP 连接（半关闭或读错误）。
    TcpConnectionClosed { conn_id: u64, received_at_ms: u64 },
    /// Incoming RTCP datagram (non-RTP UDP arriving on the RTCP port). Used to update
    /// peer-feedback statistics and reset the RR-timeout sender shutdown.
    ///
    /// 入站 RTCP 数据报（RTCP 端口上收到的非 RTP UDP）。用于更新对端反馈统计并重置
    /// 发送者的 RR 超时关闭。
    RtcpPacket(RtpDatagram),
    /// Periodic timer tick with the current driver time in milliseconds.
    ///
    /// This value only needs to be monotonic and consistent with the timestamps
    /// on `UdpPacket` / `TcpBytes` / `TcpConnectionClosed` inputs; it is used for
    /// RTCP scheduling, idle timeout and RR-timeout tracking. `core` adds the
    /// configured `wall_clock_offset_ms` when producing outbound Sender Report
    /// NTP timestamps.
    ///
    /// 周期性定时器 tick，当前驱动时间（毫秒）。
    Tick { now_ms: u64 },
    /// Control command from the module/driver.
    ///
    /// 来自 module/driver 的控制命令。
    Command(RtpCoreCommand),
}

/// Commands accepted by `RtpCore`.
///
/// `RtpCore` 接受的命令。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreCommand {
    /// Create a new inbound RTP session.
    ///
    /// 创建新的入站 RTP 会话。
    CreateServer(RtpServerSpec),
    /// Create a new outbound RTP session.
    ///
    /// 创建新的出站 RTP 会话。
    CreateClient(RtpClientSpec),
    /// Packetize and send a frame.
    ///
    /// 将一帧打包并发送。
    SendFrame(RtpSendFrame),
    /// Stop and close a session by key.
    ///
    /// 按 key 停止并关闭会话。
    StopSession(RtpSessionKey),
    /// Update mutable session parameters.
    ///
    /// 更新会话可变参数。
    UpdateSession {
        session_key: RtpSessionKey,
        expected_generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
        source_policy: Option<RtpSourcePolicy>,
    },
    /// Pause or resume timeout health checks for a session.
    ///
    /// 暂停或恢复会话的超时健康检查。
    PauseCheck {
        session_key: RtpSessionKey,
        paused: bool,
    },
}

/// Outputs produced by `RtpCore` for the driver to act on.
///
/// `RtpCore` 产生、由 driver 执行的输出。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum RtpCoreOutput {
    /// Send a UDP datagram.
    ///
    /// 发送 UDP 数据报。
    SendUdp(RtpUdpSend),
    /// Send a TCP-framed chunk.
    ///
    /// 发送 TCP 分帧块。
    SendTcp(RtpTcpSend),
    /// Send an RTCP packet.
    ///
    /// 发送 RTCP 包。
    SendRtcp(RtcpSend),
    /// Emit a lifecycle or media event.
    ///
    /// 发出生命周期或媒体事件。
    Event(RtpCoreEvent),
    /// Report a non-fatal diagnostic for logging/metrics.
    ///
    /// 报告非致命诊断，用于日志/指标。
    Diagnostic(RtpCoreDiagnostic),
    /// Close a session and clean up resources.
    ///
    /// 关闭会话并清理资源。
    CloseSession(RtpSessionKey),
    /// Close the underlying TCP connection and release its writer task.
    ///
    /// 关闭底层 TCP 连接并释放其写任务。
    CloseTcpConnection { conn_id: u64 },
}
