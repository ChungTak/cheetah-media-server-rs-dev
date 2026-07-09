//! OvenMediaEngine-compatible WebSocket transport adapter.

use std::sync::Arc;
use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;

use crate::ome_signaling::{parse_ome_ws_message, OmeWsDecoderConfig, OmeWsMessage};

type BoxedSink =
    Box<dyn futures::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Send + Unpin>;

type BoxedStream = Box<
    dyn futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Send
        + Unpin,
>;

#[derive(Debug, Error)]
pub enum WebSocketOmeTransportError {
    #[error("transport closed")]
    Closed,
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("decode failed: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct OmeWsServerConfig {
    pub max_connections: usize,
    pub decoder: OmeWsDecoderConfig,
    pub accept_timeout: Duration,
}

impl Default for OmeWsServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            decoder: OmeWsDecoderConfig::default(),
            accept_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OmeWsInboundConnection {
    pub remote_addr: std::net::SocketAddr,
    pub path_and_query: String,
}

pub type OmeWsConnectionHandler = Arc<
    dyn Fn(OmeWsInboundConnection, WebSocketOmeTransport) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

#[derive(Debug, Error)]
pub enum OmeWsServerError {
    #[error("accept failed: {0}")]
    Accept(String),
}

pub struct WebSocketOmeTransport {
    sink: AsyncMutex<BoxedSink>,
    stream: AsyncMutex<BoxedStream>,
    decoder: OmeWsDecoderConfig,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

#[allow(clippy::result_large_err)]
pub async fn run_ome_ws_server(
    listener: TcpListener,
    config: OmeWsServerConfig,
    handler: OmeWsConnectionHandler,
    cancel: CancellationToken,
) -> Result<(), OmeWsServerError> {
    let connection_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let (stream, remote_addr) = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = listener.accept() => result.map_err(|err| OmeWsServerError::Accept(err.to_string()))?,
        };

        if connection_count.load(std::sync::atomic::Ordering::Acquire) >= config.max_connections {
            drop(stream);
            continue;
        }
        connection_count.fetch_add(1, std::sync::atomic::Ordering::Release);

        let handler = handler.clone();
        let connection_count_for_task = connection_count.clone();
        let decoder = config.decoder;
        let accept_timeout = config.accept_timeout;
        tokio::spawn(async move {
            let _guard = ConnectionGuard::new(connection_count_for_task);
            let path_and_query = Arc::new(parking_lot::Mutex::new(String::from("/")));
            let path_and_query_for_callback = path_and_query.clone();
            let ws = match tokio::time::timeout(
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
                        *path_and_query_for_callback.lock() = target;
                        Ok(response)
                    },
                ),
            )
            .await
            {
                Ok(Ok(ws)) => ws,
                Ok(Err(err)) => {
                    tracing::debug!("OME WebSocket handshake failed for {remote_addr}: {err}");
                    return;
                }
                Err(_) => {
                    tracing::debug!(
                        "OME WebSocket handshake for {remote_addr} timed out after {accept_timeout:?}"
                    );
                    return;
                }
            };
            let path_and_query = path_and_query.lock().clone();
            let transport = WebSocketOmeTransport::from_server_stream(ws, decoder);
            handler(
                OmeWsInboundConnection {
                    remote_addr,
                    path_and_query,
                },
                transport,
            )
            .await;
        });
    }
}

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

impl WebSocketOmeTransport {
    pub fn from_server_stream(
        stream: WebSocketStream<tokio::net::TcpStream>,
        decoder: OmeWsDecoderConfig,
    ) -> Self {
        let (sink, stream) = stream.split();
        Self {
            sink: AsyncMutex::new(Box::new(sink)),
            stream: AsyncMutex::new(Box::new(stream)),
            decoder,
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub async fn recv_message(&self) -> Result<Option<OmeWsMessage>, WebSocketOmeTransportError> {
        loop {
            let next = {
                let mut stream = self.stream.lock().await;
                stream.next().await
            };
            match next {
                Some(Ok(WsMessage::Text(text))) => {
                    let message = parse_ome_ws_message(&text, self.decoder)
                        .map_err(|err| WebSocketOmeTransportError::Decode(err.to_string()))?;
                    return Ok(Some(message));
                }
                Some(Ok(WsMessage::Close(_))) | None => {
                    self.closed
                        .store(true, std::sync::atomic::Ordering::Release);
                    return Ok(None);
                }
                Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => continue,
                Some(Ok(WsMessage::Binary(_))) => {
                    return Err(WebSocketOmeTransportError::Decode(
                        "OME signaling expects text JSON frames".into(),
                    ));
                }
                Some(Ok(_)) => continue,
                Some(Err(err)) => {
                    return Err(WebSocketOmeTransportError::WebSocket(err.to_string()))
                }
            }
        }
    }

    pub async fn send_text(&self, text: String) -> Result<(), WebSocketOmeTransportError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(WebSocketOmeTransportError::Closed);
        }
        let mut sink = self.sink.lock().await;
        sink.send(WsMessage::Text(text.into()))
            .await
            .map_err(|err| WebSocketOmeTransportError::WebSocket(err.to_string()))
    }

    pub async fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let mut sink = self.sink.lock().await;
        let _ = sink.send(WsMessage::Close(None)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{FutureExt, SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

    use super::*;
    use crate::ome_signaling::OmeWsMessage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn websocket_ome_transport_receives_ome_json_and_sends_text_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let transport = WebSocketOmeTransport::from_server_stream(
                ws,
                crate::ome_signaling::OmeWsDecoderConfig::default(),
            );
            let message = transport.recv_message().await.unwrap().unwrap();
            assert_eq!(
                message,
                OmeWsMessage::RequestOffer {
                    id: Some(7),
                    peer_id: Some(0),
                }
            );
            transport
                .send_text(json!({"command": "offer", "id": 7}).to_string())
                .await
                .unwrap();
            transport.close().await;
        });

        let url = format!("ws://{bound}/live/camera01?direction=play");
        let (mut client, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        client
            .send(WsMessage::Text(
                json!({"command": "request_offer", "id": 7, "peer_id": 0})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            received.into_text().unwrap(),
            json!({"command": "offer", "id": 7}).to_string()
        );

        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ome_ws_server_accepts_connection_and_invokes_handler() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();
        let cancel = cheetah_runtime_api::CancellationToken::new();

        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<OmeWsMessage>();
        let seen_tx = std::sync::Arc::new(parking_lot::Mutex::new(Some(seen_tx)));
        let (path_tx, path_rx) = tokio::sync::oneshot::channel::<String>();
        let path_tx = std::sync::Arc::new(parking_lot::Mutex::new(Some(path_tx)));
        let handler: OmeWsConnectionHandler = std::sync::Arc::new(move |info, transport| {
            let seen_tx = seen_tx.clone();
            let path_tx = path_tx.clone();
            async move {
                if let Some(tx) = path_tx.lock().take() {
                    let _ = tx.send(info.path_and_query);
                }
                if let Ok(Some(message)) = transport.recv_message().await {
                    if let Some(tx) = seen_tx.lock().take() {
                        let _ = tx.send(message);
                    }
                }
                transport.close().await;
            }
            .boxed()
        });

        let server_cancel = cancel.clone();
        let server = tokio::spawn(async move {
            run_ome_ws_server(
                listener,
                OmeWsServerConfig::default(),
                handler,
                server_cancel,
            )
            .await
        });

        let url = format!("ws://{bound}/live/camera01?direction=play");
        let (mut client, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        client
            .send(WsMessage::Text(
                json!({"command": "request-offer", "id": 9})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let _ = client.close(None).await;

        let message = tokio::time::timeout(Duration::from_secs(2), seen_rx)
            .await
            .unwrap()
            .unwrap();
        let path_and_query = tokio::time::timeout(Duration::from_secs(2), path_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            message,
            OmeWsMessage::RequestOffer {
                id: Some(9),
                peer_id: None,
            }
        );
        assert_eq!(path_and_query, "/live/camera01?direction=play");

        cancel.cancel();
        let _ = server.await.unwrap();
    }
}
