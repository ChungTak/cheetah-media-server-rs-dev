use base64::Engine;
use cheetah_codec::RtmpFlvPlayMode;
use sha1::{Digest, Sha1};

use crate::HttpFlvCoreError;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `HttpMethod` enumeration.
/// `HttpMethod` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `Get` variant.
    /// `Get` 变体.
    Get,
    /// `Post` variant.
    /// `Post` 变体.
    Post,
    /// `Options` variant.
    /// `Options` 变体.
    Options,
    /// `Other` variant.
    /// `Other` 变体.
    Other,
}

/// `HttpFlvTransport` enumeration.
/// `HttpFlvTransport` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvTransport {
    /// `Http` variant.
    /// `Http` 变体.
    Http,
    /// `WebSocket` variant.
    /// `WebSocket` 变体.
    WebSocket,
}

/// `HttpFlvQueryMode` enumeration.
/// `HttpFlvQueryMode` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvQueryMode {
    /// `Normal` variant.
    /// `Normal` 变体.
    Normal,
    /// `Enhanced` variant.
    /// `Enhanced` 变体.
    Enhanced,
    /// `FastPts` variant.
    /// `FastPts` 变体.
    FastPts,
}

impl HttpFlvQueryMode {
    /// Converts to `rtmp_play_mode` representation.
    /// Converts 为 `rtmp_play_mode` 表示.
    pub fn to_rtmp_play_mode(self) -> RtmpFlvPlayMode {
        match self {
            Self::Enhanced => RtmpFlvPlayMode::Enhanced,
            Self::Normal | Self::FastPts => RtmpFlvPlayMode::Normal,
        }
    }
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

/// `ParsedPlayRequest` data structure.
/// `ParsedPlayRequest` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlayRequest {
    /// `stream_key` field of type `StreamKeyParts`.
    /// `stream_key` 字段，类型为 `StreamKeyParts`.
    pub stream_key: StreamKeyParts,
    /// `mode` field of type `HttpFlvQueryMode`.
    /// `mode` 字段，类型为 `HttpFlvQueryMode`.
    pub mode: HttpFlvQueryMode,
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
}

/// Parses `play_request_target` from input.
/// 解析 `play_request_target` 来自 输入.
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

/// `validate_websocket_upgrade` function.
/// `validate_websocket_upgrade` 函数.
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
/// `websocket_accept_key` 函数.
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
