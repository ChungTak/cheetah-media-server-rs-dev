//! `tokio-tungstenite`-backed `P2pTransport` and `KeeperTransportFactory`.
//!
//! Phase 05 follow-up (round 8): completes the production transport
//! that the bridge / supervisor / hub stack has been designed against.
//! The implementation is deliberately thin — `tokio-tungstenite`
//! handles the WebSocket framing and TLS, this module only:
//!
//! * Adapts the resulting stream to the `P2pTransport` async trait.
//! * Encodes outbound `P2pMessage` values via [`super::message::render`]
//!   and decodes inbound text frames via [`super::message::parse`].
//! * Plugs into [`super::supervisor::run_supervisor_with_hub`] through
//!   [`WebSocketTransportFactory`].
//!
//! TLS uses the same `rustls` + `webpki-roots` stack as the
//! WHIP/WHEP HTTP client, so the dep tree stays homogeneous.
//!
//! ## SSRF guard
//!
//! Resolution still goes through [`super::url::SignalingUrlPolicy`]:
//! the URL is parsed and validated *before* `tokio-tungstenite` is
//! invoked. A real DNS-time check is left to the upstream
//! `tokio-tungstenite::connect_async` call; if a host resolves to a
//! private IP the connection completes anyway because the
//! resolver result isn't observable here. Operators that need a
//! strict resolver-side check can plug a custom factory into the
//! supervisor.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::message::{self, P2pDecoderConfig, P2pMessage};
use super::room::P2pRoomKeeperSnapshot;
use super::supervisor::KeeperTransportFactory;
use super::transport::{P2pTransport, P2pTransportError, P2pTransportEvent};
use super::url::{
    parse as parse_signaling_url, SignalingUrl, SignalingUrlError, SignalingUrlPolicy,
};

/// Configuration for [`WebSocketTransportFactory`].
#[derive(Debug, Clone)]
pub struct WebSocketTransportConfig {
    /// SSRF policy applied to the keeper's signaling URL.
    pub url_policy: SignalingUrlPolicy,
    /// Decoder limits applied to inbound frames.
    pub decoder: P2pDecoderConfig,
    /// Connect timeout. Mirrors the WHIP/WHEP client default.
    pub connect_timeout: Duration,
    /// Optional explicit URL override. When set, the factory ignores
    /// the keeper's `(server_host, server_port, ssl)` triple and uses
    /// this URL verbatim. Useful when the URL carries a path or
    /// query string the registry can't represent (e.g.
    /// `wss://host/index/api/webrtc?room=42`).
    pub url_override: Option<String>,
}

impl Default for WebSocketTransportConfig {
    fn default() -> Self {
        Self {
            url_policy: SignalingUrlPolicy::default(),
            decoder: P2pDecoderConfig::default(),
            connect_timeout: Duration::from_secs(10),
            url_override: None,
        }
    }
}

/// Errors specific to the WebSocket transport.
#[derive(Debug, Error)]
pub enum WebSocketTransportError {
    #[error(transparent)]
    Url(#[from] SignalingUrlError),
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(Duration),
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("invalid websocket request: {0}")]
    InvalidRequest(String),
}

impl From<WebSocketTransportError> for P2pTransportError {
    fn from(err: WebSocketTransportError) -> Self {
        match err {
            WebSocketTransportError::Url(err) => P2pTransportError::Io(err.to_string()),
            WebSocketTransportError::ConnectTimeout(d) => {
                P2pTransportError::Io(format!("connect timeout {d:?}"))
            }
            WebSocketTransportError::WebSocket(msg) => P2pTransportError::Io(msg),
            WebSocketTransportError::InvalidRequest(msg) => P2pTransportError::Io(msg),
        }
    }
}

/// Factory that builds [`WebSocketP2pTransport`] instances. Used by
/// `run_supervisor_with_hub`.
#[derive(Debug, Clone)]
pub struct WebSocketTransportFactory {
    config: WebSocketTransportConfig,
}

impl WebSocketTransportFactory {
    pub fn new(config: WebSocketTransportConfig) -> Self {
        Self { config }
    }

    /// Build the signaling URL the supervisor should connect to. Pure
    /// — does not touch the network.
    pub fn signaling_url(
        &self,
        snapshot: &P2pRoomKeeperSnapshot,
    ) -> Result<SignalingUrl, SignalingUrlError> {
        if let Some(raw) = self.config.url_override.as_deref() {
            return parse_signaling_url(raw, &self.config.url_policy);
        }
        let scheme = if snapshot.config.ssl { "wss" } else { "ws" };
        let raw = format!(
            "{scheme}://{host}:{port}/index/api/webrtc",
            host = snapshot.config.server_host,
            port = snapshot.config.server_port,
        );
        parse_signaling_url(&raw, &self.config.url_policy)
    }
}

#[async_trait]
impl KeeperTransportFactory for WebSocketTransportFactory {
    type Transport = WebSocketP2pTransport;

    async fn connect(
        &self,
        snapshot: &P2pRoomKeeperSnapshot,
    ) -> Result<Self::Transport, P2pTransportError> {
        let url = self
            .signaling_url(snapshot)
            .map_err(WebSocketTransportError::Url)?;
        let request = url
            .render()
            .into_client_request()
            .map_err(|e| WebSocketTransportError::InvalidRequest(e.to_string()))?;
        let (stream, _resp) =
            match tokio::time::timeout(self.config.connect_timeout, connect_async(request)).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) => {
                    return Err(WebSocketTransportError::WebSocket(err.to_string()).into());
                }
                Err(_) => {
                    return Err(WebSocketTransportError::ConnectTimeout(
                        self.config.connect_timeout,
                    )
                    .into());
                }
            };
        Ok(WebSocketP2pTransport::new(stream, self.config.decoder))
    }
}

/// Type alias for the type-erased outbound sink shared by the
/// client- and server-side transports.
type BoxedSink =
    Box<dyn futures::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Send + Unpin>;

/// Type alias for the type-erased inbound stream shared by the
/// client- and server-side transports.
type BoxedStream = Box<
    dyn futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Send
        + Unpin,
>;

/// `tokio-tungstenite`-backed transport that satisfies the workspace
/// `P2pTransport` trait. Uses type-erased sink + stream halves so the
/// same struct can wrap both client (`MaybeTlsStream<TcpStream>`) and
/// server (`TcpStream`) WebSocket connections.
pub struct WebSocketP2pTransport {
    sink: AsyncMutex<BoxedSink>,
    stream: AsyncMutex<BoxedStream>,
    decoder: P2pDecoderConfig,
    closed: Arc<std::sync::atomic::AtomicBool>,
    /// Per-instance counters for tests / diagnostics.
    pub counters: Arc<WebSocketCounters>,
}

/// Lightweight counters that production code can probe without
/// reaching into the `tokio-tungstenite` types.
#[derive(Debug, Default)]
pub struct WebSocketCounters {
    pub messages_sent: std::sync::atomic::AtomicU64,
    pub messages_received: std::sync::atomic::AtomicU64,
    pub decode_errors: std::sync::atomic::AtomicU64,
}

impl WebSocketP2pTransport {
    /// Wrap an existing client-side WebSocket stream produced by
    /// `tokio_tungstenite::connect_async`.
    pub fn new(
        stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        decoder: P2pDecoderConfig,
    ) -> Self {
        let (sink, stream) = stream.split();
        Self::from_split(Box::new(sink), Box::new(stream), decoder)
    }

    /// Wrap an existing server-side WebSocket stream produced by
    /// `tokio_tungstenite::accept_async`. The stream type differs
    /// from the client side (plain `TcpStream` vs.
    /// `MaybeTlsStream<TcpStream>`), so we expose a separate
    /// constructor that erases both into `dyn` traits and shares the
    /// rest of the transport plumbing.
    pub fn from_server_stream(
        stream: WebSocketStream<tokio::net::TcpStream>,
        decoder: P2pDecoderConfig,
    ) -> Self {
        let (sink, stream) = stream.split();
        Self::from_split(Box::new(sink), Box::new(stream), decoder)
    }

    fn from_split(sink: BoxedSink, stream: BoxedStream, decoder: P2pDecoderConfig) -> Self {
        Self {
            sink: AsyncMutex::new(sink),
            stream: AsyncMutex::new(stream),
            decoder,
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            counters: Arc::new(WebSocketCounters::default()),
        }
    }
}

#[async_trait]
impl P2pTransport for WebSocketP2pTransport {
    async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(P2pTransportError::Closed);
        }
        let payload =
            message::render(&message).map_err(|e| P2pTransportError::Encode(e.to_string()))?;
        let mut sink = self.sink.lock().await;
        sink.send(WsMessage::Text(payload.into()))
            .await
            .map_err(|e| P2pTransportError::Io(e.to_string()))?;
        self.counters
            .messages_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError> {
        loop {
            let next = {
                let mut stream = self.stream.lock().await;
                stream.next().await
            };
            match next {
                None => {
                    self.closed
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Ok(P2pTransportEvent::Closed);
                }
                Some(Err(err)) => {
                    self.closed
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Ok(P2pTransportEvent::Error(err.to_string()));
                }
                Some(Ok(WsMessage::Text(raw))) => {
                    self.counters
                        .messages_received
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let parsed = match message::parse(&raw, self.decoder) {
                        Ok(m) => m,
                        Err(err) => {
                            self.counters
                                .decode_errors
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return Ok(P2pTransportEvent::Error(err.to_string()));
                        }
                    };
                    return Ok(P2pTransportEvent::Message(parsed));
                }
                Some(Ok(WsMessage::Binary(bytes))) => {
                    // Some signaling deployments send text-as-binary.
                    // Try to decode as UTF-8 + JSON; fall back to a
                    // diagnostic if either fails.
                    let text = match std::str::from_utf8(&bytes) {
                        Ok(t) => t.to_string(),
                        Err(_) => {
                            self.counters
                                .decode_errors
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return Ok(P2pTransportEvent::Error(
                                "received non-utf8 binary frame".into(),
                            ));
                        }
                    };
                    let parsed = match message::parse(&text, self.decoder) {
                        Ok(m) => m,
                        Err(err) => {
                            self.counters
                                .decode_errors
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return Ok(P2pTransportEvent::Error(err.to_string()));
                        }
                    };
                    self.counters
                        .messages_received
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(P2pTransportEvent::Message(parsed));
                }
                Some(Ok(WsMessage::Ping(payload))) => {
                    let mut sink = self.sink.lock().await;
                    let _ = sink.send(WsMessage::Pong(payload)).await;
                    continue;
                }
                Some(Ok(WsMessage::Pong(_))) => continue,
                Some(Ok(WsMessage::Close(_))) => {
                    self.closed
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Ok(P2pTransportEvent::Closed);
                }
                Some(Ok(WsMessage::Frame(_))) => continue,
            }
        }
    }

    async fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let mut sink = self.sink.lock().await;
        let _ = sink.send(WsMessage::Close(None)).await;
        let _ = sink.close().await;
    }
}

/// Snapshot helper used by integration tests to read counter values
/// without reaching into the atomic loads inline.
pub fn snapshot_counters(counters: &WebSocketCounters) -> WebSocketCounterSnapshot {
    use std::sync::atomic::Ordering;
    WebSocketCounterSnapshot {
        messages_sent: counters.messages_sent.load(Ordering::Relaxed),
        messages_received: counters.messages_received.load(Ordering::Relaxed),
        decode_errors: counters.decode_errors.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSocketCounterSnapshot {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub decode_errors: u64,
}

/// Lightweight smoke type used by [`super::WebSocketTransportFactory`]
/// callers to keep `tokio-tungstenite` types out of their own APIs.
#[derive(Debug, Default)]
pub struct WebSocketDecodeStats {
    inner: Mutex<Vec<String>>,
}

impl WebSocketDecodeStats {
    pub fn record(&self, msg: impl Into<String>) {
        self.inner.lock().push(msg.into());
    }
    pub fn snapshot(&self) -> Vec<String> {
        self.inner.lock().clone()
    }
}

// Suppress an unused-import warning on the `Bytes` re-export — we
// keep it around for downstream APIs that might want to surface
// binary payload counts.
#[allow(dead_code)]
fn _bytes_marker() -> Bytes {
    Bytes::new()
}
