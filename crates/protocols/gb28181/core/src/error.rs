use thiserror::Error;

/// Errors returned by the GB28181 core state machine.
///
/// GB28181 核心状态机返回的错误。
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

/// Diagnostic events produced by the core state machine for observability.
///
/// 核心状态机为可观测性产生的诊断事件。
#[derive(Debug, Clone)]
pub enum Gb28181Diagnostic {
    RegisterFailed { device_id: String, reason: String },
    KeepaliveTimeout { device_id: String },
    InviteTimeout { session_key: String },
    DialogClosed { session_key: String, reason: String },
    SyntaxWarning { raw: String, issue: String },
}
