use thiserror::Error;

/// Error returned by `HLS Core` operations.
/// `HLS Core` 操作返回的错误。
#[derive(Debug, Error)]
pub enum HlsCoreError {
    #[error("invalid HLS path: {path}")]
    InvalidPath { path: String },
    #[error("stream not found: {stream_key}")]
    StreamNotFound { stream_key: String },
    #[error("segment not found: {name}")]
    SegmentNotFound { name: String },
    #[error("not ready: waiting for initial segments")]
    NotReady,
}
