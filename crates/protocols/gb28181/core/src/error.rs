use thiserror::Error;

/// `Gb28181CoreError` enumeration.
/// `Gb28181CoreError` 枚举.
#[derive(Debug, Clone, Error)]
pub enum Gb28181CoreError {
    /// `SipSyntax` variant.
    /// `SipSyntax` 变体.
    #[error("SIP syntax error: {0}")]
    SipSyntax(String),

    /// `SdpError` variant.
    /// `SdpError` 变体.
    #[error("SDP parsing error: {0}")]
    SdpError(String),

    /// `DialogError` variant.
    /// `DialogError` 变体.
    #[error("dialog state error: {0}")]
    DialogError(String),

    /// `TransactionError` variant.
    /// `TransactionError` 变体.
    #[error("invalid transaction state: {0}")]
    TransactionError(String),
}

/// `Gb28181Diagnostic` enumeration.
/// `Gb28181Diagnostic` 枚举.
#[derive(Debug, Clone)]
pub enum Gb28181Diagnostic {
    /// `RegisterFailed` variant.
    /// `RegisterFailed` 变体.
    RegisterFailed { device_id: String, reason: String },
    /// `KeepaliveTimeout` variant.
    /// `KeepaliveTimeout` 变体.
    KeepaliveTimeout { device_id: String },
    /// `InviteTimeout` variant.
    /// `InviteTimeout` 变体.
    InviteTimeout { session_key: String },
    /// `DialogClosed` variant.
    /// `DialogClosed` 变体.
    DialogClosed { session_key: String, reason: String },
    /// `SyntaxWarning` variant.
    /// `SyntaxWarning` 变体.
    SyntaxWarning { raw: String, issue: String },
}
