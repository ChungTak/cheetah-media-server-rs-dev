//! gRPC adapter for the signaling control plane.
//!
//! `cheetah-media-grpc-adapter` exposes a runtime-neutral handle while using
//! `tonic` internally. Public API types do not leak `tonic` or `tokio`
//! primitives.
//!
//! 信号控制面的 gRPC adapter。公开 API 不暴露 tonic/tokio 类型。

use std::net::SocketAddr;

use thiserror::Error;

pub mod health;

pub use health::{GrpcHealthHandle, GrpcServingStatus, HealthCategory};

/// TLS configuration for the gRPC adapter listener.
///
/// gRPC adapter 监听器 TLS 配置。
#[derive(Debug, Clone)]
pub struct GrpcTlsConfig {
    /// Server certificate PEM.
    pub server_cert_pem: String,
    /// Server private key PEM.
    pub server_key_pem: String,
    /// Trusted client CA certificate PEM. If empty, client certificates are
    /// not validated.
    pub client_ca_pem: String,
    /// Whether a client certificate is required when `client_ca_pem` is set.
    pub client_cert_required: bool,
}

impl GrpcTlsConfig {
    /// Create a TLS config from server cert/key. Client CA is optional.
    pub fn new(server_cert_pem: String, server_key_pem: String) -> Self {
        Self {
            server_cert_pem,
            server_key_pem,
            client_ca_pem: String::new(),
            client_cert_required: false,
        }
    }
}

/// gRPC per-message size limits.
///
/// gRPC 单条消息大小限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrpcMessageLimits {
    /// Maximum decoded (inbound) message size in bytes.
    pub max_decoding_message_size: usize,
    /// Maximum encoded (outbound) message size in bytes.
    pub max_encoding_message_size: usize,
}

impl Default for GrpcMessageLimits {
    fn default() -> Self {
        Self {
            max_decoding_message_size: 4 * 1024 * 1024,
            max_encoding_message_size: 4 * 1024 * 1024,
        }
    }
}

/// Configuration for the gRPC adapter listener.
///
/// gRPC adapter 监听器配置。
#[derive(Debug, Clone)]
pub struct GrpcAdapterConfig {
    /// Address the gRPC listener binds to. Port 0 may be used to request an
    /// ephemeral port.
    pub bind_addr: SocketAddr,
    /// Whether gRPC reflection is enabled. Disabled by default.
    ///
    /// 是否启用 gRPC reflection。默认关闭。
    pub enable_reflection: bool,
    /// Optional TLS configuration.
    pub tls: Option<GrpcTlsConfig>,
    /// gRPC per-message size limits.
    pub message_limits: GrpcMessageLimits,
}

impl GrpcAdapterConfig {
    /// Create a default config bound to the given address.
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            enable_reflection: false,
            tls: None,
            message_limits: GrpcMessageLimits::default(),
        }
    }
}

/// Errors returned by the gRPC adapter.
///
/// gRPC adapter 返回的错误。
#[derive(Debug, Error)]
pub enum GrpcAdapterError {
    /// Failed to bind the configured address.
    #[error("bind failed: {0}")]
    Bind(String),
    /// Failed to serve gRPC requests.
    #[error("serve failed: {0}")]
    Serve(String),
    /// Failed to configure TLS.
    #[error("invalid TLS configuration: {0}")]
    InvalidTls(String),
}

/// A running gRPC adapter.
///
/// 运行中的 gRPC adapter。
pub struct GrpcAdapter {
    bound_addr: SocketAddr,
    handle: Option<tokio::task::JoinHandle<Result<(), GrpcAdapterError>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl std::fmt::Debug for GrpcAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcAdapter")
            .field("bound_addr", &self.bound_addr)
            .finish_non_exhaustive()
    }
}

impl GrpcAdapter {
    /// Build a `tonic` server builder from the TLS config, if any.
    fn make_server_builder(
        tls: Option<&GrpcTlsConfig>,
    ) -> Result<tonic::transport::server::Server, GrpcAdapterError> {
        let Some(tls) = tls else {
            return Ok(tonic::transport::Server::builder());
        };

        let identity =
            tonic::transport::Identity::from_pem(&tls.server_cert_pem, &tls.server_key_pem);
        let mut tls_config = tonic::transport::ServerTlsConfig::new().identity(identity);

        if !tls.client_ca_pem.is_empty() {
            let ca = tonic::transport::Certificate::from_pem(&tls.client_ca_pem);
            tls_config = tls_config
                .client_ca_root(ca)
                .client_auth_optional(!tls.client_cert_required);
        }

        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .map_err(|e| GrpcAdapterError::InvalidTls(e.to_string()))
    }

    /// Start the gRPC listener and health service.
    ///
    /// Returns the running adapter and a handle for updating health status.
    ///
    /// 启动 gRPC 监听器与健康服务。
    pub async fn start(
        config: GrpcAdapterConfig,
    ) -> Result<(Self, GrpcHealthHandle), GrpcAdapterError> {
        let listener = tokio::net::TcpListener::bind(config.bind_addr)
            .await
            .map_err(|e| GrpcAdapterError::Bind(e.to_string()))?;
        let bound_addr = listener
            .local_addr()
            .map_err(|e| GrpcAdapterError::Bind(e.to_string()))?;

        let incoming = tonic::transport::server::TcpIncoming::from_listener(listener, true, None)
            .map_err(|e| GrpcAdapterError::Bind(e.to_string()))?;

        let (reporter, health_server) = tonic_health::server::health_reporter();
        let handle = GrpcHealthHandle::new(reporter);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let shutdown = async {
            let _ = shutdown_rx.await;
        };

        let mut server_builder = Self::make_server_builder(config.tls.as_ref())?;

        let serve = if config.enable_reflection {
            let reflection = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
                .build_v1()
                .map_err(|e| GrpcAdapterError::Serve(e.to_string()))?;

            server_builder
                .add_service(health_server)
                .add_service(reflection)
                .serve_with_incoming_shutdown(incoming, shutdown)
        } else {
            server_builder
                .add_service(health_server)
                .serve_with_incoming_shutdown(incoming, shutdown)
        };

        let handle_task = tokio::spawn(async move {
            serve
                .await
                .map_err(|e| GrpcAdapterError::Serve(e.to_string()))
        });

        Ok((
            Self {
                bound_addr,
                handle: Some(handle_task),
                shutdown_tx: Some(shutdown_tx),
            },
            handle,
        ))
    }

    /// Address the gRPC listener actually bound to.
    ///
    /// gRPC 监听器实际绑定的地址。
    pub fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    /// Request a graceful shutdown and wait for the server to stop.
    ///
    /// 请求优雅关闭并等待服务器停止。
    pub async fn stop(&mut self) -> Result<(), GrpcAdapterError> {
        let _ = self.shutdown_tx.take();
        if let Some(handle) = self.handle.take() {
            handle
                .await
                .map_err(|e| GrpcAdapterError::Serve(e.to_string()))??;
        }
        Ok(())
    }

    /// Rotate the listener to a new configuration.
    ///
    /// This validates the new TLS material (if any), stops the current gRPC
    /// listener, and starts a new one with `new_config`. Because the old listener
    /// is stopped before the new one is bound, rotation on a fixed `bind_addr`
    /// succeeds; the brief unavailability is the unavoidable cost of re-binding
    /// the same port. Parsing/validation errors are caught before the old listener
    /// is torn down, so a bad certificate does not cause an outage.
    ///
    /// 轮换监听器到新配置。先校验新 TLS 材料，再停止旧 listener 并以 `new_config`
    /// 启动新 listener。固定 `bind_addr` 也能成功轮换；解析/校验错误会在关闭旧
    /// listener 前返回，避免坏证书导致服务中断。
    pub async fn rotate(
        &mut self,
        new_config: GrpcAdapterConfig,
    ) -> Result<GrpcHealthHandle, GrpcAdapterError> {
        // Validate TLS material before tearing down the existing listener.
        Self::make_server_builder(new_config.tls.as_ref())?;
        self.stop().await?;

        let (new_adapter, new_health) = Self::start(new_config).await?;
        self.bound_addr = new_adapter.bound_addr;
        self.handle = new_adapter.handle;
        self.shutdown_tx = new_adapter.shutdown_tx;

        Ok(new_health)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_server_starts_and_stops() {
        let config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        let (mut adapter, mut health) = GrpcAdapter::start(config).await.unwrap();

        assert!(adapter.bound_addr().port() > 0);

        health.set_overall(GrpcServingStatus::Serving).await;
        health
            .set_service("cheetah.media.v1.Media", GrpcServingStatus::NotServing)
            .await;

        adapter.stop().await.unwrap();
    }

    #[test]
    fn config_clones_and_keeps_address() {
        let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
        let config = GrpcAdapterConfig::new(addr);
        let cloned = config.clone();
        assert_eq!(config.bind_addr, cloned.bind_addr);
        assert!(!config.enable_reflection);
    }

    #[tokio::test]
    async fn reflection_can_be_enabled() {
        let mut config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        config.enable_reflection = true;

        let (mut adapter, mut health) = GrpcAdapter::start(config).await.unwrap();
        health.set_overall(GrpcServingStatus::Serving).await;
        assert!(adapter.bound_addr().port() > 0);

        adapter.stop().await.unwrap();
    }

    #[tokio::test]
    async fn health_categories_are_reported() {
        let config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        let (mut adapter, mut health) = GrpcAdapter::start(config).await.unwrap();

        health.set_overall(GrpcServingStatus::Serving).await;
        health
            .set_category(HealthCategory::Store, GrpcServingStatus::Serving)
            .await;
        health
            .set_category(HealthCategory::Capacity, GrpcServingStatus::NotServing)
            .await;

        assert!(adapter.bound_addr().port() > 0);
        adapter.stop().await.unwrap();
    }

    /// Generate a self-signed CA and a leaf certificate signed by that CA.
    /// Returns `(ca_pem, cert_pem, key_pem)`.
    fn generate_test_certs() -> (String, String, String) {
        use rcgen::{
            BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
            KeyUsagePurpose, SanType,
        };

        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(vec!["ca.local".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        let server_key = KeyPair::generate().unwrap();
        let mut server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        server_params.is_ca = IsCa::NoCa;
        server_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        server_params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];

        let ca_issuer = Issuer::new(ca_params, ca_key);
        let server_cert = server_params.signed_by(&server_key, &ca_issuer).unwrap();

        (ca_cert.pem(), server_cert.pem(), server_key.serialize_pem())
    }

    #[tokio::test]
    async fn mtls_server_starts_and_stops() {
        let (ca_pem, server_cert, server_key) = generate_test_certs();

        let mut tls = GrpcTlsConfig::new(server_cert, server_key);
        tls.client_ca_pem = ca_pem;
        tls.client_cert_required = true;

        let mut config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        config.tls = Some(tls);

        let (mut adapter, mut health) = GrpcAdapter::start(config).await.unwrap();
        assert!(adapter.bound_addr().port() > 0);

        health.set_overall(GrpcServingStatus::Serving).await;
        adapter.stop().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_tls_pem_fails_to_start() {
        let mut tls = GrpcTlsConfig::new("not-a-cert".to_string(), "not-a-key".to_string());
        tls.client_ca_pem = "not-a-ca".to_string();

        let mut config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        config.tls = Some(tls);

        let result = GrpcAdapter::start(config).await;
        assert!(matches!(result, Err(GrpcAdapterError::InvalidTls(_))));
    }

    #[test]
    fn message_limits_default_to_four_megabytes() {
        let config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        assert_eq!(
            config.message_limits.max_decoding_message_size,
            4 * 1024 * 1024
        );
        assert_eq!(
            config.message_limits.max_encoding_message_size,
            4 * 1024 * 1024
        );
    }

    #[tokio::test]
    async fn rotate_restarts_with_new_tls_config() {
        let first_certs = generate_test_certs();
        let second_certs = generate_test_certs();

        let mut initial = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        initial.tls = Some(GrpcTlsConfig::new(first_certs.1, first_certs.2));

        let (mut adapter, mut health) = GrpcAdapter::start(initial).await.unwrap();
        health.set_overall(GrpcServingStatus::Serving).await;

        // Use the same bound port for the rotated config to exercise fixed-port
        // certificate rotation.
        let fixed_addr = adapter.bound_addr();
        let mut rotated = GrpcAdapterConfig::new(fixed_addr);
        rotated.tls = Some(GrpcTlsConfig::new(second_certs.1, second_certs.2));

        let _new_health = adapter.rotate(rotated).await.unwrap();

        assert_eq!(adapter.bound_addr(), fixed_addr);
        adapter.stop().await.unwrap();
    }
}
