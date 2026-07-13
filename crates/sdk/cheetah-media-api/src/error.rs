use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Boxed string to keep `MediaError` small enough for `Result<T, MediaError>`.
///
/// 将字符串装箱，使 `MediaError` 保持足够小。
type StrBox = Box<str>;

/// Stable error codes used by the media-domain API.
///
/// Adapters are responsible for mapping these codes to their protocol-specific
/// representations. The domain layer does not carry HTTP status codes.
///
/// 媒体领域 API 使用的稳定错误码。
///
/// Adapter 负责将这些码映射到各自的协议表示。领域层不携带 HTTP 状态码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaErrorCode {
    InvalidArgument,
    Unauthenticated,
    PermissionDenied,
    NotFound,
    AlreadyExists,
    Conflict,
    Busy,
    Timeout,
    Unavailable,
    Unsupported,
    StorageFailed,
    ProtocolFailed,
    Internal,
}

impl MediaErrorCode {
    /// Stable machine-readable string for the error code.
    ///
    /// 错误码的稳定机器可读字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaErrorCode::InvalidArgument => "invalid_argument",
            MediaErrorCode::Unauthenticated => "unauthenticated",
            MediaErrorCode::PermissionDenied => "permission_denied",
            MediaErrorCode::NotFound => "not_found",
            MediaErrorCode::AlreadyExists => "already_exists",
            MediaErrorCode::Conflict => "conflict",
            MediaErrorCode::Busy => "busy",
            MediaErrorCode::Timeout => "timeout",
            MediaErrorCode::Unavailable => "unavailable",
            MediaErrorCode::Unsupported => "unsupported",
            MediaErrorCode::StorageFailed => "storage_failed",
            MediaErrorCode::ProtocolFailed => "protocol_failed",
            MediaErrorCode::Internal => "internal",
        }
    }

    /// ZLMediaKit-compatible legacy numeric code.
    ///
    /// ZLMediaKit 兼容的旧数字码。
    pub fn legacy_code(&self) -> i32 {
        match self {
            MediaErrorCode::InvalidArgument => -300,
            MediaErrorCode::Unauthenticated => -100,
            MediaErrorCode::PermissionDenied => -100,
            MediaErrorCode::NotFound => -500,
            MediaErrorCode::AlreadyExists => -300,
            MediaErrorCode::Conflict => -300,
            MediaErrorCode::Busy => -400,
            MediaErrorCode::Timeout => -400,
            MediaErrorCode::Unavailable => -400,
            MediaErrorCode::Unsupported => -501,
            MediaErrorCode::StorageFailed => -200,
            MediaErrorCode::ProtocolFailed => -400,
            MediaErrorCode::Internal => -400,
        }
    }
}

impl fmt::Display for MediaErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

use std::fmt;

/// Domain error returned by media API operations.
///
/// 媒体 API 操作返回的领域错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaError {
    pub code: MediaErrorCode,
    pub message: StrBox,
    pub retryable: bool,
    pub request_id: Option<StrBox>,
    pub correlation_id: Option<StrBox>,
    pub details: Box<HashMap<String, serde_json::Value>>,
}

impl MediaError {
    /// Create an error with the given code and message.
    ///
    /// 用给定码和消息创建错误。
    pub fn new(code: MediaErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into().into_boxed_str(),
            retryable: false,
            request_id: None,
            correlation_id: None,
            details: Box::new(HashMap::new()),
        }
    }

    /// Invalid argument error.
    ///
    /// 参数错误。
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::InvalidArgument, message)
    }

    /// Not found error.
    ///
    /// 未找到错误。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::NotFound, message)
    }

    /// Already exists error.
    ///
    /// 已存在错误。
    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::AlreadyExists, message)
    }

    /// Conflict error.
    ///
    /// 冲突错误。
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::Conflict, message)
    }

    /// Unsupported capability error.
    ///
    /// 不支持的能力错误。
    pub fn unsupported(capability: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::Unsupported, capability)
    }

    /// Unavailable error.
    ///
    /// 不可用错误。
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::Unavailable, message)
    }

    /// Internal error.
    ///
    /// 内部错误。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::Internal, message)
    }

    /// Mark the error as retryable.
    ///
    /// 标记错误为可重试。
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Attach a request id.
    ///
    /// 附加 request id。
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into().into_boxed_str());
        self
    }

    /// Attach a correlation id.
    ///
    /// 附加 correlation id。
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into().into_boxed_str());
        self
    }

    /// Attach a detail entry.
    ///
    /// 附加 detail 项。
    pub fn with_detail(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    /// Convenience error for an unsupported capability.
    ///
    /// 不支持能力的便捷错误。
    pub fn unsupported_capability(capability: &str) -> Self {
        Self::unsupported(format!("capability {capability} is not available"))
    }
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MediaError {}

/// Result type alias for media-domain operations.
///
/// 媒体领域操作的结果类型别名。
pub type Result<T> = std::result::Result<T, MediaError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_error_codes_are_stable() {
        assert_eq!(MediaErrorCode::Unsupported.legacy_code(), -501);
        assert_eq!(MediaErrorCode::InvalidArgument.legacy_code(), -300);
        assert_eq!(MediaErrorCode::NotFound.legacy_code(), -500);
    }

    #[test]
    fn unsupported_capability_has_expected_message() {
        let err = MediaError::unsupported_capability("record");
        assert_eq!(err.code, MediaErrorCode::Unsupported);
        assert!(err.message.contains("record"));
    }
}
