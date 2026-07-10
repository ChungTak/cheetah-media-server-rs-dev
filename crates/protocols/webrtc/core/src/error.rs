//! Error and diagnostic types for [`crate::WebRtcCore`].

use thiserror::Error;

use crate::types::WebRtcSessionId;

/// Errors returned synchronously from `WebRtcCore` operations.
///
/// These are non-fatal at the boundary by default. The core never panics
/// or unwinds when fed malformed inputs; instead the caller is expected to
/// react (close session, surface 4xx HTTP, etc.).
#[derive(Debug, Error)]
pub enum WebRtcCoreError {
    /// `SessionAlreadyExists` variant.
    /// `SessionAlreadyExists` 变体.
    #[error("session {0} already exists")]
    SessionAlreadyExists(WebRtcSessionId),

    /// `SessionNotFound` variant.
    /// `SessionNotFound` 变体.
    #[error("session {0} not found")]
    SessionNotFound(WebRtcSessionId),

    /// `SessionCapacityExhausted` variant.
    /// `SessionCapacityExhausted` 变体.
    #[error("session capacity exhausted (max={max})")]
    SessionCapacityExhausted { max: usize },

    /// `SdpTooLarge` variant.
    /// `SdpTooLarge` 变体.
    #[error("remote sdp size {size} exceeds limit {limit}")]
    SdpTooLarge { size: usize, limit: usize },

    /// `TooManyRemoteCandidates` variant.
    /// `TooManyRemoteCandidates` 变体.
    #[error("remote candidate quota exceeded (limit={limit})")]
    TooManyRemoteCandidates { limit: usize },

    /// `InvalidSdp` variant.
    /// `InvalidSdp` 变体.
    #[error("invalid sdp offer/answer: {message}")]
    InvalidSdp { message: String },

    /// `InvalidCandidate` variant.
    /// `InvalidCandidate` 变体.
    #[error("invalid ice candidate: {message}")]
    InvalidCandidate { message: String },

    /// `SessionNotAlive` variant.
    /// `SessionNotAlive` 变体.
    #[error("session {session} is no longer alive")]
    SessionNotAlive { session: WebRtcSessionId },

    /// `InvalidState` variant.
    /// `InvalidState` 变体.
    #[error("operation not supported in current state: {message}")]
    InvalidState { message: String },

    /// `Rtc` variant.
    /// `Rtc` 变体.
    #[error("str0m rtc error: {message}")]
    Rtc { message: String },
}

/// Diagnostic record emitted by the core for tracing / metrics.
///
/// Diagnostics are non-fatal. They surface conditions the operator might
/// care about — SDP compatibility patches, ICE state churn, dropped output
/// because of internal queue limits, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcCoreDiagnostic {
    /// `session_id` field.
    /// `session_id` 字段.
    pub session_id: Option<WebRtcSessionId>,
    /// `kind` field of type `WebRtcCoreDiagnosticKind`.
    /// `kind` 字段，类型为 `WebRtcCoreDiagnosticKind`.
    pub kind: WebRtcCoreDiagnosticKind,
    /// `message` field of type `String`.
    /// `message` 字段，类型为 `String`.
    pub message: String,
}

/// `WebRtcCoreDiagnosticKind` enumeration.
/// `WebRtcCoreDiagnosticKind` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcCoreDiagnosticKind {
    /// SDP was rewritten by the compatibility preprocessor before being
    /// handed to `str0m`.
    SdpCompatRewrite,
    /// `str0m` returned an error while consuming a network packet; the
    /// session was closed.
    NetworkInputRejected,
    /// `str0m` returned an error while processing a timeout; the session
    /// was closed.
    TimeoutRejected,
    /// Output items were dropped because the per-session pending queue was
    /// full.
    PendingOutputDropped,
    /// The session emitted an unexpected `str0m::Event` variant; included
    /// for forward compatibility with future `str0m` releases.
    UnhandledEvent,
}
