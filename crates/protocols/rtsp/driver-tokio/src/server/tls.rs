use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tracing::warn;

use super::command::{
    handle_driver_command, ConnectionCommand, ConnectionHandle, ConnectionMap,
    RtspCoreCommandSender,
};
use super::connection::{run_connection, ConnectionRuntime};
use super::{DriverConfig, DriverEvent, RtspServerHandle};

/// TLS configuration for the RTSPS listener.
#[derive(Debug, Clone)]
pub struct DriverTlsConfig {
    /// `listen` field of type `SocketAddr`.
    /// `listen` 字段，类型为 `SocketAddr`.
    pub listen: SocketAddr,
    /// `server_config` field.
    /// `server_config` 字段.
    pub server_config: Arc<ServerConfig>,
    /// `handshake_timeout` field of type `Duration`.
    /// `handshake_timeout` 字段，类型为 `Duration`.
    pub handshake_timeout: Duration,
}

/// Start a TLS-enabled RTSP server (RTSPS).
///
/// Accepted connections are TLS-terminated then handled identically to plain RTSP.
pub fn start_tls_server(
    runtime_api: Arc<dyn RuntimeApi>,
    tls_config: DriverTlsConfig,
    config: DriverConfig,
    cancel: CancellationToken,
) -> io::Result<RtspServerHandle> {
    let listener = std::net::TcpListener::bind(tls_config.listen)?;
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
    let acceptor = TlsAcceptor::from(tls_config.server_config);
    let handshake_timeout = tls_config.handshake_timeout;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtspCoreCommandSender::new(cmd_tx.clone());

    let conn_map: ConnectionMap = Arc::new(Mutex::new(HashMap::new()));
    let conn_ids = Arc::new(AtomicU64::new(1));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let runtime_api = runtime_api.clone();
        let config = config.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => break,
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break };
                        if handle_driver_command(cmd, &conn_map, &join_cancel).await {
                            break;
                        }
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((tcp_stream, peer)) => {
                                let acceptor = acceptor.clone();
                                let event_tx = event_tx.clone();
                                let conn_map = conn_map.clone();
                                let join_cancel = join_cancel.clone();
                                let config = config.clone();
                                let runtime_api_clone = runtime_api.clone();
                                let conn_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let _ = runtime_api.spawn(Box::pin(async move {
                                    handle_tls_accept(
                                        conn_id,
                                        tcp_stream,
                                        peer,
                                        acceptor,
                                        handshake_timeout,
                                        event_tx,
                                        conn_map,
                                        join_cancel,
                                        config,
                                        runtime_api_clone,
                                    ).await;
                                }));
                            }
                            Err(err) => {
                                warn!(%err, "rtsps listener accept failed");
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                }
            }

            let connections: Vec<ConnectionHandle> = conn_map.lock().values().cloned().collect();
            for connection in connections {
                connection.cancel.cancel();
                let _ = connection.tx.try_send(ConnectionCommand::Close);
            }
        }
    }));

    Ok(RtspServerHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_tls_accept(
    connection_id: u64,
    tcp_stream: tokio::net::TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    handshake_timeout: Duration,
    event_tx: mpsc::Sender<DriverEvent>,
    conn_map: ConnectionMap,
    join_cancel: CancellationToken,
    config: DriverConfig,
    _runtime_api: Arc<dyn RuntimeApi>,
) {
    let tls_stream =
        match tokio::time::timeout(handshake_timeout, acceptor.accept(tcp_stream)).await {
            Ok(Ok(tls_stream)) => tls_stream,
            Ok(Err(err)) => {
                warn!(%peer, %err, "rtsps tls handshake failed");
                return;
            }
            Err(_) => {
                warn!(%peer, "rtsps tls handshake timed out");
                return;
            }
        };

    let wrapped: Box<dyn cheetah_runtime_api::AsyncTcpStream> = Box::new(TlsStreamWrapper {
        inner: tls_stream,
        peer,
    });

    let (conn_tx, conn_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let child_cancel = join_cancel.child_token();
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: conn_tx,
            cancel: child_cancel.clone(),
        },
    );

    if event_tx
        .send(DriverEvent::ConnectionOpened {
            connection_id,
            peer: Some(peer),
        })
        .await
        .is_err()
    {
        conn_map.lock().remove(&connection_id);
        child_cancel.cancel();
        return;
    }

    let runtime = ConnectionRuntime {
        event_tx: event_tx.clone(),
        conn_map: conn_map.clone(),
        cancel: child_cancel,
        config,
    };
    run_connection(connection_id, wrapped, Bytes::new(), conn_rx, runtime).await;
}

/// Wrapper around `tokio_rustls::server::TlsStream<TcpStream>` implementing `AsyncTcpStream`.
struct TlsStreamWrapper {
    inner: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    peer: SocketAddr,
}

#[async_trait::async_trait]
impl cheetah_runtime_api::AsyncTcpStream for TlsStreamWrapper {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}
