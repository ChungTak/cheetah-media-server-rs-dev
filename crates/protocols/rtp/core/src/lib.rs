//! `cheetah-rtp-core` is the Sans-I/O RTP/RTCP protocol state machine for the project.
//!
//! It owns the RTP/RTCP session lifecycle, UDP/TCP ingress framing, sequence-number
//! tracking, and a small RTCP sender/receiver-report heartbeat. Higher-layer concerns
//! such as jitter buffering, NACK/PLI generation, and socket I/O are intentionally kept in
//! the driver/module layers.
//!
//! `cheetah-rtp-core` 是项目 RTP/RTCP 协议的 Sans-I/O 状态机。
//!
//! 它负责 RTP/RTCP 会话生命周期、UDP/TCP 入口分帧、序列号跟踪以及最小 RTCP 发送者/接收者
//! 报告心跳。抖动缓冲、NACK/PLI 生成、套接字 I/O 等更高层职责被刻意保留在 driver/module 层。

/// Error and diagnostic types produced by the RTP core.
///
/// RTP core 产生的错误和诊断类型。
pub mod error;

/// RTP/RTCP session state machine and packet processing.
///
/// RTP/RTCP 会话状态机与包处理。
pub mod session;

/// Input/output/event/command types exchanged with `RtpCore`.
///
/// 与 `RtpCore` 交互的输入/输出/事件/命令类型。
pub mod types;

/// Re-exported RTP payload mode and TCP framing constants from `cheetah-codec`.
///
/// 从 `cheetah-codec` 重导出的 RTP 负载模式与 TCP 分帧常量。
pub use cheetah_codec::{RtpPayloadMode, RtpTcpFraming};

/// Re-exported RTP core error and diagnostic types.
///
/// 重导出的 RTP core 错误与诊断类型。
pub use error::{RtpCoreDiagnostic, RtpCoreError};

/// Re-exported RTP/RTCP session state machine.
///
/// 重导出的 RTP/RTCP 会话状态机。
pub use session::RtpCore;

/// Re-exported public input/output/event/command types.
///
/// 重导出的公共输入/输出/事件/命令类型。
pub use types::{
    RtcpSend, RtpClientSpec, RtpConnectionType, RtpCoreCommand, RtpCoreEvent, RtpCoreInput,
    RtpCoreOutput, RtpDatagram, RtpSendFrame, RtpServerSpec, RtpSessionKey, RtpTcpChunk,
    RtpTcpSend, RtpTrackFilter, RtpTransportMode, RtpUdpSend,
};
