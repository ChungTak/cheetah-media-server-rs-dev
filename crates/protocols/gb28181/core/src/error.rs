use thiserror::Error;

/// Error returned by `Gb 28181 Core` operations.
/// `Gb 28181 Core` 操作返回的错误。
#[derive(Debug, Clone, Error)]
pub enum Gb28181CoreError {
    #[error("SIP syntax error: {0}")]
    SipSyntax(String),

    #[error("SDP parsing error: {0}")]
    SdpError(String),

    #[error("dialog state error: {0}")]
    DialogError(String),

    #[error("invalid transaction state: {0}")]
    TransactionError(String),
}

/// `Gb28181Diagnostic` enumeration.
/// `Gb28181Diagnostic` 枚举。
#[derive(Debug, Clone)]
pub enum Gb28181Diagnostic {
    RegisterFailed { device_id: String, reason: String },
    KeepaliveTimeout { device_id: String },
    InviteTimeout { session_key: String },
    DialogClosed { session_key: String, reason: String },
    SyntaxWarning { raw: String, issue: String },
}
