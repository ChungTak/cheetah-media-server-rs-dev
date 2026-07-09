use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cheetah_runtime_api::{AsyncTcpStream, CancellationToken, RuntimeApi};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::warn;

use crate::server::{
    run_connection, ConnectionCommand, ConnectionControl, HttpFlvConnectionId,
    HttpFlvCoreCommandSender, HttpFlvDriverConfig, HttpFlvDriverEvent, HttpFlvServerHandle,
};

/// TLS configuration for HTTPS-FLV / WSS-FLV server.
#[derive(Debug, Clone)]
pub struct HttpFlvTlsDriverConfig {
    pub cert_path: String,
    pub key_path: String,
    pub handshake_timeout: Duration,
}

/// Wrapper to implement AsyncTcpStream for TLS streams.
struct TlsStreamWrapper {
    inner: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    peer: SocketAddr,
}

#[async_trait::async_trait]
impl AsyncTcpStream for TlsStreamWrapper {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        AsyncReadExt::read(&mut self.inner, buf).await
    }
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        AsyncWriteExt::write_all(&mut self.inner, buf).await
    }
    async fn shutdown(&mut self) -> io::Result<()> {
        AsyncWriteExt::shutdown(&mut self.inner).await
    }
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

/// Start an HTTPS-FLV server with TLS encryption.
pub fn start_tls_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: HttpFlvDriverConfig,
    tls_config: HttpFlvTlsDriverConfig,
    cancel: CancellationToken,
) -> io::Result<HttpFlvServerHandle> {
    let rustls_config = load_tls_config(&tls_config.cert_path, &tls_config.key_path)?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_config));
    let listener = std::net::TcpListener::bind(listen)?;
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
    let local_addr = listener.local_addr()?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = HttpFlvCoreCommandSender { tx: cmd_tx };

    let conn_ids = Arc::new(AtomicU64::new(1_000_000));
    let conn_map: Arc<Mutex<HashMap<HttpFlvConnectionId, ConnectionControl>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let join_cancel = cancel.clone();
    let handshake_timeout = tls_config.handshake_timeout;
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let config = config.clone();
        let runtime_api = runtime_api.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => break,
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break; };
                        if crate::server::handle_driver_command_with_map(cmd, &conn_map) {
                            join_cancel.cancel();
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((tcp_stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let acceptor = acceptor.clone();
                                let (conn_tx, conn_rx) = mpsc::channel(config.write_queue_capacity.max(1));
                                let connection_cancel = join_cancel.child_token();
                                conn_map.lock().insert(connection_id, ConnectionControl {
                                    tx: conn_tx,
                                    cancel: connection_cancel.clone(),
                                });
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let config2 = config.clone();
                                let runtime_api2 = runtime_api.clone();
                                let _ = runtime_api2.spawn(Box::pin(async move {
                                    let tls_stream = match tokio::time::timeout(
                                        handshake_timeout,
                                        acceptor.accept(tcp_stream),
                                    ).await {
                                        Ok(Ok(s)) => s,
                                        _ => {
                                            conn_map2.lock().remove(&connection_id);
                                            return;
                                        }
                                    };
                                    let _ = event_tx2.send(HttpFlvDriverEvent::ConnectionOpened {
                                        connection_id,
                                        peer: Some(peer),
                                    }).await;
                                    let wrapped: Box<dyn AsyncTcpStream> = Box::new(TlsStreamWrapper {
                                        inner: tls_stream,
                                        peer,
                                    });
                                    run_connection(
                                        connection_id,
                                        wrapped,
                                        conn_rx,
                                        event_tx2,
                                        conn_map2,
                                        connection_cancel,
                                        config2,
                                    ).await;
                                }));
                            }
                            Err(err) => {
                                warn!(%err, "https-flv tls accept failed");
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

fn load_tls_config(cert_path: &str, key_path: &str) -> io::Result<rustls::ServerConfig> {
    let cert_data =
        std::fs::read(cert_path).map_err(|e| io::Error::other(format!("read cert: {e}")))?;
    let key_data =
        std::fs::read(key_path).map_err(|e| io::Error::other(format!("read key: {e}")))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_data.as_slice())
        .filter_map(|r| r.ok())
        .collect();
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no certificates found in cert file",
        ));
    }

    let key = rustls_pemfile::private_key(&mut key_data.as_slice())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse key: {e}")))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "no private key found in key file",
            )
        })?;

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("tls config: {e}")))
}
