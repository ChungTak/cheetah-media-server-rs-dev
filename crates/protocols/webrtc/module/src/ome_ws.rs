//! OvenMediaEngine-compatible WebSocket transport adapter.
//!
//! The WebSocket framing / TCP accept loop lives in the driver
//! (`cheetah_webrtc_driver_tokio::ws`); this module is a thin adapter
//! that wraps a runtime-neutral [`WsConnection`] and layers OME
//! signaling encode/decode on top.

use std::sync::Arc;
use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_driver_tokio::{
    WsConnection, WsConnectionHandler, WsError, WsFrame, WsInbound, WsServerConfig, WsServerError,
    WsServerListener,
};
use thiserror::Error;

use crate::ome_signaling::{parse_ome_ws_message, OmeWsDecoderConfig, OmeWsMessage};

/// Error returned by `Web Socket Ome Transport` operations.
/// `Web Socket Ome Transport` 操作返回的错误。
#[derive(Debug, Error)]
pub enum WebSocketOmeTransportError {
    #[error("transport closed")]
    Closed,
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("decode failed: {0}")]
    Decode(String),
}

/// Configuration for `Ome Ws Server`.
/// `Ome Ws Server` 的配置。
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

/// `OmeWsInboundConnection` data structure.
/// `OmeWsInboundConnection` 数据结构。
#[derive(Debug, Clone)]
pub struct OmeWsInboundConnection {
    pub remote_addr: std::net::SocketAddr,
    pub path_and_query: String,
}

/// `OmeWsConnectionHandler` type alias.
/// `OmeWsConnectionHandler` 类型别名。
pub type OmeWsConnectionHandler = Arc<
    dyn Fn(OmeWsInboundConnection, WebSocketOmeTransport) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

/// Error returned by `Ome Ws Server` operations.
/// `Ome Ws Server` 操作返回的错误。
#[derive(Debug, Error)]
pub enum OmeWsServerError {
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("accept failed: {0}")]
    Accept(String),
}

/// OME signaling transport over a runtime-neutral [`WsConnection`].
pub struct WebSocketOmeTransport {
    connection: Box<dyn WsConnection>,
    decoder: OmeWsDecoderConfig,
}

/// Run the OME WebSocket signaling server on a driver-bound listener.
///
/// Wraps each accepted [`WsConnection`] into a [`WebSocketOmeTransport`]
/// and forwards it to `handler`. Returns when the listener errors or
/// `cancel` fires.
pub async fn run_ome_ws_server(
    listener: WsServerListener,
    config: OmeWsServerConfig,
    handler: OmeWsConnectionHandler,
    cancel: CancellationToken,
) -> Result<(), OmeWsServerError> {
    let decoder = config.decoder;
    let ws_handler: WsConnectionHandler = Arc::new(move |inbound: WsInbound, connection| {
        let handler = handler.clone();
        Box::pin(async move {
            let transport = WebSocketOmeTransport::new(connection, decoder);
            handler(
                OmeWsInboundConnection {
                    remote_addr: inbound.remote_addr,
                    path_and_query: inbound.path_and_query,
                },
                transport,
            )
            .await;
        })
    });
    let ws_config = WsServerConfig {
        max_connections: config.max_connections,
        accept_timeout: config.accept_timeout,
    };
    listener
        .serve(ws_config, ws_handler, cancel)
        .await
        .map_err(|err| match err {
            WsServerError::Bind(msg) => OmeWsServerError::Bind(msg),
            WsServerError::Accept(msg) => OmeWsServerError::Accept(msg),
        })
}

impl WebSocketOmeTransport {
    /// Wrap a neutral [`WsConnection`] with OME signaling codec.
    pub fn new(connection: Box<dyn WsConnection>, decoder: OmeWsDecoderConfig) -> Self {
        Self {
            connection,
            decoder,
        }
    }

    /// Receives `message` from the peer.
    /// 从对端接收 `message`。
    pub async fn recv_message(&self) -> Result<Option<OmeWsMessage>, WebSocketOmeTransportError> {
        match self.connection.recv().await {
            Ok(WsFrame::Text(text)) => {
                let message = parse_ome_ws_message(&text, self.decoder)
                    .map_err(|err| WebSocketOmeTransportError::Decode(err.to_string()))?;
                Ok(Some(message))
            }
            Ok(WsFrame::Binary(_)) => Err(WebSocketOmeTransportError::Decode(
                "OME signaling expects text JSON frames".into(),
            )),
            Ok(WsFrame::Closed) => Ok(None),
            Err(err) => Err(WebSocketOmeTransportError::WebSocket(err.to_string())),
        }
    }

    /// Sends `text` to the peer.
    /// 向对端发送 `text`。
    pub async fn send_text(&self, text: String) -> Result<(), WebSocketOmeTransportError> {
        self.connection
            .send_text(text)
            .await
            .map_err(|err| match err {
                WsError::Closed => WebSocketOmeTransportError::Closed,
                other => WebSocketOmeTransportError::WebSocket(other.to_string()),
            })
    }

    /// Closes the resource.
    /// 关闭资源。
    pub async fn close(&self) {
        self.connection.close().await;
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
            let transport = WebSocketOmeTransport::new(
                Box::new(cheetah_webrtc_driver_tokio::TokioWsConnection::from_server_stream(ws)),
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
        let (listener, bound) = cheetah_webrtc_driver_tokio::bind_ws_server("127.0.0.1:0")
            .await
            .unwrap();
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
