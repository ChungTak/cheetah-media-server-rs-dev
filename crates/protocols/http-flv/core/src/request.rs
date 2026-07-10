use base64::Engine;
use cheetah_codec::RtmpFlvPlayMode;
use sha1::{Digest, Sha1};

use crate::HttpFlvCoreError;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Supported HTTP methods for HTTP-FLV requests.
///
/// HTTP-FLV 请求支持的 HTTP 方法。
pub enum HttpMethod {
    Get,
    Post,
    Options,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Transport used by the HTTP-FLV client.
///
/// HTTP-FLV 客户端使用的传输方式。
pub enum HttpFlvTransport {
    Http,
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Query-string play mode requested by the client.
///
/// 客户端通过查询字符串请求的播放模式。
pub enum HttpFlvQueryMode {
    Normal,
    Enhanced,
    FastPts,
}

/// `HttpFlvQueryMode` helpers.
///
/// `HttpFlvQueryMode` 辅助。
impl HttpFlvQueryMode {
    /// Map the query mode to the internal RTMP/FLV play mode.
    ///
    /// 将查询模式映射为内部 RTMP/FLV 播放模式。
    pub fn to_rtmp_play_mode(self) -> RtmpFlvPlayMode {
        match self {
            Self::Enhanced => RtmpFlvPlayMode::Enhanced,
            Self::Normal | Self::FastPts => RtmpFlvPlayMode::Normal,
        }
    }
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
/// Parsed HTTP-FLV play request with stream key and play mode.
///
/// 解析后的 HTTP-FLV 播放请求，包含流密钥与播放模式。
pub struct ParsedPlayRequest {
    pub stream_key: StreamKeyParts,
    pub mode: HttpFlvQueryMode,
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
}

/// Parse an `.flv` request target into stream key and play mode.
///
/// 将 `.flv` 请求目标解析为流密钥与播放模式。
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

/// Validate a WebSocket upgrade request and return the accept key.
///
/// 校验 WebSocket 升级请求并返回 accept key。
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

/// Compute the RFC 6455 `Sec-WebSocket-Accept` value.
///
/// 计算 RFC 6455 `Sec-WebSocket-Accept` 值。
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

/// Parse the `type` query parameter into a play mode.
///
/// 将 `type` 查询参数解析为播放模式。
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
