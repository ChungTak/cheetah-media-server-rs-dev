use thiserror::Error;

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

#[derive(Debug, Clone)]
pub enum Gb28181Diagnostic {
    RegisterFailed { device_id: String, reason: String },
    KeepaliveTimeout { device_id: String },
    InviteTimeout { session_key: String },
    DialogClosed { session_key: String, reason: String },
    SyntaxWarning { raw: String, issue: String },
}
