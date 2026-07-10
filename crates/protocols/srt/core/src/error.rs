use thiserror::Error;

pub type SrtCoreResult<T> = Result<T, SrtCoreError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SrtCoreError {
    #[error("invalid stream id: {0}")]
    InvalidStreamId(String),
    #[error("invalid SRT url: {0}")]
    InvalidUrl(String),
    #[error("invalid SRT config: {0}")]
    InvalidConfig(String),
    #[error("SRT connection error: {0}")]
    Connection(String),
}
