//! Stable boundary types exposed to the driver and module layers.
//!
//! The crate intentionally keeps these types `Copy` or owned-string-based and
//! avoids re-exporting `str0m`-specific identifiers so that downstream
//! crates do not gain transitive coupling to a particular `str0m` version.
//!
//! 本模块包含暴露给驱动层与模块层的稳定边界类型。
//!
//! crate 刻意使这些类型保持 `Copy` 或基于自有字符串，并避免重新导出
//! `str0m` 专用标识符，使下游 crate 不会与某个特定 `str0m` 版本产生
//! 传递耦合。

use core::fmt;

use serde::{Deserialize, Serialize};

/// Identifier for a WebRTC session managed by the core.
///
/// Driver and module layers create sessions ahead of time and own the id.
/// The core does not allocate ids — keeping allocation outside makes the
/// state machine a pure function over its inputs.
///
/// 核心管理的 WebRTC 会话标识符。
///
/// 驱动层与模块层预先创建会话并持有 id。核心不分配 id，将分配保持在
/// 外部，使状态机成为输入的纯函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WebRtcSessionId(pub u64);

impl WebRtcSessionId {
    /// Construct a new session id from a raw u64.
    ///
    /// 从 u64 构造新的会话 id。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw numeric value.
    ///
    /// 返回原始数值。
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WebRtcSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "webrtc-session-{}", self.0)
    }
}

/// SDP m-line label as a small owned string.
///
/// Mirrors `str0m::media::Mid` but does not leak the underlying type. The
/// label format is opaque outside this crate — it is produced by the core
/// and consumed by other layers as a stable identifier.
///
/// 以短自有字符串表示的 SDP m-line 标签。
///
/// 与 `str0m::media::Mid` 对应但不泄漏底层类型。标签格式对本 crate 外部
/// 不透明；由核心产生并作为稳定标识符供其他层消费。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MidLabel(pub String);

impl MidLabel {
    /// Construct a `MidLabel` from any string-like value.
    ///
    /// 从任何类字符串值构造 `MidLabel`。
    pub fn new(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    /// Borrow the underlying string.
    ///
    /// 借用底层字符串。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MidLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier for a WebRTC DataChannel within a session.
///
/// Wraps the integer id `str0m` returns through `Event::ChannelOpen` so that
/// downstream code does not depend on `str0m::channel::ChannelId` directly.
///
/// 会话内 WebRTC DataChannel 的标识符。
///
/// 包装 `str0m` 通过 `Event::ChannelOpen` 返回的整数 id，使下游代码
/// 不必直接依赖 `str0m::channel::ChannelId`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataChannelId(pub u32);

impl DataChannelId {
    /// Construct a new DataChannel id.
    ///
    /// 构造新的 DataChannel id。
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the raw numeric id.
    ///
    /// 返回原始数值 id。
    pub const fn value(self) -> u32 {
        self.0
    }
}

/// Direction the local endpoint plays for a given session.
///
/// This is the SDP-level direction, independent of the high-level role
/// (`WebRtcSessionRole`). It is used both for offer/answer creation and for
/// sanity-checking the negotiated track direction.
///
/// 本地端点在特定会话中的方向。
///
/// 这是 SDP 级方向，与高层角色（`WebRtcSessionRole`）无关。它用于创建
/// offer/answer 并检查协商后的 track 方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcDirection {
    /// Inbound media: remote peer sends, we receive.
    ///
    /// 入站媒体：远端发送，我们接收。
    RecvOnly,
    /// Outbound media: we send, remote peer receives.
    ///
    /// 出站媒体：我们发送，远端接收。
    SendOnly,
    /// Bi-directional media flow.
    ///
    /// 双向媒体流。
    SendRecv,
    /// No media; only DataChannel or signaling.
    ///
    /// 无媒体；仅 DataChannel 或信令。
    Inactive,
}

/// High-level role of a session within the cheetah stack.
///
/// A publisher ingests media into the engine; a player consumes media from
/// the engine; a bidirectional session may be used for P2P, echo, or control.
///
/// cheetah 栈中会话的高层角色。
///
/// 发布者将媒体接入引擎；播放器从引擎消费媒体；双向会话可用于 P2P、
/// 回声或控制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcSessionRole {
    /// Remote peer publishes media into the engine via this session.
    ///
    /// 远端通过本会话将媒体发布到引擎。
    Publisher,
    /// Engine plays media to remote peer via this session.
    ///
    /// 引擎通过本会话向远端播放媒体。
    Player,
    /// Bi-directional or DataChannel-only session (echo / P2P / control).
    ///
    /// 双向或仅 DataChannel 会话（回声 / P2P / 控制）。
    Bidirectional,
}

/// ICE role assignment for a session.
///
/// The controlling agent is responsible for finalizing the candidate pair
/// selection. In a typical server-as-answerer setup the server is the
/// controlled agent, but this can be overridden by policy.
///
/// 会话的 ICE 角色分配。
///
/// 控制端负责最终确定 candidate pair 选择。在典型的服务器作为应答方
/// 场景中，服务器是受控端，但策略可以覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcIceRole {
    /// Local endpoint is the controlling agent.
    ///
    /// 本地端为控制端。
    Controlling,
    /// Local endpoint is the controlled agent.
    ///
    /// 本地端为受控端。
    Controlled,
}

/// Session lifecycle state visible at the boundary.
///
/// This intentionally collapses some `str0m::IceConnectionState` values so
/// that downstream code only deals with the transitions it actually needs.
///
/// 边界可见的会话生命周期状态。
///
/// 它有意合并部分 `str0m::IceConnectionState` 值，使下游代码只处理
/// 实际需要的迁移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcSessionState {
    /// Session created but no SDP exchanged yet.
    ///
    /// 会话已创建但尚未交换 SDP。
    Created,
    /// SDP applied; ICE/DTLS handshake in progress.
    ///
    /// SDP 已应用；ICE/DTLS 握手进行中。
    Connecting,
    /// ICE+DTLS+SRTP up.
    ///
    /// ICE+DTLS+SRTP 已建立。
    Connected,
    /// Closing was requested; cleanup in progress.
    ///
    /// 已请求关闭；清理中。
    Closing,
    /// Session has been closed.
    ///
    /// 会话已关闭。
    Closed,
    /// Session ended because of an unrecoverable error.
    ///
    /// 会话因不可恢复错误而结束。
    Failed,
}

/// Codec negotiation profile applied to a session at construction time.
///
/// It controls which codecs are enabled and whether RTP-mode passthrough is
/// permitted. The profile is used to initialize `str0m::format::CodecConfig`.
///
/// 构造会话时应用的编解码器协商配置。
///
/// 它控制启用哪些编解码器以及是否允许 RTP 模式透传。该配置用于初始化
/// `str0m::format::CodecConfig`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcCodecProfile {
    /// Browser-friendly codec set: H264, VP8, VP9, AV1 and Opus.
    ///
    /// 面向浏览器的编解码器集合：H264、VP8、VP9、AV1 与 Opus。
    #[default]
    Browser,
    /// Device-friendly codec set: also H265, PCMA, PCMU.
    ///
    /// 面向设备的编解码器集合：额外包含 H265、PCMA、PCMU。
    Device,
    /// Pass-through profile for non-browser peers; allows RTP mode.
    ///
    /// 面向非浏览器对端的透传配置；允许 RTP 模式。
    Passthrough,
}
