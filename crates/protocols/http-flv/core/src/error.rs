#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum HttpFlvCoreError {
    #[error("method is not supported: {method}")]
    UnsupportedMethod { method: String },
    #[error("request path is invalid: {path}")]
    InvalidPath { path: String },
    #[error("request path must end with .flv: {path}")]
    InvalidFlvPath { path: String },
    #[error("stream namespace is empty")]
    EmptyNamespace,
    #[error("stream path is empty")]
    EmptyStreamPath,
    #[error("unsupported play mode query: {value}")]
    InvalidPlayMode { value: String },
    #[error("websocket upgrade requires Sec-WebSocket-Version: 13")]
    InvalidWebSocketVersion,
    #[error("websocket upgrade requires Sec-WebSocket-Key")]
    MissingWebSocketKey,
    #[error("websocket connection is not in binary transport mode")]
    NotWebSocketTransport,
    #[error("http connection is not in stream transport mode")]
    NotHttpTransport,
    #[error("FLV demux failed: {0}")]
    FlvDemux(String),
}
