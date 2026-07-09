//! HTTP(S)/WS(S)-TS pull client.

use std::sync::Arc;

use bytes::Bytes;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Configuration for the pull client.
#[derive(Debug, Clone)]
pub struct TsPullClientConfig {
    pub url: String,
    pub read_buffer_size: usize,
    pub insecure_tls: bool,
}

/// Events from the pull client.
#[derive(Debug)]
pub enum TsPullEvent {
    /// Received TS bytes from remote.
    Bytes(Bytes),
    /// Connection closed.
    Closed { reason: String },
}

/// TS pull client (connects to remote HTTP/WS TS source).
pub struct TsPullClient;

impl TsPullClient {
    /// Connect and start reading. Returns a stream of events.
    pub async fn connect(
        config: TsPullClientConfig,
    ) -> Result<tokio::sync::mpsc::Receiver<TsPullEvent>, String> {
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            if let Err(e) = run_pull(&config, &tx).await {
                let _ = tx.send(TsPullEvent::Closed { reason: e }).await;
            }
        });

        Ok(rx)
    }
}

async fn run_pull(
    config: &TsPullClientConfig,
    tx: &tokio::sync::mpsc::Sender<TsPullEvent>,
) -> Result<(), String> {
    let scheme = detect_scheme(&config.url)?;
    match scheme {
        PullScheme::Http | PullScheme::Https => run_http_pull(config, scheme, tx).await,
        PullScheme::Ws | PullScheme::Wss => run_ws_pull(config, scheme, tx).await,
    }
}

#[derive(Debug, Clone, Copy)]
enum PullScheme {
    Http,
    Https,
    Ws,
    Wss,
}

impl PullScheme {
    fn is_secure(self) -> bool {
        matches!(self, PullScheme::Https | PullScheme::Wss)
    }
}

trait AsyncPullStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> AsyncPullStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

async fn connect_stream(
    host: &str,
    port: u16,
    secure: bool,
    insecure_tls: bool,
) -> Result<Box<dyn AsyncPullStream>, String> {
    let addr = format!("{host}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;

    if !secure {
        return Ok(Box::new(tcp));
    }

    let connector = tls_connector(insecure_tls);
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| format!("invalid tls server name {host}: {e}"))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("tls connect {addr}: {e}"))?;
    Ok(Box::new(tls))
}

fn tls_connector(insecure_tls: bool) -> TlsConnector {
    let config = if insecure_tls {
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

fn detect_scheme(url: &str) -> Result<PullScheme, String> {
    if url.starts_with("http://") {
        Ok(PullScheme::Http)
    } else if url.starts_with("https://") {
        Ok(PullScheme::Https)
    } else if url.starts_with("ws://") {
        Ok(PullScheme::Ws)
    } else if url.starts_with("wss://") {
        Ok(PullScheme::Wss)
    } else {
        Err(format!("unsupported URL scheme: {url}"))
    }
}

async fn run_http_pull(
    config: &TsPullClientConfig,
    scheme: PullScheme,
    tx: &tokio::sync::mpsc::Sender<TsPullEvent>,
) -> Result<(), String> {
    let (host, port, path) = parse_url(&config.url)?;
    let mut stream = connect_stream(&host, port, scheme.is_secure(), config.insecure_tls).await?;

    // Send HTTP GET
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: keep-alive\r\nUser-Agent: cheetah-ts/1.0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Read response header
    let mut header_buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1];
    loop {
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| format!("read header: {e}"))?;
        if n == 0 {
            return Err("connection closed during header".to_string());
        }
        header_buf.push(tmp[0]);
        if header_buf.len() > 32768 {
            return Err("header too large".to_string());
        }
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = String::from_utf8_lossy(&header_buf);

    // Accept 200 and 206 status codes
    let status_line = header_str.lines().next().unwrap_or("");
    let status_ok = status_line.contains("200") || status_line.contains("206");
    if !status_ok {
        return Err(format!("HTTP error: {status_line}"));
    }

    // Loose Content-Type validation: warn but don't reject
    // Accept video/mp2t, video/mpeg, application/octet-stream, or anything else
    // (ZLMediaKit compat: only log warning for unexpected types)

    // Read body bytes continuously
    let mut received_body = false;
    let mut buf = vec![0u8; config.read_buffer_size.max(4096)];
    loop {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            if !received_body {
                return Err("empty body: no TS data received".to_string());
            }
            let _ = tx
                .send(TsPullEvent::Closed {
                    reason: "EOF".to_string(),
                })
                .await;
            return Ok(());
        }
        received_body = true;
        if tx
            .send(TsPullEvent::Bytes(Bytes::copy_from_slice(&buf[..n])))
            .await
            .is_err()
        {
            return Ok(());
        }
    }
}

async fn run_ws_pull(
    config: &TsPullClientConfig,
    scheme: PullScheme,
    tx: &tokio::sync::mpsc::Sender<TsPullEvent>,
) -> Result<(), String> {
    let (host, port, path) = parse_url(&config.url)?;
    let mut stream = connect_stream(&host, port, scheme.is_secure(), config.insecure_tls).await?;

    // Generate a random WebSocket key (16 bytes base64-encoded)
    let ws_key = generate_ws_key();

    // Send WebSocket upgrade request
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: Upgrade\r\n\
         Upgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: {ws_key}\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Read 101 response
    let mut header_buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1];
    loop {
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| format!("read ws header: {e}"))?;
        if n == 0 {
            return Err("connection closed during WS handshake".to_string());
        }
        header_buf.push(tmp[0]);
        if header_buf.len() > 32768 {
            return Err("WS header too large".to_string());
        }
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = String::from_utf8_lossy(&header_buf);
    if !header_str.contains("101") {
        return Err(format!(
            "WS upgrade failed: {}",
            header_str.lines().next().unwrap_or("")
        ));
    }

    // Validate Sec-WebSocket-Accept
    let expected_accept = compute_ws_accept(&ws_key);
    let has_valid_accept = header_str.lines().any(|line| {
        let lower = line.to_lowercase();
        if lower.starts_with("sec-websocket-accept:") {
            let val = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
            val == expected_accept
        } else {
            false
        }
    });
    if !has_valid_accept {
        return Err("invalid Sec-WebSocket-Accept".to_string());
    }

    // Read WebSocket frames
    let mut received_body = false;
    loop {
        // Read frame header (2 bytes minimum)
        let mut frame_header = [0u8; 2];
        if stream.read_exact(&mut frame_header).await.is_err() {
            if !received_body {
                return Err("empty body: no WS frames received".to_string());
            }
            let _ = tx
                .send(TsPullEvent::Closed {
                    reason: "EOF".to_string(),
                })
                .await;
            return Ok(());
        }

        let opcode = frame_header[0] & 0x0F;
        let masked = frame_header[1] & 0x80 != 0;
        let mut payload_len = (frame_header[1] & 0x7F) as u64;

        if payload_len == 126 {
            let mut ext = [0u8; 2];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("read ws len16: {e}"))?;
            payload_len = u16::from_be_bytes(ext) as u64;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            stream
                .read_exact(&mut ext)
                .await
                .map_err(|e| format!("read ws len64: {e}"))?;
            payload_len = u64::from_be_bytes(ext);
        }

        // Read mask if present
        let mask = if masked {
            let mut m = [0u8; 4];
            stream
                .read_exact(&mut m)
                .await
                .map_err(|e| format!("read ws mask: {e}"))?;
            Some(m)
        } else {
            None
        };

        // Read payload
        if payload_len > 16 * 1024 * 1024 {
            return Err("WS frame too large".to_string());
        }
        let mut payload = vec![0u8; payload_len as usize];
        if !payload.is_empty() {
            stream
                .read_exact(&mut payload)
                .await
                .map_err(|e| format!("read ws payload: {e}"))?;
        }

        // Unmask if needed
        if let Some(m) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= m[i % 4];
            }
        }

        match opcode {
            0x02 => {
                // Binary frame — TS data
                received_body = true;
                if tx
                    .send(TsPullEvent::Bytes(Bytes::from(payload)))
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
            0x08 => {
                // Close frame
                if !received_body {
                    return Err("empty body: closed before data".to_string());
                }
                let _ = tx
                    .send(TsPullEvent::Closed {
                        reason: "WS close".to_string(),
                    })
                    .await;
                return Ok(());
            }
            0x09 => {
                // Ping — respond with pong
                let pong = masked_ws_frame(0x0A, &payload)?;
                let _ = stream.write_all(&pong).await;
            }
            0x0A => {} // Pong — ignore
            0x01 => {} // Text — ignore (diagnostic only)
            _ => {}    // Unknown — ignore
        }
    }
}

fn masked_ws_frame(opcode: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() > 125 {
        return Err("control frame payload too large".to_string());
    }

    let mask = websocket_mask();
    let mut frame = Vec::with_capacity(6 + payload.len());
    frame.push(0x80 | (opcode & 0x0F));
    frame.push(0x80 | payload.len() as u8);
    frame.extend_from_slice(&mask);
    for (idx, byte) in payload.iter().enumerate() {
        frame.push(*byte ^ mask[idx % 4]);
    }
    Ok(frame)
}

fn websocket_mask() -> [u8; 4] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    [
        (nanos & 0xFF) as u8,
        ((nanos >> 8) & 0xFF) as u8,
        ((nanos >> 16) & 0xFF) as u8,
        ((nanos >> 24) & 0xFF) as u8,
    ]
}

fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    // Strip scheme
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .or_else(|| url.strip_prefix("ws://"))
        .or_else(|| url.strip_prefix("wss://"))
        .ok_or_else(|| format!("unsupported URL scheme: {url}"))?;

    let default_port = if url.starts_with("https://") || url.starts_with("wss://") {
        443
    } else {
        80
    };

    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.find(':') {
        Some(i) => (
            &host_port[..i],
            host_port[i + 1..]
                .parse::<u16>()
                .map_err(|e| e.to_string())?,
        ),
        None => (host_port, default_port),
    };
    Ok((host.to_string(), port, path.to_string()))
}

/// Generate a random 16-byte WebSocket key, base64-encoded.
fn generate_ws_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Simple pseudo-random key using time + counter (no crypto needed for WS key)
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((seed >> (i * 4)) & 0xFF) as u8 ^ (i as u8).wrapping_mul(37);
    }
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Compute expected Sec-WebSocket-Accept from key.
fn compute_ws_accept(key: &str) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-5AB0DC85B11B");
    let hash = hasher.finalize();
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_url() {
        let (host, port, path) = parse_url("http://example.com:8080/live/test.ts").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
        assert_eq!(path, "/live/test.ts");
    }

    #[test]
    fn parse_http_url_default_port() {
        let (host, port, path) = parse_url("http://example.com/live/test.ts").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/live/test.ts");
    }

    #[test]
    fn parse_ws_url() {
        let (host, port, path) = parse_url("ws://localhost:9090/app/stream.live.ts").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 9090);
        assert_eq!(path, "/app/stream.live.ts");
    }

    #[test]
    fn parse_wss_url_default_port() {
        let (host, port, path) = parse_url("wss://secure.example.com/live/test.ts").unwrap();
        assert_eq!(host, "secure.example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/live/test.ts");
    }

    #[test]
    fn detect_scheme_all_variants() {
        assert!(matches!(detect_scheme("http://x"), Ok(PullScheme::Http)));
        assert!(matches!(detect_scheme("https://x"), Ok(PullScheme::Https)));
        assert!(matches!(detect_scheme("ws://x"), Ok(PullScheme::Ws)));
        assert!(matches!(detect_scheme("wss://x"), Ok(PullScheme::Wss)));
        assert!(detect_scheme("ftp://x").is_err());
    }

    #[tokio::test]
    async fn http_pull_accepts_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\n\r\n\x47\x00\x11\x10")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let config = TsPullClientConfig {
            url: format!("http://127.0.0.1:{}/live/test.ts", addr.port()),
            read_buffer_size: 4096,
            insecure_tls: false,
        };
        let mut rx = TsPullClient::connect(config).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, TsPullEvent::Bytes(_)));
    }

    #[tokio::test]
    async fn http_pull_accepts_206() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp2t\r\n\r\n\x47\x00",
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let config = TsPullClientConfig {
            url: format!("http://127.0.0.1:{}/live/test.ts", addr.port()),
            read_buffer_size: 4096,
            insecure_tls: false,
        };
        let mut rx = TsPullClient::connect(config).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, TsPullEvent::Bytes(_)));
    }

    #[tokio::test]
    async fn http_pull_empty_body_fails() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await.unwrap();
            // Send 200 but close immediately with no body
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\n\r\n")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let config = TsPullClientConfig {
            url: format!("http://127.0.0.1:{}/live/test.ts", addr.port()),
            read_buffer_size: 4096,
            insecure_tls: false,
        };
        let mut rx = TsPullClient::connect(config).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            TsPullEvent::Closed { reason } => {
                assert!(
                    reason.contains("empty body"),
                    "should report empty body: {reason}"
                );
            }
            _ => panic!("expected Closed event for empty body"),
        }
    }

    #[tokio::test]
    async fn https_pull_uses_tls_when_scheme_is_secure() {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;

        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("generate self-signed cert");
        let cert_der = CertificateDer::from(cert.cert.der().to_vec());
        let key_der = PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .expect("server tls config");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.expect("tls handshake");
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(
                String::from_utf8_lossy(&buf[..n]).starts_with("GET /live/test.ts HTTP/1.1"),
                "expected HTTP request inside TLS"
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\n\r\n\x47\x00\x11\x10")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        });

        let config = TsPullClientConfig {
            url: format!("https://127.0.0.1:{}/live/test.ts", addr.port()),
            read_buffer_size: 4096,
            insecure_tls: true,
        };
        let mut rx = TsPullClient::connect(config).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, TsPullEvent::Bytes(_)));
    }

    #[tokio::test]
    async fn ws_pull_masks_pong_response_to_server_ping() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).await.unwrap();
                req.push(byte[0]);
                if req.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&req);
            let key = request
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("Sec-WebSocket-Key"))
                        .map(|(_, value)| value.trim().to_string())
                })
                .expect("client sends websocket key");
            let accept = compute_ws_accept(&key);
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {accept}\r\n\
                 \r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();

            stream.write_all(&[0x89, 0x02, b'o', b'k']).await.unwrap();

            let mut header = [0u8; 2];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0], 0x8A);
            assert_ne!(header[1] & 0x80, 0, "client pong frames must be masked");
            assert_eq!(header[1] & 0x7F, 2);

            let mut mask = [0u8; 4];
            stream.read_exact(&mut mask).await.unwrap();
            let mut payload = [0u8; 2];
            stream.read_exact(&mut payload).await.unwrap();
            for (idx, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[idx % 4];
            }
            assert_eq!(&payload, b"ok");
        });

        let config = TsPullClientConfig {
            url: format!("ws://127.0.0.1:{}/live/test.ts", addr.port()),
            read_buffer_size: 4096,
            insecure_tls: false,
        };
        let mut rx = TsPullClient::connect(config).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, TsPullEvent::Closed { .. }));
        server.await.unwrap();
    }
}
