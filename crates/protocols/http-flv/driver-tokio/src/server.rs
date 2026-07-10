use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use cheetah_http_flv_core::{
    HttpFlvCore, HttpFlvCoreCommand, HttpFlvCoreInput, HttpFlvCoreOutput, HttpFlvEvent, HttpMethod,
    HttpRequestHead,
};
use cheetah_runtime_api::{CancellationToken, JoinHandle, RuntimeApi, TaskJoinError};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;

/// Identifier for `HTTP FLV Connection`.
/// `HTTP FLV Connection` 的标识符。
pub type HttpFlvConnectionId = u64;

#[derive(Clone)]
pub(crate) struct ConnectionControl {
    pub(crate) tx: mpsc::Sender<ConnectionCommand>,
    pub(crate) cancel: CancellationToken,
}

/// Configuration for `HTTP FLV Driver`.
/// `HTTP FLV Driver` 的配置。
#[derive(Debug, Clone)]
pub struct HttpFlvDriverConfig {
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub max_request_header_bytes: usize,
    pub max_body_buffer_bytes: usize,
    pub max_websocket_message_bytes: usize,
    pub command_queue_capacity: usize,
    pub event_queue_capacity: usize,
}

impl Default for HttpFlvDriverConfig {
    fn default() -> Self {
        Self {
            write_queue_capacity: 256,
            read_buffer_size: 16 * 1024,
            max_request_header_bytes: 32 * 1024,
            max_body_buffer_bytes: 4 * 1024 * 1024,
            max_websocket_message_bytes: 1024 * 1024,
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
        }
    }
}

/// Events produced by the `HTTP FLV Driver` subsystem.
/// `HTTP FLV Driver` 子系统产生的事件。
#[derive(Debug)]
pub enum HttpFlvDriverEvent {
    ConnectionOpened {
        connection_id: HttpFlvConnectionId,
        peer: Option<SocketAddr>,
    },
    ConnectionClosed {
        connection_id: HttpFlvConnectionId,
        reason: String,
    },
    Core {
        connection_id: HttpFlvConnectionId,
        event: HttpFlvEvent,
    },
}

/// Command for `HTTP FLV Driver`.
/// `HTTP FLV Driver` 的命令。
#[derive(Debug, Clone)]
pub enum HttpFlvDriverCommand {
    SendFlvBytes {
        connection_id: HttpFlvConnectionId,
        bytes: Bytes,
    },
    CloseConnection {
        connection_id: HttpFlvConnectionId,
    },
    Shutdown,
}

/// `HttpFlvCoreCommandSender` data structure.
/// `HttpFlvCoreCommandSender` 数据结构。
#[derive(Clone)]
pub struct HttpFlvCoreCommandSender {
    pub(crate) tx: mpsc::Sender<HttpFlvDriverCommand>,
}

/// Error returned by `Driver Send` operations.
/// `Driver Send` 操作返回的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverSendError {
    ChannelClosed,
}

impl HttpFlvCoreCommandSender {
    /// Sends data to the peer.
    /// 向对端发送数据。
    pub async fn send(&self, command: HttpFlvDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }

    /// Sends `FLV bytes` to the peer.
    /// 向对端发送 `FLV bytes`。
    pub async fn send_flv_bytes(
        &self,
        connection_id: HttpFlvConnectionId,
        bytes: Bytes,
    ) -> Result<(), DriverSendError> {
        self.send(HttpFlvDriverCommand::SendFlvBytes {
            connection_id,
            bytes,
        })
        .await
    }

    /// Closes the `connection`.
    /// 关闭 `connection`。
    pub async fn close_connection(
        &self,
        connection_id: HttpFlvConnectionId,
    ) -> Result<(), DriverSendError> {
        self.send(HttpFlvDriverCommand::CloseConnection { connection_id })
            .await
    }
}

/// Handle to a `HTTP FLV Server` resource.
/// `HTTP FLV Server` 资源的句柄。
pub struct HttpFlvServerHandle {
    pub(crate) listen: SocketAddr,
    pub(crate) events_rx: mpsc::Receiver<HttpFlvDriverEvent>,
    pub(crate) cmd_tx: HttpFlvCoreCommandSender,
    pub(crate) cancel: CancellationToken,
    pub(crate) join: Box<dyn JoinHandle>,
}

impl HttpFlvServerHandle {
    /// Receives `event` from the peer.
    /// 从对端接收 `event`。
    pub async fn recv_event(&mut self) -> Option<HttpFlvDriverEvent> {
        self.events_rx.recv().await
    }

    /// `local_addr` function of `HttpFlvServerHandle`.
    /// `HttpFlvServerHandle` 的 `local_addr` 函数。
    pub fn local_addr(&self) -> SocketAddr {
        self.listen
    }

    /// `core_command_sender` function of `HttpFlvServerHandle`.
    /// `HttpFlvServerHandle` 的 `core_command_sender` 函数。
    pub fn core_command_sender(&self) -> HttpFlvCoreCommandSender {
        self.cmd_tx.clone()
    }

    /// Shuts down the send or receive side of the stream.
    /// 关闭流的发送或接收端。
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    /// `wait` function of `HttpFlvServerHandle`.
    /// `HttpFlvServerHandle` 的 `wait` 函数。
    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}

#[derive(Debug)]
pub(crate) enum ConnectionCommand {
    SendFlvBytes(Bytes),
    Close,
}

/// Starts the `server`.
/// 启动 `server`。
pub fn start_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: HttpFlvDriverConfig,
    cancel: CancellationToken,
) -> io::Result<HttpFlvServerHandle> {
    let listener = runtime_api.bind_tcp(listen)?;
    let local_addr = listener.local_addr()?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = HttpFlvCoreCommandSender { tx: cmd_tx };

    let conn_ids = Arc::new(AtomicU64::new(1));
    let conn_map: Arc<Mutex<HashMap<HttpFlvConnectionId, ConnectionControl>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let runtime_api = runtime_api.clone();
        let conn_map = conn_map.clone();
        let config = config.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => {
                        break;
                    }
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break; };
                        if handle_driver_command_with_map(cmd, &conn_map) {
                            join_cancel.cancel();
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let (conn_tx, conn_rx) = mpsc::channel(config.write_queue_capacity.max(1));
                                let connection_cancel = join_cancel.child_token();
                                conn_map.lock().insert(
                                    connection_id,
                                    ConnectionControl {
                                        tx: conn_tx,
                                        cancel: connection_cancel.clone(),
                                    },
                                );
                                if event_tx.send(HttpFlvDriverEvent::ConnectionOpened {
                                    connection_id,
                                    peer: Some(peer),
                                }).await.is_err() {
                                    break;
                                }
                                let runtime_api2 = runtime_api.clone();
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let config2 = config.clone();
                                let _ = runtime_api2.spawn(Box::pin(async move {
                                    run_connection(
                                        connection_id,
                                        stream,
                                        conn_rx,
                                        event_tx2,
                                        conn_map2,
                                        connection_cancel,
                                        config2,
                                    ).await;
                                }));
                            }
                            Err(err) => {
                                warn!(%err, "http-flv accept failed");
                            }
                        }
                    }
                }
            }

            let controls: Vec<ConnectionControl> = conn_map.lock().values().cloned().collect();
            for control in controls {
                control.cancel.cancel();
                let _ = control.tx.try_send(ConnectionCommand::Close);
            }
        }
    }));

    Ok(HttpFlvServerHandle {
        listen: local_addr,
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

pub(crate) fn handle_driver_command_with_map(
    command: HttpFlvDriverCommand,
    conn_map: &Arc<Mutex<HashMap<HttpFlvConnectionId, ConnectionControl>>>,
) -> bool {
    match command {
        HttpFlvDriverCommand::SendFlvBytes {
            connection_id,
            bytes,
        } => {
            send_connection_command(
                connection_id,
                ConnectionCommand::SendFlvBytes(bytes),
                conn_map,
            );
            false
        }
        HttpFlvDriverCommand::CloseConnection { connection_id } => {
            send_connection_command(connection_id, ConnectionCommand::Close, conn_map);
            false
        }
        HttpFlvDriverCommand::Shutdown => true,
    }
}

fn send_connection_command(
    connection_id: HttpFlvConnectionId,
    command: ConnectionCommand,
    conn_map: &Arc<Mutex<HashMap<HttpFlvConnectionId, ConnectionControl>>>,
) {
    let Some(control) = conn_map.lock().get(&connection_id).cloned() else {
        return;
    };
    match control.tx.try_send(command) {
        Ok(()) => {}
        Err(TrySendError::Closed(_)) => {
            control.cancel.cancel();
            conn_map.lock().remove(&connection_id);
        }
        Err(TrySendError::Full(_)) => {
            control.cancel.cancel();
            conn_map.lock().remove(&connection_id);
        }
    }
}

pub(crate) async fn run_connection(
    connection_id: HttpFlvConnectionId,
    mut stream: Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    mut conn_rx: mpsc::Receiver<ConnectionCommand>,
    event_tx: mpsc::Sender<HttpFlvDriverEvent>,
    conn_map: Arc<Mutex<HashMap<HttpFlvConnectionId, ConnectionControl>>>,
    cancel: CancellationToken,
    config: HttpFlvDriverConfig,
) {
    let mut core = HttpFlvCore::new();
    let mut is_publish = false;
    let close_reason = match read_request_head(&mut stream, &config, &cancel).await {
        Ok(head) => {
            is_publish = head.method == HttpMethod::Post;
            match core.handle_input(HttpFlvCoreInput::RequestHead(head)) {
                Ok(outputs) => apply_core_outputs(connection_id, &mut stream, outputs, &event_tx)
                    .await
                    .err(),
                Err(err) => Some(format!("core request error: {err}")),
            }
        }
        Err(err) => Some(format!("invalid request: {err}")),
    };

    if let Some(reason) = close_reason {
        let _ = event_tx
            .send(HttpFlvDriverEvent::ConnectionClosed {
                connection_id,
                reason,
            })
            .await;
        conn_map.lock().remove(&connection_id);
        let _ = stream.shutdown().await;
        return;
    }

    // POST push: read body bytes and feed to core for FLV demuxing.
    if is_publish {
        let mut read_buf = vec![0u8; config.read_buffer_size.max(4096)];
        let mut close_sent = false;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => { break; }
                maybe_cmd = conn_rx.recv() => {
                    match maybe_cmd {
                        Some(ConnectionCommand::Close) | None => break,
                        _ => {}
                    }
                }
                read_res = stream.read(&mut read_buf) => {
                    match read_res {
                        Ok(0) => break,
                        Ok(n) => {
                            let bytes = Bytes::copy_from_slice(&read_buf[..n]);
                            match core.handle_input(HttpFlvCoreInput::BodyBytes(bytes)) {
                                Ok(outputs) => {
                                    if let Err(err) = apply_core_outputs(connection_id, &mut stream, outputs, &event_tx).await {
                                        close_sent = true;
                                        let _ = event_tx.send(HttpFlvDriverEvent::ConnectionClosed {
                                            connection_id, reason: err,
                                        }).await;
                                        break;
                                    }
                                }
                                Err(err) => {
                                    close_sent = true;
                                    let _ = event_tx.send(HttpFlvDriverEvent::ConnectionClosed {
                                        connection_id, reason: format!("flv demux error: {err}"),
                                    }).await;
                                    break;
                                }
                            }
                        }
                        Err(err) => {
                            close_sent = true;
                            let _ = event_tx.send(HttpFlvDriverEvent::ConnectionClosed {
                                connection_id, reason: format!("read error: {err}"),
                            }).await;
                            break;
                        }
                    }
                }
            }
        }
        conn_map.lock().remove(&connection_id);
        if !close_sent {
            let _ = event_tx
                .send(HttpFlvDriverEvent::ConnectionClosed {
                    connection_id,
                    reason: "publish connection closed".to_string(),
                })
                .await;
        }
        let _ = stream.shutdown().await;
        return;
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            maybe_cmd = conn_rx.recv() => {
                let Some(cmd) = maybe_cmd else { break; };
                match cmd {
                    ConnectionCommand::SendFlvBytes(bytes) => {
                        match core.handle_input(HttpFlvCoreInput::Command(HttpFlvCoreCommand::SendFlvBytes(bytes))) {
                            Ok(outputs) => {
                                if let Err(err) = apply_core_outputs(connection_id, &mut stream, outputs, &event_tx).await {
                                    let _ = event_tx.send(HttpFlvDriverEvent::ConnectionClosed {
                                        connection_id,
                                        reason: err,
                                    }).await;
                                    break;
                                }
                            }
                            Err(err) => {
                                let _ = event_tx.send(HttpFlvDriverEvent::ConnectionClosed {
                                    connection_id,
                                    reason: format!("core send error: {err}"),
                                }).await;
                                break;
                            }
                        }
                    }
                    ConnectionCommand::Close => {
                        break;
                    }
                }
            }
        }
    }

    conn_map.lock().remove(&connection_id);
    let _ = event_tx
        .send(HttpFlvDriverEvent::ConnectionClosed {
            connection_id,
            reason: "connection closed".to_string(),
        })
        .await;
    let _ = stream.shutdown().await;
}

async fn apply_core_outputs(
    connection_id: HttpFlvConnectionId,
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    outputs: Vec<HttpFlvCoreOutput>,
    event_tx: &mpsc::Sender<HttpFlvDriverEvent>,
) -> Result<(), String> {
    for output in outputs {
        match output {
            HttpFlvCoreOutput::SendHttpResponse(head) => {
                let payload = encode_http_response(head);
                stream
                    .write_all(payload.as_bytes())
                    .await
                    .map_err(|err| err.to_string())?;
            }
            HttpFlvCoreOutput::SendBytes(bytes) => {
                stream
                    .write_all(&bytes)
                    .await
                    .map_err(|err| err.to_string())?;
            }
            HttpFlvCoreOutput::SendWebSocketBinary(bytes) => {
                let frame = encode_ws_binary_frame(&bytes);
                stream
                    .write_all(&frame)
                    .await
                    .map_err(|err| err.to_string())?;
            }
            HttpFlvCoreOutput::Event(event) => {
                event_tx
                    .send(HttpFlvDriverEvent::Core {
                        connection_id,
                        event,
                    })
                    .await
                    .map_err(|_| "event channel closed".to_string())?;
            }
            HttpFlvCoreOutput::Close { reason } => {
                return Err(format!("core requested close: {reason:?}"));
            }
        }
    }
    Ok(())
}

async fn read_request_head(
    stream: &mut Box<dyn cheetah_runtime_api::AsyncTcpStream>,
    config: &HttpFlvDriverConfig,
    cancel: &CancellationToken,
) -> Result<HttpRequestHead, String> {
    let mut buffered = Vec::new();
    let mut chunk = vec![0u8; config.read_buffer_size.max(1024)];
    loop {
        let n = tokio::select! {
            _ = cancel.cancelled() => {
                return Err("connection cancelled while reading request head".to_string());
            }
            result = stream.read(&mut chunk) => {
                result.map_err(|err| err.to_string())?
            }
        };
        if n == 0 {
            return Err("peer closed before request head".to_string());
        }
        buffered.extend_from_slice(&chunk[..n]);
        if buffered.len() > config.max_request_header_bytes {
            return Err("request head exceeds limit".to_string());
        }
        if let Some(end) = find_header_end(&buffered) {
            return parse_http_request_head(&buffered[..end]);
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|win| win == b"\r\n\r\n")
        .map(|idx| idx + 4)
}

fn parse_http_request_head(raw: &[u8]) -> Result<HttpRequestHead, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "request head is not utf8")?;
    let mut lines = text.split("\r\n").filter(|line| !line.is_empty());
    let first = lines.next().ok_or("request line missing")?;
    let mut parts = first.split_whitespace();
    let method_raw = parts.next().ok_or("request method missing")?.to_string();
    let method = if method_raw.eq_ignore_ascii_case("GET") {
        HttpMethod::Get
    } else if method_raw.eq_ignore_ascii_case("POST") {
        HttpMethod::Post
    } else if method_raw.eq_ignore_ascii_case("OPTIONS") {
        HttpMethod::Options
    } else {
        HttpMethod::Other
    };
    let target = parts.next().ok_or("request target missing")?.to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(HttpRequestHead {
        method,
        method_raw,
        target,
        headers,
    })
}

fn encode_http_response(head: cheetah_http_flv_core::HttpResponseHead) -> String {
    let mut out = format!("HTTP/1.1 {} {}\r\n", head.status_code, head.reason);
    for (name, value) in head.headers {
        out.push_str(&name);
        out.push_str(": ");
        out.push_str(&value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

fn encode_ws_binary_frame(payload: &[u8]) -> Bytes {
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
    Bytes::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_runtime_api::RuntimeApi;
    use cheetah_runtime_tokio::TokioRuntime;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_get_receives_flv_bytes() {
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();
        let mut handle = start_server(
            runtime,
            "127.0.0.1:0".parse().expect("listen"),
            HttpFlvDriverConfig::default(),
            cancel,
        )
        .expect("start");
        let addr = handle.local_addr();

        let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        client
            .write_all(b"GET /live/stream.flv HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write");

        let connection_id = loop {
            let event = timeout(std::time::Duration::from_secs(2), handle.recv_event())
                .await
                .expect("timeout")
                .expect("event");
            match event {
                HttpFlvDriverEvent::Core {
                    connection_id,
                    event: HttpFlvEvent::PlayRequested { .. },
                } => break connection_id,
                _ => {}
            }
        };

        handle
            .core_command_sender()
            .send_flv_bytes(connection_id, Bytes::from_static(b"FLVBYTES"))
            .await
            .expect("send flv");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut accumulated = Vec::with_capacity(1024);
        let mut buf = [0u8; 256];
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let n = timeout(remaining, client.read(&mut buf))
                .await
                .expect("timeout")
                .expect("read");
            if n == 0 {
                break;
            }
            accumulated.extend_from_slice(&buf[..n]);
            if accumulated
                .windows(b"FLVBYTES".len())
                .any(|window| window == b"FLVBYTES")
            {
                break;
            }
        }

        let text = String::from_utf8_lossy(&accumulated);
        assert!(text.contains("HTTP/1.1 200 OK"));
        assert!(text.contains("video/x-flv"));
        assert!(text.contains("FLVBYTES"));

        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn websocket_upgrade_receives_binary_frame() {
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();
        let mut handle = start_server(
            runtime,
            "127.0.0.1:0".parse().expect("listen"),
            HttpFlvDriverConfig::default(),
            cancel,
        )
        .expect("start");
        let addr = handle.local_addr();

        let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        client
            .write_all(
                b"GET /live/stream.flv?type=enhanced HTTP/1.1\r\n\
Host: localhost\r\n\
Connection: Upgrade\r\n\
Upgrade: websocket\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            )
            .await
            .expect("write");

        let connection_id = loop {
            let event = timeout(std::time::Duration::from_secs(2), handle.recv_event())
                .await
                .expect("timeout")
                .expect("event");
            match event {
                HttpFlvDriverEvent::Core {
                    connection_id,
                    event: HttpFlvEvent::PlayRequested { .. },
                } => break connection_id,
                _ => {}
            }
        };

        handle
            .core_command_sender()
            .send_flv_bytes(connection_id, Bytes::from_static(b"ABC"))
            .await
            .expect("send flv");

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut accumulated = Vec::with_capacity(1024);
        let mut buf = [0u8; 256];
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let n = timeout(remaining, client.read(&mut buf))
                .await
                .expect("timeout")
                .expect("read");
            if n == 0 {
                break;
            }
            accumulated.extend_from_slice(&buf[..n]);
            if let Some(frame_start) = accumulated
                .windows(4)
                .position(|win| win == b"\r\n\r\n")
                .map(|idx| idx + 4)
            {
                if accumulated.len() >= frame_start + 5 {
                    break;
                }
            }
        }

        let response_text = String::from_utf8_lossy(&accumulated);
        assert!(response_text.contains("101 Switching Protocols"));
        assert!(response_text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

        let frame_start = accumulated
            .windows(4)
            .position(|win| win == b"\r\n\r\n")
            .map(|idx| idx + 4)
            .expect("frame start");
        let frame = &accumulated[frame_start..];
        assert!(frame.len() >= 5);
        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 3);
        assert_eq!(&frame[2..5], b"ABC");

        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_queue_full_closes_slow_connection_and_driver_keeps_running() {
        let runtime = Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>;
        let cancel = CancellationToken::new();
        let mut handle = start_server(
            runtime,
            "127.0.0.1:0".parse().expect("listen"),
            HttpFlvDriverConfig {
                write_queue_capacity: 1,
                ..HttpFlvDriverConfig::default()
            },
            cancel,
        )
        .expect("start");
        let addr = handle.local_addr();

        let mut slow_client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        slow_client
            .write_all(b"GET /live/stream.flv HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .expect("write partial");

        let slow_connection_id = loop {
            let event = timeout(Duration::from_secs(2), handle.recv_event())
                .await
                .expect("timeout")
                .expect("event");
            match event {
                HttpFlvDriverEvent::ConnectionOpened { connection_id, .. } => break connection_id,
                _ => {}
            }
        };

        handle
            .core_command_sender()
            .send_flv_bytes(slow_connection_id, Bytes::from_static(b"A"))
            .await
            .expect("send first");
        handle
            .core_command_sender()
            .send_flv_bytes(slow_connection_id, Bytes::from_static(b"B"))
            .await
            .expect("send second");

        loop {
            let event = timeout(Duration::from_secs(2), handle.recv_event())
                .await
                .expect("timeout")
                .expect("event");
            match event {
                HttpFlvDriverEvent::ConnectionClosed { connection_id, .. }
                    if connection_id == slow_connection_id =>
                {
                    break;
                }
                _ => {}
            }
        }

        let mut normal_client = tokio::net::TcpStream::connect(addr).await.expect("connect");
        normal_client
            .write_all(b"GET /live/ok.flv HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write");

        loop {
            let event = timeout(Duration::from_secs(2), handle.recv_event())
                .await
                .expect("timeout")
                .expect("event");
            match event {
                HttpFlvDriverEvent::Core {
                    event: HttpFlvEvent::PlayRequested { .. },
                    ..
                } => break,
                _ => {}
            }
        }

        handle.shutdown();
        let _ = handle.wait().await;
    }
}
