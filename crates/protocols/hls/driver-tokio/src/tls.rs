//! HLS TLS support: loads certificates and wraps TCP streams with TLS.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_runtime_api::{AsyncTcpStream, CancellationToken, RuntimeApi};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::warn;

use crate::server::{
    HlsCommandSender, HlsConnectionId, HlsDriverCommand, HlsDriverConfig, HlsDriverEvent,
    HlsServerHandle,
};

/// TLS configuration for HTTPS HLS server.
#[derive(Debug, Clone)]
pub struct HlsTlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

/// Start an HTTPS HLS server with TLS.
pub fn start_tls_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: HlsDriverConfig,
    tls_config: HlsTlsConfig,
    cancel: CancellationToken,
) -> io::Result<HlsServerHandle> {
    let rustls_config = load_tls_config(&tls_config.cert_path, &tls_config.key_path)?;
    let acceptor = TlsAcceptor::from(Arc::new(rustls_config));

    let std_listener = std::net::TcpListener::bind(listen)?;
    std_listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(std_listener)?;
    let local_addr = listener.local_addr()?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = HlsCommandSender::new(cmd_tx);

    let conn_ids = Arc::new(AtomicU64::new(1));
    let conn_map: Arc<Mutex<HashMap<HlsConnectionId, ConnectionState>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let config = config.clone();
        let runtime_api = runtime_api.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => break,
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break };
                        if handle_command(&conn_map, cmd, &join_cancel) {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((tcp_stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let acceptor = acceptor.clone();
                                let (resp_tx, resp_rx) = mpsc::channel(1);
                                let connection_cancel = join_cancel.child_token();
                                conn_map.lock().insert(connection_id, ConnectionState {
                                    response_tx: resp_tx,
                                    cancel: connection_cancel.clone(),
                                });
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let config2 = config.clone();
                                let _ = runtime_api.spawn(Box::pin(async move {
                                    // TLS handshake with 5s timeout
                                    let tls_stream = match tokio::time::timeout(
                                        std::time::Duration::from_secs(5),
                                        acceptor.accept(tcp_stream),
                                    ).await {
                                        Ok(Ok(s)) => s,
                                        _ => {
                                            conn_map2.lock().remove(&connection_id);
                                            return;
                                        }
                                    };
                                    let _ = event_tx2.send(HlsDriverEvent::ConnectionOpened {
                                        connection_id,
                                        peer: Some(peer),
                                    }).await;
                                    let wrapped: Box<dyn AsyncTcpStream> = Box::new(TlsStreamWrapper {
                                        inner: tls_stream,
                                        peer,
                                    });
                                    crate::server::run_connection(
                                        connection_id,
                                        wrapped,
                                        resp_rx,
                                        event_tx2,
                                        connection_cancel,
                                        config2,
                                    ).await;
                                    conn_map2.lock().remove(&connection_id);
                                }));
                            }
                            Err(err) => {
                                warn!("HLS TLS accept error: {err}");
                            }
                        }
                    }
                }
            }
        }
    }));

    Ok(HlsServerHandle::new(
        local_addr,
        event_rx,
        command_sender,
        cancel,
        join,
    ))
}

struct ConnectionState {
    response_tx: mpsc::Sender<crate::server::HttpResponseData>,
    cancel: CancellationToken,
}

fn handle_command(
    conn_map: &Arc<Mutex<HashMap<HlsConnectionId, ConnectionState>>>,
    cmd: HlsDriverCommand,
    join_cancel: &CancellationToken,
) -> bool {
    match cmd {
        HlsDriverCommand::SendResponse {
            connection_id,
            status,
            content_type,
            body,
            headers,
        } => {
            let map = conn_map.lock();
            if let Some(state) = map.get(&connection_id) {
                let _ = state.response_tx.try_send(crate::server::HttpResponseData {
                    status,
                    content_type,
                    body,
                    headers,
                });
            }
            false
        }
        HlsDriverCommand::CloseConnection { connection_id } => {
            let map = conn_map.lock();
            if let Some(state) = map.get(&connection_id) {
                state.cancel.cancel();
            }
            false
        }
        HlsDriverCommand::Shutdown => {
            join_cancel.cancel();
            true
        }
    }
}

/// Wrapper implementing AsyncTcpStream for TLS streams.
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
