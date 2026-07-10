use thiserror::Error;

/// SDK errors returned by module-facing APIs.
///
/// 模块 API 返回的 SDK 错误。
#[derive(Debug, Error)]
pub enum SdkError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
}
