use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_rtsp_core::RtspResponseMessage;
use cheetah_runtime_api::{CancellationToken, JoinHandle, RuntimeApi, TaskJoinError};
use tokio::sync::mpsc;

mod auth;
mod command;
mod connection;
mod http_tunnel;
mod udp;

pub use auth::{authorization_header_from_response, RtspClientCredentials};
pub use command::{RtspClientCommand, RtspClientCommandSender};
pub use udp::{
    allocate_udp_endpoint, configure_udp_remote_and_punch, spawn_udp_receive_tasks,
    RtspClientPortRange, RtspClientUdpEndpoint, RtspClientUdpRemote,
};

/// Configuration for the RTSP client driver.
///
/// Capacities and buffer sizes are bounded to prevent unbounded memory growth under
/// backpressure. The `udp_port_range` and `http_tunnel_header_limit` fields are optional
/// and default to unconstrained UDP and a 64 KiB tunnel header limit.
///
/// RTSP 客户端驱动配置。
///
/// 容量与缓冲区大小均有界，防止背压下内存无限增长。`udp_port_range` 与
/// `http_tunnel_header_limit` 为可选字段，默认不限制 UDP 端口并将隧道头大小限制
/// 设为 64 KiB。
#[derive(Debug, Clone)]
pub struct RtspClientConfig {
    pub command_queue_capacity: usize,
    pub event_queue_capacity: usize,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub udp_port_range: Option<RtspClientPortRange>,
    pub http_tunnel_header_limit: usize,
}

impl Default for RtspClientConfig {
    fn default() -> Self {
        Self {
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
            write_queue_capacity: 256,
            read_buffer_size: 64 * 1024,
            udp_port_range: None,
            http_tunnel_header_limit: 64 * 1024,
        }
    }
}

/// Events emitted by the RTSP client driver to the application.
///
/// `Connected` is emitted once the transport is ready. `Response` carries parsed RTSP
/// responses. `InterleavedFrame` carries RTP/RTCP frames received over TCP or HTTP tunnel.
/// `UdpRtp`/`UdpRtcp` carry UDP payloads for a track. `Closed` is emitted once before the
/// driver task exits.
///
/// RTSP 客户端驱动向应用层发出的事件。
///
/// `Connected` 在传输就绪时发出一次。`Response` 携带解析后的 RTSP 响应。`InterleavedFrame`
/// 携带通过 TCP 或 HTTP 隧道收到的 RTP/RTCP 帧。`UdpRtp`/`UdpRtcp` 携带轨道的 UDP 负载。
/// `Closed` 在驱动任务退出前发出一次。
#[derive(Debug, Clone)]
pub enum RtspClientEvent {
    /// Transport is connected and ready for commands.
    ///
    /// 传输已连接并准备好接收命令。
    Connected { peer: SocketAddr },

    /// A complete RTSP response was received.
    ///
    /// 收到完整的 RTSP 响应。
    Response { response: RtspResponseMessage },

    /// An interleaved RTP/RTCP frame was received on the TCP/TLS/HTTP tunnel.
    ///
    /// 在 TCP/TLS/HTTP 隧道上收到交错 RTP/RTCP 帧。
    InterleavedFrame { channel: u8, payload: Bytes },

    /// A UDP RTP payload was received for a track.
    ///
    /// 收到某个轨道的 UDP RTP 负载。
    UdpRtp {
        track_id: u32,
        from: SocketAddr,
        payload: Bytes,
    },

    /// A UDP RTCP payload was received for a track.
    ///
    /// 收到某个轨道的 UDP RTCP 负载。
    UdpRtcp {
        track_id: u32,
        from: SocketAddr,
        payload: Bytes,
    },

    /// The driver task is shutting down.
    ///
    /// 驱动任务正在关闭。
    Closed { reason: String },
}

/// Error returned when a command cannot be sent to the client driver.
///
/// 向客户端驱动发送命令失败时返回的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtspClientSendError {
    /// The command channel has closed.
    ///
    /// 命令通道已关闭。
    ChannelClosed,
}

impl std::fmt::Display for RtspClientSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChannelClosed => f.write_str("command channel closed"),
        }
    }
}

impl std::error::Error for RtspClientSendError {}

/// Handle to a running RTSP client driver.
///
/// Owns the event receiver, the command sender, the cancellation token, and the task join
/// handle. Dropping the handle does not stop the task; call `shutdown` or `wait` explicitly.
///
/// 运行中 RTSP 客户端驱动的句柄。
///
/// 拥有事件接收器、命令发送器、取消令牌和任务句柄。丢弃句柄不会停止任务；需要显式
/// 调用 `shutdown` 或 `wait`。
pub struct RtspClientHandle {
    events_rx: mpsc::Receiver<RtspClientEvent>,
    event_tx: mpsc::Sender<RtspClientEvent>,
    cmd_tx: RtspClientCommandSender,
    cancel: CancellationToken,
    join: Box<dyn JoinHandle>,
}

impl RtspClientHandle {
    /// Receive the next event from the driver.
    ///
    /// Returns `None` when the event channel has closed.
    ///
    /// 从驱动接收下一个事件。
    ///
    /// 事件通道关闭时返回 `None`。
    pub async fn recv_event(&mut self) -> Option<RtspClientEvent> {
        self.events_rx.recv().await
    }

    /// Send a command to the client driver.
    ///
    /// 向客户端驱动发送命令。
    pub async fn send_command(
        &self,
        command: RtspClientCommand,
    ) -> Result<(), RtspClientSendError> {
        self.cmd_tx.send(command).await
    }

    /// Clone the command sender so other tasks can drive the client.
    ///
    /// 克隆命令发送器，使其他任务也能控制客户端。
    pub fn command_sender(&self) -> RtspClientCommandSender {
        self.cmd_tx.clone()
    }

    /// Clone the event sender; useful when forwarding events to another consumer.
    ///
    /// 克隆事件发送器；适用于将事件转发给其他消费者。
    pub fn event_sender(&self) -> mpsc::Sender<RtspClientEvent> {
        self.event_tx.clone()
    }

    /// Request a graceful shutdown of the client driver.
    ///
    /// 请求优雅关闭客户端驱动。
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    /// Wait for the driver task to complete.
    ///
    /// 等待驱动任务完成。
    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}

/// Start a plain TCP RTSP client connection.
///
/// The `RuntimeApi::connect_tcp` call must be synchronous; the actual async I/O is then
/// driven by a spawned task. The caller receives events through `RtspClientHandle`.
///
/// 启动一个普通 TCP RTSP 客户端连接。
///
/// `RuntimeApi::connect_tcp` 调用为同步；真正的异步 I/O 由生成的任务驱动。调用者通过
/// `RtspClientHandle` 接收事件。
pub fn start_tcp_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    config: RtspClientConfig,
    cancel: CancellationToken,
) -> io::Result<RtspClientHandle> {
    let stream = runtime_api.connect_tcp(peer)?;
    let (cmd_tx, cmd_rx) = mpsc::channel(config.command_queue_capacity.max(8));
    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(8));
    let child_cancel = cancel.child_token();
    let event_tx_for_task = event_tx.clone();
    let task_config = config.clone();

    let join = runtime_api.spawn(Box::pin(async move {
        let _ = event_tx_for_task
            .send(RtspClientEvent::Connected { peer })
            .await;
        connection::run_tcp_client_connection(
            stream,
            cmd_rx,
            event_tx_for_task,
            child_cancel,
            task_config,
        )
        .await;
    }));

    Ok(RtspClientHandle {
        events_rx: event_rx,
        event_tx,
        cmd_tx: RtspClientCommandSender::new(cmd_tx),
        cancel,
        join,
    })
}

/// Start a TLS-encrypted RTSP client connection (rtsps://).
///
/// Performs a TCP connect, then a TLS handshake using the supplied `ClientConfig` and
/// `ServerName`. After the handshake succeeds, the connection is driven identically to
/// `start_tcp_client`.
///
/// 启动 TLS 加密的 RTSP 客户端连接（rtsps://）。
///
/// 先进行 TCP 连接，再使用给定的 `ClientConfig` 与 `ServerName` 完成 TLS 握手。握手
/// 成功后，连接以与 `start_tcp_client` 相同的方式运行。
pub fn start_tls_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    server_name: tokio_rustls::rustls::pki_types::ServerName<'static>,
    tls_config: Arc<tokio_rustls::rustls::ClientConfig>,
    config: RtspClientConfig,
    cancel: CancellationToken,
) -> io::Result<RtspClientHandle> {
    let connector = tokio_rustls::TlsConnector::from(tls_config);

    let (cmd_tx, cmd_rx) = mpsc::channel(config.command_queue_capacity.max(8));
    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(8));
    let child_cancel = cancel.child_token();
    let event_tx_for_task = event_tx.clone();
    let task_config = config.clone();

    let join = runtime_api.spawn(Box::pin(async move {
        let tcp_stream = match tokio::net::TcpStream::connect(peer).await {
            Ok(stream) => stream,
            Err(err) => {
                let _ = event_tx_for_task
                    .send(RtspClientEvent::Closed {
                        reason: format!("TCP connect failed: {err}"),
                    })
                    .await;
                return;
            }
        };
        let tls_stream = match connector.connect(server_name, tcp_stream).await {
            Ok(stream) => stream,
            Err(err) => {
                let _ = event_tx_for_task
                    .send(RtspClientEvent::Closed {
                        reason: format!("TLS handshake failed: {err}"),
                    })
                    .await;
                return;
            }
        };
        let wrapped: Box<dyn cheetah_runtime_api::AsyncTcpStream> =
            Box::new(TlsClientStreamWrapper {
                inner: tls_stream,
                peer,
            });
        let _ = event_tx_for_task
            .send(RtspClientEvent::Connected { peer })
            .await;
        connection::run_tcp_client_connection(
            wrapped,
            cmd_rx,
            event_tx_for_task,
            child_cancel,
            task_config,
        )
        .await;
    }));

    Ok(RtspClientHandle {
        events_rx: event_rx,
        event_tx,
        cmd_tx: RtspClientCommandSender::new(cmd_tx),
        cancel,
        join,
    })
}

/// Wrapper for a client-side TLS stream implementing `AsyncTcpStream`.
///
/// 客户端 TLS 流的包装器，实现 `AsyncTcpStream`。
struct TlsClientStreamWrapper {
    inner: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    peer: SocketAddr,
}

#[async_trait::async_trait]
impl cheetah_runtime_api::AsyncTcpStream for TlsClientStreamWrapper {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.inner.read(buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

/// Start an RTSP client over a pair of HTTP tunnels.
///
/// Requires two TCP connections to the same peer: a GET connection for downstream data and
/// a POST connection for upstream data. Both are opened with the same `x-sessioncookie`.
///
/// 通过一对 HTTP 隧道启动 RTSP 客户端。
///
/// 需要到同一对端的两个 TCP 连接：GET 连接用于下行数据，POST 连接用于上行数据。
/// 两者使用相同的 `x-sessioncookie` 打开。
pub fn start_http_tunnel_client(
    runtime_api: Arc<dyn RuntimeApi>,
    peer: SocketAddr,
    path: String,
    session_cookie: String,
    config: RtspClientConfig,
    cancel: CancellationToken,
) -> io::Result<RtspClientHandle> {
    let normalized_path = http_tunnel::normalize_http_tunnel_path(&path);
    if session_cookie.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "http tunnel session cookie must not be empty",
        ));
    }
    let get_stream = runtime_api.connect_tcp(peer)?;
    let post_stream = runtime_api.connect_tcp(peer)?;

    let (cmd_tx, cmd_rx) = mpsc::channel(config.command_queue_capacity.max(8));
    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(8));
    let child_cancel = cancel.child_token();
    let task_config = config.clone();
    let event_tx_for_task = event_tx.clone();

    let join = runtime_api.spawn(Box::pin(async move {
        let ctx = connection::HttpTunnelClientContext {
            peer,
            path: normalized_path,
            session_cookie,
            event_tx: event_tx_for_task,
            cancel: child_cancel,
            config: task_config,
        };
        connection::run_http_tunnel_client_connection(get_stream, post_stream, cmd_rx, ctx).await;
    }));

    Ok(RtspClientHandle {
        events_rx: event_rx,
        event_tx,
        cmd_tx: RtspClientCommandSender::new(cmd_tx),
        cancel,
        join,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use cheetah_rtsp_core::{
        encode_interleaved_frame, encode_rtsp_response, RtspHeader, RtspRequestDecoder,
        RtspRequestMessage, RtspResponseMessage,
    };
    use cheetah_runtime_tokio::TokioRuntime;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{timeout, Duration};

    fn sample_options_request(uri: &str) -> RtspRequestMessage {
        RtspRequestMessage {
            method: "OPTIONS".to_string(),
            uri: uri.to_string(),
            version: "RTSP/1.0".to_string(),
            headers: vec![RtspHeader {
                name: "CSeq".to_string(),
                value: "1".to_string(),
            }],
            body: Bytes::new(),
        }
    }

    async fn expect_connected(handle: &mut RtspClientHandle) {
        let event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("connected event timeout")
            .expect("event");
        assert!(matches!(event, RtspClientEvent::Connected { .. }));
    }

    async fn read_http_headers(stream: &mut tokio::net::TcpStream) -> String {
        let mut out = Vec::<u8>::new();
        loop {
            let mut one = [0_u8; 1];
            timeout(Duration::from_secs(1), stream.read_exact(&mut one))
                .await
                .expect("read header timeout")
                .expect("read header");
            out.push(one[0]);
            if out.len() >= 4 && out[out.len() - 4..] == *b"\r\n\r\n" {
                return String::from_utf8(out).expect("header utf8");
            }
        }
    }

    fn sample_request(
        method: &str,
        uri: &str,
        cseq: u32,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> RtspRequestMessage {
        let mut all_headers = Vec::<RtspHeader>::with_capacity(headers.len() + 1);
        all_headers.push(RtspHeader {
            name: "CSeq".to_string(),
            value: cseq.to_string(),
        });
        all_headers.extend(headers.iter().map(|(name, value)| RtspHeader {
            name: (*name).to_string(),
            value: (*value).to_string(),
        }));
        RtspRequestMessage {
            method: method.to_string(),
            uri: uri.to_string(),
            version: "RTSP/1.0".to_string(),
            headers: all_headers,
            body: Bytes::copy_from_slice(body),
        }
    }

    async fn expect_response_event(handle: &mut RtspClientHandle, expected_cseq: &str, code: u16) {
        let response_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("response event timeout")
            .expect("event");
        match response_event {
            RtspClientEvent::Response { response } => {
                assert_eq!(response.status_code, code);
                assert_eq!(response.header_value("CSeq"), Some(expected_cseq));
            }
            other => panic!("expected response event, got {other:?}"),
        }
    }

    async fn read_next_rtsp_request(
        socket: &mut TcpStream,
        decoder: &mut RtspRequestDecoder,
    ) -> RtspRequestMessage {
        let mut read_buf = vec![0_u8; 4096];
        loop {
            if let Some(request) = decoder.decode().expect("decode request") {
                return request;
            }
            let n = timeout(Duration::from_secs(1), socket.read(&mut read_buf))
                .await
                .expect("request read timeout")
                .expect("request read");
            assert!(n > 0, "peer closed before request completed");
            decoder
                .feed(&read_buf[..n])
                .expect("feed request bytes into decoder");
        }
    }

    async fn write_ok_response(
        socket: &mut TcpStream,
        cseq: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) {
        let mut response_headers = Vec::<RtspHeader>::with_capacity(headers.len() + 1);
        response_headers.push(RtspHeader {
            name: "CSeq".to_string(),
            value: cseq.to_string(),
        });
        response_headers.extend(headers.iter().map(|(name, value)| RtspHeader {
            name: (*name).to_string(),
            value: (*value).to_string(),
        }));
        let response = encode_rtsp_response(&RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 200,
            reason_phrase: "OK".to_string(),
            headers: response_headers,
            body: Bytes::copy_from_slice(body),
        })
        .expect("encode rtsp response");
        socket
            .write_all(response.as_ref())
            .await
            .expect("write rtsp response");
    }

    struct TestBase64StreamDecoder {
        buffer: Vec<u8>,
    }

    impl TestBase64StreamDecoder {
        fn new() -> Self {
            Self { buffer: Vec::new() }
        }

        fn push(&mut self, input: &[u8]) -> Result<Vec<Bytes>, String> {
            for byte in input {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                self.buffer.push(*byte);
            }
            let complete = (self.buffer.len() / 4) * 4;
            if complete == 0 {
                return Ok(Vec::new());
            }
            let chunk = self.buffer[..complete].to_vec();
            self.buffer.drain(..complete);
            let decoded = STANDARD
                .decode(chunk)
                .map_err(|_| "decode tunnel base64 failed".to_string())?;
            if decoded.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![Bytes::from(decoded)])
            }
        }
    }

    async fn read_next_tunnel_request(
        post_socket: &mut TcpStream,
        base64_decoder: &mut TestBase64StreamDecoder,
        rtsp_decoder: &mut RtspRequestDecoder,
    ) -> RtspRequestMessage {
        let mut read_buf = vec![0_u8; 4096];
        loop {
            if let Some(request) = rtsp_decoder.decode().expect("decode tunnel request") {
                return request;
            }
            let n = timeout(Duration::from_secs(1), post_socket.read(&mut read_buf))
                .await
                .expect("tunnel post read timeout")
                .expect("tunnel post read");
            assert!(n > 0, "tunnel post closed before request completed");
            let decoded_chunks = base64_decoder
                .push(&read_buf[..n])
                .expect("decode tunnel base64");
            for chunk in decoded_chunks {
                rtsp_decoder
                    .feed(chunk.as_ref())
                    .expect("feed tunnel request bytes");
            }
        }
    }

    async fn send_close_or_allow_channel_closed(handle: &mut RtspClientHandle) {
        if let Err(err) = handle.send_command(RtspClientCommand::Close).await {
            assert_eq!(
                err,
                RtspClientSendError::ChannelClosed,
                "unexpected close send error: {err:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_client_sends_request_and_receives_response_and_interleaved_frame() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");

        let server = tokio::spawn(async move {
            let (mut socket, _peer) = listener.accept().await.expect("accept");
            let mut read_buf = vec![0u8; 2048];
            let n = socket.read(&mut read_buf).await.expect("read request");
            let request_text = std::str::from_utf8(&read_buf[..n]).expect("request utf8");
            assert!(request_text.contains("OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0"));

            let response = encode_rtsp_response(&RtspResponseMessage {
                version: "RTSP/1.0".to_string(),
                status_code: 200,
                reason_phrase: "OK".to_string(),
                headers: vec![RtspHeader {
                    name: "CSeq".to_string(),
                    value: "1".to_string(),
                }],
                body: Bytes::new(),
            })
            .expect("encode response");
            socket
                .write_all(response.as_ref())
                .await
                .expect("write response");

            let interleaved = encode_interleaved_frame(0, b"RTP!").expect("encode interleaved");
            socket
                .write_all(interleaved.as_ref())
                .await
                .expect("write interleaved");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_tcp_client(runtime_api, listen, RtspClientConfig::default(), cancel)
            .expect("start client");
        expect_connected(&mut handle).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_options_request(
                "rtsp://127.0.0.1/live/test",
            )))
            .await
            .expect("send request");

        let response_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("response event timeout")
            .expect("event");
        match response_event {
            RtspClientEvent::Response { response } => {
                assert_eq!(response.status_code, 200);
                assert_eq!(response.header_value("CSeq"), Some("1"));
            }
            other => panic!("expected response event, got {other:?}"),
        }

        let interleaved_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("interleaved event timeout")
            .expect("event");
        match interleaved_event {
            RtspClientEvent::InterleavedFrame { channel, payload } => {
                assert_eq!(channel, 0);
                assert_eq!(payload.as_ref(), b"RTP!");
            }
            other => panic!("expected interleaved event, got {other:?}"),
        }

        send_close_or_allow_channel_closed(&mut handle).await;
        let _ = server.await;
        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_client_reports_closed_when_peer_disconnects() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (_socket, _peer) = listener.accept().await.expect("accept");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_tcp_client(runtime_api, listen, RtspClientConfig::default(), cancel)
            .expect("start client");
        expect_connected(&mut handle).await;

        let closed_event = timeout(Duration::from_secs(2), async {
            loop {
                match handle.recv_event().await {
                    Some(RtspClientEvent::Closed { reason }) => break reason,
                    Some(_) => {}
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("closed event timeout");
        assert!(
            closed_event.contains("peer closed")
                || closed_event.contains("read failed")
                || closed_event.contains("cancelled")
        );
        let _ = server.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_tunnel_client_sends_base64_post_and_receives_response_and_interleaved() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut get_socket, _) = listener.accept().await.expect("accept get");
            let get_headers = read_http_headers(&mut get_socket).await;
            assert!(get_headers.starts_with("GET /live/http-tunnel-client HTTP/1.0"));
            assert!(get_headers.contains("x-sessioncookie: cookie-01"));
            get_socket
                .write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: application/x-rtsp-tunnelled\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write get response");

            let (mut post_socket, _) = listener.accept().await.expect("accept post");
            let post_headers = read_http_headers(&mut post_socket).await;
            assert!(post_headers.starts_with("POST /live/http-tunnel-client HTTP/1.0"));
            assert!(post_headers.contains("x-sessioncookie: cookie-01"));
            assert!(post_headers.contains("Content-Type: application/x-rtsp-tunnelled"));
            post_socket
                .write_all(b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n")
                .await
                .expect("write post response");

            let mut post_payload = vec![0_u8; 2048];
            let post_n = timeout(Duration::from_secs(1), post_socket.read(&mut post_payload))
                .await
                .expect("read post payload timeout")
                .expect("read post payload");
            assert!(post_n > 0, "post payload should not be empty");
            let decoded = STANDARD
                .decode(&post_payload[..post_n])
                .expect("decode base64 post payload");
            let decoded_text = std::str::from_utf8(&decoded).expect("decoded utf8");
            assert!(decoded_text.contains("OPTIONS rtsp://127.0.0.1/live/test RTSP/1.0"));

            let response = encode_rtsp_response(&RtspResponseMessage {
                version: "RTSP/1.0".to_string(),
                status_code: 200,
                reason_phrase: "OK".to_string(),
                headers: vec![RtspHeader {
                    name: "CSeq".to_string(),
                    value: "1".to_string(),
                }],
                body: Bytes::new(),
            })
            .expect("encode response");
            get_socket
                .write_all(response.as_ref())
                .await
                .expect("write rtsp response");
            let interleaved = encode_interleaved_frame(0, b"HTTP-RTP!").expect("encode frame");
            get_socket
                .write_all(interleaved.as_ref())
                .await
                .expect("write interleaved");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_http_tunnel_client(
            runtime_api,
            listen,
            "live/http-tunnel-client".to_string(),
            "cookie-01".to_string(),
            RtspClientConfig::default(),
            cancel,
        )
        .expect("start http tunnel client");
        expect_connected(&mut handle).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_options_request(
                "rtsp://127.0.0.1/live/test",
            )))
            .await
            .expect("send request");

        let response_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("response event timeout")
            .expect("event");
        match response_event {
            RtspClientEvent::Response { response } => {
                assert_eq!(response.status_code, 200);
                assert_eq!(response.header_value("CSeq"), Some("1"));
            }
            other => panic!("expected response event, got {other:?}"),
        }

        let interleaved_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("interleaved event timeout")
            .expect("event");
        match interleaved_event {
            RtspClientEvent::InterleavedFrame { channel, payload } => {
                assert_eq!(channel, 0);
                assert_eq!(payload.as_ref(), b"HTTP-RTP!");
            }
            other => panic!("expected interleaved event, got {other:?}"),
        }

        send_close_or_allow_channel_closed(&mut handle).await;
        let _ = server.await;
        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_client_state_machine_options_describe_setup_play() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut decoder = RtspRequestDecoder::new();

            let options = read_next_rtsp_request(&mut socket, &mut decoder).await;
            assert_eq!(options.method, "OPTIONS");
            assert_eq!(options.uri, "rtsp://127.0.0.1/live/state-machine");
            write_ok_response(
                &mut socket,
                options.header_value("CSeq").expect("cseq"),
                &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
                &[],
            )
            .await;

            let describe = read_next_rtsp_request(&mut socket, &mut decoder).await;
            assert_eq!(describe.method, "DESCRIBE");
            assert_eq!(
                describe.header_value("Accept"),
                Some("application/sdp"),
                "describe should carry accept header"
            );
            write_ok_response(
                &mut socket,
                describe.header_value("CSeq").expect("cseq"),
                &[("Content-Type", "application/sdp")],
                b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=state-machine\r\nt=0 0\r\n",
            )
            .await;

            let setup = read_next_rtsp_request(&mut socket, &mut decoder).await;
            assert_eq!(setup.method, "SETUP");
            assert!(
                setup
                    .header_value("Transport")
                    .is_some_and(|v| v.contains("interleaved=0-1")),
                "setup transport must request interleaved channel pair"
            );
            write_ok_response(
                &mut socket,
                setup.header_value("CSeq").expect("cseq"),
                &[
                    ("Session", "sess-tcp-01"),
                    (
                        "Transport",
                        "RTP/AVP/TCP;unicast;interleaved=0-1;ssrc=01020304",
                    ),
                ],
                &[],
            )
            .await;

            let play = read_next_rtsp_request(&mut socket, &mut decoder).await;
            assert_eq!(play.method, "PLAY");
            assert_eq!(play.header_value("Session"), Some("sess-tcp-01"));
            write_ok_response(
                &mut socket,
                play.header_value("CSeq").expect("cseq"),
                &[
                    ("Session", "sess-tcp-01"),
                    (
                        "RTP-Info",
                        "url=rtsp://127.0.0.1/live/state-machine/trackID=0;seq=1100;rtptime=99000",
                    ),
                ],
                &[],
            )
            .await;

            let interleaved = encode_interleaved_frame(0, b"TCP-PLAY-RTP").expect("encode frame");
            socket
                .write_all(interleaved.as_ref())
                .await
                .expect("write interleaved");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_tcp_client(runtime_api, listen, RtspClientConfig::default(), cancel)
            .expect("start client");
        expect_connected(&mut handle).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "OPTIONS",
                "rtsp://127.0.0.1/live/state-machine",
                1,
                &[],
                &[],
            )))
            .await
            .expect("send options");
        expect_response_event(&mut handle, "1", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "DESCRIBE",
                "rtsp://127.0.0.1/live/state-machine",
                2,
                &[("Accept", "application/sdp")],
                &[],
            )))
            .await
            .expect("send describe");
        expect_response_event(&mut handle, "2", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "SETUP",
                "rtsp://127.0.0.1/live/state-machine/trackID=0",
                3,
                &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
                &[],
            )))
            .await
            .expect("send setup");
        expect_response_event(&mut handle, "3", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "PLAY",
                "rtsp://127.0.0.1/live/state-machine",
                4,
                &[("Session", "sess-tcp-01")],
                &[],
            )))
            .await
            .expect("send play");
        expect_response_event(&mut handle, "4", 200).await;

        let play_packet_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("interleaved timeout")
            .expect("event");
        match play_packet_event {
            RtspClientEvent::InterleavedFrame { channel, payload } => {
                assert_eq!(channel, 0);
                assert_eq!(payload.as_ref(), b"TCP-PLAY-RTP");
            }
            other => panic!("expected interleaved event, got {other:?}"),
        }

        send_close_or_allow_channel_closed(&mut handle).await;
        let _ = server.await;
        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_tunnel_client_streams_base64_without_midstream_padding() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut get_socket, _) = listener.accept().await.expect("accept get");
            let _ = read_http_headers(&mut get_socket).await;
            get_socket
                .write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: application/x-rtsp-tunnelled\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write get response");

            let (mut post_socket, _) = listener.accept().await.expect("accept post");
            let _ = read_http_headers(&mut post_socket).await;
            post_socket
                .write_all(b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n")
                .await
                .expect("write post response");

            let mut encoded = Vec::new();
            let mut buf = [0_u8; 128];
            while encoded.len() < 12 {
                let n = timeout(Duration::from_secs(1), post_socket.read(&mut buf))
                    .await
                    .expect("read post payload timeout")
                    .expect("read post payload");
                assert!(n > 0, "post payload should not be empty");
                encoded.extend_from_slice(&buf[..n]);
            }
            assert!(
                !encoded[..encoded.len().saturating_sub(4)].contains(&b'='),
                "base64 padding must only appear at the end of the HTTP tunnel stream"
            );
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_http_tunnel_client(
            runtime_api,
            listen,
            "live/http-tunnel-padding".to_string(),
            "cookie-padding".to_string(),
            RtspClientConfig::default(),
            cancel,
        )
        .expect("start http tunnel client");
        expect_connected(&mut handle).await;

        handle
            .send_command(RtspClientCommand::SendInterleaved {
                channel: 0,
                payload: Bytes::from_static(b"A"),
            })
            .await
            .expect("send first interleaved");
        handle
            .send_command(RtspClientCommand::SendInterleaved {
                channel: 0,
                payload: Bytes::from_static(b"B"),
            })
            .await
            .expect("send second interleaved");

        let _ = server.await;
        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_tunnel_client_state_machine_options_describe_setup_play() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut get_socket, _) = listener.accept().await.expect("accept get");
            let get_headers = read_http_headers(&mut get_socket).await;
            assert!(get_headers.starts_with("GET /live/http-state-machine HTTP/1.0"));
            assert!(get_headers.contains("x-sessioncookie: cookie-state"));
            get_socket
                .write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: application/x-rtsp-tunnelled\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write get response");

            let (mut post_socket, _) = listener.accept().await.expect("accept post");
            let post_headers = read_http_headers(&mut post_socket).await;
            assert!(post_headers.starts_with("POST /live/http-state-machine HTTP/1.0"));
            assert!(post_headers.contains("x-sessioncookie: cookie-state"));
            post_socket
                .write_all(b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n")
                .await
                .expect("write post response");

            let mut base64_decoder = TestBase64StreamDecoder::new();
            let mut request_decoder = RtspRequestDecoder::new();

            let options = read_next_tunnel_request(
                &mut post_socket,
                &mut base64_decoder,
                &mut request_decoder,
            )
            .await;
            assert_eq!(options.method, "OPTIONS");
            write_ok_response(
                &mut get_socket,
                options.header_value("CSeq").expect("cseq"),
                &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
                &[],
            )
            .await;

            let describe = read_next_tunnel_request(
                &mut post_socket,
                &mut base64_decoder,
                &mut request_decoder,
            )
            .await;
            assert_eq!(describe.method, "DESCRIBE");
            assert_eq!(describe.header_value("Accept"), Some("application/sdp"));
            write_ok_response(
                &mut get_socket,
                describe.header_value("CSeq").expect("cseq"),
                &[("Content-Type", "application/sdp")],
                b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=http-state-machine\r\nt=0 0\r\n",
            )
            .await;

            let setup = read_next_tunnel_request(
                &mut post_socket,
                &mut base64_decoder,
                &mut request_decoder,
            )
            .await;
            assert_eq!(setup.method, "SETUP");
            assert!(
                setup
                    .header_value("Transport")
                    .is_some_and(|v| v.contains("interleaved=6-7")),
                "setup transport must request expected interleaved channels"
            );
            write_ok_response(
                &mut get_socket,
                setup.header_value("CSeq").expect("cseq"),
                &[
                    ("Session", "sess-http-01"),
                    (
                        "Transport",
                        "RTP/AVP/TCP;unicast;interleaved=6-7;ssrc=10203040",
                    ),
                ],
                &[],
            )
            .await;

            let play = read_next_tunnel_request(
                &mut post_socket,
                &mut base64_decoder,
                &mut request_decoder,
            )
            .await;
            assert_eq!(play.method, "PLAY");
            assert_eq!(play.header_value("Session"), Some("sess-http-01"));
            write_ok_response(
                &mut get_socket,
                play.header_value("CSeq").expect("cseq"),
                &[("Session", "sess-http-01"), ("RTP-Info", "url=rtsp://127.0.0.1/live/http-state-machine/trackID=0;seq=1200;rtptime=100000")],
                &[],
            )
            .await;

            let interleaved = encode_interleaved_frame(6, b"HTTP-PLAY-RTP").expect("encode frame");
            get_socket
                .write_all(interleaved.as_ref())
                .await
                .expect("write interleaved");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_http_tunnel_client(
            runtime_api,
            listen,
            "live/http-state-machine".to_string(),
            "cookie-state".to_string(),
            RtspClientConfig::default(),
            cancel,
        )
        .expect("start http tunnel client");
        expect_connected(&mut handle).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "OPTIONS",
                "rtsp://127.0.0.1/live/http-state-machine",
                11,
                &[],
                &[],
            )))
            .await
            .expect("send options");
        expect_response_event(&mut handle, "11", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "DESCRIBE",
                "rtsp://127.0.0.1/live/http-state-machine",
                12,
                &[("Accept", "application/sdp")],
                &[],
            )))
            .await
            .expect("send describe");
        expect_response_event(&mut handle, "12", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "SETUP",
                "rtsp://127.0.0.1/live/http-state-machine/trackID=0",
                13,
                &[("Transport", "RTP/AVP/TCP;unicast;interleaved=6-7")],
                &[],
            )))
            .await
            .expect("send setup");
        expect_response_event(&mut handle, "13", 200).await;

        handle
            .send_command(RtspClientCommand::SendRequest(sample_request(
                "PLAY",
                "rtsp://127.0.0.1/live/http-state-machine",
                14,
                &[("Session", "sess-http-01")],
                &[],
            )))
            .await
            .expect("send play");
        expect_response_event(&mut handle, "14", 200).await;

        let play_packet_event = timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .expect("interleaved timeout")
            .expect("event");
        match play_packet_event {
            RtspClientEvent::InterleavedFrame { channel, payload } => {
                assert_eq!(channel, 6);
                assert_eq!(payload.as_ref(), b"HTTP-PLAY-RTP");
            }
            other => panic!("expected interleaved event, got {other:?}"),
        }

        send_close_or_allow_channel_closed(&mut handle).await;
        let _ = server.await;
        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_tunnel_client_reports_closed_when_get_open_fails() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let listen = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let (mut get_socket, _) = listener.accept().await.expect("accept get");
            let _ = read_http_headers(&mut get_socket).await;
            get_socket
                .write_all(b"HTTP/1.0 403 Forbidden\r\nConnection: close\r\n\r\n")
                .await
                .expect("write forbidden");
        });

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let cancel = CancellationToken::new();
        let mut handle = start_http_tunnel_client(
            runtime_api,
            listen,
            "/live/http-tunnel-fail".to_string(),
            "cookie-02".to_string(),
            RtspClientConfig::default(),
            cancel,
        )
        .expect("start http tunnel client");

        let closed_reason = timeout(Duration::from_secs(2), async {
            loop {
                match handle.recv_event().await {
                    Some(RtspClientEvent::Closed { reason }) => break reason,
                    Some(_) => {}
                    None => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("closed event timeout");
        assert!(closed_reason.contains("GET tunnel open failed"));
        assert!(closed_reason.contains("403"));
        let _ = server.await;
    }
}
