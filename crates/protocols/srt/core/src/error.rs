use thiserror::Error;

/// `SrtCoreResult` type alias.
/// `SrtCoreResult` 类型别名.
pub type SrtCoreResult<T> = Result<T, SrtCoreError>;

/// `SrtCoreError` enumeration.
/// `SrtCoreError` 枚举.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SrtCoreError {
    /// `InvalidStreamId` variant.
    /// `InvalidStreamId` 变体.
    #[error("invalid stream id: {0}")]
    InvalidStreamId(String),
    /// `InvalidUrl` variant.
    /// `InvalidUrl` 变体.
    #[error("invalid SRT url: {0}")]
    InvalidUrl(String),
    /// `InvalidConfig` variant.
    /// `InvalidConfig` 变体.
    #[error("invalid SRT config: {0}")]
    InvalidConfig(String),
    /// `Connection` variant.
    /// `Connection` 变体.
    #[error("SRT connection error: {0}")]
    Connection(String),
}
