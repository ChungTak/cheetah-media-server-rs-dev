use base64::Engine;
use cheetah_codec::RtmpFlvPlayMode;
use sha1::{Digest, Sha1};

use crate::HttpFlvCoreError;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `HttpMethod` enumeration.
/// `HttpMethod` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Options,
    Other,
}

/// `HttpFlvTransport` enumeration.
/// `HttpFlvTransport` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvTransport {
    Http,
    WebSocket,
}

/// Mode selecting `HTTP FLV Query` behavior.
/// 选择 `HTTP FLV Query` 行为的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvQueryMode {
    Normal,
    Enhanced,
    FastPts,
}

impl HttpFlvQueryMode {
    /// Converts to `RTMP play mode` representation.
    /// 转换为 `RTMP play mode` 表示。
    pub fn to_rtmp_play_mode(self) -> RtmpFlvPlayMode {
        match self {
            Self::Enhanced => RtmpFlvPlayMode::Enhanced,
            Self::Normal | Self::FastPts => RtmpFlvPlayMode::Normal,
        }
    }
}

/// `StreamKeyParts` data structure.
/// `StreamKeyParts` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamKeyParts {
    pub namespace: String,
    pub stream_path: String,
}

/// Request for `Parsed Play`.
/// `Parsed Play` 的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlayRequest {
    pub stream_key: StreamKeyParts,
    pub mode: HttpFlvQueryMode,
}

/// `HttpRequestHead` data structure.
/// `HttpRequestHead` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestHead {
    pub method: HttpMethod,
    pub method_raw: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl HttpRequestHead {
    /// `header` function of `HttpRequestHead`.
    /// `HttpRequestHead` 的 `header` 函数。
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .rfind(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Returns `true` if `websocket upgrade` is true.
    /// 当 `websocket upgrade` 为真时返回 `true`。
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
/// `HttpResponseHead` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponseHead {
    pub status_code: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
}

/// Message used by `Web Socket`.
/// `Web Socket` 使用的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Binary(bytes::Bytes),
    Close,
    Ping(bytes::Bytes),
    Pong(bytes::Bytes),
    Text(String),
}

/// Parses `play request target` from input.
/// 从输入解析 `play request target`。
pub fn parse_play_request_target(target: &str) -> Result<ParsedPlayRequest, HttpFlvCoreError> {
    let (path, query) = split_target_query(target);
    if !path.ends_with(".flv") {
        return Err(HttpFlvCoreError::InvalidFlvPath {
            path: path.to_string(),
        });
    }

    let trimmed = path.trim_start_matches('/');
    let without_suffix =
        trimmed
            .strip_suffix(".flv")
            .ok_or_else(|| HttpFlvCoreError::InvalidFlvPath {
                path: path.to_string(),
            })?;
    let (namespace, stream_path) =
        without_suffix
            .split_once('/')
            .ok_or_else(|| HttpFlvCoreError::InvalidPath {
                path: path.to_string(),
            })?;
    let namespace = namespace.trim_matches('/');
    let stream_path = stream_path.trim_matches('/');
    if namespace.is_empty() {
        return Err(HttpFlvCoreError::EmptyNamespace);
    }
    if stream_path.is_empty() {
        return Err(HttpFlvCoreError::EmptyStreamPath);
    }

    let mode = parse_query_mode(query)?;
    Ok(ParsedPlayRequest {
        stream_key: StreamKeyParts {
            namespace: namespace.to_string(),
            stream_path: stream_path.to_string(),
        },
        mode,
    })
}

/// Validates the `websocket upgrade` and returns errors if invalid.
/// 验证 `websocket upgrade`，无效时返回错误。
pub fn validate_websocket_upgrade(head: &HttpRequestHead) -> Result<String, HttpFlvCoreError> {
    let Some(version) = head.header("Sec-WebSocket-Version") else {
        return Err(HttpFlvCoreError::InvalidWebSocketVersion);
    };
    if version.trim() != "13" {
        return Err(HttpFlvCoreError::InvalidWebSocketVersion);
    }
    let Some(key) = head.header("Sec-WebSocket-Key") else {
        return Err(HttpFlvCoreError::MissingWebSocketKey);
    };
    websocket_accept_key(key)
}

/// `websocket_accept_key` function.
/// `websocket_accept_key` 函数。
pub fn websocket_accept_key(client_key: &str) -> Result<String, HttpFlvCoreError> {
    let key = client_key.trim();
    if key.is_empty() {
        return Err(HttpFlvCoreError::MissingWebSocketKey);
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

fn parse_query_mode(query: &str) -> Result<HttpFlvQueryMode, HttpFlvCoreError> {
    for part in query.split('&') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or_default();
        let value = kv.next().unwrap_or_default();
        if !key.eq_ignore_ascii_case("type") {
            continue;
        }
        if value.eq_ignore_ascii_case("enhanced") {
            return Ok(HttpFlvQueryMode::Enhanced);
        }
        if value.eq_ignore_ascii_case("fastPts") {
            return Ok(HttpFlvQueryMode::FastPts);
        }
        if value.is_empty() {
            return Ok(HttpFlvQueryMode::Normal);
        }
        return Err(HttpFlvCoreError::InvalidPlayMode {
            value: value.to_string(),
        });
    }
    Ok(HttpFlvQueryMode::Normal)
}
