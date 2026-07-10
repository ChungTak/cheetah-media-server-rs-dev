use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use cheetah_rtsp_core::{CoreInput, CoreOutput, RtspCore};
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::sync::mpsc;

use super::command::ConnectionCommand;
use super::connection::ConnectionRuntime;
use super::{DriverEvent, RtspConnectionId};

const HTTP_TUNNEL_HEADER_LIMIT: usize = 64 * 1024;

/// `HttpTunnelRegistryConfig` data structure.
/// `HttpTunnelRegistryConfig` 数据结构.
#[derive(Debug, Clone)]
pub(super) struct HttpTunnelRegistryConfig {
    /// `max_pending_tunnels` field of type `usize`.
    /// `max_pending_tunnels` 字段，类型为 `usize`.
    pub(super) max_pending_tunnels: usize,
    /// `pending_timeout_ms` field of type `u64`.
    /// `pending_timeout_ms` 字段，类型为 `u64`.
    pub(super) pending_timeout_ms: u64,
    /// `max_decoded_chunk_bytes` field of type `usize`.
    /// `max_decoded_chunk_bytes` 字段，类型为 `usize`.
    pub(super) max_decoded_chunk_bytes: usize,
    /// `max_base64_buffer_bytes` field of type `usize`.
    /// `max_base64_buffer_bytes` 字段，类型为 `usize`.
    pub(super) max_base64_buffer_bytes: usize,
}

impl HttpTunnelRegistryConfig {
    /// Creates `driver_config` from input.
    /// 创建 `driver_config` 来自 输入.
    pub(super) fn from_driver_config(config: &super::DriverConfig) -> Self {
        Self {
            max_pending_tunnels: config.http_tunnel_max_pending.max(8),
            pending_timeout_ms: config.http_tunnel_pending_timeout_ms.max(1_000),
            max_decoded_chunk_bytes: config.http_tunnel_max_decoded_chunk_bytes.max(1024),
            max_base64_buffer_bytes: config.http_tunnel_max_base64_buffer_bytes.max(4096),
        }
    }
}

/// `PendingGetHalf` data structure.
/// `PendingGetHalf` 数据结构.
pub(super) struct PendingGetHalf {
    /// `stream` field.
    /// `stream` 字段.
    pub(super) stream: Box<dyn AsyncTcpStream>,
    /// `path` field of type `String`.
    /// `path` 字段，类型为 `String`.
    pub(super) path: String,
    /// `expires_at_micros` field of type `u64`.
    /// `expires_at_micros` 字段，类型为 `u64`.
    pub(super) expires_at_micros: u64,
}

/// `PendingPostHalf` data structure.
/// `PendingPostHalf` 数据结构.
pub(super) struct PendingPostHalf {
    /// `stream` field.
    /// `stream` 字段.
    pub(super) stream: Box<dyn AsyncTcpStream>,
    /// `peer` field.
    /// `peer` 字段.
    pub(super) peer: std::net::SocketAddr,
    /// `path` field of type `String`.
    /// `path` 字段，类型为 `String`.
    pub(super) path: String,
    /// `initial_body` field of type `Bytes`.
    /// `initial_body` 字段，类型为 `Bytes`.
    pub(super) initial_body: Bytes,
    /// `expires_at_micros` field of type `u64`.
    /// `expires_at_micros` 字段，类型为 `u64`.
    pub(super) expires_at_micros: u64,
}

/// `PendingPair` data structure.
/// `PendingPair` 数据结构.
pub(super) struct PendingPair {
    /// `cookie` field of type `String`.
    /// `cookie` 字段，类型为 `String`.
    pub(super) cookie: String,
    /// `get` field of type `PendingGetHalf`.
    /// `get` 字段，类型为 `PendingGetHalf`.
    pub(super) get: PendingGetHalf,
    /// `post` field of type `PendingPostHalf`.
    /// `post` 字段，类型为 `PendingPostHalf`.
    pub(super) post: PendingPostHalf,
}

/// `HttpTunnelRegistry` data structure.
/// `HttpTunnelRegistry` 数据结构.
pub(super) struct HttpTunnelRegistry {
    /// `config` field of type `HttpTunnelRegistryConfig`.
    /// `config` 字段，类型为 `HttpTunnelRegistryConfig`.
    config: HttpTunnelRegistryConfig,
    /// `gets` field.
    /// `gets` 字段.
    gets: HashMap<String, PendingGetHalf>,
    /// `posts` field.
    /// `posts` 字段.
    posts: HashMap<String, PendingPostHalf>,
    /// `fifo` field.
    /// `fifo` 字段.
    fifo: VecDeque<String>,
}

impl HttpTunnelRegistry {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub(super) fn new(config: HttpTunnelRegistryConfig) -> Self {
        Self {
            config,
            gets: HashMap::new(),
            posts: HashMap::new(),
            fifo: VecDeque::new(),
        }
    }

    /// `config` function.
    /// `config` 函数.
    pub(super) fn config(&self) -> &HttpTunnelRegistryConfig {
        &self.config
    }

    /// `upsert_get` function.
    /// `upsert_get` 函数.
    pub(super) fn upsert_get(
        &mut self,
        cookie: String,
        stream: Box<dyn AsyncTcpStream>,
        path: String,
        now_micros: u64,
    ) -> Result<Option<PendingPair>, &'static str> {
        let expires_at_micros = now_micros.saturating_add(self.config.pending_timeout_ms * 1000);
        if let Some(post) = self.posts.get(&cookie) {
            if post.path != path {
                return Err("http tunnel path mismatch");
            }
            let post = self
                .posts
                .remove(&cookie)
                .ok_or("http tunnel pending post half disappeared")?;
            return Ok(Some(PendingPair {
                cookie,
                get: PendingGetHalf {
                    stream,
                    path,
                    expires_at_micros,
                },
                post,
            }));
        }
        self.evict_if_needed()?;
        self.gets.insert(
            cookie.clone(),
            PendingGetHalf {
                stream,
                path,
                expires_at_micros,
            },
        );
        self.fifo.push_back(cookie);
        Ok(None)
    }

    /// `upsert_post` function.
    /// `upsert_post` 函数.
    pub(super) fn upsert_post(
        &mut self,
        cookie: String,
        stream: Box<dyn AsyncTcpStream>,
        peer: std::net::SocketAddr,
        path: String,
        initial_body: Bytes,
        now_micros: u64,
    ) -> Result<Option<PendingPair>, &'static str> {
        let expires_at_micros = now_micros.saturating_add(self.config.pending_timeout_ms * 1000);
        if let Some(get) = self.gets.get(&cookie) {
            if get.path != path {
                return Err("http tunnel path mismatch");
            }
            let get = self
                .gets
                .remove(&cookie)
                .ok_or("http tunnel pending get half disappeared")?;
            return Ok(Some(PendingPair {
                cookie,
                get,
                post: PendingPostHalf {
                    stream,
                    peer,
                    path,
                    initial_body,
                    expires_at_micros,
                },
            }));
        }
        self.evict_if_needed()?;
        self.posts.insert(
            cookie.clone(),
            PendingPostHalf {
                stream,
                peer,
                path,
                initial_body,
                expires_at_micros,
            },
        );
        self.fifo.push_back(cookie);
        Ok(None)
    }

    /// `drain_expired` function.
    /// `drain_expired` 函数.
    pub(super) fn drain_expired(
        &mut self,
        now_micros: u64,
    ) -> Vec<(Option<PendingGetHalf>, Option<PendingPostHalf>)> {
        let mut expired = Vec::new();
        let keys: Vec<String> = self
            .fifo
            .iter()
            .filter(|cookie| {
                self.gets
                    .get(*cookie)
                    .is_some_and(|half| half.expires_at_micros <= now_micros)
                    || self
                        .posts
                        .get(*cookie)
                        .is_some_and(|half| half.expires_at_micros <= now_micros)
            })
            .cloned()
            .collect();
        for cookie in keys {
            let get = self.gets.remove(&cookie);
            let post = self.posts.remove(&cookie);
            if get.is_some() || post.is_some() {
                expired.push((get, post));
            }
        }
        self.fifo
            .retain(|cookie| self.gets.contains_key(cookie) || self.posts.contains_key(cookie));
        expired
    }

    fn evict_if_needed(&mut self) -> Result<(), &'static str> {
        let pending_count = self.gets.len().saturating_add(self.posts.len());
        if pending_count < self.config.max_pending_tunnels {
            return Ok(());
        }
        while let Some(cookie) = self.fifo.pop_front() {
            let dropped_get = self.gets.remove(&cookie);
            let dropped_post = self.posts.remove(&cookie);
            if dropped_get.is_some() || dropped_post.is_some() {
                return Ok(());
            }
        }
        Err("http tunnel pending registry is full")
    }
}

/// `HttpTunnelMethod` enumeration.
/// `HttpTunnelMethod` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpTunnelMethod {
    /// `Get` variant.
    /// `Get` 变体.
    Get,
    /// `Post` variant.
    /// `Post` 变体.
    Post,
}

/// `HttpTunnelOpenRequest` data structure.
/// `HttpTunnelOpenRequest` 数据结构.
pub(super) struct HttpTunnelOpenRequest {
    /// `method` field of type `HttpTunnelMethod`.
    /// `方法` 字段，类型为 `HttpTunnelMethod`.
    pub(super) method: HttpTunnelMethod,
    /// `cookie` field of type `String`.
    /// `cookie` 字段，类型为 `String`.
    pub(super) cookie: String,
    /// `path` field of type `String`.
    /// `path` 字段，类型为 `String`.
    pub(super) path: String,
    /// `initial_post_body` field of type `Bytes`.
    /// `initial_post_body` 字段，类型为 `Bytes`.
    pub(super) initial_post_body: Bytes,
}

/// `HttpTunnelParseResult` enumeration.
/// `HttpTunnelParseResult` 枚举.
pub(super) enum HttpTunnelParseResult {
    /// `Tunnel` variant.
    /// `Tunnel` 变体.
    Tunnel(HttpTunnelOpenRequest),
    /// `NotTunnel` variant.
    /// `NotTunnel` 变体.
    NotTunnel(Bytes),
}

/// `HttpTunnelProbeResult` enumeration.
/// `HttpTunnelProbeResult` 枚举.
pub(super) enum HttpTunnelProbeResult {
    /// `Parsed` variant.
    /// `Parsed` 变体.
    Parsed(Result<HttpTunnelParseResult, &'static str>),
    /// `TimedOut` variant.
    /// `TimedOut` 变体.
    TimedOut(Bytes),
}

/// `probe_http_tunnel_open_request` function.
/// `probe_http_tunnel_open_request` 函数.
pub(super) async fn probe_http_tunnel_open_request(
    stream: &mut Box<dyn AsyncTcpStream>,
    initial_bytes: Bytes,
    timeout: Duration,
) -> HttpTunnelProbeResult {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut raw = initial_bytes.to_vec();
    loop {
        if let Some(header_end) = find_header_end(&raw) {
            return HttpTunnelProbeResult::Parsed(parse_http_tunnel_header(raw, header_end));
        }
        if raw.len() >= HTTP_TUNNEL_HEADER_LIMIT {
            return HttpTunnelProbeResult::Parsed(Err("http tunnel header too large"));
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return HttpTunnelProbeResult::TimedOut(Bytes::from(raw));
        }
        let remaining = deadline.saturating_duration_since(now);
        let mut buf = vec![0u8; 4096];
        let read_res = tokio::time::timeout(remaining, stream.read(&mut buf)).await;
        match read_res {
            Ok(Ok(0)) => {
                return HttpTunnelProbeResult::Parsed(Err(
                    "http tunnel peer closed before full header",
                ))
            }
            Ok(Ok(n)) => raw.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => {
                return HttpTunnelProbeResult::Parsed(Err("read http tunnel header failed"))
            }
            Err(_) => return HttpTunnelProbeResult::TimedOut(Bytes::from(raw)),
        }
    }
}

fn parse_http_tunnel_header(
    raw: Vec<u8>,
    header_end: usize,
) -> Result<HttpTunnelParseResult, &'static str> {
    let header = &raw[..header_end];
    let header_text = std::str::from_utf8(header).map_err(|_| "invalid http header")?;
    let mut lines = header_text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Err("invalid http request line");
    };
    let mut parts = request_line.split_whitespace();
    let Some(method_raw) = parts.next() else {
        return Err("invalid http request line");
    };
    let Some(path) = parts.next() else {
        return Err("invalid http request line");
    };
    let Some(version) = parts.next() else {
        return Err("invalid http request line");
    };
    if parts.next().is_some() {
        return Err("invalid http request line");
    }
    let method = match method_raw {
        "GET" => HttpTunnelMethod::Get,
        "POST" => HttpTunnelMethod::Post,
        _ => {
            return Ok(HttpTunnelParseResult::NotTunnel(Bytes::copy_from_slice(
                &raw,
            )))
        }
    };
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Ok(HttpTunnelParseResult::NotTunnel(Bytes::copy_from_slice(
            &raw,
        )));
    }

    let mut cookie: Option<String> = None;
    let mut content_type: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("x-sessioncookie") {
            let value = value.trim();
            if !value.is_empty() {
                cookie = Some(value.to_string());
            }
        } else if name.trim().eq_ignore_ascii_case("content-type") {
            content_type = Some(value.trim().to_string());
        }
    }
    let Some(cookie) = cookie else {
        return Err("missing x-sessioncookie");
    };
    if matches!(method, HttpTunnelMethod::Post) {
        let Some(content_type) = content_type else {
            return Err("missing content-type");
        };
        if !content_type.eq_ignore_ascii_case("application/x-rtsp-tunnelled") {
            return Err("invalid tunnel content-type");
        }
    }
    let body_offset = header_end + 4;
    let body = if body_offset < raw.len() {
        Bytes::copy_from_slice(&raw[body_offset..])
    } else {
        Bytes::new()
    };
    Ok(HttpTunnelParseResult::Tunnel(HttpTunnelOpenRequest {
        method,
        cookie,
        path: path.to_string(),
        initial_post_body: body,
    }))
}

/// `looks_like_http_tunnel_candidate` function.
/// `looks_like_http_tunnel_candidate` 函数.
pub(super) fn looks_like_http_tunnel_candidate(input: &[u8]) -> bool {
    input.starts_with(b"GET ") || input.starts_with(b"POST ")
}

/// Builds `http_tunnel_get_ok_response` output.
/// 构建 `http_tunnel_get_ok_response` 输出.
pub(super) fn build_http_tunnel_get_ok_response() -> Bytes {
    Bytes::from_static(
        b"HTTP/1.0 200 OK\r\nContent-Type: application/x-rtsp-tunnelled\r\nCache-Control: no-cache\r\nPragma: no-cache\r\n\r\n",
    )
}

/// Builds `http_tunnel_post_ok_response` output.
/// 构建 `http_tunnel_post_ok_response` 输出.
pub(super) fn build_http_tunnel_post_ok_response() -> Bytes {
    Bytes::from_static(b"HTTP/1.0 200 OK\r\nCache-Control: no-cache\r\nPragma: no-cache\r\n\r\n")
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

struct Base64StreamDecoder {
    buffer: Vec<u8>,
    max_base64_buffer_bytes: usize,
    max_decoded_chunk_bytes: usize,
}

impl Base64StreamDecoder {
    fn new(max_base64_buffer_bytes: usize, max_decoded_chunk_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_base64_buffer_bytes,
            max_decoded_chunk_bytes,
        }
    }

    fn push(&mut self, input: &[u8]) -> Result<Vec<Bytes>, String> {
        for byte in input {
            if byte.is_ascii_whitespace() {
                continue;
            }
            self.buffer.push(*byte);
            if self.buffer.len() > self.max_base64_buffer_bytes {
                return Err("http tunnel base64 buffer overflow".to_string());
            }
        }

        let mut out = Vec::new();
        loop {
            let complete = match self.buffer.iter().position(|byte| *byte == b'=') {
                Some(pad_index) => {
                    let padded_quantum_end = ((pad_index / 4) + 1) * 4;
                    if self.buffer.len() < padded_quantum_end {
                        break;
                    }
                    padded_quantum_end
                }
                None => (self.buffer.len() / 4) * 4,
            };
            if complete == 0 {
                break;
            }
            let chunk = self.buffer[..complete].to_vec();
            self.buffer.drain(..complete);
            let decoded = STANDARD
                .decode(chunk)
                .map_err(|_| "invalid base64 payload in http tunnel".to_string())?;
            if decoded.len() > self.max_decoded_chunk_bytes {
                return Err("decoded http tunnel chunk exceeds limit".to_string());
            }
            if !decoded.is_empty() {
                out.push(Bytes::from(decoded));
            }
        }
        Ok(out)
    }
}

/// `run_http_tunnel_connection` function.
/// `run_http_tunnel_connection` 函数.
pub(super) async fn run_http_tunnel_connection(
    connection_id: RtspConnectionId,
    mut get_stream: Box<dyn AsyncTcpStream>,
    mut post_stream: Box<dyn AsyncTcpStream>,
    initial_post_body: Bytes,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    runtime: ConnectionRuntime,
    decoder_limits: (usize, usize),
) {
    let mut pending_writes = VecDeque::<Bytes>::new();
    let max_write_queue = runtime.config.write_queue_capacity.max(8);
    let mut core = RtspCore::new();
    let mut close_requested = false;
    let mut read_buf = vec![0u8; runtime.config.read_buffer_size.max(1024)];
    let mut decoder = Base64StreamDecoder::new(decoder_limits.0, decoder_limits.1);

    let init_reason = feed_post_payload_to_core(
        &mut core,
        &mut decoder,
        &initial_post_body,
        connection_id,
        &runtime.event_tx,
        &mut pending_writes,
        max_write_queue,
    )
    .await;
    if let Err(reason) = init_reason {
        let _ = get_stream.shutdown().await;
        let _ = post_stream.shutdown().await;
        runtime.conn_map.lock().remove(&connection_id);
        let _ = runtime
            .event_tx
            .send(DriverEvent::ConnectionClosed {
                connection_id,
                reason,
            })
            .await;
        return;
    }

    let reason = loop {
        if close_requested && pending_writes.is_empty() {
            break "closed by command".to_string();
        }
        if let Some(bytes) = pending_writes.front().cloned() {
            if let Err(reason) =
                write_pending_bytes(get_stream.as_mut(), &bytes, &runtime.cancel).await
            {
                break reason;
            }
            pending_writes.pop_front();
            continue;
        }

        tokio::select! {
            _ = runtime.cancel.cancelled() => {
                break "cancelled".to_string();
            }
            maybe_cmd = cmd_rx.recv(), if !close_requested => {
                match maybe_cmd {
                    Some(ConnectionCommand::Core(command)) => {
                        let outputs = match core.handle_input(CoreInput::Command(command)) {
                            Ok(outputs) => outputs,
                            Err(err) => break format!("core command error: {err}"),
                        };
                        if let Err(reason) = flush_outputs(
                            outputs,
                            connection_id,
                            &runtime.event_tx,
                            &mut pending_writes,
                            max_write_queue,
                        ).await {
                            break reason;
                        }
                    }
                    Some(ConnectionCommand::Close) => {
                        close_requested = true;
                    }
                    None => break "command channel closed".to_string(),
                }
            }
            read_res = post_stream.read(&mut read_buf), if !close_requested => {
                match read_res {
                    Ok(0) => {
                        if let Ok(outputs) = core.handle_input(CoreInput::PeerClosed) {
                            let _ = flush_outputs(
                                outputs,
                                connection_id,
                                &runtime.event_tx,
                                &mut pending_writes,
                                max_write_queue,
                            ).await;
                        }
                        break "peer closed".to_string();
                    }
                    Ok(n) => {
                        let payload = &read_buf[..n];
                        if let Err(reason) = feed_post_payload_to_core(
                            &mut core,
                            &mut decoder,
                            payload,
                            connection_id,
                            &runtime.event_tx,
                            &mut pending_writes,
                            max_write_queue,
                        ).await {
                            break reason;
                        }
                    }
                    Err(err) => break format!("read failed: {err}"),
                }
            }
        }
    };

    let _ = get_stream.shutdown().await;
    let _ = post_stream.shutdown().await;
    runtime.conn_map.lock().remove(&connection_id);
    let _ = runtime
        .event_tx
        .send(DriverEvent::ConnectionClosed {
            connection_id,
            reason,
        })
        .await;
}

async fn feed_post_payload_to_core(
    core: &mut RtspCore,
    decoder: &mut Base64StreamDecoder,
    payload: &[u8],
    connection_id: RtspConnectionId,
    event_tx: &mpsc::Sender<DriverEvent>,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
) -> Result<(), String> {
    let decoded_chunks = decoder.push(payload)?;
    for chunk in decoded_chunks {
        let outputs = core
            .handle_input(CoreInput::Bytes(chunk))
            .map_err(|err| format!("core read error: {err}"))?;
        flush_outputs(
            outputs,
            connection_id,
            event_tx,
            pending_writes,
            max_write_queue,
        )
        .await?;
    }
    Ok(())
}

async fn write_pending_bytes(
    stream: &mut dyn AsyncTcpStream,
    bytes: &[u8],
    cancel: &CancellationToken,
) -> Result<(), String> {
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".to_string()),
        write_res = stream.write_all(bytes) => {
            write_res.map_err(|err| format!("write failed: {err}"))?;
            Ok(())
        }
    }
}

async fn flush_outputs(
    outputs: Vec<CoreOutput>,
    connection_id: RtspConnectionId,
    event_tx: &mpsc::Sender<DriverEvent>,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
) -> Result<(), String> {
    for output in outputs {
        match output {
            CoreOutput::Write(bytes) => {
                if pending_writes.len() >= max_write_queue {
                    return Err("write queue overflow".to_string());
                }
                pending_writes.push_back(bytes);
            }
            CoreOutput::Event(event) => {
                event_tx
                    .send(DriverEvent::Core {
                        connection_id,
                        event,
                    })
                    .await
                    .map_err(|_| "event channel closed".to_string())?;
            }
            CoreOutput::Close => {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    struct NoopTcpStream {
        peer: SocketAddr,
    }

    impl NoopTcpStream {
        fn boxed() -> Box<dyn AsyncTcpStream> {
            Box::new(Self {
                peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9_000),
            })
        }
    }

    #[async_trait::async_trait]
    impl AsyncTcpStream for NoopTcpStream {
        async fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }

        async fn write_all(&mut self, _buf: &[u8]) -> io::Result<()> {
            Ok(())
        }

        async fn shutdown(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn peer_addr(&self) -> io::Result<SocketAddr> {
            Ok(self.peer)
        }
    }

    fn test_registry() -> HttpTunnelRegistry {
        HttpTunnelRegistry::new(HttpTunnelRegistryConfig {
            max_pending_tunnels: 16,
            pending_timeout_ms: 5_000,
            max_decoded_chunk_bytes: 8 * 1024,
            max_base64_buffer_bytes: 8 * 1024,
        })
    }

    #[test]
    fn base64_stream_decoder_accepts_padded_segments_in_one_read() {
        let mut decoder = Base64StreamDecoder::new(1024, 1024);
        let chunks = decoder.push(b"QQ==Qg==").expect("decode padded segments");
        let joined = chunks
            .into_iter()
            .flat_map(|chunk| chunk.into_iter())
            .collect::<Vec<_>>();
        assert_eq!(joined, b"AB");
    }

    #[test]
    fn tunnel_ok_responses_do_not_advertise_connection_close() {
        let get = build_http_tunnel_get_ok_response();
        let post = build_http_tunnel_post_ok_response();
        let get_text = std::str::from_utf8(get.as_ref()).expect("get response utf8");
        let post_text = std::str::from_utf8(post.as_ref()).expect("post response utf8");

        assert!(!get_text.to_ascii_lowercase().contains("connection: close"));
        assert!(!post_text.to_ascii_lowercase().contains("connection: close"));
    }

    #[test]
    fn path_mismatch_get_does_not_drop_existing_post_half() {
        let mut registry = test_registry();
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5_544);
        let cookie = "cookie-a".to_string();

        let first = registry.upsert_post(
            cookie.clone(),
            NoopTcpStream::boxed(),
            peer,
            "/live/a".to_string(),
            Bytes::new(),
            1_000,
        );
        assert!(matches!(first, Ok(None)));

        let mismatch = registry.upsert_get(
            cookie.clone(),
            NoopTcpStream::boxed(),
            "/live/b".to_string(),
            1_200,
        );
        assert!(matches!(mismatch, Err("http tunnel path mismatch")));

        let matched =
            registry.upsert_get(cookie, NoopTcpStream::boxed(), "/live/a".to_string(), 1_300);
        assert!(matches!(matched, Ok(Some(_))));
    }

    #[test]
    fn path_mismatch_post_does_not_drop_existing_get_half() {
        let mut registry = test_registry();
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5_545);
        let cookie = "cookie-b".to_string();

        let first = registry.upsert_get(
            cookie.clone(),
            NoopTcpStream::boxed(),
            "/cam/1".to_string(),
            2_000,
        );
        assert!(matches!(first, Ok(None)));

        let mismatch = registry.upsert_post(
            cookie.clone(),
            NoopTcpStream::boxed(),
            peer,
            "/cam/2".to_string(),
            Bytes::new(),
            2_200,
        );
        assert!(matches!(mismatch, Err("http tunnel path mismatch")));

        let matched = registry.upsert_post(
            cookie,
            NoopTcpStream::boxed(),
            peer,
            "/cam/1".to_string(),
            Bytes::new(),
            2_300,
        );
        assert!(matches!(matched, Ok(Some(_))));
    }
}
