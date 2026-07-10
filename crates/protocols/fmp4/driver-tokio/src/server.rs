//! fMP4 TCP server, HTTP request parsing, WebSocket framing, and connection management.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::{BufMut, Bytes, BytesMut};
use cheetah_fmp4_core::{Fmp4Transport, StreamKeyParts};
use cheetah_runtime_api::CancellationToken;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Unique connection identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fmp4ConnectionId(pub u64);

/// Driver configuration.
#[derive(Debug, Clone)]
pub struct Fmp4DriverConfig {
    /// `listen` field of type `SocketAddr`.
    /// `listen` 字段，类型为 `SocketAddr`.
    pub listen: SocketAddr,
    /// `write_queue_capacity` field of type `usize`.
    /// `write_queue_capacity` 字段，类型为 `usize`.
    pub write_queue_capacity: usize,
    /// `read_buffer_size` field of type `usize`.
    /// `read_buffer_size` 字段，类型为 `usize`.
    pub read_buffer_size: usize,
    /// `tls` field.
    /// `tls` 字段.
    pub tls: Option<Fmp4TlsConfig>,
}

/// TLS configuration.
#[derive(Debug, Clone)]
pub struct Fmp4TlsConfig {
    /// `listen` field of type `SocketAddr`.
    /// `listen` 字段，类型为 `SocketAddr`.
    pub listen: SocketAddr,
    /// `cert_path` field of type `String`.
    /// `cert_path` 字段，类型为 `String`.
    pub cert_path: String,
    /// `key_path` field of type `String`.
    /// `key_path` 字段，类型为 `String`.
    pub key_path: String,
    /// `handshake_timeout_ms` field of type `u64`.
    /// `handshake_timeout_ms` 字段，类型为 `u64`.
    pub handshake_timeout_ms: u64,
}

/// Events from driver to module.
#[derive(Debug, Clone)]
pub enum Fmp4DriverEvent {
    /// `PlayRequested` variant.
    /// `PlayRequested` 变体.
    PlayRequested {
        connection_id: Fmp4ConnectionId,
        stream_key: StreamKeyParts,
        transport: Fmp4Transport,
    },
    /// `ConnectionClosed` variant.
    /// `ConnectionClosed` 变体.
    ConnectionClosed { connection_id: Fmp4ConnectionId },
}

/// Commands from module to driver.
#[derive(Debug, Clone)]
pub enum Fmp4DriverCommand {
    /// `SendData` variant.
    /// `SendData` 变体.
    SendData {
        connection_id: Fmp4ConnectionId,
        data: Bytes,
    },
    /// `CloseConnection` variant.
    /// `CloseConnection` 变体.
    CloseConnection { connection_id: Fmp4ConnectionId },
}

/// Handle for receiving driver events.
pub struct Fmp4DriverHandle {
    /// `event_rx` field.
    /// `event_rx` 字段.
    event_rx: mpsc::Receiver<Fmp4DriverEvent>,
}

impl Fmp4DriverHandle {
    /// `recv_event` function.
    /// `recv_event` 函数.
    pub async fn recv_event(&mut self) -> Option<Fmp4DriverEvent> {
        self.event_rx.recv().await
    }
}

/// Sender for driver commands.
#[derive(Clone)]
pub struct Fmp4CommandSender {
    /// `cmd_tx` field.
    /// `cmd_tx` 字段.
    cmd_tx: mpsc::Sender<Fmp4DriverCommand>,
}

impl Fmp4CommandSender {
    /// `send` function.
    /// `send` 函数.
    pub async fn send(&self, cmd: Fmp4DriverCommand) {
        let _ = self.cmd_tx.send(cmd).await;
    }
}

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Start the fMP4 driver server. Returns a command sender and event handle.
pub fn start_server(
    config: Fmp4DriverConfig,
    cancel: CancellationToken,
) -> (Fmp4CommandSender, Fmp4DriverHandle) {
    let (event_tx, event_rx) = mpsc::channel(256);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Fmp4DriverCommand>(256);
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<Fmp4ConnectionId>();

    let handle = Fmp4DriverHandle { event_rx };
    let sender = Fmp4CommandSender { cmd_tx };

    tokio::spawn(async move {
        let listener = match TcpListener::bind(config.listen).await {
            Ok(l) => l,
            Err(e) => {
                warn!("fMP4 driver bind failed: {e}");
                return;
            }
        };
        debug!(addr = %config.listen, "fMP4 driver listening");

        // Optional TLS listener
        let tls_listener = if let Some(ref tls_cfg) = config.tls {
            let server_config =
                match crate::tls::load_tls_config(&tls_cfg.cert_path, &tls_cfg.key_path) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("fMP4 TLS config failed: {e}");
                        return;
                    }
                };
            let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server_config));
            match TcpListener::bind(tls_cfg.listen).await {
                Ok(l) => {
                    debug!(addr = %tls_cfg.listen, "fMP4 TLS listening");
                    Some((l, acceptor, tls_cfg.handshake_timeout_ms))
                }
                Err(e) => {
                    warn!("fMP4 TLS bind failed: {e}");
                    return;
                }
            }
        } else {
            None
        };

        // Per-connection data channels
        let mut connections: HashMap<Fmp4ConnectionId, mpsc::Sender<ConnCmd>> = HashMap::new();

        loop {
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
                    let (stream, _peer_addr) = match accept {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("fMP4 accept error: {e}");
                            continue;
                        }
                    };
                    let conn_id = Fmp4ConnectionId(NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed));
                    let (conn_tx, conn_rx) = mpsc::channel(config.write_queue_capacity);
                    connections.insert(conn_id, conn_tx);
                    let event_tx2 = event_tx.clone();
                    let cancel2 = cancel.clone();
                    let closed_tx2 = closed_tx.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, conn_id, conn_rx, event_tx2, closed_tx2, cancel2).await;
                    });
                }
                accept = tls_accept => {
                    let (stream, _peer_addr) = match accept {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("fMP4 TLS accept error: {e}");
                            continue;
                        }
                    };
                    let conn_id = Fmp4ConnectionId(NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed));
                    let (conn_tx, conn_rx) = mpsc::channel(config.write_queue_capacity);
                    connections.insert(conn_id, conn_tx);
                    let event_tx2 = event_tx.clone();
                    let cancel2 = cancel.clone();
                    let closed_tx2 = closed_tx.clone();
                    let (_, ref acceptor, timeout_ms) = tls_listener.as_ref().unwrap();
                    let acceptor = acceptor.clone();
                    let timeout_ms = *timeout_ms;
                    tokio::spawn(async move {
                        let tls_result = tokio::time::timeout(
                            std::time::Duration::from_millis(timeout_ms),
                            acceptor.accept(stream),
                        ).await;
                        match tls_result {
                            Ok(Ok(tls_stream)) => {
                                handle_tls_connection(tls_stream, conn_id, conn_rx, event_tx2, closed_tx2, cancel2).await;
                            }
                            _ => {
                                debug!("fMP4 TLS handshake failed/timeout");
                                let _ = closed_tx2.send(conn_id);
                            }
                        }
                    });
                }
                Some(connection_id) = closed_rx.recv() => {
                    connections.remove(&connection_id);
                }
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        Fmp4DriverCommand::SendData { connection_id, data } => {
                            if let Some(tx) = connections.get(&connection_id) {
                                if tx.try_send(ConnCmd::Send(data)).is_err() {
                                    // Queue full or closed — drop connection
                                    connections.remove(&connection_id);
                                }
                            }
                        }
                        Fmp4DriverCommand::CloseConnection { connection_id } => {
                            if let Some(tx) = connections.remove(&connection_id) {
                                let _ = tx.try_send(ConnCmd::Close);
                            }
                        }
                    }
                }
            }
        }
    });

    (sender, handle)
}

/// Internal command sent to a per-connection task.
enum ConnCmd {
    Send(Bytes),
    Close,
}

struct ConnectionCloseGuard {
    conn_id: Fmp4ConnectionId,
    closed_tx: mpsc::UnboundedSender<Fmp4ConnectionId>,
}

impl Drop for ConnectionCloseGuard {
    fn drop(&mut self) {
        let _ = self.closed_tx.send(self.conn_id);
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    conn_id: Fmp4ConnectionId,
    conn_rx: mpsc::Receiver<ConnCmd>,
    event_tx: mpsc::Sender<Fmp4DriverEvent>,
    closed_tx: mpsc::UnboundedSender<Fmp4ConnectionId>,
    cancel: CancellationToken,
) {
    let (reader, writer) = tokio::io::split(stream);
    handle_generic_connection(
        reader, writer, conn_id, conn_rx, event_tx, closed_tx, cancel,
    )
    .await;
}

async fn handle_generic_connection<R, W>(
    reader: R,
    mut writer: W,
    conn_id: Fmp4ConnectionId,
    mut conn_rx: mpsc::Receiver<ConnCmd>,
    event_tx: mpsc::Sender<Fmp4DriverEvent>,
    closed_tx: mpsc::UnboundedSender<Fmp4ConnectionId>,
    cancel: CancellationToken,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use cheetah_fmp4_core::{Fmp4Core, Fmp4CoreInput, Fmp4CoreOutput, HttpMethod, HttpRequestHead};
    use tokio::io::AsyncWriteExt;

    let mut buf_reader = BufReader::new(reader);
    let _close_guard = ConnectionCloseGuard {
        conn_id,
        closed_tx: closed_tx.clone(),
    };

    // Read HTTP request line (limit 8KB)
    let mut request_line = String::new();
    if buf_reader.read_line(&mut request_line).await.is_err() || request_line.len() > 8192 {
        return;
    }
    let parts: Vec<&str> = request_line.trim().splitn(3, ' ').collect();
    if parts.len() < 2 {
        return;
    }
    let method = match parts[0] {
        "GET" => HttpMethod::Get,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => HttpMethod::Other,
    };
    let target = parts[1].to_string();

    // Read headers (limit 64 headers, 8KB each)
    let mut headers = Vec::new();
    for _ in 0..64 {
        let mut line = String::new();
        if buf_reader.read_line(&mut line).await.is_err() || line.len() > 8192 {
            return;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim().split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let head = HttpRequestHead {
        method,
        method_raw: parts[0].to_string(),
        target,
        headers,
    };

    let mut core = Fmp4Core::new();
    let outputs = core.process(Fmp4CoreInput::RequestHead(head));

    let mut transport = Fmp4Transport::Http;
    for output in outputs {
        match output {
            Fmp4CoreOutput::SendHttpResponse(resp) => {
                let response = format_http_response(&resp);
                if writer.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
                if resp.status_code == 101 {
                    transport = Fmp4Transport::WebSocket;
                }
            }
            Fmp4CoreOutput::Event(cheetah_fmp4_core::Fmp4CoreEvent::PlayRequested {
                stream_key,
                transport: t,
            }) => {
                transport = t;
                let _ = event_tx
                    .send(Fmp4DriverEvent::PlayRequested {
                        connection_id: conn_id,
                        stream_key,
                        transport: t,
                    })
                    .await;
            }
            Fmp4CoreOutput::Close { .. } => return,
            _ => {}
        }
    }

    // Bidirectional loop: send data to client, read WS frames from client (if WebSocket)
    if transport == Fmp4Transport::WebSocket {
        const MAX_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
        let mut continuation_buf: Vec<u8> = Vec::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                cmd = conn_rx.recv() => {
                    match cmd {
                        Some(ConnCmd::Send(data)) => {
                            let encoded = encode_ws_binary_frame(&data);
                            if writer.write_all(&encoded).await.is_err() {
                                break;
                            }
                        }
                        Some(ConnCmd::Close) | None => break,
                    }
                }
                frame = read_ws_frame(&mut buf_reader, MAX_WS_MESSAGE_BYTES) => {
                    match frame {
                        Ok(WsFrame { fin, opcode, payload }) => {
                            match opcode {
                                0x00 => {
                                    // Continuation
                                    if continuation_buf.len() + payload.len() > MAX_WS_MESSAGE_BYTES {
                                        break; // Message too large
                                    }
                                    continuation_buf.extend_from_slice(&payload);
                                    if fin {
                                        continuation_buf.clear();
                                    }
                                }
                                0x01 => {
                                    // Text - close connection per spec
                                    break;
                                }
                                0x02 => {
                                    // Binary from client
                                    if !fin {
                                        continuation_buf = payload;
                                    }
                                    // We don't process client binary data for play sessions
                                }
                                0x08 => {
                                    // Close - send close frame back
                                    let close_frame = encode_ws_close_frame();
                                    let _ = writer.write_all(&close_frame).await;
                                    break;
                                }
                                0x09 => {
                                    // Ping - respond with pong
                                    let pong = encode_ws_pong_frame(&payload);
                                    if writer.write_all(&pong).await.is_err() {
                                        break;
                                    }
                                }
                                0x0A => {} // Pong - ignore
                                _ => break, // Unknown opcode or RSV bits set
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    } else {
        // HTTP mode: just send data, no reading needed
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                cmd = conn_rx.recv() => {
                    match cmd {
                        Some(ConnCmd::Send(data)) => {
                            let encoded = encode_http_chunk(&data);
                            if writer.write_all(&encoded).await.is_err() {
                                break;
                            }
                        }
                        Some(ConnCmd::Close) | None => break,
                    }
                }
            }
        }
    }

    let _ = event_tx
        .send(Fmp4DriverEvent::ConnectionClosed {
            connection_id: conn_id,
        })
        .await;
}

fn format_http_response(resp: &cheetah_fmp4_core::HttpResponseHead) -> String {
    let mut s = format!("HTTP/1.1 {} {}\r\n", resp.status_code, resp.reason);
    for (name, value) in &resp.headers {
        s.push_str(name);
        s.push_str(": ");
        s.push_str(value);
        s.push_str("\r\n");
    }
    s.push_str("\r\n");
    s
}

async fn handle_tls_connection(
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    conn_id: Fmp4ConnectionId,
    conn_rx: mpsc::Receiver<ConnCmd>,
    event_tx: mpsc::Sender<Fmp4DriverEvent>,
    closed_tx: mpsc::UnboundedSender<Fmp4ConnectionId>,
    cancel: CancellationToken,
) {
    let (reader, writer) = tokio::io::split(stream);
    handle_generic_connection(
        reader, writer, conn_id, conn_rx, event_tx, closed_tx, cancel,
    )
    .await;
}

/// A parsed WebSocket frame from the client.
struct WsFrame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// Read a single WebSocket frame from the client (expects masked frames).
async fn read_ws_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    max_message_bytes: usize,
) -> Result<WsFrame, ()> {
    use tokio::io::AsyncReadExt;

    let mut hdr = [0u8; 2];
    reader.read_exact(&mut hdr).await.map_err(|_| ())?;

    let fin = hdr[0] & 0x80 != 0;
    let rsv = hdr[0] & 0x70;
    if rsv != 0 {
        return Err(()); // RSV bits must be 0 unless extension negotiated
    }
    let opcode = hdr[0] & 0x0F;
    let masked = hdr[1] & 0x80 != 0;
    let mut payload_len = (hdr[1] & 0x7F) as u64;

    if payload_len == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext).await.map_err(|_| ())?;
        payload_len = u16::from_be_bytes(ext) as u64;
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext).await.map_err(|_| ())?;
        payload_len = u64::from_be_bytes(ext);
    }

    if payload_len > max_message_bytes as u64 {
        return Err(());
    }

    let mask_key = if masked {
        let mut m = [0u8; 4];
        reader.read_exact(&mut m).await.map_err(|_| ())?;
        Some(m)
    } else {
        // Client frames MUST be masked per RFC 6455
        return Err(());
    };

    let mut payload = vec![0u8; payload_len as usize];
    reader.read_exact(&mut payload).await.map_err(|_| ())?;

    if let Some(mask) = mask_key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }

    Ok(WsFrame {
        fin,
        opcode,
        payload,
    })
}

/// Encode a WebSocket close frame (server-to-client, no mask).
fn encode_ws_close_frame() -> Bytes {
    Bytes::from_static(&[0x88, 0x00]) // FIN + close opcode, 0 payload
}

/// Encode a WebSocket pong frame (server-to-client, no mask).
fn encode_ws_pong_frame(data: &[u8]) -> Bytes {
    let len = data.len();
    let mut buf = BytesMut::with_capacity(2 + len);
    buf.put_u8(0x8A); // FIN + pong opcode
    buf.put_u8(len as u8); // pong payload must be <= 125 bytes
    buf.extend_from_slice(&data[..len.min(125)]);
    buf.freeze()
}

/// Encode data as HTTP chunked transfer encoding.
fn encode_http_chunk(data: &[u8]) -> Bytes {
    // Format: {hex_size}\r\n{data}\r\n
    let hex = format!("{:x}\r\n", data.len());
    let mut buf = BytesMut::with_capacity(hex.len() + data.len() + 2);
    buf.extend_from_slice(hex.as_bytes());
    buf.extend_from_slice(data);
    buf.extend_from_slice(b"\r\n");
    buf.freeze()
}

/// Encode data as a WebSocket binary frame (server-to-client, no mask).
fn encode_ws_binary_frame(data: &[u8]) -> Bytes {
    let len = data.len();
    let header_len = if len < 126 {
        2
    } else if len <= 65535 {
        4
    } else {
        10
    };
    let mut buf = BytesMut::with_capacity(header_len + len);
    // FIN=1, opcode=0x02 (binary)
    buf.put_u8(0x82);
    if len < 126 {
        buf.put_u8(len as u8);
    } else if len <= 65535 {
        buf.put_u8(126);
        buf.put_u16(len as u16);
    } else {
        buf.put_u8(127);
        buf.put_u64(len as u64);
    }
    buf.extend_from_slice(data);
    buf.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_chunk_encoding() {
        let data = b"hello";
        let chunk = encode_http_chunk(data);
        assert_eq!(&chunk[..], b"5\r\nhello\r\n");
    }

    #[test]
    fn http_chunk_encoding_large() {
        let data = vec![0u8; 256];
        let chunk = encode_http_chunk(&data);
        assert!(chunk.starts_with(b"100\r\n"));
        assert!(chunk.ends_with(b"\r\n"));
        assert_eq!(chunk.len(), 5 + 256 + 2); // "100\r\n" + data + "\r\n"
    }

    #[test]
    fn ws_binary_frame_small() {
        let data = b"test";
        let frame = encode_ws_binary_frame(data);
        assert_eq!(frame[0], 0x82); // FIN + binary
        assert_eq!(frame[1], 4); // length
        assert_eq!(&frame[2..], b"test");
    }

    #[test]
    fn ws_binary_frame_medium() {
        let data = vec![0xAA; 200];
        let frame = encode_ws_binary_frame(&data);
        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 126); // extended 16-bit length
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 200);
        assert_eq!(frame.len(), 4 + 200);
    }

    #[test]
    fn ws_binary_frame_large() {
        let data = vec![0xBB; 70000];
        let frame = encode_ws_binary_frame(&data);
        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 127); // extended 64-bit length
        let len = u64::from_be_bytes([
            frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8], frame[9],
        ]);
        assert_eq!(len, 70000);
        assert_eq!(frame.len(), 10 + 70000);
    }

    #[tokio::test]
    async fn ws_read_rejects_unmasked_client_frame() {
        // Build a binary frame WITHOUT mask bit set (invalid from client)
        let payload = b"hello";
        let mut frame_bytes = vec![0x82u8, payload.len() as u8]; // FIN+binary, no mask bit
        frame_bytes.extend_from_slice(payload);

        let mut cursor = std::io::Cursor::new(frame_bytes);
        let result = read_ws_frame(&mut cursor, 4 * 1024 * 1024).await;
        assert!(result.is_err(), "unmasked client frame must be rejected");
    }

    #[tokio::test]
    async fn ws_read_rejects_oversized_frame() {
        // Build a masked binary frame claiming 8MB payload (exceeds 4MB limit)
        let mut frame_bytes = Vec::new();
        frame_bytes.push(0x82); // FIN + binary
        frame_bytes.push(0x80 | 127); // mask bit + 64-bit length
        frame_bytes.extend_from_slice(&(8_000_000u64).to_be_bytes());
        frame_bytes.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]); // mask key
                                                                  // Don't need actual payload — read_ws_frame should reject before reading it

        let mut cursor = std::io::Cursor::new(frame_bytes);
        let result = read_ws_frame(&mut cursor, 4 * 1024 * 1024).await;
        assert!(result.is_err(), "oversized frame must be rejected");
    }
}
