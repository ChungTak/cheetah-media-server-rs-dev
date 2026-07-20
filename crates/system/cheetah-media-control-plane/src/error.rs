//! Control-plane error type.
//!
//! 控制面错误类型。

use cheetah_media_api::error::{MediaError, MediaErrorCode};
use thiserror::Error;

/// Errors returned by the control-plane facade and store implementations.
///
/// 控制面 facade 与 store 实现返回的错误。
#[derive(Debug, Error, Clone, PartialEq)]
pub enum ControlPlaneError {
    /// A media-domain error propagated from `cheetah-media-api`.
    #[error("{0}")]
    Media(#[from] MediaError),
    /// The store is unavailable for a transient or persistent reason.
    #[error("store unavailable: {0}")]
    StoreUnavailable(String),
    /// An idempotency record is in an unexpected state.
    #[error("idempotency record is in an invalid state")]
    InvalidIdempotencyState,
    /// Serialization or deserialization of a stored value failed.
    #[error("serialization failed: {0}")]
    Serialization(String),
    /// A controlled resource or idempotency key already exists.
    #[error("conflict: {0}")]
    Conflict(String),
    /// An argument was invalid for the requested operation.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The requested controlled resource was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// The runtime shut down before the blocking task completed.
    #[error("runtime shutdown")]
    RuntimeShutdown,
    /// A runtime operation failed.
    #[error("runtime error: {0}")]
    RuntimeError(String),
    /// An internal control-plane error.
    #[error("internal error: {0}")]
    Internal(String),
    /// A database operation failed.
    #[error("database error: {0}")]
    Db(String),
}

impl ControlPlaneError {
    /// Return the stable media error code for the failure.
    pub fn code(&self) -> MediaErrorCode {
        match self {
            ControlPlaneError::Media(e) => e.code,
            ControlPlaneError::StoreUnavailable(_) => MediaErrorCode::Unavailable,
            ControlPlaneError::InvalidIdempotencyState => MediaErrorCode::Conflict,
            ControlPlaneError::Serialization(_) => MediaErrorCode::StorageFailed,
            ControlPlaneError::Conflict(_) => MediaErrorCode::Conflict,
            ControlPlaneError::InvalidArgument(_) => MediaErrorCode::InvalidArgument,
            ControlPlaneError::NotFound(_) => MediaErrorCode::NotFound,
            ControlPlaneError::RuntimeShutdown => MediaErrorCode::Unavailable,
            ControlPlaneError::RuntimeError(_) => MediaErrorCode::Internal,
            ControlPlaneError::Internal(_) => MediaErrorCode::Internal,
            ControlPlaneError::Db(_) => MediaErrorCode::StorageFailed,
        }
    }
}
