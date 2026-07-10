use std::net::SocketAddr;
use thiserror::Error;

/// Error returned by `RTP Core` operations.
/// `RTP Core` 操作返回的错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RtpCoreError {
    #[error("Session limit reached: {limit}")]
    SessionLimitReached { limit: usize },

    #[error("Session key already exists: {key:?}")]
    SessionAlreadyExists { key: String },

    #[error("Session not found: {key:?}")]
    SessionNotFound { key: String },

    #[error("TCP connection ID already exists: {conn_id}")]
    TcpConnectionAlreadyExists { conn_id: u64 },
}

/// `RtpCoreDiagnostic` enumeration.
/// `RtpCoreDiagnostic` 枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtpCoreDiagnostic {
    InvalidRtpVersion {
        version: u8,
    },
    RtpHeaderError,
    EmptyPayload {
        ssrc: u32,
    },
    UnknownPayload {
        ssrc: u32,
    },
    SequenceGap {
        ssrc: u32,
        expected: u16,
        got: u16,
    },
    SourceAddressChanged {
        ssrc: u32,
        old: SocketAddr,
        new: SocketAddr,
    },
    /// An incoming RTP payload exceeded the configured `max_rtp_len_cap`. The packet is still
    /// routed, but operators are notified via this diagnostic. Mirrors ABL's dynamic
    /// `nMaxRtpLength` learner that grows the maximum frame size for huge keyframes.
    OversizedPayload {
        ssrc: u32,
        len: usize,
        cap: usize,
    },
}
