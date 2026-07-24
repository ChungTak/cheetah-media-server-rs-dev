use thiserror::Error;

/// Errors that can be returned by the HLS core.
///
/// HLS core 可能返回的错误。
#[derive(Debug, Error)]
pub enum HlsCoreError {
    /// Request target path could not be parsed into a known HLS resource.
    ///
    /// 请求目标路径无法解析为已知的 HLS 资源。
    #[error("invalid HLS path: {path}")]
    InvalidPath { path: String },
    /// The requested stream does not exist or is not published.
    ///
    /// 请求的流不存在或未发布。
    #[error("stream not found: {stream_key}")]
    StreamNotFound { stream_key: String },
    /// The requested segment/part has expired or was never produced.
    ///
    /// 请求的分片/片段已过期或从未生成。
    #[error("segment not found: {name}")]
    SegmentNotFound { name: String },
    /// Playlist is not yet ready because the first segments are still being produced.
    ///
    /// 播放列表尚未就绪，因为前几个分片仍在生成中。
    #[error("not ready: waiting for initial segments")]
    NotReady,
    /// Timestamp or cue timing is invalid (e.g. end before start, or non-monotonic segment boundary).
    ///
    /// 时间戳或 cue 时间无效。
    #[error("invalid timestamp")]
    InvalidTimestamp,
    /// Playlist text is malformed or contains values that violate the HLS spec.
    ///
    /// 播放列表文本格式错误或包含违反 HLS 规范的内容。
    #[error("invalid playlist: {reason}")]
    InvalidPlaylist { reason: String },
}
