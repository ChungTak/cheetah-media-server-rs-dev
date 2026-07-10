use thiserror::Error;

/// `SdkError` enumeration.
/// `SdkError` 枚举.
#[derive(Debug, Error)]
pub enum SdkError {
    /// `NotFound` variant.
    /// `NotFound` 变体.
    #[error("not found: {0}")]
    NotFound(String),
    /// `AlreadyExists` variant.
    /// `AlreadyExists` 变体.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// `InvalidArgument` variant.
    /// `InvalidArgument` 变体.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// `Conflict` variant.
    /// `Conflict` 变体.
    #[error("conflict: {0}")]
    Conflict(String),
    /// `Unavailable` variant.
    /// `Unavailable` 变体.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// `Internal` variant.
    /// `Internal` 变体.
    #[error("internal error: {0}")]
    Internal(String),
}
