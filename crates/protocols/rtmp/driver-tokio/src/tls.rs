use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_runtime_api::AsyncTcpStream;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// TLS configuration for the RTMP server.
#[derive(Clone)]
pub struct RtmpTlsConfig {
    pub acceptor: TlsAcceptor,
}

impl RtmpTlsConfig {
    /// Load TLS config from PEM certificate and key files.
    pub fn from_pem_files(cert_path: &Path, key_path: &Path) -> io::Result<Self> {
        let cert_data = std::fs::read(cert_path).map_err(|e| {
            io::Error::new(e.kind(), format!("read cert {}: {e}", cert_path.display()))
        })?;
        let key_data = std::fs::read(key_path).map_err(|e| {
            io::Error::new(e.kind(), format!("read key {}: {e}", key_path.display()))
        })?;

        let certs = rustls_pemfile::certs(&mut cert_data.as_slice())
            .collect::<Result<Vec<CertificateDer<'static>>, _>>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse certs: {e}")))?;

        if certs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no certificates found in PEM file",
            ));
        }

        let key = rustls_pemfile::private_key(&mut key_data.as_slice())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("parse key: {e}")))?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "no private key found in PEM file",
                )
            })?;

        Self::from_der(certs, key)
    }

    /// Build TLS config from DER-encoded certificates and key.
    pub fn from_der(
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> io::Result<Self> {
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("tls config: {e}")))?;

        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(config)),
        })
    }
}

/// TLS client configuration for outbound RTMPS connections.
#[derive(Clone)]
pub struct RtmpTlsClientConfig {
    pub connector: TlsConnector,
}

impl RtmpTlsClientConfig {
    /// Create a client TLS config that trusts the system root certificates.
    pub fn with_native_roots() -> io::Result<Self> {
        let mut root_store = rustls::RootCertStore::empty();
        // Use webpki built-in roots as fallback
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    /// Create a client TLS config that trusts a specific CA certificate (for testing/internal).
    pub fn with_custom_ca(ca_cert_path: &Path) -> io::Result<Self> {
        let ca_data = std::fs::read(ca_cert_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("read CA cert {}: {e}", ca_cert_path.display()),
            )
        })?;

        let ca_certs = rustls_pemfile::certs(&mut ca_data.as_slice())
            .collect::<Result<Vec<CertificateDer<'static>>, _>>()
            .map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("parse CA certs: {e}"))
            })?;

        let mut root_store = rustls::RootCertStore::empty();
        for cert in ca_certs {
            root_store.add(cert).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("add CA cert: {e}"))
            })?;
        }

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    /// Create a client TLS config that skips certificate verification (DANGEROUS, testing only).
    pub fn insecure_no_verify() -> Self {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();

        Self {
            connector: TlsConnector::from(Arc::new(config)),
        }
    }
}

/// Wraps a TLS server stream to implement `AsyncTcpStream`.
pub struct TlsServerStream {
    inner: ServerTlsStream<TcpStream>,
    peer: SocketAddr,
}

impl TlsServerStream {
    /// Creates a new `TlsServerStream` instance.
    /// 创建新的 `TlsServerStream` 实例。
    pub fn new(inner: ServerTlsStream<TcpStream>, peer: SocketAddr) -> Self {
        Self { inner, peer }
    }
}

#[async_trait]
impl AsyncTcpStream for TlsServerStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.inner.read(buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

/// Wraps a TLS client stream to implement `AsyncTcpStream`.
pub struct TlsClientStream {
    inner: ClientTlsStream<TcpStream>,
    peer: SocketAddr,
}

impl TlsClientStream {
    /// Creates a new `TlsClientStream` instance.
    /// 创建新的 `TlsClientStream` 实例。
    pub fn new(inner: ClientTlsStream<TcpStream>, peer: SocketAddr) -> Self {
        Self { inner, peer }
    }
}

#[async_trait]
impl AsyncTcpStream for TlsClientStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.inner.read(buf).await
    }

    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.write_all(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.inner.shutdown().await
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }
}

/// Accept a TLS connection with timeout.
pub async fn accept_tls(
    tcp_stream: TcpStream,
    peer: SocketAddr,
    acceptor: &TlsAcceptor,
    timeout: std::time::Duration,
) -> io::Result<TlsServerStream> {
    let tls_stream = tokio::time::timeout(timeout, acceptor.accept(tcp_stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timeout"))?
        .map_err(|e| {
            io::Error::new(io::ErrorKind::ConnectionAborted, format!("TLS accept: {e}"))
        })?;
    Ok(TlsServerStream::new(tls_stream, peer))
}

/// Connect with TLS to a remote server.
pub async fn connect_tls(
    tcp_stream: TcpStream,
    peer: SocketAddr,
    server_name: ServerName<'static>,
    connector: &TlsConnector,
) -> io::Result<TlsClientStream> {
    let tls_stream = connector
        .connect(server_name, tcp_stream)
        .await
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                format!("TLS connect: {e}"),
            )
        })?;
    Ok(TlsClientStream::new(tls_stream, peer))
}

/// Dangerous: skip certificate verification. Only for testing.
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
