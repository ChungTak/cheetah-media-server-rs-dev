use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use cheetah_hls_core::session::HttpMethod;
use cheetah_hls_core::{HlsCore, HlsCoreEvent, HlsCoreInput, HlsCoreOutput};
use cheetah_runtime_api::{CancellationToken, JoinHandle, RuntimeApi, TaskJoinError};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// `HlsConnectionId` type alias.
/// `HlsConnectionId` 类型别名.
pub type HlsConnectionId = u64;

/// `HlsDriverConfig` data structure.
/// `HlsDriverConfig` 数据结构.
#[derive(Debug, Clone)]
pub struct HlsDriverConfig {
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `max_request_header_bytes` field of type `usize`.
    /// `max_request_header_bytes` 字段，类型为 `usize`.
    pub max_request_header_bytes: usize,
    /// `command_queue_capacity` field of type `usize`.
    /// `command_queue_capacity` 字段，类型为 `usize`.
    pub command_queue_capacity: usize,
    /// `event_queue_capacity` field of type `usize`.
    /// `event_queue_capacity` 字段，类型为 `usize`.
    pub event_queue_capacity: usize,
    /// Optional module response timeout in milliseconds. `None` lets the module own
    /// LL-HLS blocking reload/preload timeouts.
    pub module_response_timeout_ms: Option<u64>,
    /// Whether the driver should issue HLS_SESSION cookies when clients have none.
    pub set_session_cookie: bool,
    /// Optional root directory for serving HLS files from disk.
    pub file_root: Option<std::path::PathBuf>,
}

impl Default for HlsDriverConfig {
    fn default() -> Self {
        Self {
            read_buffer_size: 8 * 1024,
            max_request_header_bytes: 16 * 1024,
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
            module_response_timeout_ms: None,
            set_session_cookie: true,
            file_root: None,
        }
    }
}

/// `HlsDriverEvent` enumeration.
/// `HlsDriverEvent` 枚举.
#[derive(Debug)]
pub enum HlsDriverEvent {
    /// `ConnectionOpened` variant.
    /// `ConnectionOpened` 变体.
    ConnectionOpened {
        connection_id: HlsConnectionId,
        peer: Option<SocketAddr>,
    },
    /// `ConnectionClosed` variant.
    /// `ConnectionClosed` 变体.
    ConnectionClosed { connection_id: HlsConnectionId },
    /// `Core` variant.
    /// `Core` 变体.
    Core {
        connection_id: HlsConnectionId,
        event: HlsCoreEvent,
    },
}

/// `HlsDriverCommand` enumeration.
/// `HlsDriverCommand` 枚举.
#[derive(Debug)]
pub enum HlsDriverCommand {
    /// Send a complete HTTP response to a connection.
    SendResponse {
        connection_id: HlsConnectionId,
        status: u16,
        content_type: &'static str,
        body: Bytes,
        headers: Vec<(&'static str, String)>,
    },
    /// `CloseConnection` variant.
    /// `CloseConnection` 变体.
    CloseConnection { connection_id: HlsConnectionId },
    /// `Shutdown` variant.
    /// `Shutdown` 变体.
    Shutdown,
}

/// `HlsCommandSender` data structure.
/// `HlsCommandSender` 数据结构.
#[derive(Clone)]
pub struct HlsCommandSender {
    /// `tx` field.
    /// `tx` 字段.
    tx: mpsc::Sender<HlsDriverCommand>,
}

/// `DriverSendError` enumeration.
/// `DriverSendError` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverSendError {
    /// `ChannelClosed` variant.
    /// `ChannelClosed` 变体.
    ChannelClosed,
}

impl HlsCommandSender {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub(crate) fn new(tx: mpsc::Sender<HlsDriverCommand>) -> Self {
        Self { tx }
    }

    /// `send` function.
    /// `send` 函数.
    pub async fn send(&self, command: HlsDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }
}

/// `HlsServerHandle` data structure.
/// `HlsServerHandle` 数据结构.
pub struct HlsServerHandle {
    /// `listen` field of type `SocketAddr`.
    /// `listen` 字段，类型为 `SocketAddr`.
    listen: SocketAddr,
    /// `events_rx` field.
    /// `events_rx` 字段.
    events_rx: mpsc::Receiver<HlsDriverEvent>,
    /// `cmd_tx` field of type `HlsCommandSender`.
    /// `cmd_tx` 字段，类型为 `HlsCommandSender`.
    cmd_tx: HlsCommandSender,
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    cancel: CancellationToken,
    /// `join` field.
    /// `join` 字段.
    join: Box<dyn JoinHandle>,
}

impl HlsServerHandle {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub(crate) fn new(
        listen: SocketAddr,
        events_rx: mpsc::Receiver<HlsDriverEvent>,
        cmd_tx: HlsCommandSender,
        cancel: CancellationToken,
        join: Box<dyn JoinHandle>,
    ) -> Self {
        Self {
            listen,
            events_rx,
            cmd_tx,
            cancel,
            join,
        }
    }

    /// `recv_event` function.
    /// `recv_event` 函数.
    pub async fn recv_event(&mut self) -> Option<HlsDriverEvent> {
        self.events_rx.recv().await
    }

    /// `local_addr` function.
    /// `local_addr` 函数.
    pub fn local_addr(&self) -> SocketAddr {
        self.listen
    }

    /// `command_sender` function.
    /// `command_sender` 函数.
    pub fn command_sender(&self) -> HlsCommandSender {
        self.cmd_tx.clone()
    }

    /// `shutdown` function.
    /// `shutdown` 函数.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    /// `wait` function.
    /// `wait` 函数.
    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}

struct ConnectionState {
    response_tx: mpsc::Sender<HttpResponseData>,
    cancel: CancellationToken,
}

/// `HttpResponseData` data structure.
/// `HttpResponseData` 数据结构.
pub(crate) struct HttpResponseData {
    /// `status` field of type `u16`.
    /// `status` 字段，类型为 `u16`.
    pub(crate) status: u16,
    /// `content_type` field of type `&'static str`.
    /// `content_type` 字段，类型为 `&'static str`.
    pub(crate) content_type: &'static str,
    /// `body` field of type `Bytes`.
    /// `body` 字段，类型为 `Bytes`.
    pub(crate) body: Bytes,
    /// `headers` field.
    /// `headers` 字段.
    pub(crate) headers: Vec<(&'static str, String)>,
}

/// `start_server` function.
/// `start_server` 函数.
pub fn start_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: HlsDriverConfig,
    cancel: CancellationToken,
) -> io::Result<HlsServerHandle> {
    let listener = runtime_api.bind_tcp(listen)?;
    let local_addr = listener.local_addr()?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = HlsCommandSender::new(cmd_tx);

    let conn_ids = Arc::new(AtomicU64::new(1));
    let conn_map: Arc<Mutex<HashMap<HlsConnectionId, ConnectionState>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let runtime_api = runtime_api.clone();
        let conn_map = conn_map.clone();
        let config = config.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => break,
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break };
                        match cmd {
                            HlsDriverCommand::SendResponse {
                                connection_id,
                                status,
                                content_type,
                                body,
                                headers,
                            } => {
                                let map = conn_map.lock();
                                if let Some(state) = map.get(&connection_id) {
                                    let try_result = state.response_tx.try_send(HttpResponseData {
                                        status,
                                        content_type,
                                        body,
                                        headers,
                                    });
                                    debug!(
                                        "hls driver: SendResponse cmd processed conn={} status={} ok={}",
                                        connection_id,
                                        status,
                                        try_result.is_ok()
                                    );
                                } else {
                                    warn!(
                                        "hls driver: SendResponse for unknown conn={} status={}",
                                        connection_id, status
                                    );
                                }
                            }
                            HlsDriverCommand::CloseConnection { connection_id } => {
                                let map = conn_map.lock();
                                if let Some(state) = map.get(&connection_id) {
                                    state.cancel.cancel();
                                }
                            }
                            HlsDriverCommand::Shutdown => {
                                join_cancel.cancel();
                                break;
                            }
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                debug!(
                                    "hls driver: accepted connection conn={} peer={:?}",
                                    connection_id, peer
                                );
                                let (resp_tx, resp_rx) = mpsc::channel(1);
                                let connection_cancel = join_cancel.child_token();
                                conn_map.lock().insert(connection_id, ConnectionState {
                                    response_tx: resp_tx,
                                    cancel: connection_cancel.clone(),
                                });
                                if event_tx.send(HlsDriverEvent::ConnectionOpened {
                                    connection_id,
                                    peer: Some(peer),
                                }).await.is_err() {
                                    break;
                                }
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let config2 = config.clone();
                                let _ = runtime_api.spawn(Box::pin(async move {
                                    run_connection(
                                        connection_id,
                                        stream,
                                        resp_rx,
                                        event_tx2,
                                        connection_cancel,
                                        config2,
                                    ).await;
                                    conn_map2.lock().remove(&connection_id);
                                }));
                            }
                            Err(err) => {
                                warn!("HLS accept error: {err}");
                            }
                        }
                    }
                }
            }
        }
    }));

    Ok(HlsServerHandle::new(
        local_addr,
        event_rx,
        command_sender,
        cancel,
        join,
    ))
}

/// Maximum bytes per write chunk for segment data (128KB).
const SEND_CHUNK_SIZE: usize = 128 * 1024;
/// Keep-Alive idle timeout in seconds.
const KEEP_ALIVE_TIMEOUT_SECS: u64 = 30;
/// Maximum requests per connection before closing.
const KEEP_ALIVE_MAX_REQUESTS: u32 = 100;
/// Write timeout per chunk in seconds.
const WRITE_TIMEOUT_SECS: u64 = 10;

/// `run_connection` function.
/// `run_connection` 函数.
pub(crate) async fn run_connection(
    connection_id: HlsConnectionId,
    mut stream: Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    mut resp_rx: mpsc::Receiver<HttpResponseData>,
    event_tx: mpsc::Sender<HlsDriverEvent>,
    cancel: CancellationToken,
    config: HlsDriverConfig,
) {
    let mut requests_served: u32 = 0;

    loop {
        if requests_served >= KEEP_ALIVE_MAX_REQUESTS {
            break;
        }

        // Read next request with idle timeout
        let request_head = tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(KEEP_ALIVE_TIMEOUT_SECS)) => break,
            result = read_request_head(&mut stream, &config, &cancel) => {
                match result {
                    Ok(head) => head,
                    Err(_) => {
                        if requests_served == 0 {
                            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                        }
                        break;
                    }
                }
            }
        };

        requests_served += 1;
        let (
            method,
            target,
            cookie_session,
            client_wants_close,
            authorization,
            user_agent,
            accept_gzip,
        ) = request_head;
        let is_head = method == HttpMethod::Head;
        let keep_alive = !client_wants_close && requests_served < KEEP_ALIVE_MAX_REQUESTS;

        // Security validation
        if let Err(status) = validate_request(&target) {
            let _ = write_response(
                &mut stream,
                status,
                "text/plain",
                &[],
                b"Bad Request",
                false,
                false,
            )
            .await;
            break;
        }

        // Generate session ID for Set-Cookie if client has no cookie
        let new_session_id = if config.set_session_cookie && cookie_session.is_none() {
            Some(connection_id)
        } else {
            None
        };

        // Feed to core
        let mut core = HlsCore::new();
        let outputs = core.handle_input(HlsCoreInput::HttpRequest {
            method,
            target: target.clone(),
            connection_id,
            headers: cheetah_hls_core::HlsRequestHeaders {
                authorization,
                user_agent,
                if_none_match: None,
                accept_gzip,
            },
        });

        let mut needs_module_response = false;
        for output in outputs {
            match output {
                HlsCoreOutput::SendResponse {
                    status,
                    content_type,
                    body,
                    headers,
                    ..
                } => {
                    if write_response(
                        &mut stream,
                        status,
                        content_type,
                        &headers,
                        &body,
                        is_head,
                        keep_alive,
                    )
                    .await
                    .is_err()
                    {
                        let _ = event_tx
                            .send(HlsDriverEvent::ConnectionClosed { connection_id })
                            .await;
                        return;
                    }
                }
                HlsCoreOutput::Event(event) => {
                    if event_tx
                        .send(HlsDriverEvent::Core {
                            connection_id,
                            event,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    needs_module_response = true;
                }
            }
        }

        // Wait for module response if needed. LL-HLS blocking requests are timed out
        // by the module according to its HLS configuration, so the driver does not
        // impose a shorter default timeout.
        if needs_module_response {
            let resp = if let Some(timeout_ms) = config.module_response_timeout_ms {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
                        let _ = write_response(
                            &mut stream,
                            503,
                            "text/plain",
                            &[],
                            b"Service Unavailable",
                            is_head,
                            false,
                        ).await;
                        break;
                    }
                    r = resp_rx.recv() => r,
                }
            } else {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    r = resp_rx.recv() => r,
                }
            };
            if let Some(mut data) = resp {
                if let Some(sid) = new_session_id {
                    data.headers
                        .push(("Set-Cookie", format_session_cookie(sid, &target)));
                }
                let status = data.status;
                let body_len = data.body.len();
                let write_result = write_response(
                    &mut stream,
                    data.status,
                    data.content_type,
                    &data.headers,
                    &data.body,
                    is_head,
                    keep_alive,
                )
                .await;
                debug!(
                    "hls driver: wrote response conn={} status={} body_len={} ok={}",
                    connection_id,
                    status,
                    body_len,
                    write_result.is_ok()
                );
                if write_result.is_err() {
                    break;
                }
            } else {
                break;
            }
        }

        if !keep_alive {
            break;
        }
    }

    let _ = event_tx
        .send(HlsDriverEvent::ConnectionClosed { connection_id })
        .await;
}

/// Write a complete HTTP response with chunked body sending and write timeout.
async fn write_response(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    status: u16,
    content_type: &str,
    headers: &[(&str, String)],
    body: &[u8],
    is_head: bool,
    keep_alive: bool,
) -> Result<(), ()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Unknown",
    };

    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    if !content_type.is_empty() {
        head.push_str("Content-Type: ");
        head.push_str(content_type);
        head.push_str("\r\n");
    }
    head.push_str("Content-Length: ");
    head.push_str(&body.len().to_string());
    head.push_str("\r\n");
    for (key, value) in headers {
        head.push_str(key);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if keep_alive {
        head.push_str("Connection: keep-alive\r\nKeep-Alive: timeout=30, max=100\r\n");
    } else {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");

    // Write headers with timeout
    if write_with_timeout(stream, head.as_bytes()).await.is_err() {
        return Err(());
    }

    // HEAD: skip body
    if is_head || body.is_empty() {
        return Ok(());
    }

    // Write body in chunks for backpressure control
    for chunk in body.chunks(SEND_CHUNK_SIZE) {
        if write_with_timeout(stream, chunk).await.is_err() {
            return Err(());
        }
    }

    Ok(())
}

/// Write data with a timeout to detect slow/dead clients.
async fn write_with_timeout(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    data: &[u8],
) -> Result<(), ()> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(WRITE_TIMEOUT_SECS),
        stream.write_all(data),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

async fn read_request_head(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    config: &HlsDriverConfig,
    cancel: &CancellationToken,
) -> Result<ParsedRequest, String> {
    let mut buffered = Vec::new();
    let mut chunk = vec![0u8; config.read_buffer_size.max(1024)];
    loop {
        let n = tokio::select! {
            _ = cancel.cancelled() => {
                return Err("cancelled".to_string());
            }
            result = stream.read(&mut chunk) => {
                result.map_err(|err| err.to_string())?
            }
        };
        if n == 0 {
            return Err("peer closed".to_string());
        }
        buffered.extend_from_slice(&chunk[..n]);
        if buffered.len() > config.max_request_header_bytes {
            return Err("header too large".to_string());
        }
        if find_header_end(&buffered).is_some() {
            return parse_request_line(&buffered);
        }
    }
}

/// Format a Set-Cookie header with 2-minute expiry and path scoped to stream.
fn format_session_cookie(session_id: u64, target: &str) -> String {
    // Extract path scope: /{app}/{stream}/ from target like /live/test.m3u8
    let path_scope = target.rfind('/').map(|i| &target[..=i]).unwrap_or("/");
    format!("HLS_SESSION={session_id}; Max-Age=120; Path={path_scope}; HttpOnly")
}

/// Validate request for security (illegal chars, path traversal, length).
/// Returns Err with HTTP status code if request should be rejected.
fn validate_request(target: &str) -> Result<(), u16> {
    // Reject % encoding (potential injection/traversal)
    if target.contains('%') {
        return Err(400);
    }
    // Reject path traversal
    if target.contains("..") {
        return Err(400);
    }
    // Max path length
    if target.len() > 512 {
        return Err(414);
    }
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Parsed request: (method, target, cookie_session, wants_close, authorization, user_agent, accept_gzip)
type ParsedRequest = (
    HttpMethod,
    String,
    Option<u64>,
    bool,
    Option<String>,
    Option<String>,
    bool,
);

fn parse_request_line(raw: &[u8]) -> Result<ParsedRequest, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "not utf8")?;
    let first_line = text.lines().next().ok_or("empty")?;
    let mut parts = first_line.split_whitespace();
    let method_str = parts.next().ok_or("no method")?;
    let target = parts.next().ok_or("no target")?;

    let method = match method_str {
        "GET" => HttpMethod::Get,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => HttpMethod::Other,
    };

    let mut cookie_session = None;
    let mut client_wants_close = false;
    let mut authorization = None;
    let mut user_agent = None;
    let mut accept_gzip = false;

    for line in text.lines().skip(1) {
        let line = line.trim();
        if let Some(value) = line
            .strip_prefix("Cookie:")
            .or_else(|| line.strip_prefix("cookie:"))
        {
            cookie_session = value.split(';').find_map(|pair| {
                pair.trim()
                    .strip_prefix("HLS_SESSION=")
                    .and_then(|v| v.trim().parse::<u64>().ok())
            });
        } else if let Some(value) = line
            .strip_prefix("Connection:")
            .or_else(|| line.strip_prefix("connection:"))
        {
            if value.trim().eq_ignore_ascii_case("close") {
                client_wants_close = true;
            }
        } else if let Some(value) = line
            .strip_prefix("Authorization:")
            .or_else(|| line.strip_prefix("authorization:"))
        {
            authorization = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("User-Agent:")
            .or_else(|| line.strip_prefix("user-agent:"))
        {
            user_agent = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("Accept-Encoding:")
            .or_else(|| line.strip_prefix("accept-encoding:"))
        {
            accept_gzip = accept_encoding_allows_gzip(value);
        }
    }

    Ok((
        method,
        target.to_string(),
        cookie_session,
        client_wants_close,
        authorization,
        user_agent,
        accept_gzip,
    ))
}

fn accept_encoding_allows_gzip(value: &str) -> bool {
    let mut wildcard_q = None;
    for token in value.split(',') {
        let mut parts = token.trim().split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let mut q = 1.0_f32;
        for param in parts {
            let mut kv = param.trim().splitn(2, '=');
            let key = kv.next().unwrap_or_default().trim();
            let value = kv.next().unwrap_or_default().trim();
            if key.eq_ignore_ascii_case("q") {
                q = value.parse::<f32>().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }
        if coding.eq_ignore_ascii_case("gzip") {
            return q > 0.0;
        }
        if coding == "*" {
            wildcard_q = Some(q);
        }
    }
    wildcard_q.is_some_and(|q| q > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_does_not_preempt_module_managed_blocking_responses() {
        assert_eq!(HlsDriverConfig::default().module_response_timeout_ms, None);
    }

    #[test]
    fn accept_encoding_gzip_q_zero_disables_gzip() {
        let raw = b"GET /live/test.m3u8 HTTP/1.1\r\nAccept-Encoding: gzip;q=0, br\r\n\r\n";
        let parsed = parse_request_line(raw).expect("parse request");

        assert!(!parsed.6);
    }

    #[test]
    fn accept_encoding_wildcard_q_zero_does_not_enable_gzip() {
        let raw = b"GET /live/test.m3u8 HTTP/1.1\r\nAccept-Encoding: *;q=0\r\n\r\n";
        let parsed = parse_request_line(raw).expect("parse request");

        assert!(!parsed.6);
    }

    #[test]
    fn accept_encoding_wildcard_enables_gzip_when_not_disabled() {
        let raw = b"GET /live/test.m3u8 HTTP/1.1\r\nAccept-Encoding: br, *;q=0.5\r\n\r\n";
        let parsed = parse_request_line(raw).expect("parse request");

        assert!(parsed.6);
    }
}
