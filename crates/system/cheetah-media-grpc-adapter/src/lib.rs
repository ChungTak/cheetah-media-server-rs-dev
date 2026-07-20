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
}

impl GrpcAdapterConfig {
    /// Create a default config bound to the given address.
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            enable_reflection: false,
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
}

/// A running gRPC adapter.
///
/// 运行中的 gRPC adapter。
pub struct GrpcAdapter {
    bound_addr: SocketAddr,
    handle: tokio::task::JoinHandle<Result<(), GrpcAdapterError>>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl std::fmt::Debug for GrpcAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcAdapter")
            .field("bound_addr", &self.bound_addr)
            .finish_non_exhaustive()
    }
}

impl GrpcAdapter {
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

        let serve = if config.enable_reflection {
            let reflection = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
                .build_v1()
                .map_err(|e| GrpcAdapterError::Serve(e.to_string()))?;

            tonic::transport::Server::builder()
                .add_service(health_server)
                .add_service(reflection)
                .serve_with_incoming_shutdown(incoming, shutdown)
        } else {
            tonic::transport::Server::builder()
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
                handle: handle_task,
                shutdown_tx,
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
    pub async fn stop(self) -> Result<(), GrpcAdapterError> {
        let _ = self.shutdown_tx.send(());
        self.handle
            .await
            .map_err(|e| GrpcAdapterError::Serve(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_server_starts_and_stops() {
        let config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        let (adapter, mut health) = GrpcAdapter::start(config).await.unwrap();

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

        let (adapter, mut health) = GrpcAdapter::start(config).await.unwrap();
        health.set_overall(GrpcServingStatus::Serving).await;
        assert!(adapter.bound_addr().port() > 0);

        adapter.stop().await.unwrap();
    }

    #[tokio::test]
    async fn health_categories_are_reported() {
        let config = GrpcAdapterConfig::new("127.0.0.1:0".parse().unwrap());
        let (adapter, mut health) = GrpcAdapter::start(config).await.unwrap();

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
}
