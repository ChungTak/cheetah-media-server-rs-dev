use cheetah_media_api::error::{MediaError, MediaErrorCode};

/// Errors returned by HTTP media adapters.
///
/// HTTP 媒体 adapter 返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdapterError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("media error: {0}")]
    Media(#[from] MediaError),
    #[error("serialization failed: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for AdapterError {
    fn from(err: serde_json::Error) -> Self {
        AdapterError::InvalidRequest(err.to_string())
    }
}

impl From<AdapterError> for cheetah_sdk::SdkError {
    fn from(err: AdapterError) -> Self {
        match err {
            AdapterError::InvalidRequest(msg) => cheetah_sdk::SdkError::InvalidArgument(msg),
            AdapterError::Media(e) => cheetah_sdk::SdkError::Internal(e.message.to_string()),
            AdapterError::Serialization(msg) => cheetah_sdk::SdkError::Internal(msg),
        }
    }
}

/// Convert a media-domain error into a native HTTP response.
///
/// 将媒体领域错误转换为 native HTTP 响应。
pub fn native_error_response(
    err: &MediaError,
    request_id: Option<&str>,
) -> (u16, serde_json::Value) {
    let status = match err.code {
        MediaErrorCode::InvalidArgument => 400,
        MediaErrorCode::Unauthenticated => 401,
        MediaErrorCode::PermissionDenied => 403,
        MediaErrorCode::NotFound => 404,
        MediaErrorCode::AlreadyExists => 409,
        MediaErrorCode::Conflict => 409,
        MediaErrorCode::StaleOwner => 409,
        MediaErrorCode::VersionMismatch => 409,
        MediaErrorCode::Busy => 503,
        MediaErrorCode::RateLimited => 429,
        MediaErrorCode::Timeout => 504,
        MediaErrorCode::Cancelled => 499,
        MediaErrorCode::Unavailable => 503,
        MediaErrorCode::Unsupported => 501,
        MediaErrorCode::CursorExpired => 400,
        MediaErrorCode::StorageFailed => 500,
        MediaErrorCode::ProtocolFailed => 500,
        MediaErrorCode::Internal => 500,
        MediaErrorCode::UnknownOutcome => 500,
        _ => 500,
    };
    let body = serde_json::json!({
        "error": {
            "code": err.code.as_str(),
            "message": err.message,
            "retryable": err.retryable,
            "request_id": request_id,
            "details": err.details,
        }
    });
    (status, body)
}

/// Convert a media-domain error into a ZLMediaKit-compatible response.
///
/// 将媒体领域错误转换为 ZLMediaKit 兼容响应。
pub fn zlm_error_response(err: &MediaError) -> serde_json::Value {
    serde_json::json!({
        "code": err.code.legacy_code(),
        "msg": err.message,
    })
}
