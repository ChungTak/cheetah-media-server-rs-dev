use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::{FlvDemuxEvent, FlvDemuxer, FlvHeader, FlvStreamError, FlvTag};
use cheetah_http_flv_core::websocket_accept_key;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::warn;

use crate::config::HttpFlvPullJobConfig;

/// `PullReadLimits` data structure.
/// `PullReadLimits` 数据结构.
#[derive(Debug, Clone, Copy)]
pub struct PullReadLimits {
    /// `max_response_header_bytes` field of type `usize`.
    /// `max_response_header_bytes` 字段，类型为 `usize`.
    pub max_response_header_bytes: usize,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `max_demux_buffer_bytes` field of type `usize`.
    /// `max_demux_buffer_bytes` 字段，类型为 `usize`.
    pub max_demux_buffer_bytes: usize,
    /// `max_websocket_message_bytes` field of type `usize`.
    /// `max_websocket_message_bytes` 字段，类型为 `usize`.
    pub max_websocket_message_bytes: usize,
}

impl Default for PullReadLimits {
    fn default() -> Self {
        Self {
            max_response_header_bytes: 32 * 1024,
            read_buffer_size: 16 * 1024,
            max_demux_buffer_bytes: 4 * 1024 * 1024,
            max_websocket_message_bytes: 1024 * 1024,
        }
    }
}

/// `HttpFlvPullResult` data structure.
/// `HttpFlvPullResult` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFlvPullResult {
    /// `header` field.
    /// `header` 字段.
    pub header: Option<FlvHeader>,
    /// `tags` field.
    /// `tags` 字段.
    pub tags: Vec<FlvTag>,
    /// `previous_tag_size_mismatch_count` field of type `u64`.
    /// `previous_tag_size_mismatch_count` 字段，类型为 `u64`.
    pub previous_tag_size_mismatch_count: u64,
}

/// `HttpFlvPullError` enumeration.
/// `HttpFlvPullError` 枚举.
#[derive(Debug, thiserror::Error)]
pub enum HttpFlvPullError {
    /// `InvalidUrl` variant.
    /// `InvalidUrl` 变体.
    #[error("invalid pull url: {0}")]
    InvalidUrl(String),
    /// `UnsupportedScheme` variant.
    /// `UnsupportedScheme` 变体.
    #[error("unsupported pull url scheme: {scheme}")]
    UnsupportedScheme { scheme: String },
    /// `Resolve` variant.
    /// `Resolve` 变体.
    #[error("resolve source host failed: {0}")]
    Resolve(String),
    /// `Connect` variant.
    /// `Connect` 变体.
    #[error("connect source failed: {0}")]
    Connect(String),
    /// `WriteRequest` variant.
    /// `WriteRequest` 变体.
    #[error("write pull request failed: {0}")]
    WriteRequest(String),
    /// `ResponseHeaderTooLarge` variant.
    /// `ResponseHeaderTooLarge` 变体.
    #[error("response header exceeds limit: {actual} > {limit}")]
    ResponseHeaderTooLarge { actual: usize, limit: usize },
    /// `ResponseHeaderIncomplete` variant.
    /// `ResponseHeaderIncomplete` 变体.
    #[error("source closed before response header completed")]
    ResponseHeaderIncomplete,
    /// `InvalidStatusLine` variant.
    /// `InvalidStatusLine` 变体.
    #[error("invalid response status line")]
    InvalidStatusLine,
    /// `BadStatusCode` variant.
    /// `BadStatusCode` 变体.
    #[error("source response status is not success: {status_code}")]
    BadStatusCode { status_code: u16 },
    /// `InvalidWebSocketAccept` variant.
    /// `InvalidWebSocketAccept` 变体.
    #[error("invalid websocket accept header")]
    InvalidWebSocketAccept,
    /// `WebSocketProtocol` variant.
    /// `WebSocketProtocol` 变体.
    #[error("websocket protocol error: {0}")]
    WebSocketProtocol(String),
    /// `InvalidChunkedEncoding` variant.
    /// `InvalidChunkedEncoding` 变体.
    #[error("invalid chunked response body: {0}")]
    InvalidChunkedEncoding(String),
    /// `ReadBody` variant.
    /// `ReadBody` 变体.
    #[error("read source body failed: {0}")]
    ReadBody(String),
    /// `Cancelled` variant.
    /// `Cancelled` 变体.
    #[error("pull cancelled")]
    Cancelled,
    /// `FlvDemux` variant.
    /// `FlvDemux` 变体.
    #[error("flv demux failed: {0}")]
    FlvDemux(FlvStreamError),
}

impl HttpFlvPullError {
    fn retryable(&self) -> bool {
        !matches!(
            self,
            Self::InvalidUrl(_) | Self::UnsupportedScheme { .. } | Self::InvalidWebSocketAccept
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullScheme {
    Http,
    Ws,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChunkedBodyFrame {
    Data(Vec<u8>),
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPullUrl {
    scheme: PullScheme,
    host: String,
    port: u16,
    authority: String,
    path_and_query: String,
}

impl ParsedPullUrl {
    fn parse(source_url: &str) -> Result<Self, HttpFlvPullError> {
        let trimmed = source_url.trim();
        let Some((scheme_raw, rest)) = trimmed.split_once("://") else {
            return Err(HttpFlvPullError::InvalidUrl(
                "missing scheme separator".to_string(),
            ));
        };
        let scheme = if scheme_raw.eq_ignore_ascii_case("http") {
            PullScheme::Http
        } else if scheme_raw.eq_ignore_ascii_case("ws") {
            PullScheme::Ws
        } else {
            return Err(HttpFlvPullError::UnsupportedScheme {
                scheme: scheme_raw.to_string(),
            });
        };

        let (authority, mut path_and_query) = if let Some(split_at) = rest.find(['/', '?', '#']) {
            let (auth, suffix) = rest.split_at(split_at);
            let path = if suffix.starts_with('/') {
                suffix.to_string()
            } else if suffix.starts_with('?') {
                format!("/{suffix}")
            } else {
                "/".to_string()
            };
            (auth, path)
        } else {
            (rest, "/".to_string())
        };
        if let Some(fragment_index) = path_and_query.find('#') {
            path_and_query.truncate(fragment_index);
        }
        if path_and_query.is_empty() {
            path_and_query = "/".to_string();
        }
        if authority.is_empty() {
            return Err(HttpFlvPullError::InvalidUrl(
                "missing host in authority".to_string(),
            ));
        }
        if authority.contains('@') {
            return Err(HttpFlvPullError::InvalidUrl(
                "userinfo in authority is not supported".to_string(),
            ));
        }

        let (host, mut port) = parse_host_port(authority)?;
        if port == 0 {
            port = match scheme {
                PullScheme::Http => 80,
                PullScheme::Ws => 80,
            };
        }
        Ok(Self {
            scheme,
            host,
            port,
            authority: authority.to_string(),
            path_and_query,
        })
    }
}

fn parse_host_port(authority: &str) -> Result<(String, u16), HttpFlvPullError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host_part, tail)) = rest.split_once(']') else {
            return Err(HttpFlvPullError::InvalidUrl(
                "invalid ipv6 authority".to_string(),
            ));
        };
        let host = host_part.trim().to_string();
        if host.is_empty() {
            return Err(HttpFlvPullError::InvalidUrl(
                "empty host in authority".to_string(),
            ));
        }
        if tail.is_empty() {
            return Ok((host, 0));
        }
        let Some(port_raw) = tail.strip_prefix(':') else {
            return Err(HttpFlvPullError::InvalidUrl(
                "invalid ipv6 authority suffix".to_string(),
            ));
        };
        return Ok((host, parse_port(port_raw)?));
    }

    if let Some((host_raw, port_raw)) = authority.rsplit_once(':') {
        if host_raw.contains(':') {
            return Err(HttpFlvPullError::InvalidUrl(
                "ipv6 host must be bracketed".to_string(),
            ));
        }
        let host = host_raw.trim().to_string();
        if host.is_empty() {
            return Err(HttpFlvPullError::InvalidUrl(
                "empty host in authority".to_string(),
            ));
        }
        return Ok((host, parse_port(port_raw)?));
    }

    let host = authority.trim().to_string();
    if host.is_empty() {
        return Err(HttpFlvPullError::InvalidUrl(
            "empty host in authority".to_string(),
        ));
    }
    Ok((host, 0))
}

fn parse_port(port_raw: &str) -> Result<u16, HttpFlvPullError> {
    let port = port_raw
        .trim()
        .parse::<u16>()
        .map_err(|_| HttpFlvPullError::InvalidUrl("invalid port in authority".to_string()))?;
    if port == 0 {
        return Err(HttpFlvPullError::InvalidUrl(
            "port must be greater than 0".to_string(),
        ));
    }
    Ok(port)
}

/// `run_pull_job_supervisor` function.
/// `run_pull_job_supervisor` 函数.
pub async fn run_pull_job_supervisor(
    runtime_api: Arc<dyn RuntimeApi>,
    job: HttpFlvPullJobConfig,
    cancel: CancellationToken,
    limits: PullReadLimits,
) {
    let base_backoff_ms = job.retry_backoff_ms.max(1);
    let max_backoff_ms = job.max_retry_backoff_ms.max(base_backoff_ms);
    let mut next_backoff_ms = base_backoff_ms;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let pull_result = pull_flv_once(
            runtime_api.clone(),
            job.source_url.as_str(),
            &cancel,
            limits,
        )
        .await;

        let wait_ms = match pull_result {
            Ok(_result) => {
                next_backoff_ms = base_backoff_ms;
                base_backoff_ms
            }
            Err(err) => {
                warn!(
                    job = %job.name,
                    source = %job.source_url,
                    error = %err,
                    "http-flv pull job failed"
                );
                if !err.retryable() {
                    break;
                }
                let current = next_backoff_ms;
                next_backoff_ms = next_backoff_ms.saturating_mul(2).min(max_backoff_ms);
                current
            }
        };

        if wait_or_cancel(
            runtime_api.as_ref(),
            &cancel,
            Duration::from_millis(wait_ms),
        )
        .await
        {
            break;
        }
    }
}

/// `pull_flv_once` function.
/// `pull_flv_once` 函数.
pub async fn pull_flv_once(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let parsed = ParsedPullUrl::parse(source_url)?;
    match parsed.scheme {
        PullScheme::Http => pull_http_flv_once_parsed(runtime_api, parsed, cancel, limits).await,
        PullScheme::Ws => pull_ws_flv_once_parsed(runtime_api, parsed, cancel, limits).await,
    }
}

/// `pull_http_flv_once` function.
/// `pull_http_flv_once` 函数.
pub async fn pull_http_flv_once(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let parsed = ParsedPullUrl::parse(source_url)?;
    if parsed.scheme != PullScheme::Http {
        return Err(HttpFlvPullError::UnsupportedScheme {
            scheme: source_url
                .split("://")
                .next()
                .unwrap_or_default()
                .to_string(),
        });
    }
    pull_http_flv_once_parsed(runtime_api, parsed, cancel, limits).await
}

/// `pull_ws_flv_once` function.
/// `pull_ws_flv_once` 函数.
pub async fn pull_ws_flv_once(
    runtime_api: Arc<dyn RuntimeApi>,
    source_url: &str,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let parsed = ParsedPullUrl::parse(source_url)?;
    if parsed.scheme != PullScheme::Ws {
        return Err(HttpFlvPullError::UnsupportedScheme {
            scheme: source_url
                .split("://")
                .next()
                .unwrap_or_default()
                .to_string(),
        });
    }
    pull_ws_flv_once_parsed(runtime_api, parsed, cancel, limits).await
}

async fn pull_http_flv_once_parsed(
    runtime_api: Arc<dyn RuntimeApi>,
    parsed: ParsedPullUrl,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let mut stream = connect_stream(runtime_api, &parsed)?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: video/x-flv\r\n\r\n",
        parsed.path_and_query, parsed.authority
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| HttpFlvPullError::WriteRequest(err.to_string()))?;

    let (status, headers, body_prefix) =
        read_http_response_head(&mut stream, cancel, limits).await?;
    if !(200..=299).contains(&status) {
        return Err(HttpFlvPullError::BadStatusCode {
            status_code: status,
        });
    }

    if response_is_chunked(&headers) {
        read_chunked_flv_stream(stream, body_prefix, cancel, limits).await
    } else {
        read_flv_stream(stream, body_prefix, cancel, limits).await
    }
}

async fn pull_ws_flv_once_parsed(
    runtime_api: Arc<dyn RuntimeApi>,
    parsed: ParsedPullUrl,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let mut stream = connect_stream(runtime_api, &parsed)?;
    let ws_key = "dGhlIHNhbXBsZSBub25jZQ==";
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {}\r\n\r\n",
        parsed.path_and_query, parsed.authority, ws_key
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| HttpFlvPullError::WriteRequest(err.to_string()))?;

    let (status, headers, body_prefix) =
        read_http_response_head(&mut stream, cancel, limits).await?;
    if status != 101 {
        return Err(HttpFlvPullError::BadStatusCode {
            status_code: status,
        });
    }
    let expected_accept =
        websocket_accept_key(ws_key).map_err(|_| HttpFlvPullError::InvalidWebSocketAccept)?;
    let actual_accept = find_header_value(&headers, "sec-websocket-accept")
        .ok_or(HttpFlvPullError::InvalidWebSocketAccept)?;
    if actual_accept.trim() != expected_accept {
        return Err(HttpFlvPullError::InvalidWebSocketAccept);
    }

    read_ws_flv_stream(stream, body_prefix, cancel, limits).await
}

fn connect_stream(
    runtime_api: Arc<dyn RuntimeApi>,
    parsed: &ParsedPullUrl,
) -> Result<Box<dyn cheetah_runtime_api::AsyncTcpStream>, HttpFlvPullError> {
    let addr = (parsed.host.as_str(), parsed.port)
        .to_socket_addrs()
        .map_err(|err| HttpFlvPullError::Resolve(err.to_string()))?
        .next()
        .ok_or_else(|| HttpFlvPullError::Resolve("no socket address resolved".to_string()))?;
    runtime_api
        .connect_tcp(addr)
        .map_err(|err| HttpFlvPullError::Connect(err.to_string()))
}

async fn read_http_response_head(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>), HttpFlvPullError> {
    let mut buffered = Vec::<u8>::with_capacity(limits.read_buffer_size.max(1024));
    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];
    let header_end = loop {
        if let Some(end) = find_http_header_end(&buffered) {
            break end;
        }
        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            return Err(HttpFlvPullError::ResponseHeaderIncomplete);
        }
        buffered.extend_from_slice(&chunk[..n]);
        if buffered.len() > limits.max_response_header_bytes.max(1024) {
            return Err(HttpFlvPullError::ResponseHeaderTooLarge {
                actual: buffered.len(),
                limit: limits.max_response_header_bytes.max(1024),
            });
        }
    };

    let head_raw = &buffered[..header_end];
    let status = parse_http_status_code(head_raw)?;
    let headers = parse_http_headers(head_raw)?;
    let tail = buffered[header_end..].to_vec();
    Ok((status, headers, tail))
}

async fn read_flv_stream(
    mut stream: Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    body_prefix: Vec<u8>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let mut demuxer = FlvDemuxer::new(limits.max_demux_buffer_bytes.max(1024));
    let mut result = HttpFlvPullResult {
        header: None,
        tags: Vec::new(),
        previous_tag_size_mismatch_count: 0,
    };

    if !body_prefix.is_empty() {
        let events = demuxer
            .push(&body_prefix)
            .map_err(HttpFlvPullError::FlvDemux)?;
        apply_demux_events(&mut result, events);
    }

    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];
    loop {
        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            break;
        }
        let events = demuxer
            .push(&chunk[..n])
            .map_err(HttpFlvPullError::FlvDemux)?;
        apply_demux_events(&mut result, events);
    }
    Ok(result)
}

async fn read_chunked_flv_stream(
    mut stream: Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    body_prefix: Vec<u8>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let mut demuxer = FlvDemuxer::new(limits.max_demux_buffer_bytes.max(1024));
    let mut result = HttpFlvPullResult {
        header: None,
        tags: Vec::new(),
        previous_tag_size_mismatch_count: 0,
    };
    let mut buffered = body_prefix;
    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];
    let max_chunk_bytes = limits.max_demux_buffer_bytes.max(1024);

    loop {
        while let Some(frame) = try_decode_chunked_body_frame(&mut buffered, max_chunk_bytes)? {
            match frame {
                ChunkedBodyFrame::Data(payload) => {
                    let events = demuxer.push(&payload).map_err(HttpFlvPullError::FlvDemux)?;
                    apply_demux_events(&mut result, events);
                }
                ChunkedBodyFrame::End => return Ok(result),
            }
        }

        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            return Err(HttpFlvPullError::InvalidChunkedEncoding(
                "unexpected EOF before terminating chunk".to_string(),
            ));
        }
        buffered.extend_from_slice(&chunk[..n]);
        if buffered.len() > max_chunk_bytes {
            return Err(HttpFlvPullError::InvalidChunkedEncoding(format!(
                "chunked decoder buffer too large: {} > {}",
                buffered.len(),
                max_chunk_bytes
            )));
        }
    }
}

async fn read_ws_flv_stream(
    mut stream: Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    body_prefix: Vec<u8>,
    cancel: &CancellationToken,
    limits: PullReadLimits,
) -> Result<HttpFlvPullResult, HttpFlvPullError> {
    let mut demuxer = FlvDemuxer::new(limits.max_demux_buffer_bytes.max(1024));
    let mut result = HttpFlvPullResult {
        header: None,
        tags: Vec::new(),
        previous_tag_size_mismatch_count: 0,
    };

    let mut buffered = body_prefix;
    let mut chunk = vec![0u8; limits.read_buffer_size.max(1024)];

    loop {
        while let Some((frame, consumed)) =
            decode_ws_frame(&buffered, limits.max_websocket_message_bytes)?
        {
            buffered.drain(..consumed);
            match frame.opcode {
                0x2 => {
                    let events = demuxer
                        .push(&frame.payload)
                        .map_err(HttpFlvPullError::FlvDemux)?;
                    apply_demux_events(&mut result, events);
                }
                0x8 => return Ok(result),
                0x9 | 0xA => {}
                0x1 => {}
                _ => {
                    return Err(HttpFlvPullError::WebSocketProtocol(format!(
                        "unsupported opcode {}",
                        frame.opcode
                    )));
                }
            }
        }

        let n = select_read_or_cancel(cancel, stream.read(&mut chunk)).await?;
        if n == 0 {
            break;
        }
        buffered.extend_from_slice(&chunk[..n]);
    }

    Ok(result)
}

#[derive(Debug)]
struct WsFrame {
    opcode: u8,
    payload: Vec<u8>,
}

fn decode_ws_frame(
    raw: &[u8],
    max_payload_bytes: usize,
) -> Result<Option<(WsFrame, usize)>, HttpFlvPullError> {
    if raw.len() < 2 {
        return Ok(None);
    }

    let fin = (raw[0] & 0x80) != 0;
    let opcode = raw[0] & 0x0f;
    if !fin {
        return Err(HttpFlvPullError::WebSocketProtocol(
            "fragmented websocket frames are not supported".to_string(),
        ));
    }

    let masked = (raw[1] & 0x80) != 0;
    if masked {
        return Err(HttpFlvPullError::WebSocketProtocol(
            "masked server frame is invalid".to_string(),
        ));
    }

    let mut offset = 2usize;
    let payload_len_flag = (raw[1] & 0x7f) as usize;
    let payload_len = if payload_len_flag <= 125 {
        payload_len_flag
    } else if payload_len_flag == 126 {
        if raw.len() < offset + 2 {
            return Ok(None);
        }
        let len = u16::from_be_bytes([raw[offset], raw[offset + 1]]) as usize;
        offset += 2;
        len
    } else {
        if raw.len() < offset + 8 {
            return Ok(None);
        }
        let len = u64::from_be_bytes([
            raw[offset],
            raw[offset + 1],
            raw[offset + 2],
            raw[offset + 3],
            raw[offset + 4],
            raw[offset + 5],
            raw[offset + 6],
            raw[offset + 7],
        ]);
        offset += 8;
        usize::try_from(len).map_err(|_| {
            HttpFlvPullError::WebSocketProtocol("payload length overflows usize".to_string())
        })?
    };

    if payload_len > max_payload_bytes.max(1024) {
        return Err(HttpFlvPullError::WebSocketProtocol(format!(
            "websocket payload too large: {payload_len} > {}",
            max_payload_bytes.max(1024)
        )));
    }
    if raw.len() < offset + payload_len {
        return Ok(None);
    }
    let payload = raw[offset..offset + payload_len].to_vec();
    Ok(Some((WsFrame { opcode, payload }, offset + payload_len)))
}

/// `fuzz_http_response_head` function.
/// `fuzz_http_response_head` 函数.
#[doc(hidden)]
pub fn fuzz_http_response_head(
    raw: &[u8],
    max_response_header_bytes: usize,
) -> Result<(), HttpFlvPullError> {
    if raw.len() > max_response_header_bytes.max(1024) {
        return Err(HttpFlvPullError::ResponseHeaderTooLarge {
            actual: raw.len(),
            limit: max_response_header_bytes.max(1024),
        });
    }
    let Some(end) = find_http_header_end(raw) else {
        return Err(HttpFlvPullError::ResponseHeaderIncomplete);
    };
    let head = &raw[..end];
    let _ = parse_http_status_code(head)?;
    let _ = parse_http_headers(head)?;
    Ok(())
}

/// `fuzz_decode_ws_frames` function.
/// `fuzz_decode_ws_frames` 函数.
#[doc(hidden)]
pub fn fuzz_decode_ws_frames(
    raw: &[u8],
    max_websocket_message_bytes: usize,
) -> Result<usize, HttpFlvPullError> {
    let mut offset = 0usize;
    while offset < raw.len() {
        match decode_ws_frame(&raw[offset..], max_websocket_message_bytes)? {
            Some((_frame, consumed)) => {
                if consumed == 0 {
                    break;
                }
                offset += consumed;
            }
            None => break,
        }
    }
    Ok(offset)
}

async fn select_read_or_cancel(
    cancel: &CancellationToken,
    read_future: impl std::future::Future<Output = std::io::Result<usize>>,
) -> Result<usize, HttpFlvPullError> {
    let cancel_fut = cancel.cancelled().fuse();
    let read_fut = read_future.fuse();
    pin_mut!(cancel_fut, read_fut);
    select_biased! {
        _ = cancel_fut => Err(HttpFlvPullError::Cancelled),
        read_result = read_fut => read_result.map_err(|err| HttpFlvPullError::ReadBody(err.to_string())),
    }
}

fn apply_demux_events(result: &mut HttpFlvPullResult, events: Vec<FlvDemuxEvent>) {
    for event in events {
        match event {
            FlvDemuxEvent::Header(header) => {
                result.header = Some(header);
            }
            FlvDemuxEvent::Tag(tag) => {
                result.tags.push(tag);
            }
            FlvDemuxEvent::PreviousTagSizeMismatch(_) => {
                result.previous_tag_size_mismatch_count =
                    result.previous_tag_size_mismatch_count.saturating_add(1);
            }
        }
    }
}

fn response_is_chunked(headers: &[(String, String)]) -> bool {
    let Some(value) = find_header_value(headers, "transfer-encoding") else {
        return false;
    };
    value
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
}

fn try_decode_chunked_body_frame(
    buffered: &mut Vec<u8>,
    max_chunk_bytes: usize,
) -> Result<Option<ChunkedBodyFrame>, HttpFlvPullError> {
    let Some(line_end) = buffered.windows(2).position(|window| window == b"\r\n") else {
        return Ok(None);
    };
    let line = std::str::from_utf8(&buffered[..line_end])
        .map_err(|_| HttpFlvPullError::InvalidChunkedEncoding("non-utf8 chunk size".to_string()))?;
    let size_token = line.split(';').next().unwrap_or("").trim();
    if size_token.is_empty() {
        return Err(HttpFlvPullError::InvalidChunkedEncoding(
            "empty chunk size".to_string(),
        ));
    }
    let chunk_size = usize::from_str_radix(size_token, 16).map_err(|_| {
        HttpFlvPullError::InvalidChunkedEncoding(format!("invalid chunk size: {size_token}"))
    })?;
    if chunk_size > max_chunk_bytes {
        return Err(HttpFlvPullError::InvalidChunkedEncoding(format!(
            "chunk too large: {} > {}",
            chunk_size, max_chunk_bytes
        )));
    }

    let data_start = line_end + 2;
    let data_end = data_start.saturating_add(chunk_size);
    let record_end = data_end.saturating_add(2);
    if buffered.len() < record_end {
        return Ok(None);
    }
    if &buffered[data_end..record_end] != b"\r\n" {
        return Err(HttpFlvPullError::InvalidChunkedEncoding(
            "missing chunk data terminator".to_string(),
        ));
    }

    if chunk_size == 0 {
        buffered.drain(..record_end);
        return Ok(Some(ChunkedBodyFrame::End));
    }

    let payload = buffered[data_start..data_end].to_vec();
    buffered.drain(..record_end);
    Ok(Some(ChunkedBodyFrame::Data(payload)))
}

fn parse_http_headers(raw_head: &[u8]) -> Result<Vec<(String, String)>, HttpFlvPullError> {
    let text = std::str::from_utf8(raw_head).map_err(|_| HttpFlvPullError::InvalidStatusLine)?;
    let mut lines = text.split("\r\n");
    let _first = lines.next().ok_or(HttpFlvPullError::InvalidStatusLine)?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(headers)
}

fn find_header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rfind(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn find_http_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| idx + 4)
}

fn parse_http_status_code(raw_header: &[u8]) -> Result<u16, HttpFlvPullError> {
    let text = std::str::from_utf8(raw_header).map_err(|_| HttpFlvPullError::InvalidStatusLine)?;
    let Some(first_line) = text.split("\r\n").next() else {
        return Err(HttpFlvPullError::InvalidStatusLine);
    };
    let mut parts = first_line.split_whitespace();
    let _version = parts.next().ok_or(HttpFlvPullError::InvalidStatusLine)?;
    let status = parts.next().ok_or(HttpFlvPullError::InvalidStatusLine)?;
    status
        .parse::<u16>()
        .map_err(|_| HttpFlvPullError::InvalidStatusLine)
}

fn runtime_now_micros(runtime_api: &dyn RuntimeApi) -> u64 {
    runtime_api.now().as_micros()
}

fn runtime_deadline_after(
    runtime_api: &dyn RuntimeApi,
    duration: Duration,
) -> cheetah_codec::MonoTime {
    let duration_micros = duration.as_micros();
    let delta = u64::try_from(duration_micros).unwrap_or(u64::MAX);
    cheetah_codec::MonoTime::from_micros(runtime_now_micros(runtime_api).saturating_add(delta))
}

async fn runtime_sleep(runtime_api: &dyn RuntimeApi, duration: Duration) {
    let mut timer = runtime_api.sleep_until(runtime_deadline_after(runtime_api, duration));
    timer.wait().await;
}

async fn wait_or_cancel(
    runtime_api: &dyn RuntimeApi,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let cancel_fut = cancel.cancelled().fuse();
    let sleep_fut = runtime_sleep(runtime_api, duration).fuse();
    pin_mut!(cancel_fut, sleep_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = sleep_fut => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{FlvHeader, FlvTag, FlvTagType};
    use cheetah_runtime_tokio::TokioRuntime;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_one_shot_http_server(response: Vec<u8>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request_buf = vec![0u8; 4096];
            let _ = socket.read(&mut request_buf).await.expect("read request");
            socket.write_all(&response).await.expect("write response");
            let _ = socket.shutdown().await;
        });
        addr
    }

    fn encode_ws_binary_frame(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(payload.len() + 16);
        out.push(0x82);
        if payload.len() <= 125 {
            out.push(payload.len() as u8);
        } else if payload.len() <= 0xFFFF {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_http_flv_once_reads_header_and_tags() {
        let flv_header = FlvHeader {
            has_audio: false,
            has_video: true,
        }
        .encode();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 10,
            payload: Bytes::from_static(b"\x17\x01\x00\x00\x00"),
        }
        .encode_with_previous_tag_size();

        let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: video/x-flv\r\n\r\n".to_vec();
        response.extend_from_slice(&flv_header);
        response.extend_from_slice(&tag);

        let addr = spawn_one_shot_http_server(response).await;
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();

        let result = pull_http_flv_once(
            runtime,
            &format!("http://{addr}/live/test.flv"),
            &cancel,
            PullReadLimits::default(),
        )
        .await
        .expect("pull ok");

        assert_eq!(
            result.header,
            Some(FlvHeader {
                has_audio: false,
                has_video: true
            })
        );
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].tag_type, FlvTagType::Video);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_http_flv_once_rejects_non_success_status() {
        let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
        let addr = spawn_one_shot_http_server(response).await;
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();

        let err = pull_http_flv_once(
            runtime,
            &format!("http://{addr}/live/notfound.flv"),
            &cancel,
            PullReadLimits::default(),
        )
        .await
        .expect_err("must reject 404");
        assert!(matches!(
            err,
            HttpFlvPullError::BadStatusCode { status_code: 404 }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_http_flv_once_rejects_too_large_response_header() {
        let mut response = b"HTTP/1.1 200 OK\r\nX-Long: ".to_vec();
        response.extend_from_slice(&vec![b'a'; 4000]);
        response.extend_from_slice(b"\r\n\r\n");
        let addr = spawn_one_shot_http_server(response).await;
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();

        let err = pull_http_flv_once(
            runtime,
            &format!("http://{addr}/live/test.flv"),
            &cancel,
            PullReadLimits {
                max_response_header_bytes: 512,
                ..PullReadLimits::default()
            },
        )
        .await
        .expect_err("must reject too large header");
        assert!(matches!(
            err,
            HttpFlvPullError::ResponseHeaderTooLarge { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_http_flv_once_reads_chunked_body() {
        let flv_header = FlvHeader {
            has_audio: false,
            has_video: true,
        }
        .encode();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 30,
            payload: Bytes::from_static(b"\x17\x01\x00\x00\x02"),
        }
        .encode_with_previous_tag_size();
        let mut flv_stream = Vec::new();
        flv_stream.extend_from_slice(&flv_header);
        flv_stream.extend_from_slice(&tag);

        let split = flv_stream.len() / 2;
        let chunk_a = &flv_stream[..split];
        let chunk_b = &flv_stream[split..];
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: video/x-flv\r\nTransfer-Encoding: chunked\r\n\r\n"
                .to_vec();
        response.extend_from_slice(format!("{:X}\r\n", chunk_a.len()).as_bytes());
        response.extend_from_slice(chunk_a);
        response.extend_from_slice(b"\r\n");
        response.extend_from_slice(format!("{:X}\r\n", chunk_b.len()).as_bytes());
        response.extend_from_slice(chunk_b);
        response.extend_from_slice(b"\r\n");
        response.extend_from_slice(b"0\r\n\r\n");

        let addr = spawn_one_shot_http_server(response).await;
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();
        let result = pull_http_flv_once(
            runtime,
            &format!("http://{addr}/live/test.flv"),
            &cancel,
            PullReadLimits::default(),
        )
        .await
        .expect("chunked pull ok");

        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.tags[0].tag_type, FlvTagType::Video);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pull_ws_flv_once_reads_binary_frames() {
        let flv_header = FlvHeader {
            has_audio: false,
            has_video: true,
        }
        .encode();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 20,
            payload: Bytes::from_static(b"\x17\x01\x00\x00\x01"),
        }
        .encode_with_previous_tag_size();

        let mut payload = Vec::new();
        payload.extend_from_slice(&flv_header);
        payload.extend_from_slice(&tag);
        let ws_frame = encode_ws_binary_frame(&payload);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request_buf = vec![0u8; 4096];
            let _ = socket.read(&mut request_buf).await.expect("read request");
            socket
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await
                .expect("write handshake");
            socket.write_all(&ws_frame).await.expect("write frame");
            let _ = socket.shutdown().await;
        });

        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();
        let result = pull_ws_flv_once(
            runtime,
            &format!("ws://{addr}/live/test.flv"),
            &cancel,
            PullReadLimits::default(),
        )
        .await
        .expect("pull ws ok");

        assert_eq!(
            result.header,
            Some(FlvHeader {
                has_audio: false,
                has_video: true
            })
        );
        assert_eq!(result.tags.len(), 1);
    }

    #[test]
    fn parsed_pull_url_preserves_query_without_path_segment() {
        let parsed =
            ParsedPullUrl::parse("http://example.com?type=enhanced&token=abc").expect("parse");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.authority, "example.com");
        assert_eq!(parsed.path_and_query, "/?type=enhanced&token=abc");
    }
}
