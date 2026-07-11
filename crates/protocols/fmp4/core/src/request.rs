//! HTTP request parsing and WebSocket upgrade for the fMP4 protocol.
//!
//! fMP4 协议的 HTTP 请求解析与 WebSocket 升级。

use base64::Engine;
use sha1::{Digest, Sha1};
use thiserror::Error;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Error)]
/// Error cases returned by fMP4 core request parsing.
///
/// fMP4 core 请求解析返回的错误情况。
pub enum Fmp4CoreError {
    #[error("invalid .mp4 path: {path}")]
    InvalidMp4Path { path: String },
    #[error("empty namespace")]
    EmptyNamespace,
    #[error("empty stream path")]
    EmptyStreamPath,
    #[error("invalid WebSocket version")]
    InvalidWebSocketVersion,
    #[error("missing Sec-WebSocket-Key")]
    MissingWebSocketKey,
    #[error("path too long or contains invalid characters")]
    InvalidPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Supported HTTP methods for fMP4 requests.
///
/// fMP4 请求支持的 HTTP 方法。
pub enum HttpMethod {
    Get,
    Head,
    Options,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Transport used by the fMP4 client.
///
/// fMP4 客户端使用的传输方式。
pub enum Fmp4Transport {
    Http,
    WebSocket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed `namespace/stream` components from the request path.
///
/// 从请求路径解析的 `namespace/stream` 组成部分。
pub struct StreamKeyParts {
    pub namespace: String,
    pub stream_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed fMP4 request with the target stream key.
///
/// 解析后的 fMP4 请求，包含目标流密钥。
pub struct ParsedFmp4Request {
    pub stream_key: StreamKeyParts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed HTTP request head with method, target, and headers.
///
/// 解析后的 HTTP 请求头，包含方法、目标与头部。
pub struct HttpRequestHead {
    pub method: HttpMethod,
    pub method_raw: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

/// `HttpRequestHead` helpers: header lookup and WebSocket upgrade detection.
///
/// `HttpRequestHead` 辅助：头部查找与 WebSocket 升级检测。
impl HttpRequestHead {
    /// Look up a header value by name (case-insensitive).
    ///
    /// 按名称（不区分大小写）查找头部值。
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .rfind(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Check if the request headers indicate a WebSocket upgrade.
    ///
    /// 检查请求头是否表示 WebSocket 升级。
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

#[derive(Debug, Clone, PartialEq, Eq)]
/// HTTP response head used for handshake and error responses.
///
/// 用于握手与错误响应的 HTTP 响应头。
pub struct HttpResponseHead {
    pub status_code: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// WebSocket message variants decoded by the driver.
///
/// 驱动层解码的 WebSocket 消息变体。
pub enum WebSocketMessage {
    Binary(bytes::Bytes),
    Close,
    Ping(bytes::Bytes),
    Pong(bytes::Bytes),
    Text(String),
    Unmasked,
}

/// Parse a `.mp4` or `.live.mp4` request target into stream key parts.
///
/// 将 `.mp4` 或 `.live.mp4` 请求目标解析为流密钥组成部分。
pub fn parse_fmp4_request_target(target: &str) -> Result<ParsedFmp4Request, Fmp4CoreError> {
    let (path, _query) = split_target_query(target);

    if path.contains("..") || path.contains('%') || path.len() > 512 {
        return Err(Fmp4CoreError::InvalidPath);
    }

    let suffix = if path.ends_with(".live.mp4") {
        ".live.mp4"
    } else if path.ends_with(".mp4") {
        ".mp4"
    } else {
        return Err(Fmp4CoreError::InvalidMp4Path {
            path: path.to_string(),
        });
    };

    let trimmed = path.trim_start_matches('/');
    let without_suffix =
        trimmed
            .strip_suffix(suffix)
            .ok_or_else(|| Fmp4CoreError::InvalidMp4Path {
                path: path.to_string(),
            })?;

    let (namespace, stream_path) =
        without_suffix
            .split_once('/')
            .ok_or_else(|| Fmp4CoreError::InvalidMp4Path {
                path: path.to_string(),
            })?;

    if namespace.is_empty() {
        return Err(Fmp4CoreError::EmptyNamespace);
    }
    if stream_path.is_empty() {
        return Err(Fmp4CoreError::EmptyStreamPath);
    }

    Ok(ParsedFmp4Request {
        stream_key: StreamKeyParts {
            namespace: namespace.to_string(),
            stream_path: stream_path.to_string(),
        },
    })
}

/// Validate a WebSocket upgrade request and return the accept key.
///
/// 校验 WebSocket 升级请求并返回 accept key。
pub fn validate_websocket_upgrade(head: &HttpRequestHead) -> Result<String, Fmp4CoreError> {
    let Some(version) = head.header("Sec-WebSocket-Version") else {
        return Err(Fmp4CoreError::InvalidWebSocketVersion);
    };
    if version.trim() != "13" {
        return Err(Fmp4CoreError::InvalidWebSocketVersion);
    }
    let Some(key) = head.header("Sec-WebSocket-Key") else {
        return Err(Fmp4CoreError::MissingWebSocketKey);
    };
    websocket_accept_key(key)
}

/// Compute the RFC 6455 `Sec-WebSocket-Accept` value.
///
/// 计算 RFC 6455 的 `Sec-WebSocket-Accept` 值。
pub fn websocket_accept_key(client_key: &str) -> Result<String, Fmp4CoreError> {
    let key = client_key.trim();
    if key.is_empty() {
        return Err(Fmp4CoreError::MissingWebSocketKey);
    }
    let mut sha1 = Sha1::new();
    sha1.update(key.as_bytes());
    sha1.update(WEBSOCKET_ACCEPT_MAGIC.as_bytes());
    let digest = sha1.finalize();
    Ok(base64::engine::general_purpose::STANDARD.encode(digest))
}

/// Split the request target at the first `?` to separate path and query.
///
/// 在第一个 `?` 处分割请求目标，分离路径与查询。
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
    fn parses_valid_mp4_path() {
        let parsed = parse_fmp4_request_target("/live/stream.mp4").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "stream");
    }

    #[test]
    fn parses_live_mp4_path() {
        let parsed = parse_fmp4_request_target("/live/stream.live.mp4").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "stream");
    }

    #[test]
    fn parses_nested_stream_path() {
        let parsed = parse_fmp4_request_target("/app/sub/path.mp4").unwrap();
        assert_eq!(parsed.stream_key.namespace, "app");
        assert_eq!(parsed.stream_key.stream_path, "sub/path");
    }

    #[test]
    fn parses_with_query_string() {
        let parsed = parse_fmp4_request_target("/live/test.mp4?token=abc").unwrap();
        assert_eq!(parsed.stream_key.namespace, "live");
        assert_eq!(parsed.stream_key.stream_path, "test");
    }

    #[test]
    fn rejects_non_mp4_path() {
        assert!(parse_fmp4_request_target("/live/stream.flv").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(parse_fmp4_request_target("/live/../etc/passwd.mp4").is_err());
    }

    #[test]
    fn rejects_empty_namespace() {
        assert!(parse_fmp4_request_target("//stream.mp4").is_err());
    }

    #[test]
    fn websocket_accept_key_correct() {
        let accept = websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ==").unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
