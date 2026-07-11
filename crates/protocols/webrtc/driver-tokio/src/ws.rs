//! Runtime-neutral WebSocket abstractions with a `tokio-tungstenite`
//! implementation.
//!
//! The WebRTC signaling paths (OME WHIP/WHEP WebSocket server, P2P
//! signaling client/server) need WebSocket framing and TLS, both of
//! which are inherently `tokio`-bound. Per `AGENTS.md` §5/§6 that I/O
//! must live in the driver layer, not in the module. This module
//! exposes:
//!
//! * [`WsFrame`] / [`WsError`] — neutral message + error types.
//! * [`WsConnection`] — a single upgraded WebSocket connection
//!   (`send_text` / `recv` / `close`). Ping/Pong are handled internally
//!   (inbound pings are auto-ponged); callers only observe application
//!   frames.
//! * [`WsConnector`] — a client-side connector ([`TokioWsConnector`]).
//! * [`bind_ws_server`] + [`WsServerListener::serve`] — a server-side
//!   accept loop that hands each upgraded connection to a handler.
//!
//! The module layer wraps these neutral handles and keeps all
//! signaling message encode/decode (OME / P2P JSON) on its side.
//!
//! 具有 `tokio-tungstenite` 实现的运行时中立的 WebSocket 抽象。
//!
//! WebRTC 信令路径（OME WHIP/WHEP WebSocket 服务器、P2P 信令客户端/服务器）需要 WebSocket 帧和 TLS，两者本质上都是 `tokio` 绑定的。
//! 根据 `AGENTS.md` §5/§6，I/O 必须位于 driver 层，而不是模块中。
//! 该模块公开：
//!
//! * [`WsFrame`] / [`WsError`] — 中性消息 + 错误类型。
//! * [`WsConnection`] — 单个升级的 WebSocket 连接 (`send_text` / `recv` / `close`)。
//!   Ping/Pong 在内部处理（入站 ping 会自动进行）；
//!   调用者仅观察应用程序框架。
//! * [`WsConnector`] — 客户端连接器 ([`TokioWsConnector`])。
//! * [`bind_ws_server`] + [`WsServerListener::serve`] — 服务器端接受循环，将每个升级的连接交给处理程序。
//!
//! 模块层包装这些中性句柄，并将所有信令消息编码/解码（OME / P2P JSON）保留在其一侧。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use cheetah_runtime_api::CancellationToken;

/// Application-level WebSocket frame surfaced to callers. Control
/// frames (ping/pong) are handled inside the connection and never
/// reach the caller.
///
/// 应用程序级 WebSocket 框架向调用者显现。
/// 控制帧（ping/pong）在连接内部处理，并且永远不会到达调用者。
#[derive(Debug, Clone)]
pub enum WsFrame {
    /// A UTF-8 text frame.
    ///
    /// UTF-8 文本框架。
    Text(String),
    /// A binary frame.
    ///
    /// 一个二进制框架。
    Binary(Vec<u8>),
    /// The peer closed the connection (or the stream ended).
    ///
    /// 对等方关闭连接（或流结束）。
    Closed,
}

/// Errors raised by the WebSocket abstractions.
///
/// WebSocket 抽象引发的错误。
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// The connection is already closed.
    ///
    /// 连接已经关闭。
    #[error("websocket closed")]
    Closed,
    /// A protocol / transport error surfaced by the underlying stack.
    ///
    /// 底层堆栈出现协议/传输错误。
    #[error("websocket error: {0}")]
    Protocol(String),
    /// The client connect handshake exceeded the configured timeout.
    ///
    /// 客户端连接握手超出了配置的超时时间。
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(Duration),
    /// The client request URL could not be turned into a handshake.
    ///
    /// 客户端请求 URL 无法转换为握手。
    #[error("invalid websocket request: {0}")]
    InvalidRequest(String),
}

/// A single upgraded WebSocket connection.
///
/// 单个升级的 WebSocket 连接。
#[async_trait]
pub trait WsConnection: Send + Sync {
    /// Send a text frame.
    ///
    /// 发送文本框架。
    async fn send_text(&self, text: String) -> Result<(), WsError>;
    /// Await the next application frame. Returns [`WsFrame::Closed`]
    /// when the peer closes or the stream ends. Inbound pings are
    /// auto-ponged and skipped.
    ///
    /// 等待下一个应用程序框架。
    /// 当对等点关闭或流结束时返回 [`WsFrame::Closed`]。
    /// 入站 ping 会自动进行并跳过。
    async fn recv(&self) -> Result<WsFrame, WsError>;
    /// Send a close frame and mark the connection closed.
    ///
    /// 发送关闭帧并将连接标记为关闭。
    async fn close(&self);
}

/// Client-side connector that establishes outbound WebSocket
/// connections (with TLS for `wss://`).
///
/// 建立出站 WebSocket 连接的客户端连接器（使用 TLS 表示 `wss://`）。
#[async_trait]
pub trait WsConnector: Send + Sync {
    /// Connect to `url`, bounded by `connect_timeout`.
    ///
    /// 连接到 `url`，以 `connect_timeout` 为界。
    async fn connect(
        &self,
        url: &str,
        connect_timeout: Duration,
    ) -> Result<Box<dyn WsConnection>, WsError>;
}

type BoxedSink =
    Box<dyn futures::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Send + Unpin>;

type BoxedStream = Box<
    dyn futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Send
        + Unpin,
>;

/// `tokio-tungstenite`-backed [`WsConnection`]. Type-erases the sink and
/// stream halves so the same struct wraps both client
/// (`MaybeTlsStream<TcpStream>`) and server (`TcpStream`) connections.
///
/// `tokio-tungstenite` 支持 [`WsConnection`]。
/// 对接收器和流的一半进行类型擦除，以便同一结构包装客户端 (`MaybeTlsStream<TcpStream>`) 和服务器 (`TcpStream`) 连接。
pub struct TokioWsConnection {
    sink: tokio::sync::Mutex<BoxedSink>,
    stream: tokio::sync::Mutex<BoxedStream>,
    closed: AtomicBool,
}

impl TokioWsConnection {
    fn from_split(sink: BoxedSink, stream: BoxedStream) -> Self {
        Self {
            sink: tokio::sync::Mutex::new(sink),
            stream: tokio::sync::Mutex::new(stream),
            closed: AtomicBool::new(false),
        }
    }

    /// Wrap a client-side WebSocket stream produced by
    /// `tokio_tungstenite::connect_async`.
    ///
    /// 包装由 `tokio_tungstenite::connect_async` 生成的客户端 WebSocket 流。
    pub fn from_client_stream(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        let (sink, stream) = stream.split();
        Self::from_split(Box::new(sink), Box::new(stream))
    }

    /// Wrap a server-side WebSocket stream produced by
    /// `tokio_tungstenite::accept_async` / `accept_hdr_async`.
    ///
    /// 包装由 `tokio_tungstenite::accept_async` / `accept_hdr_async` 生成的服务器端 WebSocket 流。
    pub fn from_server_stream(stream: WebSocketStream<TcpStream>) -> Self {
        let (sink, stream) = stream.split();
        Self::from_split(Box::new(sink), Box::new(stream))
    }
}

#[async_trait]
impl WsConnection for TokioWsConnection {
    async fn send_text(&self, text: String) -> Result<(), WsError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(WsError::Closed);
        }
        let mut sink = self.sink.lock().await;
        sink.send(WsMessage::Text(text.into()))
            .await
            .map_err(|err| WsError::Protocol(err.to_string()))
    }

    async fn recv(&self) -> Result<WsFrame, WsError> {
        loop {
            let next = {
                let mut stream = self.stream.lock().await;
                stream.next().await
            };
            match next {
                None => {
                    self.closed.store(true, Ordering::Release);
                    return Ok(WsFrame::Closed);
                }
                Some(Err(err)) => {
                    self.closed.store(true, Ordering::Release);
                    return Err(WsError::Protocol(err.to_string()));
                }
                Some(Ok(WsMessage::Text(text))) => return Ok(WsFrame::Text(text.to_string())),
                Some(Ok(WsMessage::Binary(bytes))) => return Ok(WsFrame::Binary(bytes.to_vec())),
                Some(Ok(WsMessage::Ping(payload))) => {
                    let mut sink = self.sink.lock().await;
                    let _ = sink.send(WsMessage::Pong(payload)).await;
                    continue;
                }
                Some(Ok(WsMessage::Pong(_))) => continue,
                Some(Ok(WsMessage::Close(_))) => {
                    self.closed.store(true, Ordering::Release);
                    return Ok(WsFrame::Closed);
                }
                Some(Ok(WsMessage::Frame(_))) => continue,
            }
        }
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let mut sink = self.sink.lock().await;
        let _ = sink.send(WsMessage::Close(None)).await;
        let _ = sink.close().await;
    }
}

/// Client-side [`WsConnector`] built on `tokio_tungstenite::connect_async`.
///
/// 客户端 [`WsConnector`] 构建于 `tokio_tungstenite::connect_async` 之上。
#[derive(Debug, Clone, Default)]
pub struct TokioWsConnector;

#[async_trait]
impl WsConnector for TokioWsConnector {
    async fn connect(
        &self,
        url: &str,
        connect_timeout: Duration,
    ) -> Result<Box<dyn WsConnection>, WsError> {
        let request = url
            .into_client_request()
            .map_err(|err| WsError::InvalidRequest(err.to_string()))?;
        let (stream, _resp) =
            match tokio::time::timeout(connect_timeout, connect_async(request)).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) => return Err(WsError::Protocol(err.to_string())),
                Err(_) => return Err(WsError::ConnectTimeout(connect_timeout)),
            };
        Ok(Box::new(TokioWsConnection::from_client_stream(stream)))
    }
}

/// Configuration for the WebSocket accept loop.
///
/// WebSocket 接受循环的配置。
#[derive(Debug, Clone)]
pub struct WsServerConfig {
    /// Maximum concurrent inbound connections; reaching the cap drops
    /// the next accept until an existing connection closes.
    ///
    /// 最大并发入站连接数；
    /// 达到上限会丢弃下一个接受，直到现有连接关闭。
    pub max_connections: usize,
    /// Per-connection handshake timeout.
    ///
    /// 每个连接的握手超时。
    pub accept_timeout: Duration,
}

impl Default for WsServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            accept_timeout: Duration::from_secs(5),
        }
    }
}

/// Metadata surfaced to the connection handler alongside the upgraded
/// connection.
///
/// 元数据与升级的连接一起出现在连接处理程序中。
#[derive(Debug, Clone)]
pub struct WsInbound {
    /// Remote peer address.
    ///
    /// 远程对等地址。
    pub remote_addr: SocketAddr,
    /// Request path + query captured during the handshake.
    ///
    /// 握手期间捕获的请求路径+查询。
    pub path_and_query: String,
}

/// Per-connection handler invoked for each upgraded WebSocket.
///
/// 为每个升级的 WebSocket 调用的每个连接处理程序。
pub type WsConnectionHandler =
    Arc<dyn Fn(WsInbound, Box<dyn WsConnection>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Errors raised while binding or running the WebSocket server.
///
/// 绑定或运行 WebSocket 服务器时引发错误。
#[derive(Debug, thiserror::Error)]
pub enum WsServerError {
    /// Binding the TCP listener failed.
    ///
    /// 绑定 TCP 侦听器失败。
    #[error("bind failed: {0}")]
    Bind(String),
    /// Accepting a TCP connection failed.
    ///
    /// 接受 TCP 连接失败。
    #[error("accept failed: {0}")]
    Accept(String),
}

/// A bound WebSocket server listener, ready to [`serve`](Self::serve).
///
/// 绑定的 WebSocket 服务器监听器，准备好 [`serve`](Self::serve)。
pub struct WsServerListener {
    listener: TcpListener,
}

/// Bind a TCP listener for a WebSocket server, returning the listener
/// and its resolved local address (useful for logging an ephemeral
/// port before the accept loop starts).
///
/// 为 WebSocket 服务器绑定 TCP 侦听器，返回侦听器及其解析的本地地址（对于在接受循环开始之前记录临时端口很有用）。
pub async fn bind_ws_server(addr: &str) -> Result<(WsServerListener, SocketAddr), WsServerError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|err| WsServerError::Bind(err.to_string()))?;
    let local_addr = listener
        .local_addr()
        .map_err(|err| WsServerError::Bind(err.to_string()))?;
    Ok((WsServerListener { listener }, local_addr))
}

impl WsServerListener {
    /// Run the accept loop until the listener errors or `cancel` fires.
    /// Each upgraded connection is handed to `handler` on its own task.
    ///
    /// 运行接受循环，直到侦听器出错或 `cancel` 触发。
    /// 每个升级的连接都会交给 `handler` 执行其自己的任务。
    // The tungstenite handshake callback returns a large `Result` whose
    // shape we don't control.
    #[allow(clippy::result_large_err)]
    pub async fn serve(
        self,
        config: WsServerConfig,
        handler: WsConnectionHandler,
        cancel: CancellationToken,
    ) -> Result<(), WsServerError> {
        let connection_count = Arc::new(AtomicUsize::new(0));
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let (stream, remote_addr) = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                result = self.listener.accept() => {
                    result.map_err(|err| WsServerError::Accept(err.to_string()))?
                }
            };

            if connection_count.load(Ordering::Acquire) >= config.max_connections {
                drop(stream);
                continue;
            }
            connection_count.fetch_add(1, Ordering::Release);

            let handler = handler.clone();
            let connection_count_for_task = connection_count.clone();
            let accept_timeout = config.accept_timeout;
            tokio::spawn(async move {
                let _guard = ConnectionGuard::new(connection_count_for_task);
                let path_and_query = Arc::new(parking_lot::Mutex::new(String::from("/")));
                let path_and_query_for_cb = path_and_query.clone();
                let upgrade = tokio::time::timeout(
                    accept_timeout,
                    tokio_tungstenite::accept_hdr_async(
                        stream,
                        move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                              response| {
                            let target = request
                                .uri()
                                .path_and_query()
                                .map(|value| value.as_str().to_string())
                                .unwrap_or_else(|| "/".to_string());
                            *path_and_query_for_cb.lock() = target;
                            Ok(response)
                        },
                    ),
                )
                .await;
                let ws = match upgrade {
                    Ok(Ok(ws)) => ws,
                    Ok(Err(err)) => {
                        tracing::debug!("WebSocket handshake failed for {remote_addr}: {err}");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!(
                            "WebSocket handshake for {remote_addr} timed out after {accept_timeout:?}"
                        );
                        return;
                    }
                };
                let path_and_query = path_and_query.lock().clone();
                let connection: Box<dyn WsConnection> =
                    Box::new(TokioWsConnection::from_server_stream(ws));
                handler(
                    WsInbound {
                        remote_addr,
                        path_and_query,
                    },
                    connection,
                )
                .await;
            });
        }
    }
}

struct ConnectionGuard {
    counter: Arc<AtomicUsize>,
}

impl ConnectionGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        Self { counter }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Release);
    }
}
