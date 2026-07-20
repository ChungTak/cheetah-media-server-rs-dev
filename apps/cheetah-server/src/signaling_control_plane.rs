//! Signaling control-plane assembly for MIG-01.
//!
//! `signaling-control-plane` feature 关闭时此模块不编译。

// Ensure the control-plane and gRPC adapter crates are linked when the feature is enabled.
#[cfg(feature = "signaling-control-plane")]
extern crate cheetah_media_control_plane;
#[cfg(feature = "signaling-control-plane")]
extern crate cheetah_media_grpc_adapter;

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Deployment rollout mode for the signaling control plane.
///
/// 信号控制面的部署灰度阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMode {
    /// Register heartbeat only; all mutation gates are closed.
    #[default]
    RegisterOnly,
    /// Consume query/event without driving business.
    ShadowQuery,
    /// Allow typed creates for an allowlisted subset.
    Canary,
    /// Full typed control plane.
    Production,
}

/// gRPC listener configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MessageLimits {
    pub max_inbound_size: u64,
    pub max_outbound_size: u64,
}

impl Default for MessageLimits {
    fn default() -> Self {
        Self {
            max_inbound_size: 4 * 1024 * 1024,
            max_outbound_size: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GrpcConfig {
    pub listen: String,
    pub advertised_endpoint: Option<String>,
    pub message_limits: MessageLimits,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegistryConfig {
    pub endpoint: Option<String>,
    pub zone: String,
    pub node_identity: String,
    #[serde(default)]
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContractConfig {
    pub min_version: u64,
    pub max_version: u64,
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StoreConfig {
    pub path: String,
    pub max_size_mb: u64,
    pub retention_hours: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventConfig {
    pub max_events: u64,
    pub retention_hours: u64,
    pub cursor_key_handle: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CapacityConfig {
    pub max_nodes: u64,
    pub max_resources: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_ca_pem: String,
    pub client_cert_required: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecretExchangeConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub renewal_margin_sec: u64,
}

/// Top-level configuration for the signaling control plane.
///
/// 信号控制面的顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct SignalingControlPlaneConfig {
    pub enabled: bool,
    pub rollout: RolloutMode,
    pub grpc: GrpcConfig,
    pub registry: RegistryConfig,
    pub contract: ContractConfig,
    pub store: StoreConfig,
    pub events: EventConfig,
    pub capacity: CapacityConfig,
    pub tls: TlsConfig,
    pub secret_exchange: SecretExchangeConfig,
}

#[cfg(feature = "signaling-control-plane")]
use cheetah_media_grpc_adapter::GrpcAdapterConfig;

/// Placeholder assembly handle for the signaling control plane.
#[cfg(feature = "signaling-control-plane")]
pub struct Assembly {
    pub grpc_config: GrpcAdapterConfig,
}

#[cfg(feature = "signaling-control-plane")]
impl Assembly {
    /// Create a default assembly configuration.
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            grpc_config: GrpcAdapterConfig::new(bind_addr),
        }
    }
}

impl SignalingControlPlaneConfig {
    /// Return the default configuration as a JSON value.
    pub fn default_json() -> serde_json::Value {
        serde_json::to_value(Self::default()).expect("default config serializes")
    }

    /// Validate the configuration when the control plane is enabled.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.grpc.listen.is_empty() {
            return Err(
                "grpc.listen is required when signaling-control-plane is enabled".to_string(),
            );
        }
        self.grpc
            .listen
            .parse::<SocketAddr>()
            .map_err(|e| format!("grpc.listen is not a valid socket address: {e}"))?;
        if self.store.path.is_empty() {
            return Err(
                "store.path is required when signaling-control-plane is enabled".to_string(),
            );
        }
        if self.registry.node_identity.is_empty() {
            return Err(
                "registry.node_identity is required when signaling-control-plane is enabled"
                    .to_string(),
            );
        }
        if self.contract.min_version > self.contract.max_version {
            return Err("contract.min_version must not exceed max_version".to_string());
        }
        if self.secret_exchange.enabled {
            if self.secret_exchange.endpoint.is_empty() {
                return Err(
                    "secret_exchange.endpoint is required when secret exchange is enabled"
                        .to_string(),
                );
            }
            if self.secret_exchange.renewal_margin_sec == 0 {
                return Err("secret_exchange.renewal_margin_sec must be non-zero".to_string());
            }
        }
        if self.tls.client_cert_required || !self.tls.server_cert_pem.is_empty() {
            if self.tls.server_cert_pem.is_empty() {
                return Err("tls.server_cert_pem is required when TLS is enabled".to_string());
            }
            if self.tls.server_key_pem.is_empty() {
                return Err("tls.server_key_pem is required when TLS is enabled".to_string());
            }
        }
        Ok(())
    }
}
