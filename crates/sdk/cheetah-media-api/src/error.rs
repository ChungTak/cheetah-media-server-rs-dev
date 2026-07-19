use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::fencing::ControlledResourceRef;

/// An error attached to a media operation outcome (e.g., the last failure recorded
/// on a session). Keeps the same stable code as `MediaError` but is intentionally
/// serializable and public.
///
/// 附加在媒体操作结果上的错误（如会话记录的最后失败）。使用与 `MediaError` 相同的稳定码，
/// 但可被序列化并公开。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaOperationError {
    pub code: MediaErrorCode,
    pub message: String,
}

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
    StaleOwner,
    Busy,
    RateLimited,
    Timeout,
    Cancelled,
    Unavailable,
    Unsupported,
    VersionMismatch,
    CursorExpired,
    StorageFailed,
    ProtocolFailed,
    Internal,
    UnknownOutcome,
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
            MediaErrorCode::StaleOwner => "stale_owner",
            MediaErrorCode::Busy => "busy",
            MediaErrorCode::RateLimited => "rate_limited",
            MediaErrorCode::Timeout => "timeout",
            MediaErrorCode::Cancelled => "cancelled",
            MediaErrorCode::Unavailable => "unavailable",
            MediaErrorCode::Unsupported => "unsupported",
            MediaErrorCode::VersionMismatch => "version_mismatch",
            MediaErrorCode::CursorExpired => "cursor_expired",
            MediaErrorCode::StorageFailed => "storage_failed",
            MediaErrorCode::ProtocolFailed => "protocol_failed",
            MediaErrorCode::Internal => "internal",
            MediaErrorCode::UnknownOutcome => "unknown_outcome",
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
            MediaErrorCode::StaleOwner => -300,
            MediaErrorCode::Busy => -400,
            MediaErrorCode::RateLimited => -400,
            MediaErrorCode::Timeout => -400,
            MediaErrorCode::Cancelled => -400,
            MediaErrorCode::Unavailable => -400,
            MediaErrorCode::Unsupported => -501,
            MediaErrorCode::VersionMismatch => -501,
            MediaErrorCode::CursorExpired => -300,
            MediaErrorCode::StorageFailed => -200,
            MediaErrorCode::ProtocolFailed => -400,
            MediaErrorCode::Internal => -400,
            MediaErrorCode::UnknownOutcome => -400,
        }
    }
}

impl fmt::Display for MediaErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

use std::fmt;

/// Whether a failed operation left side effects behind.
///
/// 失败操作是否留下了副作用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EffectOutcome {
    /// No port, file, task, lease, or controlled state was left behind.
    ///
    /// 没有留下端口、文件、任务、租约或受控状态。
    NotApplied,
    /// A side effect occurred; the caller should query or compensate.
    ///
    /// 副作用已生效；调用方应查询或补偿。
    Applied,
    /// The outcome is unknown; the caller must reconcile, not auto-retry.
    ///
    /// 结果未知；调用方必须对账，不能自动重试。
    #[default]
    Unknown,
}

impl From<MediaErrorCode> for EffectOutcome {
    fn from(code: MediaErrorCode) -> Self {
        match code {
            // Validation, auth, and pre-flight guard errors are safe to treat as
            // not-applied because they occur before any resource is allocated.
            MediaErrorCode::InvalidArgument
            | MediaErrorCode::Unauthenticated
            | MediaErrorCode::PermissionDenied
            | MediaErrorCode::NotFound
            | MediaErrorCode::AlreadyExists
            | MediaErrorCode::Conflict
            | MediaErrorCode::StaleOwner
            | MediaErrorCode::Busy
            | MediaErrorCode::RateLimited
            | MediaErrorCode::Unsupported
            | MediaErrorCode::VersionMismatch
            | MediaErrorCode::CursorExpired => EffectOutcome::NotApplied,
            // Outcomes that may occur after side effects or where we cannot prove
            // the state must remain Unknown so the caller reconciles.
            MediaErrorCode::Timeout
            | MediaErrorCode::Cancelled
            | MediaErrorCode::Unavailable
            | MediaErrorCode::StorageFailed
            | MediaErrorCode::ProtocolFailed
            | MediaErrorCode::Internal
            | MediaErrorCode::UnknownOutcome => EffectOutcome::Unknown,
        }
    }
}

/// A field-level validation violation.
///
/// 字段级校验错误。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FieldViolation {
    pub field: String,
    pub description: String,
    pub value: Option<String>,
}

/// Domain error returned by media API operations.
///
/// 媒体 API 操作返回的领域错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "MediaErrorRepr")]
pub struct MediaError {
    pub code: MediaErrorCode,
    pub message: StrBox,
    pub retryable: bool,
    pub request_id: Option<StrBox>,
    pub correlation_id: Option<StrBox>,
    pub details: Box<HashMap<String, serde_json::Value>>,
    /// Whether the operation left side effects behind.
    #[serde(default)]
    pub outcome: EffectOutcome,
    /// Reference to the controlled resource, when the error relates to one.
    #[serde(default)]
    pub resource_ref: Option<Box<ControlledResourceRef>>,
    /// Hint for retry-after in milliseconds, when the error is retryable.
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    /// Field-level violations for InvalidArgument / CursorExpired responses.
    #[serde(default)]
    pub violations: Box<Vec<FieldViolation>>,
}

/// Deserialization representation that re-derives `outcome` from `code` when the
/// field is missing, so legacy payloads are interpreted consistently with freshly
/// constructed errors.
///
/// 反序列化表示：当 `outcome` 缺失时从 `code` 重新推导，保证旧 payload 与新建错误语义一致。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct MediaErrorRepr {
    code: MediaErrorCode,
    message: String,
    #[serde(default)]
    retryable: bool,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default)]
    details: HashMap<String, serde_json::Value>,
    #[serde(default)]
    outcome: Option<EffectOutcome>,
    #[serde(default)]
    resource_ref: Option<Box<ControlledResourceRef>>,
    #[serde(default)]
    retry_after_ms: Option<u64>,
    #[serde(default)]
    violations: Vec<FieldViolation>,
}

impl From<MediaErrorRepr> for MediaError {
    fn from(repr: MediaErrorRepr) -> Self {
        Self {
            code: repr.code,
            message: repr.message.into_boxed_str(),
            retryable: repr.retryable,
            request_id: repr.request_id.map(|s| s.into_boxed_str()),
            correlation_id: repr.correlation_id.map(|s| s.into_boxed_str()),
            details: Box::new(repr.details),
            outcome: repr.outcome.unwrap_or_else(|| repr.code.into()),
            resource_ref: repr.resource_ref,
            retry_after_ms: repr.retry_after_ms,
            violations: Box::new(repr.violations),
        }
    }
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
            outcome: code.into(),
            resource_ref: None,
            retry_after_ms: None,
            violations: Box::new(Vec::new()),
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

    /// Stale owner / fencing error.
    ///
    /// 旧 owner / fencing 错误。
    pub fn stale_owner(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::StaleOwner, message)
    }

    /// Rate limited error.
    ///
    /// 限流错误。
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::RateLimited, message)
    }

    /// Cancelled error.
    ///
    /// 取消错误。
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::Cancelled, message)
    }

    /// Version mismatch error.
    ///
    /// 版本不匹配错误。
    pub fn version_mismatch(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::VersionMismatch, message)
    }

    /// Cursor expired error.
    ///
    /// 游标过期错误。
    pub fn cursor_expired(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::CursorExpired, message)
    }

    /// Unknown outcome error.
    ///
    /// 未知结果错误。
    pub fn unknown_outcome(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::UnknownOutcome, message)
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

    /// Storage operation failed.
    ///
    /// 存储操作失败。
    pub fn storage_failed(message: impl Into<String>) -> Self {
        Self::new(MediaErrorCode::StorageFailed, message)
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

    /// Set the effect outcome.
    ///
    /// 设置生效结果。
    pub fn with_outcome(mut self, outcome: EffectOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Attach a controlled resource reference.
    ///
    /// 附加受控资源引用。
    pub fn with_resource_ref(mut self, resource_ref: ControlledResourceRef) -> Self {
        self.resource_ref = Some(Box::new(resource_ref));
        self
    }

    /// Attach a retry-after hint in milliseconds.
    ///
    /// 附加 retry-after 提示（毫秒）。
    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    /// Add a field violation.
    ///
    /// 添加字段错误。
    pub fn with_violation(
        mut self,
        field: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.violations.push(FieldViolation {
            field: field.into(),
            description: description.into(),
            value: None,
        });
        self
    }

    /// Set the raw value for the most recently added violation.
    ///
    /// 为最近添加的 violation 设置原始值。
    pub fn with_violation_value(mut self, value: impl Into<String>) -> Self {
        if let Some(last) = self.violations.last_mut() {
            last.value = Some(value.into());
        }
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
        assert_eq!(MediaErrorCode::StaleOwner.legacy_code(), -300);
        assert_eq!(MediaErrorCode::RateLimited.legacy_code(), -400);
        assert_eq!(MediaErrorCode::CursorExpired.legacy_code(), -300);
    }

    #[test]
    fn unsupported_capability_has_expected_message() {
        let err = MediaError::unsupported_capability("record");
        assert_eq!(err.code, MediaErrorCode::Unsupported);
        assert!(err.message.contains("record"));
    }

    #[test]
    fn validation_errors_default_to_not_applied() {
        let err = MediaError::invalid_argument("bad input");
        assert_eq!(err.outcome, EffectOutcome::NotApplied);
    }

    #[test]
    fn runtime_errors_default_to_unknown_outcome() {
        let err = MediaError::internal("oops");
        assert_eq!(err.outcome, EffectOutcome::Unknown);
    }

    #[test]
    fn error_round_trips_through_serde() {
        let err = MediaError::rate_limited("slow down")
            .with_retryable(true)
            .with_retry_after(500)
            .with_violation("field", "too large");
        let json = serde_json::to_string(&err).unwrap();
        let decoded: MediaError = serde_json::from_str(&json).unwrap();
        assert_eq!(err.code, decoded.code);
        assert_eq!(err.outcome, decoded.outcome);
        assert_eq!(err.retry_after_ms, decoded.retry_after_ms);
        assert_eq!(err.violations.len(), decoded.violations.len());
    }

    #[test]
    fn effect_outcome_default_is_unknown() {
        assert_eq!(EffectOutcome::default(), EffectOutcome::Unknown);
    }

    #[test]
    fn legacy_error_deserializes_outcome_from_code() {
        let json = r#"{"code":"invalid_argument","message":"bad input","retryable":false}"#;
        let decoded: MediaError = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.code, MediaErrorCode::InvalidArgument);
        assert_eq!(decoded.outcome, EffectOutcome::NotApplied);
    }
}
