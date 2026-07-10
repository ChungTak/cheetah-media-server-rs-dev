use thiserror::Error;

/// Result type for `SRT Core` operations.
/// `SRT Core` 操作的结果类型。
pub type SrtCoreResult<T> = Result<T, SrtCoreError>;

/// Error returned by `SRT Core` operations.
/// `SRT Core` 操作返回的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SrtCoreError {
    #[error("invalid stream id: {0}")]
    InvalidStreamId(String),
    #[error("invalid SRT url: {0}")]
    InvalidUrl(String),
    #[error("invalid SRT config: {0}")]
    InvalidConfig(String),
    #[error("SRT connection error: {0}")]
    Connection(String),
}
