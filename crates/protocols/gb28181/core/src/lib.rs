//! Sans-I/O state machine and message types for the GB28181 video-surveillance protocol.
//!
//! 无 I/O 的 GB28181 视频监控协议状态机与消息类型。

/// Lenient SIP digest authentication primitives for registration and challenge.
///
/// 面向注册与挑战的宽松 SIP digest 认证原语。
pub mod digest;

/// Core error and diagnostic types emitted by the state machine.
///
/// 状态机产生的核心错误与诊断类型。
pub mod error;

/// Minimal SIP message parser and serializer tuned for GB28181 peers.
///
/// 面向 GB28181 对端调优的最小化 SIP 消息解析器与序列化器。
pub mod message;

/// SDP negotiation helpers: media type, transport, and SSRC extraction.
///
/// SDP 协商辅助：媒体类型、传输与 SSRC 提取。
pub mod sdp;

/// Device registration, keepalive, and INVITE/BYE session state machine.
///
/// 设备注册、保活以及 INVITE/BYE 会话状态机。
pub mod session;

/// Digest authentication primitives.
///
/// Digest 认证原语。
pub use digest::{compute_md5_response, DigestParams};

/// Core error types.
///
/// 核心错误类型。
pub use error::{Gb28181CoreError, Gb28181Diagnostic};

/// SIP message types and parsing.
///
/// SIP 消息类型与解析。
pub use message::{SipMessage, StartLine};

/// SDP negotiation helpers.
///
/// SDP 协商辅助。
pub use sdp::GbSdp;

/// State machine types and commands.
///
/// 状态机类型与命令。
pub use session::{
    Gb28181Command, Gb28181Core, Gb28181CoreInput, Gb28181CoreOutput, Gb28181Event, GbDevice,
    GbDeviceId, GbInviteSpec, GbSessionId, GbTalkSpec, SipSendAction,
};
