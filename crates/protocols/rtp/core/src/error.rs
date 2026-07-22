use std::net::SocketAddr;
use thiserror::Error;

/// Errors returned by the `RtpCore` state machine when a command cannot be honored.
///
/// `RtpCore` 状态机在命令无法执行时返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RtpCoreError {
    /// The maximum number of concurrent RTP sessions has been reached.
    ///
    /// 已达到并发 RTP 会话数量上限。
    #[error("Session limit reached: {limit}")]
    SessionLimitReached { limit: usize },

    /// A session with the same key already exists; duplicate creation is ignored.
    ///
    /// 已存在相同 key 的会话；重复创建被忽略。
    #[error("Session key already exists: {key:?}")]
    SessionAlreadyExists { key: String },

    /// The requested session was not found in the active session table.
    ///
    /// 在活动会话表中未找到请求的会话。
    #[error("Session not found: {key:?}")]
    SessionNotFound { key: String },

    /// A TCP connection ID is already bound to another session.
    ///
    /// 该 TCP 连接 ID 已绑定到另一个会话。
    #[error("TCP connection ID already exists: {conn_id}")]
    TcpConnectionAlreadyExists { conn_id: u64 },
}

/// Diagnostics emitted by the RTP core for operator visibility.
///
/// Unlike `RtpCoreError`, these are not fatal to the current command; they are surfaced as
/// `RtpCoreOutput::Diagnostic` so the driver or module can log or emit metrics.
///
/// RTP core 为便于运维而发出的诊断信息。
///
/// 与 `RtpCoreError` 不同，这些诊断对当前命令不致命；它们以 `RtpCoreOutput::Diagnostic` 形式
/// 输出，供 driver 或 module 记录或生成指标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtpCoreDiagnostic {
    /// The RTP header version field is not 2; the packet is rejected.
    ///
    /// RTP 头版本字段不是 2；该包被拒绝。
    InvalidRtpVersion { version: u8 },

    /// Generic RTP header parse failure after the version check.
    ///
    /// 版本检查通过后仍无法解析 RTP 头。
    RtpHeaderError,

    /// An RTP packet arrived with an empty payload.
    ///
    /// 收到 RTP 空负载包。
    EmptyPayload { ssrc: u32 },

    /// An RTP payload was received for an unknown SSRC while the session table is full.
    ///
    /// 在会话表已满时收到未知 SSRC 的 RTP 负载。
    UnknownPayload { ssrc: u32 },

    /// A sequence number gap was detected; the core reports it but continues processing.
    ///
    /// 检测到序列号跳变；core 上报该诊断但继续处理。
    SequenceGap { ssrc: u32, expected: u16, got: u16 },

    /// The source address for an SSRC changed mid-session.
    ///
    /// 某个 SSRC 的源地址在会话中途发生变化。
    SourceAddressChanged {
        ssrc: u32,
        old: SocketAddr,
        new: SocketAddr,
    },

    /// A packet arrived from an unexpected source address and was rejected under the
    /// current binding policy.
    ///
    /// 包来自意外源地址，被当前绑定策略拒绝。
    SourceSpoofed {
        ssrc: u32,
        expected: SocketAddr,
        got: SocketAddr,
    },

    /// An incoming RTP payload exceeded the configured `max_rtp_len_cap`. The packet is still
    /// routed, but operators are notified via this diagnostic. Mirrors ABL's dynamic
    /// `nMaxRtpLength` learner that grows the maximum frame size for huge keyframes.
    ///
    /// 入站 RTP 负载超过配置的 `max_rtp_len_cap`。该包仍被路由，但会通过此诊断通知运维。
    /// 对应 ABL 动态 `nMaxRtpLength` 学习器，用于在出现巨大关键帧时自适应最大帧大小。
    OversizedPayload { ssrc: u32, len: usize, cap: usize },
}
