//! HTTP/WS TS server driver.
//!
//! HTTP/WS TS 服务器驱动。

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_runtime_api::CancellationToken;
use cheetah_ts_core::{StreamKeyParts, TsTransport};
use tokio::sync::mpsc;

/// Unique connection identifier.
///
/// 唯一连接标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TsConnectionId(pub u64);

/// Configuration for the TS driver.
///
/// TS 驱动配置。
#[derive(Debug, Clone)]
pub struct TsDriverConfig {
    pub listen: SocketAddr,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub tls: Option<TsTlsConfig>,
}

/// TLS configuration.
///
/// TLS 配置。
#[derive(Debug, Clone)]
pub struct TsTlsConfig {
    pub listen: SocketAddr,
    pub cert_path: String,
    pub key_path: String,
    pub handshake_timeout_ms: u64,
}

/// Commands sent from module to driver.
///
/// 模块发送给驱动的命令。
#[derive(Debug)]
pub enum TsDriverCommand {
    SendBytes {
        connection_id: TsConnectionId,
        data: Bytes,
    },
    CloseConnection {
        connection_id: TsConnectionId,
    },
}

/// Events sent from driver to module.
///
/// 驱动发送给模块的事件。
#[derive(Debug)]
pub enum TsDriverEvent {
    PlayRequested {
        connection_id: TsConnectionId,
        stream_key: StreamKeyParts,
        transport: TsTransport,
    },
    ConnectionClosed {
        connection_id: TsConnectionId,
    },
}

/// Handle to the running server.
///
/// 运行中服务器的句柄。
pub struct TsServerHandle {
    event_rx: mpsc::Receiver<TsDriverEvent>,
}

/// `TsServerHandle` API.
///
/// `TsServerHandle` API。
impl TsServerHandle {
    /// Receive the next event from the driver.
    ///
    /// 从驱动接收下一个事件。
    pub async fn recv_event(&mut self) -> Option<TsDriverEvent> {
        self.event_rx.recv().await
    }
}

/// Sender for commands to the driver.
///
/// 向驱动发送命令的发送器。
#[derive(Clone)]
pub struct TsCommandSender {
    tx: mpsc::Sender<TsDriverCommand>,
}

/// `TsCommandSender` API.
///
/// `TsCommandSender` API。
impl TsCommandSender {
    /// Send a command to the driver loop.
    ///
    /// 向驱动循环发送命令。
    pub async fn send(&self, cmd: TsDriverCommand) {
        let _ = self.tx.send(cmd).await;
    }
}

/// Start the TS HTTP/WS server. Returns command sender and event receiver.
///
/// 启动 TS HTTP/WS 服务器，返回命令发送器和事件接收器。
pub fn start_server(
    config: TsDriverConfig,
    cancel: CancellationToken,
) -> (TsCommandSender, TsServerHandle) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<TsDriverCommand>(256);
    let (event_tx, event_rx) = mpsc::channel::<TsDriverEvent>(256);

    tokio::spawn(run_server(config, event_tx, cmd_rx, cancel));

    (TsCommandSender { tx: cmd_tx }, TsServerHandle { event_rx })
}

/// Main accept loop for the TS HTTP/WS server.
///
/// TS HTTP/WS 服务器的主 accept 循环。
async fn run_server(
    config: TsDriverConfig,
    event_tx: mpsc::Sender<TsDriverEvent>,
    mut cmd_rx: mpsc::Receiver<TsDriverCommand>,
    cancel: CancellationToken,
) {
    use std::collections::HashMap;
    use tokio::net::TcpListener;

    let listener = match TcpListener::bind(config.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("TS server bind failed on {}: {e}", config.listen);
            return;
        }
    };

    tracing::info!("TS server listening on {}", config.listen);

    // Optional TLS listener
    let tls_listener = if let Some(ref tls_cfg) = config.tls {
        let server_config = match crate::tls::load_tls_config(&tls_cfg.cert_path, &tls_cfg.key_path)
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("TS TLS config failed: {e}");
                return;
            }
        };
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        match TcpListener::bind(tls_cfg.listen).await {
            Ok(l) => {
                tracing::info!("TS TLS server listening on {}", tls_cfg.listen);
                Some((l, acceptor, tls_cfg.handshake_timeout_ms))
            }
            Err(e) => {
                tracing::error!("TS TLS bind failed on {}: {e}", tls_cfg.listen);
                return;
            }
        }
    } else {
        None
    };

    let mut next_conn_id: u64 = 1;
    let conn_map: Arc<parking_lot::Mutex<HashMap<u64, mpsc::Sender<ConnCmd>>>> =
        Arc::new(parking_lot::Mutex::new(HashMap::new()));

    loop {
        // Build TLS accept future (or pending if no TLS)
        let tls_accept = async {
            if let Some((ref tls_l, _, _)) = tls_listener {
                tls_l.accept().await
            } else {
                std::future::pending().await
            }
        };

        tokio::select! {
            _ = cancel.cancelled() => break,
            accept = listener.accept() => {
                let Ok((stream, addr)) = accept else { continue };
                let conn_id = TsConnectionId(next_conn_id);
                next_conn_id += 1;
                let event_tx2 = event_tx.clone();
                let wq_cap = config.write_queue_capacity;
                let read_buffer_size = config.read_buffer_size;
                let (conn_tx, conn_rx) = mpsc::channel(wq_cap.max(1));
                conn_map.lock().insert(conn_id.0, conn_tx);
                let conn_map2 = conn_map.clone();
                tokio::spawn(async move {
                    handle_connection(stream, conn_id, addr, event_tx2, conn_rx, read_buffer_size).await;
                    conn_map2.lock().remove(&conn_id.0);
                });
            }
            accept = tls_accept => {
                let Ok((stream, addr)) = accept else { continue };
                let conn_id = TsConnectionId(next_conn_id);
                next_conn_id += 1;
                let event_tx2 = event_tx.clone();
                let wq_cap = config.write_queue_capacity;
                let read_buffer_size = config.read_buffer_size;
                let (conn_tx, conn_rx) = mpsc::channel(wq_cap.max(1));
                conn_map.lock().insert(conn_id.0, conn_tx);
                let conn_map2 = conn_map.clone();
                let (_, acceptor, timeout_ms) = tls_listener.as_ref().unwrap();
                let acceptor = acceptor.clone();
                let timeout_ms = *timeout_ms;
                tokio::spawn(async move {
                    let tls_result = tokio::time::timeout(
                        std::time::Duration::from_millis(timeout_ms),
                        acceptor.accept(stream),
                    ).await;
                    match tls_result {
                        Ok(Ok(tls_stream)) => {
                            handle_tls_connection(tls_stream, conn_id, addr, event_tx2, conn_rx, read_buffer_size).await;
                        }
                        _ => {
                            tracing::debug!("TLS handshake failed/timeout for {addr}");
                        }
                    }
                    conn_map2.lock().remove(&conn_id.0);
                });
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    TsDriverCommand::SendBytes { connection_id, data } => {
                        let mut map = conn_map.lock();
                        if let Some(tx) = map.get(&connection_id.0) {
                            if tx.try_send(ConnCmd::Send(data)).is_err() {
                                // Write queue full — slow client, close connection
                                map.remove(&connection_id.0);
                            }
                        }
                    }
                    TsDriverCommand::CloseConnection { connection_id } => {
                        if let Some(tx) = conn_map.lock().remove(&connection_id.0) {
                            let _ = tx.try_send(ConnCmd::Close);
                        }
                    }
                }
            }
        }
    }
}

/// Internal command sent to a per-connection task.
///
/// 发送给每个连接任务的内部命令。
enum ConnCmd {
    Send(Bytes),
    Close,
}

/// Handle a plain TCP connection and drive it through the TS core.
///
/// 处理普通 TCP 连接并通过 TS core 驱动。
async fn handle_connection(
    stream: tokio::net::TcpStream,
    conn_id: TsConnectionId,
    addr: SocketAddr,
    event_tx: mpsc::Sender<TsDriverEvent>,
    conn_rx: mpsc::Receiver<ConnCmd>,
    read_buffer_size: usize,
) {
    let (reader, writer) = tokio::io::split(stream);
    handle_generic_connection(
        reader,
        writer,
        conn_id,
        addr,
        event_tx,
        conn_rx,
        read_buffer_size,
    )
    .await;
}

/// Encode and write a WebSocket binary frame (server-side, no mask).
///
/// 编码并写入 WebSocket 二进制帧（服务端，无掩码）。
async fn write_ws_binary_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    payload: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let len = payload.len();
    // FIN=1, opcode=0x02 (binary)
    let mut header = Vec::with_capacity(10);
    header.push(0x82);
    if len <= 125 {
        header.push(len as u8);
    } else if len <= 65535 {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    writer.write_all(&header).await?;
    writer.write_all(payload).await
}

/// Handle a TLS connection (HTTPS/WSS).
///
/// 处理 TLS 连接（HTTPS/WSS）。
async fn handle_tls_connection(
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    conn_id: TsConnectionId,
    addr: SocketAddr,
    event_tx: mpsc::Sender<TsDriverEvent>,
    conn_rx: mpsc::Receiver<ConnCmd>,
    read_buffer_size: usize,
) {
    let (reader, writer) = tokio::io::split(stream);
    handle_generic_connection(
        reader,
        writer,
        conn_id,
        addr,
        event_tx,
        conn_rx,
        read_buffer_size,
    )
    .await;
}

/// Generic connection handler for any AsyncRead + AsyncWrite stream.
///
/// 通用连接处理器，适用于任何 AsyncRead + AsyncWrite 流。
async fn handle_generic_connection<R, W>(
    reader: R,
    mut writer: W,
    conn_id: TsConnectionId,
    _addr: SocketAddr,
    event_tx: mpsc::Sender<TsDriverEvent>,
    mut conn_rx: mpsc::Receiver<ConnCmd>,
    read_buffer_size: usize,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    let mut buf_reader = tokio::io::BufReader::new(reader);
    let header_limit = read_buffer_size.max(1024);
    let mut header_bytes = 0usize;

    let request_line = match read_limited_line(&mut buf_reader, header_limit).await {
        Ok(Some(line)) => line,
        Ok(None) | Err(_) => {
            let _ = event_tx
                .send(TsDriverEvent::ConnectionClosed {
                    connection_id: conn_id,
                })
                .await;
            return;
        }
    };
    header_bytes += request_line.len();
    if header_bytes > header_limit {
        let _ = event_tx
            .send(TsDriverEvent::ConnectionClosed {
                connection_id: conn_id,
            })
            .await;
        return;
    }

    let mut headers = Vec::new();
    loop {
        let line = match read_limited_line(&mut buf_reader, header_limit).await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => {
                let _ = event_tx
                    .send(TsDriverEvent::ConnectionClosed {
                        connection_id: conn_id,
                    })
                    .await;
                return;
            }
        };
        header_bytes += line.len();
        if header_bytes > header_limit {
            let _ = event_tx
                .send(TsDriverEvent::ConnectionClosed {
                    connection_id: conn_id,
                })
                .await;
            return;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }

    let method = match parts[0] {
        "GET" => cheetah_ts_core::HttpMethod::Get,
        "HEAD" => cheetah_ts_core::HttpMethod::Head,
        "OPTIONS" => cheetah_ts_core::HttpMethod::Options,
        _ => cheetah_ts_core::HttpMethod::Other,
    };

    let head = cheetah_ts_core::HttpRequestHead {
        method,
        method_raw: parts[0].to_string(),
        target: parts[1].to_string(),
        headers,
    };

    let is_ws = head.is_websocket_upgrade();
    let mut core = cheetah_ts_core::TsCore::new();
    let outputs = core.handle_input(cheetah_ts_core::TsCoreInput::RequestHead(head));

    let mut is_streaming = false;
    for output in &outputs {
        match output {
            cheetah_ts_core::TsCoreOutput::SendHttpResponse(resp) => {
                let mut response = format!("HTTP/1.1 {} {}\r\n", resp.status_code, resp.reason);
                for (k, v) in &resp.headers {
                    response.push_str(&format!("{k}: {v}\r\n"));
                }
                response.push_str("\r\n");
                if writer.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
            }
            cheetah_ts_core::TsCoreOutput::Event(cheetah_ts_core::TsCoreEvent::PlayRequested {
                stream_key,
                transport,
            }) => {
                is_streaming = true;
                let _ = event_tx
                    .send(TsDriverEvent::PlayRequested {
                        connection_id: conn_id,
                        stream_key: stream_key.clone(),
                        transport: *transport,
                    })
                    .await;
            }
            cheetah_ts_core::TsCoreOutput::Close { .. } => {
                let _ = event_tx
                    .send(TsDriverEvent::ConnectionClosed {
                        connection_id: conn_id,
                    })
                    .await;
                return;
            }
            _ => {}
        }
    }

    if !is_streaming {
        let _ = event_tx
            .send(TsDriverEvent::ConnectionClosed {
                connection_id: conn_id,
            })
            .await;
        return;
    }

    while let Some(cmd) = conn_rx.recv().await {
        match cmd {
            ConnCmd::Send(data) => {
                let write_result = if is_ws {
                    write_ws_binary_frame(&mut writer, &data).await
                } else {
                    writer.write_all(&data).await
                };
                if write_result.is_err() {
                    break;
                }
            }
            ConnCmd::Close => break,
        }
    }

    let _ = event_tx
        .send(TsDriverEvent::ConnectionClosed {
            connection_id: conn_id,
        })
        .await;
}

/// Read bytes until LF, rejecting lines that exceed `max_len`.
///
/// 读取字节直到 LF，超过 `max_len` 则拒绝。
async fn read_limited_line<R>(reader: &mut R, max_len: usize) -> std::io::Result<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut line = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let n = reader.read(&mut byte).await?;
        if n == 0 {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        line.push(byte[0]);
        if line.len() > max_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request header line too large",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
    }

    String::from_utf8(line)
        .map(Some)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn http_get_emits_play_event() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (event_tx, mut event_rx) = mpsc::channel::<TsDriverEvent>(16);
        let (_conn_tx, conn_rx) = mpsc::channel::<ConnCmd>(16);

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                TsConnectionId(1),
                peer_addr,
                event_tx,
                conn_rx,
                65_536,
            )
            .await;
        });

        // Connect and send HTTP GET
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /live/test.ts HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        // Should receive PlayRequested event
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            TsDriverEvent::PlayRequested {
                stream_key,
                transport,
                ..
            } => {
                assert_eq!(stream_key.namespace, "live");
                assert_eq!(stream_key.stream_path, "test");
                assert_eq!(transport, TsTransport::Http);
            }
            _ => panic!("expected PlayRequested"),
        }
    }

    #[tokio::test]
    async fn websocket_upgrade_emits_ws_play_event() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (event_tx, mut event_rx) = mpsc::channel::<TsDriverEvent>(16);
        let (_conn_tx, conn_rx) = mpsc::channel::<ConnCmd>(16);

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                TsConnectionId(2),
                peer_addr,
                event_tx,
                conn_rx,
                65_536,
            )
            .await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(
                b"GET /app/stream.ts HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Connection: Upgrade\r\n\
                  Upgrade: websocket\r\n\
                  Sec-WebSocket-Version: 13\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                  \r\n",
            )
            .await
            .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            TsDriverEvent::PlayRequested {
                stream_key,
                transport,
                ..
            } => {
                assert_eq!(stream_key.namespace, "app");
                assert_eq!(stream_key.stream_path, "stream");
                assert_eq!(transport, TsTransport::WebSocket);
            }
            _ => panic!("expected PlayRequested with WebSocket transport"),
        }
    }

    #[tokio::test]
    async fn invalid_path_closes_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (event_tx, mut event_rx) = mpsc::channel::<TsDriverEvent>(16);
        let (_conn_tx, conn_rx) = mpsc::channel::<ConnCmd>(16);

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                TsConnectionId(3),
                peer_addr,
                event_tx,
                conn_rx,
                65_536,
            )
            .await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /invalid.flv HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        // Should get ConnectionClosed (no PlayRequested)
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(
            matches!(event, TsDriverEvent::ConnectionClosed { .. }),
            "invalid path should close without play event"
        );
    }

    #[tokio::test]
    async fn websocket_sends_binary_frames() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (event_tx, _event_rx) = mpsc::channel::<TsDriverEvent>(16);
        let (conn_tx, conn_rx) = mpsc::channel::<ConnCmd>(16);

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                TsConnectionId(4),
                peer_addr,
                event_tx,
                conn_rx,
                65_536,
            )
            .await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(
                b"GET /live/test.ts HTTP/1.1\r\n\
                  Host: localhost\r\n\
                  Connection: Upgrade\r\n\
                  Upgrade: websocket\r\n\
                  Sec-WebSocket-Version: 13\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                  \r\n",
            )
            .await
            .unwrap();

        // Read the 101 response
        let mut resp_buf = vec![0u8; 4096];
        let n = client.read(&mut resp_buf).await.unwrap();
        let resp = String::from_utf8_lossy(&resp_buf[..n]);
        assert!(resp.contains("101"), "should get 101 upgrade");

        // Send TS data via the connection channel
        let ts_payload = Bytes::from_static(&[0x47, 0x00, 0x11, 0x10]); // TS sync byte
        conn_tx
            .send(ConnCmd::Send(ts_payload.clone()))
            .await
            .unwrap();

        // Small delay to let the write happen
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Read the WebSocket binary frame
        let mut frame_buf = vec![0u8; 256];
        let n = client.read(&mut frame_buf).await.unwrap();
        assert!(n >= 2, "should receive at least frame header");

        // Verify WebSocket binary frame header
        assert_eq!(frame_buf[0], 0x82, "FIN=1, opcode=binary");
        let payload_len = frame_buf[1] & 0x7F;
        assert_eq!(payload_len as usize, ts_payload.len());
        // Verify payload starts with TS sync byte
        assert_eq!(frame_buf[2], 0x47, "payload should start with TS sync byte");
    }

    #[tokio::test]
    async fn oversized_request_line_is_rejected_without_play_event() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (event_tx, mut event_rx) = mpsc::channel::<TsDriverEvent>(16);
        let (_conn_tx, conn_rx) = mpsc::channel::<ConnCmd>(16);

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                TsConnectionId(5),
                peer_addr,
                event_tx,
                conn_rx,
                1024,
            )
            .await;
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let oversized = format!("GET /{}", "a".repeat(70_000));
        client.write_all(oversized.as_bytes()).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, TsDriverEvent::ConnectionClosed { .. }));

        let mut buf = [0u8; 1];
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf)).await;
        assert!(
            read_result.is_ok(),
            "server should not keep reading oversized line"
        );
    }
}
