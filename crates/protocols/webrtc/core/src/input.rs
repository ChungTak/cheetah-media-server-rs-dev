//! Input model for the core state machine.
//!
//! Drivers convert their runtime-specific inputs (UDP packets, timer ticks,
//! HTTP commands) into [`WebRtcCoreInput`] values and feed them into
//! [`crate::WebRtcCore::handle_input`].
//!
//! All time values are `u64` microseconds anchored at the `start_micros`
//! provided to [`crate::WebRtcCore::new`]. The core never reads the system
//! clock.
//!
//! 本模块包含核心状态机的输入模型。
//!
//! 驱动层将运行时相关的输入（UDP 包、定时器滴答、HTTP 命令）转换为
//! [`WebRtcCoreInput`] 并喂入 [`crate::WebRtcCore::handle_input`]。
//!
//! 所有时间值均为 `u64` 微秒，并锚定 [`crate::WebRtcCore::new`] 传入的
//! `start_micros`。核心不读取系统时钟。

use std::net::SocketAddr;

use bytes::Bytes;

use crate::types::{DataChannelId, MidLabel, WebRtcSessionId, WebRtcSessionRole};

/// Top-level input fed into [`crate::WebRtcCore::handle_input`].
///
/// The enum deliberately mirrors the three things a `str0m` session needs:
/// commands, network receives, and timeouts. The `Tick` variant is a driver
/// convenience that fans out a single timeout to every session.
///
/// 喂入 [`crate::WebRtcCore::handle_input`] 的顶层输入。
///
/// 该枚举刻意映射 `str0m` 会话需要的三类输入：命令、网络接收和超时。
/// `Tick` 变体是驱动层的便利，将一次超时扇出到所有会话。
#[derive(Debug, Clone)]
pub enum WebRtcCoreInput {
    /// Driver-to-core command (create session, send frame, etc.).
    ///
    /// 驱动层到核心的命令（创建会话、发送帧等）。
    Command(WebRtcCoreCommand),
    /// A network packet routed to a specific session.
    ///
    /// 路由到特定会话的网络包。
    Network(WebRtcNetworkInput),
    /// Per-session timeout generated from a `SetTimer` output.
    ///
    /// 由 `SetTimer` 输出产生的每会话超时。
    Timeout {
        session_id: WebRtcSessionId,
        now_micros: u64,
    },
    /// Wall-clock advancement without an associated network input.
    ///
    /// Used by drivers to ensure forward progress even when no packets are
    /// arriving. Internally translated into `Input::Timeout` for `str0m`.
    ///
    /// 无相关网络输入的墙上时间推进。
    ///
    /// 驱动层使用它确保即使没有包到达也能向前推进。内部会转换为 `str0m` 的
    /// `Input::Timeout`。
    Tick { now_micros: u64 },
}

/// User-driven commands.
///
/// The variant data deliberately keeps types small and `Clone`-able so that
/// drivers can fan-out commands across shards if needed.
///
/// 用户驱动的命令。
///
/// 变体数据刻意保持简洁且可 `Clone`，以便驱动层在需要时跨分片扇出命令。
#[derive(Debug, Clone)]
pub enum WebRtcCoreCommand {
    /// Create a new session and accept a remote SDP offer, producing an
    /// SDP answer for delivery back to the peer.
    ///
    /// 创建新会话并接受远端 SDP offer，生成 SDP answer 以回传给对端。
    AcceptOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        remote_sdp: String,
        local_candidates: Vec<String>,
        now_micros: u64,
    },
    /// Create a new session in offering mode and produce a local SDP
    /// offer. The caller is expected to deliver the offer to a remote
    /// peer and feed back the answer via `ApplyAnswer`.
    ///
    /// Phase 05 scope: this is the foundation for client pull/push and
    /// P2P, where Cheetah is the offerer.
    ///
    /// 以 offerer 模式创建新会话并生成本地 SDP offer。
    ///
    /// 调用方应将 offer 传给远端对端，并通过 `ApplyAnswer` 回传 answer。
    /// 阶段 05 范围：这是客户端拉流/推流与 P2P 的基础，Cheetah 作为 offerer。
    CreateOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        spec: WebRtcOfferSpec,
        local_candidates: Vec<String>,
        now_micros: u64,
    },
    /// Apply a remote SDP answer to a previously created offering session.
    ///
    /// 将远端 SDP answer 应用于先前创建的 offerer 会话。
    ApplyAnswer {
        session_id: WebRtcSessionId,
        remote_sdp: String,
        now_micros: u64,
    },
    /// Trickle a remote ICE candidate into an existing session.
    ///
    /// 向现有会话 trickle 注入远端 ICE candidate。
    AddRemoteCandidate {
        session_id: WebRtcSessionId,
        candidate: String,
        now_micros: u64,
    },
    /// Trigger an ICE restart on an existing session.
    ///
    /// Calls `SdpApi::ice_restart` on the wrapped `str0m` session,
    /// producing a fresh local offer with rotated ICE credentials.
    /// The offer is delivered through the same `LocalDescription`
    /// path as `CreateOffer`, so the driver surfaces it via
    /// `WebRtcDriverEvent::OfferReady`.
    ///
    /// `keep_local_candidates=true` retains the previously gathered
    /// host candidates; `false` clears them so the caller can add
    /// fresh ones (typically used when the local network has changed
    /// and the previous candidates are no longer reachable).
    ///
    /// 在现有会话上触发 ICE 重启。
    ///
    /// 对包装的 `str0m` 会话调用 `SdpApi::ice_restart`，生成带有新 ICE 凭据的
    /// 本地 offer。该 offer 通过 `CreateOffer` 相同的 `LocalDescription` 路径
    /// 传递，因此驱动层通过 `WebRtcDriverEvent::OfferReady` 展示。
    ///
    /// `keep_local_candidates=true` 保留之前收集的 host candidate；`false` 则
    /// 清除它们，调用方可以添加新的（通常用于本地网络变化、旧 candidate 不再
    /// 可达时）。
    IceRestart {
        session_id: WebRtcSessionId,
        keep_local_candidates: bool,
        now_micros: u64,
    },
    /// Send DataChannel data on a previously opened channel.
    ///
    /// 在先前打开的通道上发送 DataChannel 数据。
    SendDataChannel(WebRtcDataChannelOut),
    /// Write a media frame to the remote peer (player role only).
    ///
    /// 向远端对端写入媒体帧（仅 player 角色）。
    SendFrame(Box<WebRtcSendFrame>),
    /// Ask the local sender to emit a fresh keyframe for the given track.
    ///
    /// 请求本地发送端为指定 track 生成新的关键帧。
    RequestKeyframe {
        session_id: WebRtcSessionId,
        mid: MidLabel,
        kind: WebRtcRequestKeyframeKind,
        now_micros: u64,
    },
    /// Close the session and emit a lifecycle event.
    ///
    /// 关闭会话并发出生命周期事件。
    Close {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
}

/// Network packet routed to a specific session by the driver.
///
/// 驱动层路由到特定会话的网络包。
#[derive(Debug, Clone)]
pub struct WebRtcNetworkInput {
    pub session_id: WebRtcSessionId,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub data: Bytes,
    pub now_micros: u64,
}

/// DataChannel write request.
///
/// Drivers must only route this to a session that has already opened the
/// requested channel. The core drops unknown or closed channels with a
/// diagnostic.
///
/// DataChannel 写入请求。
///
/// 驱动层只能将其路由到已经打开请求通道的会话。核心会对未知或已关闭的通道
/// 发出诊断并丢弃。
#[derive(Debug, Clone)]
pub struct WebRtcDataChannelOut {
    pub session_id: WebRtcSessionId,
    pub channel: DataChannelId,
    pub payload: Bytes,
    pub binary: bool,
}

/// Direction request for [`WebRtcCoreCommand::CreateOffer`].
///
/// Driven by the module: a publish-style client pull session would
/// request `RecvOnly` audio+video while a push-style session would
/// request `SendOnly`.
///
/// [`WebRtcCoreCommand::CreateOffer`] 的方向请求。
///
/// 由模块驱动：拉流式客户端发布会话会请求 `RecvOnly` 的音频+视频，而推流式
/// 会话会请求 `SendOnly`。
#[derive(Debug, Clone, Default)]
pub struct WebRtcOfferSpec {
    pub video_direction: Option<WebRtcOfferDirection>,
    pub audio_direction: Option<WebRtcOfferDirection>,
    pub data_channel: bool,
}

/// Direction of a media section in an offer created by the core.
///
/// 核心创建的 offer 中媒体段的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcOfferDirection {
    /// Local endpoint sends only.
    ///
    /// 本地端仅发送。
    SendOnly,
    /// Local endpoint receives only.
    ///
    /// 本地端仅接收。
    RecvOnly,
    /// Local endpoint both sends and receives.
    ///
    /// 本地端既发送又接收。
    SendRecv,
}

/// Outbound media frame written to the remote peer.
///
/// The driver layer constructs these from engine `AVFrame`s. The core
/// converts the boundary representation back into the `str0m::Writer`
/// API. `mid` must match a previously-negotiated send-direction track.
///
/// 写入远端对端的出站媒体帧。
///
/// 驱动层从引擎 `AVFrame` 构造这些帧。核心将边界表示转换回 `str0m::Writer` API。
/// `mid` 必须匹配先前协商的发送方向 track。
#[derive(Debug, Clone)]
pub struct WebRtcSendFrame {
    pub session_id: WebRtcSessionId,
    pub mid: crate::types::MidLabel,
    pub codec: crate::event::WebRtcCodecKind,
    pub clock_rate: u32,
    pub rtp_timestamp_ticks: u32,
    pub rtp_timestamp_denom: u32,
    pub random_access: bool,
    pub payload: Bytes,
    pub network_time_micros: u64,
}

/// Kind of keyframe request the sender should emit.
///
/// 发送端应发出的关键帧请求类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcRequestKeyframeKind {
    /// Picture Loss Indication.
    ///
    /// 图像丢失指示（PLI）。
    Pli,
    /// Full Intra Request.
    ///
    /// 完整帧内请求（FIR）。
    Fir,
}

/// Reason a session is being closed.
///
/// The reason is forwarded to the driver and to module observability so
/// operators can distinguish normal shutdowns from handshake failures.
///
/// 会话关闭原因。
///
/// 原因会被转发给驱动层和模块可观测性，以便运维人员区分正常关闭与握手失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcCloseReason {
    /// Clean, requested close.
    ///
    /// 正常、请求的关闭。
    Normal,
    /// The handshake did not complete in time.
    ///
    /// 握手未在时间内完成。
    HandshakeTimeout,
    /// No activity for the configured idle timeout.
    ///
    /// 在配置的空闲超时内无活动。
    Idle,
    /// The peer closed the connection.
    ///
    /// 对端关闭连接。
    PeerClosed,
    /// Internal error with a human-readable detail.
    ///
    /// 内部错误，附带人类可读细节。
    Internal(String),
}
