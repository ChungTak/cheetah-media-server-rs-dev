//! Optional inbound signaling server adapter.
//!
//! Phase 05 follow-up — round 10: ZLMediaKit deployments often run
//! their own signaling server (`mk_signaling_server_start`), but
//! cheetah can also accept inbound P2P signaling connections so a
//! peer can drive cheetah without an intermediate signaling host.
//!
//! This module provides a `tokio-tungstenite`-backed
//! `accept_async` server that wraps each upgraded WebSocket stream
//! into a [`super::WebSocketP2pTransport`] and surfaces the result
//! through a configurable callback.
//!
//! Production code uses this when the operator opts in via module
//! config; tests use it to drive end-to-end signaling without a
//! third-party server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use thiserror::Error;
use tokio::net::TcpListener;

use super::message::P2pDecoderConfig;
use super::ws::WebSocketP2pTransport;

/// Configuration for [`run_server`].
#[derive(Debug, Clone)]
pub struct SignalingServerConfig {
    /// Maximum number of concurrent inbound connections. Reaching
    /// this cap drops the next accept until an existing connection
    /// closes.
    pub max_connections: usize,
    /// Decoder limits applied to every accepted transport.
    pub decoder: P2pDecoderConfig,
    /// Per-connection accept timeout for the WebSocket handshake.
    pub accept_timeout: Duration,
}

impl Default for SignalingServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            decoder: P2pDecoderConfig::default(),
            accept_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Error)]
pub enum SignalingServerError {
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("accept failed: {0}")]
    Accept(String),
}

/// Information surfaced to the connection handler.
#[derive(Debug, Clone)]
pub struct InboundConnection {
    pub remote_addr: SocketAddr,
}

/// Type alias for the connection handler. Production code will plug
/// `run_bridge_with_lifecycle` here; tests inspect the transport
/// directly.
pub type ConnectionHandler = Arc<
    dyn Fn(InboundConnection, WebSocketP2pTransport) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// Run the inbound WebSocket signaling server. Returns when the
/// listener errors or `cancel` fires.
///
/// The server accepts incoming TCP connections, performs a
/// `tokio-tungstenite` handshake (with timeout), and hands the
/// resulting transport to `handler`. Each handler runs on its own
/// task; the server immediately returns to accepting more.
pub async fn run_server(
    listener: TcpListener,
    config: SignalingServerConfig,
    handler: ConnectionHandler,
    cancel: CancellationToken,
) -> Result<(), SignalingServerError> {
    let connection_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        let (stream, remote_addr) = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            res = listener.accept() => match res {
                Ok(pair) => pair,
                Err(err) => return Err(SignalingServerError::Accept(err.to_string())),
            }
        };

        // Backpressure: drop the new connection if we're at capacity.
        // The peer will see a TCP RST and should retry with backoff.
        if connection_count.load(std::sync::atomic::Ordering::Acquire) >= config.max_connections {
            drop(stream);
            continue;
        }
        connection_count.fetch_add(1, std::sync::atomic::Ordering::Release);

        let handler = handler.clone();
        let connection_count_for_task = connection_count.clone();
        let accept_timeout = config.accept_timeout;
        let decoder = config.decoder;
        tokio::spawn(async move {
            // Defer the connection-count decrement until the task
            // exits so capacity is freed even if the handshake fails.
            let _guard = ConnectionGuard::new(connection_count_for_task);

            let upgrade =
                match tokio::time::timeout(accept_timeout, tokio_tungstenite::accept_async(stream))
                    .await
                {
                    Ok(Ok(ws)) => ws,
                    Ok(Err(err)) => {
                        tracing::debug!(
                            target: "webrtc::p2p::server",
                            "ws handshake failed for {remote_addr}: {err}"
                        );
                        return;
                    }
                    Err(_) => {
                        tracing::debug!(
                            target: "webrtc::p2p::server",
                            "ws handshake for {remote_addr} timed out after {accept_timeout:?}"
                        );
                        return;
                    }
                };

            let transport = WebSocketP2pTransport::from_server_stream(upgrade, decoder);
            let info = InboundConnection { remote_addr };
            handler(info, transport).await;
        });
    }
}

/// Drop-guard that decrements the connection counter on exit.
struct ConnectionGuard {
    counter: Arc<std::sync::atomic::AtomicUsize>,
}

impl ConnectionGuard {
    fn new(counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self { counter }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::message::P2pMessage;
    use crate::p2p::transport::{P2pTransport, P2pTransportEvent};
    use futures::{FutureExt, SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_accepts_websocket_and_passes_transport_to_handler() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();

        let (received_tx, received_rx) = tokio::sync::oneshot::channel::<P2pMessage>();
        let received_tx = std::sync::Arc::new(parking_lot::Mutex::new(Some(received_tx)));

        let handler: ConnectionHandler = Arc::new(move |_info, transport| {
            let received_tx = received_tx.clone();
            async move {
                if let Ok(P2pTransportEvent::Message(msg)) = transport.recv().await {
                    if let Some(tx) = received_tx.lock().take() {
                        let _ = tx.send(msg);
                    }
                }
                transport.close().await;
            }
            .boxed()
        });

        let cancel = CancellationToken::new();
        let cancel_for_server = cancel.clone();
        let server_handle = tokio::spawn(async move {
            run_server(
                listener,
                SignalingServerConfig::default(),
                handler,
                cancel_for_server,
            )
            .await
        });

        // Connect a client and send a single ping message.
        let url = format!("ws://{}/p2p", bound);
        let (mut client, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
        let payload = serde_json::json!({
            "type": "ping",
            "room_id": "r",
            "peer_id": "p",
            "transport_id": "t",
        });
        client
            .send(WsMessage::Text(payload.to_string().into()))
            .await
            .unwrap();
        // Drop client to close.
        let _ = client.close(None).await;
        while client.next().await.is_some() {}

        // Server should have routed the message to the handler.
        let received = tokio::time::timeout(Duration::from_secs(2), received_rx)
            .await
            .expect("handler runs within 2s")
            .expect("recv ok");
        match received {
            P2pMessage::Ping { header } => {
                assert_eq!(header.peer_id.as_deref(), Some("p"));
            }
            other => panic!("expected ping, got {other:?}"),
        }

        cancel.cancel();
        let _ = server_handle.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_drops_when_capacity_exceeded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();

        // Handler holds the connection open until the test cancels.
        let parking_cancel = CancellationToken::new();
        let parking_for_handler = parking_cancel.clone();
        let handler: ConnectionHandler = Arc::new(move |_info, transport| {
            let parking = parking_for_handler.clone();
            async move {
                parking.cancelled().await;
                transport.close().await;
            }
            .boxed()
        });

        let cancel = CancellationToken::new();
        let cancel_for_server = cancel.clone();
        let server_handle = tokio::spawn(async move {
            run_server(
                listener,
                SignalingServerConfig {
                    max_connections: 1,
                    ..Default::default()
                },
                handler,
                cancel_for_server,
            )
            .await
        });

        // First client succeeds.
        let url = format!("ws://{}/", bound);
        let (mut first, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        // Wait for the server to register the first connection.
        // Because backpressure is asynchronous, we briefly poll
        // until a second handshake fails.
        let mut second_dropped = false;
        for _ in 0..30 {
            match tokio::time::timeout(
                Duration::from_millis(200),
                tokio_tungstenite::connect_async(&url),
            )
            .await
            {
                Ok(Ok((mut second, _))) => {
                    // Server may have accepted it before checking
                    // capacity. We want the second connection to
                    // close quickly, indicating backpressure.
                    let res = tokio::time::timeout(Duration::from_millis(200), second.next()).await;
                    if matches!(res, Ok(Some(_)) | Ok(None)) {
                        // Connection closed by server.
                        second_dropped = true;
                        break;
                    }
                    let _ = second.close(None).await;
                }
                Ok(Err(_)) | Err(_) => {
                    second_dropped = true;
                    break;
                }
            }
        }
        assert!(
            second_dropped,
            "second connection should have been dropped due to capacity"
        );

        let _ = first.close(None).await;
        parking_cancel.cancel();
        cancel.cancel();
        let _ = server_handle.await;
    }
}
