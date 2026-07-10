//! HTTP request parsing and WebSocket upgrade for TS protocol.

use base64::Engine;
use sha1::{Digest, Sha1};
use thiserror::Error;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `TsCoreError` enumeration.
/// `TsCoreError` 枚举.
#[derive(Debug, Error)]
pub enum TsCoreError {
    /// `InvalidTsPath` variant.
    /// `InvalidTsPath` 变体.
    #[error("invalid .ts path: {path}")]
    InvalidTsPath { path: String },
    /// `EmptyNamespace` variant.
    /// `EmptyNamespace` 变体.
    #[error("empty namespace")]
    EmptyNamespace,
    /// `EmptyStreamPath` variant.
    /// `EmptyStreamPath` 变体.
    #[error("empty stream path")]
    EmptyStreamPath,
    /// `InvalidWebSocketVersion` variant.
    /// `InvalidWebSocketVersion` 变体.
    #[error("invalid WebSocket version")]
    InvalidWebSocketVersion,
    /// `MissingWebSocketKey` variant.
    /// `MissingWebSocketKey` 变体.
    #[error("missing Sec-WebSocket-Key")]
    MissingWebSocketKey,
    /// `InvalidPath` variant.
    /// `InvalidPath` 变体.
    #[error("path too long or contains invalid characters")]
    InvalidPath,
}

/// `HttpMethod` enumeration.
/// `HttpMethod` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `Get` variant.
    /// `Get` 变体.
    Get,
    /// `Head` variant.
    /// `Head` 变体.
    Head,
    /// `Options` variant.
    /// `Options` 变体.
    Options,
    /// `Other` variant.
    /// `Other` 变体.
    Other,
}

/// `TsTransport` enumeration.
/// `TsTransport` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsTransport {
    /// `Http` variant.
    /// `Http` 变体.
    Http,
    /// `WebSocket` variant.
    /// `WebSocket` 变体.
    WebSocket,
}

/// `StreamKeyParts` data structure.
/// `StreamKeyParts` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamKeyParts {
    /// `namespace` field of type `String`.
    /// `namespace` 字段，类型为 `String`.
    pub namespace: String,
    /// `stream_path` field of type `String`.
    /// `stream_path` 字段，类型为 `String`.
    pub stream_path: String,
}

/// `ParsedTsRequest` data structure.
/// `ParsedTsRequest` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTsRequest {
    /// `stream_key` field of type `StreamKeyParts`.
    /// `stream_key` 字段，类型为 `StreamKeyParts`.
    pub stream_key: StreamKeyParts,
}

/// `HttpRequestHead` data structure.
/// `HttpRequestHead` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestHead {
    /// `method` field of type `HttpMethod`.
    /// `方法` 字段，类型为 `HttpMethod`.
    pub method: HttpMethod,
    /// `method_raw` field of type `String`.
    /// `method_raw` 字段，类型为 `String`.
    pub method_raw: String,
    /// `target` field of type `String`.
    /// `target` 字段，类型为 `String`.
    pub target: String,
    /// `headers` field.
    /// `headers` 字段.
    pub headers: Vec<(String, String)>,
}

impl HttpRequestHead {
    /// `header` function.
    /// `header` 函数.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .rfind(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Returns `true` if `websocket_upgrade` is true.
    /// 返回 `真` 如果 `websocket_upgrade` is 真.
    pub fn is_websocket_upgrade(&self) -> bool {
        let Some(connection) = self.header("Connection") else {
            return false;
        };
        let Some(upgrade) = self.header("Upgrade") else {
            return false;
        };
        connection
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
            && upgrade.eq_ignore_ascii_case("websocket")
    }
}

/// `HttpResponseHead` data structure.
/// `HttpResponseHead` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponseHead {
    /// `status_code` field of type `u16`.
    /// `status_code` 字段，类型为 `u16`.
    pub status_code: u16,
    /// `reason` field of type `&'static str`.
    /// `reason` 字段，类型为 `&'static str`.
    pub reason: &'static str,
    /// `headers` field.
    /// `headers` 字段.
    pub headers: Vec<(String, String)>,
}

/// `WebSocketMessage` enumeration.
/// `WebSocketMessage` 枚举.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    /// `Binary` variant.
    /// `Binary` 变体.
    Binary(bytes::Bytes),
    /// `Close` variant.
    /// `Close` 变体.
    Close,
    /// `Ping` variant.
    /// `Ping` 变体.
    Ping(bytes::Bytes),
    /// `Pong` variant.
    /// `Pong` 变体.
    Pong(bytes::Bytes),
    /// `Text` variant.
    /// `Text` 变体.
    Text(String),
    /// Client sent an unmasked frame (protocol violation).
    Unmasked,
}

/// Parse a `.ts` or `.live.ts` request target into stream key parts.
pub fn parse_ts_request_target(target: &str) -> Result<ParsedTsRequest, TsCoreError> {
    let (path, _query) = split_target_query(target);

    // Reject dangerous paths
    if path.contains("..") || path.contains('%') || path.len() > 512 {
        return Err(TsCoreError::InvalidPath);
    }

    // Support both .ts and .live.ts suffixes
    let suffix = if path.ends_with(".live.ts") {
        ".live.ts"
    } else if path.ends_with(".ts") {
        ".ts"
    } else {
        return Err(TsCoreError::InvalidTsPath {
            path: path.to_string(),
        });
    };

    let trimmed = path.trim_start_matches('/');
    let without_suffix =
        trimmed
            .strip_suffix(suffix)
            .ok_or_else(|| TsCoreError::InvalidTsPath {
                path: path.to_string(),
            })?;

    let (namespace, stream_path) =
        without_suffix
            .split_once('/')
            .ok_or_else(|| TsCoreError::InvalidTsPath {
                path: path.to_string(),
            })?;

    if namespace.is_empty() {
        return Err(TsCoreError::EmptyNamespace);
    }
    if stream_path.is_empty() {
        return Err(TsCoreError::EmptyStreamPath);
    }

    Ok(ParsedTsRequest {
        stream_key: StreamKeyParts {
            namespace: namespace.to_string(),
            stream_path: stream_path.to_string(),
        },
    })
}

/// `validate_websocket_upgrade` function.
/// `validate_websocket_upgrade` 函数.
pub fn validate_websocket_upgrade(head: &HttpRequestHead) -> Result<String, TsCoreError> {
    let Some(version) = head.header("Sec-WebSocket-Version") else {
        return Err(TsCoreError::InvalidWebSocketVersion);
    };
    if version.trim() != "13" {
        return Err(TsCoreError::InvalidWebSocketVersion);
    }
    let Some(key) = head.header("Sec-WebSocket-Key") else {
        return Err(TsCoreError::MissingWebSocketKey);
    };
    websocket_accept_key(key)
}

/// `websocket_accept_key` function.
/// `websocket_accept_key` 函数.
pub fn websocket_accept_key(client_key: &str) -> Result<String, TsCoreError> {
    let key = client_key.trim();
    if key.is_empty() {
        return Err(TsCoreError::MissingWebSocketKey);
    }
    let mut sha1 = Sha1::new();
    sha1.update(key.as_bytes());
    sha1.update(WEBSOCKET_ACCEPT_MAGIC.as_bytes());
    let digest = sha1.finalize();
    Ok(base64::engine::general_purpose::STANDARD.encode(digest))
}

fn split_target_query(target: &str) -> (&str, &str) {
    if let Some(index) = target.find('?') {
        (&target[..index], &target[index + 1..])
    } else {
        (target, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_ts_path() {
        let parsed = parse_ts_request_target("/live/stream.ts").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "stream");
    }

    #[test]
    fn parses_live_ts_path() {
        let parsed = parse_ts_request_target("/live/stream.live.ts").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "stream");
    }

    #[test]
    fn parses_nested_stream_path() {
        let parsed = parse_ts_request_target("/app/sub/path.ts").unwrap();
        assert_eq!(parsed.stream_key.namespace, "app");
        assert_eq!(parsed.stream_key.stream_path, "sub/path");
    }

    #[test]
    fn parses_nested_live_ts_path() {
        let parsed = parse_ts_request_target("/app/sub/path.live.ts").unwrap();
        assert_eq!(parsed.stream_key.namespace, "app");
        assert_eq!(parsed.stream_key.stream_path, "sub/path");
    }

    #[test]
    fn parses_with_query_string() {
        let parsed = parse_ts_request_target("/live/test.ts?token=abc").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "test");
    }

    #[test]
    fn rejects_non_ts_path() {
        assert!(parse_ts_request_target("/live/stream.flv").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(parse_ts_request_target("/live/../etc/passwd.ts").is_err());
    }

    #[test]
    fn rejects_empty_namespace() {
        assert!(parse_ts_request_target("//stream.ts").is_err());
    }

    #[test]
    fn websocket_accept_key_correct() {
        let accept = websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ==").unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
