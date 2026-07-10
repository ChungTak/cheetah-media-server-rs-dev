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
use cheetah_webrtc_driver_tokio::{TokioWsConnector, WsConnection, WsConnector, WsError, WsFrame};
use parking_lot::Mutex;
use thiserror::Error;

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
/// `run_supervisor_with_hub`. The actual WebSocket/TLS I/O is delegated
/// to a driver-provided [`WsConnector`] (defaulting to
/// [`TokioWsConnector`]); this factory only owns the SSRF/URL policy and
/// the P2P decoder limits.
#[derive(Clone)]
pub struct WebSocketTransportFactory {
    config: WebSocketTransportConfig,
    connector: Arc<dyn WsConnector>,
}

impl std::fmt::Debug for WebSocketTransportFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketTransportFactory")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl WebSocketTransportFactory {
    /// Creates a new `WebSocketTransportFactory` instance.
    /// 创建新的 `WebSocketTransportFactory` 实例。
    pub fn new(config: WebSocketTransportConfig) -> Self {
        Self {
            config,
            connector: Arc::new(TokioWsConnector),
        }
    }

    /// Build a factory with a custom [`WsConnector`] (e.g. a test
    /// double or an alternate runtime's connector).
    pub fn with_connector(
        config: WebSocketTransportConfig,
        connector: Arc<dyn WsConnector>,
    ) -> Self {
        Self { config, connector }
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
        let connection = self
            .connector
            .connect(&url.render(), self.config.connect_timeout)
            .await
            .map_err(|err| match err {
                WsError::ConnectTimeout(d) => WebSocketTransportError::ConnectTimeout(d),
                WsError::InvalidRequest(msg) => WebSocketTransportError::InvalidRequest(msg),
                other => WebSocketTransportError::WebSocket(other.to_string()),
            })?;
        Ok(WebSocketP2pTransport::new(connection, self.config.decoder))
    }
}

/// Transport that satisfies the workspace `P2pTransport` trait by
/// wrapping a runtime-neutral driver [`WsConnection`] and layering the
/// P2P signaling encode/decode on top. WebSocket framing + TLS live in
/// the driver.
pub struct WebSocketP2pTransport {
    connection: Box<dyn WsConnection>,
    decoder: P2pDecoderConfig,
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
    /// Wrap a runtime-neutral driver [`WsConnection`].
    pub fn new(connection: Box<dyn WsConnection>, decoder: P2pDecoderConfig) -> Self {
        Self {
            connection,
            decoder,
            counters: Arc::new(WebSocketCounters::default()),
        }
    }

    fn decode_frame(&self, raw: &str) -> P2pTransportEvent {
        match message::parse(raw, self.decoder) {
            Ok(parsed) => {
                self.counters
                    .messages_received
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                P2pTransportEvent::Message(parsed)
            }
            Err(err) => {
                self.counters
                    .decode_errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                P2pTransportEvent::Error(err.to_string())
            }
        }
    }
}

#[async_trait]
impl P2pTransport for WebSocketP2pTransport {
    async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError> {
        let payload =
            message::render(&message).map_err(|e| P2pTransportError::Encode(e.to_string()))?;
        self.connection
            .send_text(payload)
            .await
            .map_err(|e| match e {
                WsError::Closed => P2pTransportError::Closed,
                other => P2pTransportError::Io(other.to_string()),
            })?;
        self.counters
            .messages_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError> {
        match self.connection.recv().await {
            Ok(WsFrame::Text(raw)) => Ok(self.decode_frame(&raw)),
            Ok(WsFrame::Binary(bytes)) => {
                // Some signaling deployments send text-as-binary.
                // Try to decode as UTF-8 + JSON; fall back to a
                // diagnostic if either fails.
                match std::str::from_utf8(&bytes) {
                    Ok(text) => Ok(self.decode_frame(text)),
                    Err(_) => {
                        self.counters
                            .decode_errors
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        Ok(P2pTransportEvent::Error(
                            "received non-utf8 binary frame".into(),
                        ))
                    }
                }
            }
            Ok(WsFrame::Closed) => Ok(P2pTransportEvent::Closed),
            Err(err) => Ok(P2pTransportEvent::Error(err.to_string())),
        }
    }

    async fn close(&self) {
        self.connection.close().await;
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

/// `WebSocketCounterSnapshot` data structure.
/// `WebSocketCounterSnapshot` 数据结构。
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
    /// `record` function of `WebSocketDecodeStats`.
    /// `WebSocketDecodeStats` 的 `record` 函数。
    pub fn record(&self, msg: impl Into<String>) {
        self.inner.lock().push(msg.into());
    }
    /// `snapshot` function of `WebSocketDecodeStats`.
    /// `WebSocketDecodeStats` 的 `snapshot` 函数。
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
