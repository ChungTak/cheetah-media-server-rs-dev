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

/// Maximum size allowed for an HTTP tunnel open request header.
///
/// HTTP 隧道打开请求头的最大允许大小。
const HTTP_TUNNEL_HEADER_LIMIT: usize = 64 * 1024;

/// Configuration for the HTTP tunnel pending-pair registry.
///
/// Derived from `DriverConfig` with lower bounds to avoid zero-sized queues or buffers.
///
/// HTTP 隧道待配对注册表的配置。
///
/// 从 `DriverConfig` 派生，并带有下限以避免零大小的队列或缓冲区。
#[derive(Debug, Clone)]
pub(super) struct HttpTunnelRegistryConfig {
    pub(super) max_pending_tunnels: usize,
    pub(super) pending_timeout_ms: u64,
    pub(super) max_decoded_chunk_bytes: usize,
    pub(super) max_base64_buffer_bytes: usize,
}

impl HttpTunnelRegistryConfig {
    pub(super) fn from_driver_config(config: &super::DriverConfig) -> Self {
        Self {
            max_pending_tunnels: config.http_tunnel_max_pending.max(8),
            pending_timeout_ms: config.http_tunnel_pending_timeout_ms.max(1_000),
            max_decoded_chunk_bytes: config.http_tunnel_max_decoded_chunk_bytes.max(1024),
            max_base64_buffer_bytes: config.http_tunnel_max_base64_buffer_bytes.max(4096),
        }
    }
}

/// The GET half of a pending HTTP tunnel pair.
///
/// HTTP 隧道对待配对 GET 半侧。
pub(super) struct PendingGetHalf {
    pub(super) stream: Box<dyn AsyncTcpStream>,
    pub(super) path: String,
    pub(super) expires_at_micros: u64,
}

/// The POST half of a pending HTTP tunnel pair.
///
/// HTTP 隧道对待配对 POST 半侧。
pub(super) struct PendingPostHalf {
    pub(super) stream: Box<dyn AsyncTcpStream>,
    pub(super) peer: std::net::SocketAddr,
    pub(super) path: String,
    pub(super) initial_body: Bytes,
    pub(super) expires_at_micros: u64,
}

/// A matched GET/POST pair ready to be promoted into a connection.
///
/// 已匹配的 GET/POST 对，准备提升为连接。
pub(super) struct PendingPair {
    pub(super) cookie: String,
    pub(super) get: PendingGetHalf,
    pub(super) post: PendingPostHalf,
}

/// Registry that pairs HTTP tunnel GET and POST halves by session cookie.
///
/// Two clients (or the same client from two sockets) open a GET and a POST request with the
/// same `x-sessioncookie`. The registry stores the first half and pairs it when the second
/// half arrives. Entries expire after `pending_timeout_ms` and are evicted when the registry
/// is full. The FIFO list preserves insertion order for eviction and expiry scans.
///
/// 按会话 cookie 配对 HTTP 隧道 GET 与 POST 半侧的注册表。
///
/// 两个客户端（或同一客户端的两个套接字）使用相同 `x-sessioncookie` 打开 GET 和 POST
/// 请求。注册表保存先到达的半侧，并在另一半到达时配对。条目在 `pending_timeout_ms` 后
/// 过期，注册表满时淘汰。FIFO 列表用于维护淘汰与过期扫描的插入顺序。
pub(super) struct HttpTunnelRegistry {
    config: HttpTunnelRegistryConfig,
    gets: HashMap<String, PendingGetHalf>,
    posts: HashMap<String, PendingPostHalf>,
    fifo: VecDeque<String>,
}

impl HttpTunnelRegistry {
    pub(super) fn new(config: HttpTunnelRegistryConfig) -> Self {
        Self {
            config,
            gets: HashMap::new(),
            posts: HashMap::new(),
            fifo: VecDeque::new(),
        }
    }

    pub(super) fn config(&self) -> &HttpTunnelRegistryConfig {
        &self.config
    }

    /// Store or pair a GET half.
    ///
    /// If the matching POST half is already pending and the path matches, a `PendingPair` is
    /// returned. Otherwise the GET half is stored and `None` is returned. Path mismatches are
    /// treated as errors and the existing pending half is preserved.
    ///
    /// 存储或配对 GET 半侧。
    ///
    /// 若匹配的 POST 半侧已在等待且路径一致，返回 `PendingPair`；否则保存 GET 半侧并
    /// 返回 `None`。路径不匹配视为错误，保留现有待配对半侧。
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

    /// Store or pair a POST half.
    ///
    /// If the matching GET half is already pending and the path matches, a `PendingPair` is
    /// returned. Otherwise the POST half is stored and `None` is returned.
    ///
    /// 存储或配对 POST 半侧。
    ///
    /// 若匹配的 GET 半侧已在等待且路径一致，返回 `PendingPair`；否则保存 POST 半侧并
    /// 返回 `None`。
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

    /// Remove and return all expired pending halves.
    ///
    /// Scans the FIFO list for entries whose `expires_at_micros` has passed. Also cleans
    /// the FIFO of entries whose halves have already been removed.
    ///
    /// 移除并返回所有过期的待配对半侧。
    ///
    /// 扫描 FIFO 列表中 `expires_at_micros` 已过的条目，并清理半侧已被移除的 FIFO 条目。
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

    /// Evict the oldest entry if the registry is at capacity.
    ///
    /// 若注册表已满，淘汰最旧的条目。
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

/// HTTP method used to open an HTTP tunnel.
///
/// HTTP 隧道打开时使用的 HTTP 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpTunnelMethod {
    Get,
    Post,
}

/// Parsed HTTP tunnel open request.
///
/// 已解析的 HTTP 隧道打开请求。
pub(super) struct HttpTunnelOpenRequest {
    pub(super) method: HttpTunnelMethod,
    pub(super) cookie: String,
    pub(super) path: String,
    pub(super) initial_post_body: Bytes,
}

/// Result of parsing the HTTP tunnel open request.
///
/// `Tunnel` means the request is a valid tunnel open request. `NotTunnel` means the bytes
/// should be treated as an ordinary HTTP or RTSP stream.
///
/// 解析 HTTP 隧道打开请求的结果。
///
/// `Tunnel` 表示有效的隧道打开请求。`NotTunnel` 表示这些字节应被视为普通 HTTP 或
/// RTSP 流。
pub(super) enum HttpTunnelParseResult {
    Tunnel(HttpTunnelOpenRequest),
    NotTunnel(Bytes),
}

/// Result of probing for a complete HTTP tunnel open request.
///
/// `Parsed` is returned once the header terminator is found. `TimedOut` is returned when the
/// probe deadline expires before the header is complete.
///
/// HTTP 隧道打开请求探测结果。
///
/// 找到头终止符后返回 `Parsed`。在头完成前探测超时时返回 `TimedOut`。
pub(super) enum HttpTunnelProbeResult {
    Parsed(Result<HttpTunnelParseResult, &'static str>),
    TimedOut(Bytes),
}

/// Read bytes from the stream until an HTTP tunnel header is complete or the probe times out.
///
/// This is the server-side counterpart to the client's HTTP tunnel open handshake. It allows
/// the header to arrive in multiple reads and returns the trailing body bytes as part of the
/// open request.
///
/// 从流读取字节直到 HTTP 隧道头完整或探测超时。
///
/// 这是客户端 HTTP 隧道打开握手的对应服务端实现。允许头分多次到达，并将后续体字节
/// 作为打开请求的一部分返回。
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

/// Parse the HTTP tunnel open request header.
///
/// Validates the request line, method, HTTP version, `x-sessioncookie`, and content type.
/// For POST, `Content-Type: application/x-rtsp-tunnelled` is required. If the method is not
/// `GET`/`POST` or the version is unsupported, the data is classified as `NotTunnel`.
///
/// 解析 HTTP 隧道打开请求头。
///
/// 验证请求行、方法、HTTP 版本、`x-sessioncookie` 与 Content-Type。POST 要求
/// `Content-Type: application/x-rtsp-tunnelled`。若方法不是 `GET`/`POST` 或版本不受
/// 支持，数据被归类为 `NotTunnel`。
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

/// Quick check whether the first bytes of a connection look like an HTTP request.
///
/// 快速检查连接的首字节是否看起来像 HTTP 请求。
pub(super) fn looks_like_http_tunnel_candidate(input: &[u8]) -> bool {
    input.starts_with(b"GET ") || input.starts_with(b"POST ")
}

/// Build the 200 OK response for the GET half of the tunnel.
///
/// 构建隧道 GET 半侧的 200 OK 响应。
pub(super) fn build_http_tunnel_get_ok_response() -> Bytes {
    Bytes::from_static(
        b"HTTP/1.0 200 OK\r\nContent-Type: application/x-rtsp-tunnelled\r\nCache-Control: no-cache\r\nPragma: no-cache\r\n\r\n",
    )
}

/// Build the 200 OK response for the POST half of the tunnel.
///
/// 构建隧道 POST 半侧的 200 OK 响应。
pub(super) fn build_http_tunnel_post_ok_response() -> Bytes {
    Bytes::from_static(b"HTTP/1.0 200 OK\r\nCache-Control: no-cache\r\nPragma: no-cache\r\n\r\n")
}

/// Find the index of the first `\r\n\r\n` sequence in the byte slice.
///
/// 返回字节切片中首个 `\r\n\r\n` 序列的索引。
fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Stateful Base64 decoder for the HTTP tunnel POST stream.
///
/// The client sends Base64-encoded RTSP bytes in the POST body. The server must decode
/// them incrementally, handling whitespace and padding only at the end of the stream.
/// The decoder maintains a small buffer of unquantized characters and enforces size limits.
///
/// HTTP 隧道 POST 流的有状态 Base64 解码器。
///
/// 客户端在 POST 请求体中发送 Base64 编码的 RTSP 字节。服务器必须增量解码，处理空白
/// 并仅允许流末尾出现填充。解码器维护少量未量化字符的缓冲区并强制执行大小限制。
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

    /// Decode incoming Base64 bytes and emit complete decoded chunks.
    ///
    /// Whitespace is skipped. Padding characters mark the end of a quantum; decoding stops
    /// until enough characters are buffered to form a full padded quantum. Non-padded data
    /// is decoded in multiples of four characters.
    ///
    /// 解码入站 Base64 字节并输出完整的解码块。
    ///
    /// 跳过空白字符。填充字符标记量子结束；直到缓冲足够字符形成完整填充量子才解码。
    /// 无填充数据以 4 个字符为单位解码。
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

/// Run an HTTP tunnel connection for the server.
///
/// The GET stream is used for outbound (server to client) writes: RTSP responses and
/// interleaved frames. The POST stream is used for inbound (client to server) data: the
/// body is Base64 decoded and fed into `RtspCore`.
///
/// 运行服务器侧 HTTP 隧道连接。
///
/// GET 流用于出站（服务端到客户端）写入：RTSP 响应与交错帧。POST 流用于入站（客户端到
/// 服务端）数据：请求体经 Base64 解码后输入 `RtspCore`。
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

/// Decode POST payload as Base64 and feed the resulting chunks into `RtspCore`.
///
/// Each decoded chunk is treated as `CoreInput::Bytes`. The resulting outputs are flushed
/// to the pending write queue and event channel.
///
/// 将 POST 负载作为 Base64 解码，并将解码后的块输入 `RtspCore`。
///
/// 每个解码块被视为 `CoreInput::Bytes`。产生的输出被刷新到待写队列与事件通道。
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

/// Write a queued byte slice to the stream, aborting if the cancellation token fires.
///
/// 将队列中的字节切片写入流，若取消令牌触发则中止。
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

/// Flush `CoreOutput` values into the pending write queue and event channel.
///
/// `Write` outputs are queued on the GET stream. `Event` outputs are forwarded as
/// `DriverEvent::Core`. `Close` returns immediately.
///
/// 将 `CoreOutput` 刷新到待写队列与事件通道。
///
/// `Write` 输出排队在 GET 流上发送。`Event` 输出作为 `DriverEvent::Core` 转发。
/// `Close` 立即返回。
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
