//! HTTP(S)/WS(S)-fMP4 pull client.
//!
//! HTTP(S)/WS(S) fMP4 拉取客户端。

use std::sync::Arc;

use bytes::Bytes;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Configuration for the pull client.
///
/// 拉取客户端配置。
#[derive(Debug, Clone)]
pub struct Fmp4PullClientConfig {
    pub url: String,
    pub read_buffer_size: usize,
    pub insecure_tls: bool,
}

/// Events from the pull client.
///
/// 拉取客户端事件。
#[derive(Debug)]
pub enum Fmp4PullEvent {
    Bytes(Bytes),
    Closed { reason: String },
}

/// Connect to a remote fMP4 source and return a receiver of pull events.
///
/// 连接远程 fMP4 源并返回拉取事件接收器。
pub async fn connect_pull(
    config: Fmp4PullClientConfig,
) -> Result<tokio::sync::mpsc::Receiver<Fmp4PullEvent>, String> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        if let Err(e) = run_pull(&config, &tx).await {
            let _ = tx.send(Fmp4PullEvent::Closed { reason: e }).await;
        }
    });
    Ok(rx)
}

/// Run the pull loop by scheme (HTTP(S) or WS(S)).
///
/// 按 scheme 运行拉取循环（HTTP(S) 或 WS(S)）。
async fn run_pull(
    config: &Fmp4PullClientConfig,
    tx: &tokio::sync::mpsc::Sender<Fmp4PullEvent>,
) -> Result<(), String> {
    let scheme = detect_scheme(&config.url)?;
    match scheme {
        Scheme::Http | Scheme::Https => run_http_pull(config, scheme, tx).await,
        Scheme::Ws | Scheme::Wss => run_ws_pull(config, scheme, tx).await,
    }
}

#[derive(Clone, Copy)]
/// URL scheme variants supported by the pull client.
///
/// 拉取客户端支持的 URL scheme 变体。
enum Scheme {
    Http,
    Https,
    Ws,
    Wss,
}

/// `Scheme` helpers.
///
/// `Scheme` 辅助。
impl Scheme {
    /// Return true if the scheme uses TLS.
    ///
    /// 返回该 scheme 是否使用 TLS。
    fn is_secure(self) -> bool {
        matches!(self, Scheme::Https | Scheme::Wss)
    }
}

/// Detect the URL scheme from a string prefix.
///
/// 从字符串前缀识别 URL scheme。
fn detect_scheme(url: &str) -> Result<Scheme, String> {
    if url.starts_with("http://") {
        Ok(Scheme::Http)
    } else if url.starts_with("https://") {
        Ok(Scheme::Https)
    } else if url.starts_with("ws://") {
        Ok(Scheme::Ws)
    } else if url.starts_with("wss://") {
        Ok(Scheme::Wss)
    } else {
        Err(format!("unsupported URL scheme: {url}"))
    }
}

/// Trait alias for async TCP/TLS streams used by the pull client.
///
/// 拉取客户端使用的异步 TCP/TLS 流 trait 别名。
trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
/// Blanket impl for types that satisfy `AsyncStream` bounds.
///
/// 为符合 `AsyncStream` 约束的类型提供统一实现。
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// Connect a TCP socket and optionally wrap it with TLS.
///
/// 连接 TCP 套接字，并可选择用 TLS 包装。
async fn connect_stream(
    host: &str,
    port: u16,
    secure: bool,
    insecure_tls: bool,
) -> Result<Box<dyn AsyncStream>, String> {
    let addr = format!("{host}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;
    if !secure {
        return Ok(Box::new(tcp));
    }
    let connector = tls_connector(insecure_tls);
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| format!("invalid server name {host}: {e}"))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("tls: {e}"))?;
    Ok(Box::new(tls))
}

/// Build a TLS connector with optional insecure certificate verification.
///
/// 构建可选不安全证书校验的 TLS 连接器。
fn tls_connector(insecure: bool) -> TlsConnector {
    let config = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    TlsConnector::from(Arc::new(config))
}

/// Pull fMP4 data over HTTP(S) with chunked or raw body handling.
///
/// 通过 HTTP(S) 拉取 fMP4 数据，支持分块或原始体。
async fn run_http_pull(
    config: &Fmp4PullClientConfig,
    scheme: Scheme,
    tx: &tokio::sync::mpsc::Sender<Fmp4PullEvent>,
) -> Result<(), String> {
    let (host, port, path) = parse_url(&config.url)?;
    let mut stream = connect_stream(&host, port, scheme.is_secure(), config.insecure_tls).await?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: keep-alive\r\nUser-Agent: cheetah-fmp4/1.0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Read response header
    let mut hdr = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1];
    loop {
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| format!("read hdr: {e}"))?;
        if n == 0 {
            return Err("closed during header".to_string());
        }
        hdr.push(tmp[0]);
        if hdr.len() > 32768 {
            return Err("header too large".to_string());
        }
        if hdr.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let hdr_str = String::from_utf8_lossy(&hdr);
    let status_line = hdr_str.lines().next().unwrap_or("");
    if !status_line.contains("200") && !status_line.contains("206") {
        return Err(format!("HTTP error: {status_line}"));
    }

    // Detect chunked transfer encoding
    let is_chunked = hdr_str.lines().any(|line| {
        if let Some((name, value)) = line.split_once(':') {
            name.trim().eq_ignore_ascii_case("Transfer-Encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        } else {
            false
        }
    });

    if is_chunked {
        read_chunked_body(&mut stream, tx, config.read_buffer_size).await
    } else {
        read_raw_body(&mut stream, tx, config.read_buffer_size).await
    }
}

/// Read HTTP chunked transfer encoding and emit chunks as events.
///
/// 读取 HTTP 分块传输编码并将每个 chunk 作为事件发出。
async fn read_chunked_body(
    stream: &mut Box<dyn AsyncStream>,
    tx: &tokio::sync::mpsc::Sender<Fmp4PullEvent>,
    _read_buffer_size: usize,
) -> Result<(), String> {
    // Read chunked transfer encoding: each chunk is "{hex_size}\r\n{data}\r\n"
    loop {
        // Read chunk size line
        let mut size_line = Vec::with_capacity(32);
        loop {
            let mut b = [0u8; 1];
            let n = stream
                .read(&mut b)
                .await
                .map_err(|e| format!("read chunk size: {e}"))?;
            if n == 0 {
                let _ = tx
                    .send(Fmp4PullEvent::Closed {
                        reason: "eof".to_string(),
                    })
                    .await;
                return Ok(());
            }
            size_line.push(b[0]);
            if size_line.len() > 64 {
                return Err("chunk size line too long".to_string());
            }
            if size_line.ends_with(b"\r\n") {
                break;
            }
        }

        let size_str = String::from_utf8_lossy(&size_line[..size_line.len() - 2]);
        let chunk_size =
            usize::from_str_radix(size_str.trim(), 16).map_err(|e| format!("chunk size: {e}"))?;

        if chunk_size == 0 {
            // Terminal chunk
            let _ = tx
                .send(Fmp4PullEvent::Closed {
                    reason: "eof".to_string(),
                })
                .await;
            return Ok(());
        }

        // Read chunk data
        let mut remaining = chunk_size;
        while remaining > 0 {
            let to_read = remaining.min(65536);
            let mut buf = vec![0u8; to_read];
            let n = stream
                .read(&mut buf[..to_read])
                .await
                .map_err(|e| format!("read chunk: {e}"))?;
            if n == 0 {
                let _ = tx
                    .send(Fmp4PullEvent::Closed {
                        reason: "eof".to_string(),
                    })
                    .await;
                return Ok(());
            }
            buf.truncate(n);
            remaining -= n;
            if tx
                .send(Fmp4PullEvent::Bytes(Bytes::from(buf)))
                .await
                .is_err()
            {
                return Ok(());
            }
        }

        // Read trailing \r\n after chunk data
        let mut crlf = [0u8; 2];
        if stream.read_exact(&mut crlf).await.is_err() {
            let _ = tx
                .send(Fmp4PullEvent::Closed {
                    reason: "eof".to_string(),
                })
                .await;
            return Ok(());
        }
    }
}

/// Read the HTTP body as a stream of fixed-size buffers.
///
/// 以固定大小缓冲区流式读取 HTTP 体。
async fn read_raw_body(
    stream: &mut Box<dyn AsyncStream>,
    tx: &tokio::sync::mpsc::Sender<Fmp4PullEvent>,
    read_buffer_size: usize,
) -> Result<(), String> {
    let mut buf = vec![0u8; read_buffer_size.max(4096)];
    loop {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            let _ = tx
                .send(Fmp4PullEvent::Closed {
                    reason: "eof".to_string(),
                })
                .await;
            return Ok(());
        }
        if tx
            .send(Fmp4PullEvent::Bytes(Bytes::copy_from_slice(&buf[..n])))
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

/// Pull fMP4 data over WebSocket(S) with RFC 6455 frame handling.
///
/// 通过 WebSocket(S) 拉取 fMP4 数据，处理 RFC 6455 帧。
async fn run_ws_pull(
    config: &Fmp4PullClientConfig,
    scheme: Scheme,
    tx: &tokio::sync::mpsc::Sender<Fmp4PullEvent>,
) -> Result<(), String> {
    let (host, port, path) = parse_url(&config.url)?;
    let mut stream = connect_stream(&host, port, scheme.is_secure(), config.insecure_tls).await?;

    // WebSocket upgrade
    let mut key_bytes = [0u8; 16];
    getrandom::getrandom(&mut key_bytes).unwrap_or_default();
    let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key_bytes);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Read 101 response
    let mut hdr = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1];
    loop {
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| format!("read hdr: {e}"))?;
        if n == 0 {
            return Err("closed during ws upgrade".to_string());
        }
        hdr.push(tmp[0]);
        if hdr.len() > 32768 {
            return Err("header too large".to_string());
        }
        if hdr.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let hdr_str = String::from_utf8_lossy(&hdr);
    if !hdr_str.contains("101") {
        return Err(format!(
            "WS upgrade failed: {}",
            hdr_str.lines().next().unwrap_or("")
        ));
    }

    // Validate Sec-WebSocket-Accept
    let expected_accept = compute_ws_accept_key(&key);
    let accept_valid = hdr_str.lines().any(|line| {
        if let Some((name, value)) = line.split_once(':') {
            name.trim().eq_ignore_ascii_case("Sec-WebSocket-Accept")
                && value.trim() == expected_accept
        } else {
            false
        }
    });
    if !accept_valid {
        return Err("invalid Sec-WebSocket-Accept".to_string());
    }

    // Read WebSocket frames with continuation reassembly
    const MAX_WS_MESSAGE: u64 = 4 * 1024 * 1024;
    let mut continuation_buf: Vec<u8> = Vec::new();

    loop {
        let mut frame_hdr = [0u8; 2];
        if stream.read_exact(&mut frame_hdr).await.is_err() {
            let _ = tx
                .send(Fmp4PullEvent::Closed {
                    reason: "eof".to_string(),
                })
                .await;
            return Ok(());
        }
        let fin = frame_hdr[0] & 0x80 != 0;
        let opcode = frame_hdr[0] & 0x0F;
        let masked = frame_hdr[1] & 0x80 != 0;
        let mut payload_len = (frame_hdr[1] & 0x7F) as u64;

        if payload_len == 126 {
            let mut ext = [0u8; 2];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("read: {e}"))?;
            payload_len = u16::from_be_bytes(ext) as u64;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("read: {e}"))?;
            payload_len = u64::from_be_bytes(ext);
        }

        if payload_len > MAX_WS_MESSAGE {
            return Err("ws frame too large".to_string());
        }

        let mask_key = if masked {
            let mut m = [0u8; 4];
            stream
                .read_exact(&mut m)
                .await
                .map_err(|e| format!("read mask: {e}"))?;
            Some(m)
        } else {
            None
        };

        let mut payload = vec![0u8; payload_len as usize];
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| format!("read payload: {e}"))?;

        if let Some(mask) = mask_key {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }

        match opcode {
            0x00 => {
                // Continuation frame
                if continuation_buf.len() + payload.len() > MAX_WS_MESSAGE as usize {
                    return Err("ws message too large".to_string());
                }
                continuation_buf.extend_from_slice(&payload);
                if fin {
                    let msg = std::mem::take(&mut continuation_buf);
                    if tx
                        .send(Fmp4PullEvent::Bytes(Bytes::from(msg)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
            }
            0x02 => {
                // Binary frame
                if fin {
                    if tx
                        .send(Fmp4PullEvent::Bytes(Bytes::from(payload)))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                } else {
                    // Start of fragmented message
                    continuation_buf = payload;
                }
            }
            0x08 => {
                // Close
                let _ = tx
                    .send(Fmp4PullEvent::Closed {
                        reason: "ws close".to_string(),
                    })
                    .await;
                // Send close back (masked)
                let close_frame = build_ws_client_frame(0x08, &[]);
                let _ = stream.write_all(&close_frame).await;
                return Ok(());
            }
            0x09 => {
                // Ping - send pong (masked from client)
                let pong = build_ws_client_frame(0x0A, &payload);
                let _ = stream.write_all(&pong).await;
            }
            0x0A => {} // Pong - ignore
            _ => {}    // Ignore text and unknown opcodes
        }
    }
}

/// Build a masked WebSocket client frame with the given opcode and payload.
///
/// 用指定 opcode 与负载构建带 mask 的 WebSocket 客户端帧。
fn build_ws_client_frame(opcode: u8, data: &[u8]) -> Vec<u8> {
    let len = data.len();
    let mut mask_key = [0u8; 4];
    getrandom::getrandom(&mut mask_key).unwrap_or_default();
    let mut frame = Vec::with_capacity(14 + len);
    frame.push(0x80 | opcode); // FIN + opcode
    if len < 126 {
        frame.push(0x80 | len as u8); // MASK bit set
    } else if len <= 65535 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(&mask_key);
    for (i, &b) in data.iter().enumerate() {
        frame.push(b ^ mask_key[i % 4]);
    }
    frame
}

/// Compute the RFC 6455 `Sec-WebSocket-Accept` key.
///
/// 计算 RFC 6455 `Sec-WebSocket-Accept` key。
fn compute_ws_accept_key(client_key: &str) -> String {
    use sha1::Digest;
    const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut sha1 = sha1::Sha1::new();
    sha1.update(client_key.trim().as_bytes());
    sha1.update(MAGIC.as_bytes());
    let digest = sha1.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, digest)
}

/// Parse an http/https/ws/wss URL into (host, port, path).
///
/// 将 http/https/ws/wss URL 解析为 (host, port, path)。
fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;

    let scheme = parsed.scheme();
    let default_port = match scheme {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        other => return Err(format!("unsupported url scheme: {other}")),
    };

    let port = parsed.port().unwrap_or(default_port);
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();

    let path = if let Some(query) = parsed.query() {
        format!("{}?{}", parsed.path(), query)
    } else {
        parsed.path().to_string()
    };

    Ok((host, port, path))
}

/// Insecure TLS certificate verifier that accepts any server certificate.
///
/// 不安全的 TLS 证书校验器，接受任何服务器证书。
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_url() {
        let (host, port, path) = parse_url("http://example.com:8083/live/test.mp4").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8083);
        assert_eq!(path, "/live/test.mp4");
    }

    #[test]
    fn parse_https_url_default_port() {
        let (host, port, path) = parse_url("https://example.com/live/test.mp4").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/live/test.mp4");
    }

    #[test]
    fn parse_ws_url() {
        let (host, port, path) = parse_url("ws://127.0.0.1:8083/live/cam.mp4").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 8083);
        assert_eq!(path, "/live/cam.mp4");
    }

    #[test]
    fn parse_url_ipv6_host_and_port() {
        let (host, port, path) = parse_url("http://[::1]:8083/live/test.mp4").unwrap();
        assert_eq!(host, "[::1]");
        assert_eq!(port, 8083);
        assert_eq!(path, "/live/test.mp4");
    }

    #[test]
    fn parse_url_ipv6_default_port() {
        let (host, port, path) = parse_url("https://[::1]/live/test.mp4").unwrap();
        assert_eq!(host, "[::1]");
        assert_eq!(port, 443);
        assert_eq!(path, "/live/test.mp4");
    }

    #[test]
    fn parse_url_preserves_query_and_ignores_userinfo() {
        let (host, port, path) =
            parse_url("http://user:pass@example.com:8083/live/test.mp4?token=secret").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8083);
        assert_eq!(path, "/live/test.mp4?token=secret");
    }

    #[test]
    fn parse_url_rejects_invalid_port() {
        assert!(parse_url("http://example.com:abc/live.mp4").is_err());
    }

    #[test]
    fn parse_url_rejects_unsupported_scheme() {
        assert!(parse_url("ftp://example.com/live.mp4").is_err());
    }

    #[test]
    fn detect_schemes() {
        assert!(matches!(detect_scheme("http://x"), Ok(Scheme::Http)));
        assert!(matches!(detect_scheme("https://x"), Ok(Scheme::Https)));
        assert!(matches!(detect_scheme("ws://x"), Ok(Scheme::Ws)));
        assert!(matches!(detect_scheme("wss://x"), Ok(Scheme::Wss)));
        assert!(detect_scheme("ftp://x").is_err());
    }
}
