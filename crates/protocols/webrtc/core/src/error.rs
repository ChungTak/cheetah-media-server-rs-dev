//! Error and diagnostic types for [`crate::WebRtcCore`].
//!
//! Errors are non-fatal and deterministic. The core never panics on malformed
//! input; it returns structured errors so the driver or module can decide how
//! to respond (close session, HTTP 4xx, drop packet, etc.).
//!
//! 本模块包含 [`crate::WebRtcCore`] 的错误与诊断类型。
//!
//! 错误是非致命且确定性的。核心不会因畸形输入而 panic，而是返回结构化错误，
//! 让驱动层或模块决定如何响应（关闭会话、返回 HTTP 4xx、丢弃包等）。

use thiserror::Error;

use crate::types::WebRtcSessionId;

/// Errors returned synchronously from `WebRtcCore` operations.
///
/// These are non-fatal at the boundary by default. The core never panics
/// or unwinds when fed malformed inputs; instead the caller is expected to
/// react (close session, surface 4xx HTTP, etc.).
///
/// `WebRtcCore` 操作同步返回的错误。
///
/// 默认情况下这些错误在边界处是非致命的。核心在收到畸形输入时不会 panic 或
/// 展开；调用方应负责响应（关闭会话、返回 4xx HTTP 等）。
#[derive(Debug, Error)]
pub enum WebRtcCoreError {
    #[error("session {0} already exists")]
    /// A session with the same id has already been inserted.
    ///
    /// 相同 id 的会话已被插入。
    SessionAlreadyExists(WebRtcSessionId),

    #[error("session {0} not found")]
    /// The requested session does not exist in this core.
    ///
    /// 此核心中不存在请求的会话。
    SessionNotFound(WebRtcSessionId),

    #[error("session capacity exhausted (max={max})")]
    /// The core is already managing [`WebRtcCoreLimits::max_sessions`] sessions.
    ///
    /// 核心已在管理 [`WebRtcCoreLimits::max_sessions`] 个会话。
    SessionCapacityExhausted { max: usize },

    #[error("remote sdp size {size} exceeds limit {limit}")]
    /// The remote SDP string is larger than the configured limit.
    ///
    /// 远端 SDP 字符串超过配置限制。
    SdpTooLarge { size: usize, limit: usize },

    #[error("remote candidate quota exceeded (limit={limit})")]
    /// Too many remote ICE candidates have been added for one session.
    ///
    /// 为单个会话添加了过多远端 ICE candidate。
    TooManyRemoteCandidates { limit: usize },

    #[error("invalid sdp offer/answer: {message}")]
    /// The remote SDP failed to parse or was rejected by `str0m`.
    ///
    /// 远端 SDP 解析失败或被 `str0m` 拒绝。
    InvalidSdp { message: String },

    #[error("invalid ice candidate: {message}")]
    /// An ICE candidate string was malformed or incompatible with `str0m`.
    ///
    /// ICE candidate 字符串格式错误或与 `str0m` 不兼容。
    InvalidCandidate { message: String },

    #[error("session {session} is no longer alive")]
    /// The operation cannot be performed because the session is not alive.
    ///
    /// 会话不再存活，无法执行操作。
    SessionNotAlive { session: WebRtcSessionId },

    #[error("operation not supported in current state: {message}")]
    /// The command is not valid for the current session state.
    ///
    /// 当前会话状态不允许该命令。
    InvalidState { message: String },

    #[error("str0m rtc error: {message}")]
    /// A low-level `str0m` error that could not be mapped to a higher-level cause.
    ///
    /// 无法映射到更高级原因的底层 `str0m` 错误。
    Rtc { message: String },
}

/// Diagnostic record emitted by the core for tracing / metrics.
///
/// Diagnostics are non-fatal. They surface conditions the operator might
/// care about — SDP compatibility patches, ICE state churn, dropped output
/// because of internal queue limits, etc.
///
/// 核心为追踪/指标发出的诊断记录。
///
/// 诊断是非致命的。它们暴露运维人员可能关心的状况——SDP 兼容性修补、
/// ICE 状态抖动、因内部队列限制导致的输出丢弃等。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcCoreDiagnostic {
    pub session_id: Option<WebRtcSessionId>,
    pub kind: WebRtcCoreDiagnosticKind,
    pub message: String,
}

/// Non-fatal diagnostic categories for the WebRTC core.
///
/// WebRTC 核心的非致命诊断分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcCoreDiagnosticKind {
    /// SDP was rewritten by the compatibility preprocessor before being
    /// handed to `str0m`.
    ///
    /// SDP 在交给 `str0m` 之前被兼容性预处理器重写。
    SdpCompatRewrite,
    /// `str0m` returned an error while consuming a network packet; the
    /// session was closed.
    ///
    /// `str0m` 消费网络包时返回错误；会话已关闭。
    NetworkInputRejected,
    /// `str0m` returned an error while processing a timeout; the session
    /// was closed.
    ///
    /// `str0m` 处理超时返回错误；会话已关闭。
    TimeoutRejected,
    /// Output items were dropped because the per-session pending queue was
    /// full.
    ///
    /// 待处理队列已满，导致输出项被丢弃。
    PendingOutputDropped,
    /// The session emitted an unexpected `str0m::Event` variant; included
    /// for forward compatibility with future `str0m` releases.
    ///
    /// 会话发出未预期的 `str0m::Event` 变体；保留以供未来 `str0m` 版本前向兼容。
    UnhandledEvent,
}
