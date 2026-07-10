use std::net::SocketAddr;
use thiserror::Error;

/// `RtpCoreError` enumeration.
/// `RtpCoreError` 枚举.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RtpCoreError {
    /// `SessionLimitReached` variant.
    /// `SessionLimitReached` 变体.
    #[error("Session limit reached: {limit}")]
    SessionLimitReached { limit: usize },

    /// `SessionAlreadyExists` variant.
    /// `SessionAlreadyExists` 变体.
    #[error("Session key already exists: {key:?}")]
    SessionAlreadyExists { key: String },

    /// `SessionNotFound` variant.
    /// `SessionNotFound` 变体.
    #[error("Session not found: {key:?}")]
    SessionNotFound { key: String },

    /// `TcpConnectionAlreadyExists` variant.
    /// `TcpConnectionAlreadyExists` 变体.
    #[error("TCP connection ID already exists: {conn_id}")]
    TcpConnectionAlreadyExists { conn_id: u64 },
}

/// `RtpCoreDiagnostic` enumeration.
/// `RtpCoreDiagnostic` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtpCoreDiagnostic {
    /// `InvalidRtpVersion` variant.
    /// `InvalidRtpVersion` 变体.
    InvalidRtpVersion { version: u8 },
    /// `RtpHeaderError` variant.
    /// `RtpHeaderError` 变体.
    RtpHeaderError,
    /// `EmptyPayload` variant.
    /// `EmptyPayload` 变体.
    EmptyPayload { ssrc: u32 },
    /// `UnknownPayload` variant.
    /// `UnknownPayload` 变体.
    UnknownPayload { ssrc: u32 },
    /// `SequenceGap` variant.
    /// `SequenceGap` 变体.
    SequenceGap { ssrc: u32, expected: u16, got: u16 },
    /// `SourceAddressChanged` variant.
    /// `SourceAddressChanged` 变体.
    SourceAddressChanged {
        ssrc: u32,
        old: SocketAddr,
        new: SocketAddr,
    },
    /// An incoming RTP payload exceeded the configured `max_rtp_len_cap`. The packet is still
    /// routed, but operators are notified via this diagnostic. Mirrors ABL's dynamic
    /// `nMaxRtpLength` learner that grows the maximum frame size for huge keyframes.
    OversizedPayload { ssrc: u32, len: usize, cap: usize },
}
