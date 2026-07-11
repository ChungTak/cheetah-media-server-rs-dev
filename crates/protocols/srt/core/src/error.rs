use thiserror::Error;

/// Result type alias for SRT core operations.
///
/// SRT core 操作的结果类型别名。
pub type SrtCoreResult<T> = Result<T, SrtCoreError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
/// Error cases for SRT core parsing and connection logic.
///
/// SRT core 解析与连接逻辑的错误情况。
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
