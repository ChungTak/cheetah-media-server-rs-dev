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
    #[error("session {0} already exists")]
    SessionAlreadyExists(WebRtcSessionId),

    #[error("session {0} not found")]
    SessionNotFound(WebRtcSessionId),

    #[error("session capacity exhausted (max={max})")]
    SessionCapacityExhausted { max: usize },

    #[error("remote sdp size {size} exceeds limit {limit}")]
    SdpTooLarge { size: usize, limit: usize },

    #[error("remote candidate quota exceeded (limit={limit})")]
    TooManyRemoteCandidates { limit: usize },

    #[error("invalid sdp offer/answer: {message}")]
    InvalidSdp { message: String },

    #[error("invalid ice candidate: {message}")]
    InvalidCandidate { message: String },

    #[error("session {session} is no longer alive")]
    SessionNotAlive { session: WebRtcSessionId },

    #[error("operation not supported in current state: {message}")]
    InvalidState { message: String },

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
    pub session_id: Option<WebRtcSessionId>,
    pub kind: WebRtcCoreDiagnosticKind,
    pub message: String,
}

/// Kind of `Web Rtc Core Diagnostic`.
/// `Web Rtc Core Diagnostic` 的种类。
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
