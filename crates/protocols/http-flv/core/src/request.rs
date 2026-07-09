use base64::Engine;
use cheetah_rtmp_core::RtmpFlvPlayMode;
use sha1::{Digest, Sha1};

use crate::HttpFlvCoreError;

const WEBSOCKET_ACCEPT_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Options,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvTransport {
    Http,
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFlvQueryMode {
    Normal,
    Enhanced,
    FastPts,
}

impl HttpFlvQueryMode {
    pub fn to_rtmp_play_mode(self) -> RtmpFlvPlayMode {
        match self {
            Self::Enhanced => RtmpFlvPlayMode::Enhanced,
            Self::Normal | Self::FastPts => RtmpFlvPlayMode::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamKeyParts {
    pub namespace: String,
    pub stream_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlayRequest {
    pub stream_key: StreamKeyParts,
    pub mode: HttpFlvQueryMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestHead {
    pub method: HttpMethod,
    pub method_raw: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl HttpRequestHead {
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .rfind(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

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
pub struct HttpResponseHead {
    pub status_code: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Binary(bytes::Bytes),
    Close,
    Ping(bytes::Bytes),
    Pong(bytes::Bytes),
    Text(String),
}

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
