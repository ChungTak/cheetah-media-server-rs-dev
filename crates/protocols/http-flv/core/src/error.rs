/// `HttpFlvCoreError` enumeration.
/// `HttpFlvCoreError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum HttpFlvCoreError {
    /// `UnsupportedMethod` variant.
    /// `UnsupportedMethod` 变体.
    #[error("method is not supported: {method}")]
    UnsupportedMethod { method: String },
    /// `InvalidPath` variant.
    /// `InvalidPath` 变体.
    #[error("request path is invalid: {path}")]
    InvalidPath { path: String },
    /// `InvalidFlvPath` variant.
    /// `InvalidFlvPath` 变体.
    #[error("request path must end with .flv: {path}")]
    InvalidFlvPath { path: String },
    /// `EmptyNamespace` variant.
    /// `EmptyNamespace` 变体.
    #[error("stream namespace is empty")]
    EmptyNamespace,
    /// `EmptyStreamPath` variant.
    /// `EmptyStreamPath` 变体.
    #[error("stream path is empty")]
    EmptyStreamPath,
    /// `InvalidPlayMode` variant.
    /// `InvalidPlayMode` 变体.
    #[error("unsupported play mode query: {value}")]
    InvalidPlayMode { value: String },
    /// `InvalidWebSocketVersion` variant.
    /// `InvalidWebSocketVersion` 变体.
    #[error("websocket upgrade requires Sec-WebSocket-Version: 13")]
    InvalidWebSocketVersion,
    /// `MissingWebSocketKey` variant.
    /// `MissingWebSocketKey` 变体.
    #[error("websocket upgrade requires Sec-WebSocket-Key")]
    MissingWebSocketKey,
    /// `NotWebSocketTransport` variant.
    /// `NotWebSocketTransport` 变体.
    #[error("websocket connection is not in binary transport mode")]
    NotWebSocketTransport,
    /// `NotHttpTransport` variant.
    /// `NotHttpTransport` 变体.
    #[error("http connection is not in stream transport mode")]
    NotHttpTransport,
    /// `FlvDemux` variant.
    /// `FlvDemux` 变体.
    #[error("FLV demux failed: {0}")]
    FlvDemux(String),
}
