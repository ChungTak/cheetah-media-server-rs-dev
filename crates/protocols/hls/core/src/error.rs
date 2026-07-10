use thiserror::Error;

/// `HlsCoreError` enumeration.
/// `HlsCoreError` 枚举.
#[derive(Debug, Error)]
pub enum HlsCoreError {
    /// `InvalidPath` variant.
    /// `InvalidPath` 变体.
    #[error("invalid HLS path: {path}")]
    InvalidPath { path: String },
    /// `StreamNotFound` variant.
    /// `StreamNotFound` 变体.
    #[error("stream not found: {stream_key}")]
    StreamNotFound { stream_key: String },
    /// `SegmentNotFound` variant.
    /// `SegmentNotFound` 变体.
    #[error("segment not found: {name}")]
    SegmentNotFound { name: String },
    /// `NotReady` variant.
    /// `NotReady` 变体.
    #[error("not ready: waiting for initial segments")]
    NotReady,
}
