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

pub use cheetah_media_api::RolloutMode;

/// gRPC message size limits.
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

/// gRPC listener configuration.
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
    pub max_sessions: u64,
    pub max_ports: u64,
    pub max_workers: u64,
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

use cheetah_media_api::capacity::CapacityLimits;
use cheetah_media_api::ids::{MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId};
use cheetah_media_api::node::NodeIdentity;
use cheetah_media_api::port::MediaCapacityApi;
use cheetah_media_control_plane::{
    CapacityLoadProvider, CapacityOrchestrator, ControlPlane, NodeSupervisor, SqliteStore,
    StoreMaintenance, SystemClock,
};
use cheetah_media_grpc_adapter::{
    GrpcAdapter, GrpcAdapterConfig, GrpcHealthHandle, GrpcMessageLimits, GrpcServingStatus,
    GrpcTlsConfig,
};
use cheetah_runtime_api::RuntimeApi;
use std::sync::Arc;

/// Running signaling control-plane assembly (store + facade + optional gRPC).
pub struct Assembly {
    pub grpc_config: GrpcAdapterConfig,
    pub control_plane: ControlPlane,
    pub capacity: Arc<CapacityOrchestrator>,
    pub node_supervisor: Option<Arc<NodeSupervisor>>,
    adapter: Option<GrpcAdapter>,
    health: Option<GrpcHealthHandle>,
}

impl Assembly {
    /// Build store, control plane and gRPC config without binding sockets.
    ///
    /// Registration against a live signaling registry requires a
    /// `RegistryClient` implementation from the gRPC adapter layer; without
    /// it the supervisor is omitted and create gate stays closed until wired.
    pub async fn bootstrap(
        cfg: &SignalingControlPlaneConfig,
        runtime: Arc<dyn RuntimeApi>,
    ) -> Result<Self, String> {
        cfg.validate()?;
        let bind_addr: SocketAddr = cfg
            .grpc
            .listen
            .parse()
            .map_err(|e| format!("grpc.listen: {e}"))?;

        let mut grpc_config = GrpcAdapterConfig::new(bind_addr);
        grpc_config.message_limits = GrpcMessageLimits {
            max_decoding_message_size: cfg.grpc.message_limits.max_inbound_size as usize,
            max_encoding_message_size: cfg.grpc.message_limits.max_outbound_size as usize,
        };
        if !cfg.tls.server_cert_pem.is_empty() {
            let mut tls =
                GrpcTlsConfig::new(cfg.tls.server_cert_pem.clone(), cfg.tls.server_key_pem.clone());
            tls.client_ca_pem = cfg.tls.client_ca_pem.clone();
            tls.client_cert_required = cfg.tls.client_cert_required;
            grpc_config.tls = Some(tls);
        }

        let store = SqliteStore::new(runtime.clone(), &cfg.store.path)
            .await
            .map_err(|e| e.to_string())?;
        let capacity = Arc::new(CapacityOrchestrator::new(CapacityLimits {
            session_count: nonzero_or(cfg.capacity.max_sessions, 10_000),
            port_count: nonzero_or(cfg.capacity.max_ports, 10_000),
            bandwidth_bps: u64::MAX,
            worker_count: nonzero_or(cfg.capacity.max_workers, 1_000),
            blocking_job_count: 256,
            file_task_count: 256,
            event_subscriber_count: 256,
            cpu_permille: 1000,
        }));
        // Create gate stays closed until a successful registry lease (NODE-02).
        capacity
            .set_node_gate(false)
            .await
            .map_err(|e| e.to_string())?;

        let control_plane = ControlPlane::new(
            runtime,
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_store_maintenance(Arc::new(store) as Arc<dyn StoreMaintenance>)
        .with_capacity(capacity.clone());

        // Node identity is validated so partial configs fail closed.
        let _identity = build_node_identity(cfg)?;

        Ok(Self {
            grpc_config,
            control_plane,
            capacity,
            node_supervisor: None,
            adapter: None,
            health: None,
        })
    }

    /// Attach a node supervisor built by the host (after providing a RegistryClient).
    #[allow(dead_code)] // Used once a RegistryClient adapter is wired (CT-01).
    pub fn attach_supervisor(&mut self, supervisor: Arc<NodeSupervisor>) {
        self.control_plane = self
            .control_plane
            .clone()
            .with_node_supervisor(supervisor.clone());
        self.node_supervisor = Some(supervisor);
    }

    /// Bind the gRPC listener. Health starts as NotServing until registration.
    pub async fn start_grpc(&mut self) -> Result<SocketAddr, String> {
        let (adapter, mut health) = GrpcAdapter::start(self.grpc_config.clone())
            .await
            .map_err(|e| e.to_string())?;
        health
            .set_overall(GrpcServingStatus::NotServing)
            .await;
        let addr = adapter.bound_addr();
        self.adapter = Some(adapter);
        self.health = Some(health);
        Ok(addr)
    }

    /// Mark the control plane Serving after successful registration.
    #[allow(dead_code)] // Used once registry registration succeeds (NODE-02).
    pub async fn mark_serving(&mut self) {
        if let Some(health) = self.health.as_mut() {
            health.set_overall(GrpcServingStatus::Serving).await;
        }
    }

    /// Mark NotServing (drain / isolation / shutdown).
    pub async fn mark_not_serving(&mut self) {
        if let Some(health) = self.health.as_mut() {
            health.set_overall(GrpcServingStatus::NotServing).await;
        }
    }

    /// Graceful stop: close create gate, stop gRPC.
    pub async fn stop(&mut self) -> Result<(), String> {
        let _ = self.capacity.set_node_gate(false).await;
        self.mark_not_serving().await;
        if let Some(sup) = &self.node_supervisor {
            let _ = sup.shutdown("assembly stop").await;
        }
        if let Some(mut adapter) = self.adapter.take() {
            adapter.stop().await.map_err(|e| e.to_string())?;
        }
        self.health = None;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.adapter.as_ref().map(|a| a.bound_addr())
    }

    /// Build a validated `NodeIdentity` from config (for host registry wiring).
    #[allow(dead_code)]
    pub fn node_identity_from_config(
        cfg: &SignalingControlPlaneConfig,
    ) -> Result<NodeIdentity, String> {
        build_node_identity(cfg)
    }

    /// Construct a `NodeSupervisor` with the host-provided registry client.
    #[allow(dead_code)] // Host wires RegistryClient after CT-01 contract lands.
    pub fn build_supervisor(
        &self,
        cfg: &SignalingControlPlaneConfig,
        registry: Arc<dyn cheetah_media_control_plane::RegistryClient>,
    ) -> Result<Arc<NodeSupervisor>, String> {
        let identity = build_node_identity(cfg)?;
        let load = Arc::new(CapacityLoadProvider::new(self.capacity.clone()));
        let clock = Arc::new(SystemClock);
        Ok(Arc::new(NodeSupervisor::new(
            identity,
            self.capacity.clone(),
            registry,
            load,
            clock,
        )))
    }
}

fn nonzero_or(value: u64, default: u64) -> u64 {
    if value == 0 {
        default
    } else {
        value
    }
}

fn build_node_identity(cfg: &SignalingControlPlaneConfig) -> Result<NodeIdentity, String> {
    let node_id = MediaNodeId::new(cfg.registry.node_identity.clone())
        .map_err(|e| format!("registry.node_identity must be a canonical UUID: {e}"))?;
    // Process instance id is generated once per assembly bootstrap.
    let instance_id = MediaNodeInstanceId::new(uuid_v4_string())
        .map_err(|e| format!("failed to build instance id: {e}"))?;
    let control_endpoint = cfg
        .grpc
        .advertised_endpoint
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("http://{}", cfg.grpc.listen));
    let checksum = cfg
        .contract
        .checksum
        .clone()
        .unwrap_or_else(|| "unset".to_string());
    Ok(NodeIdentity {
        node_id,
        instance_id,
        instance_epoch: MediaNodeInstanceEpoch(0),
        control_endpoint,
        network_zone: if cfg.registry.zone.is_empty() {
            None
        } else {
            Some(cfg.registry.zone.clone())
        },
        region: None,
        labels: Default::default(),
        advertised_media_addresses: cfg.registry.addresses.clone(),
        build_version: env!("CARGO_PKG_VERSION").to_string(),
        contract_range: format!(">={}, <={}", cfg.contract.min_version, cfg.contract.max_version),
        contract_checksum: checksum,
        capability_generation: 1,
    })
}

fn uuid_v4_string() -> String {
    // Minimal UUID v4 from getrandom without adding a uuid crate dependency.
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("getrandom");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
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
        if self.grpc.message_limits.max_inbound_size == 0 {
            return Err("grpc.message_limits.max_inbound_size must be non-zero".to_string());
        }
        if self.grpc.message_limits.max_outbound_size == 0 {
            return Err("grpc.message_limits.max_outbound_size must be non-zero".to_string());
        }
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
        // NODE-01: stable node ID must be a canonical UUID, not a hostname.
        if MediaNodeId::new(self.registry.node_identity.clone()).is_err() {
            return Err(
                "registry.node_identity must be a canonical UUID (stable deployment id)"
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
        if self.tls.client_cert_required
            || !self.tls.server_cert_pem.is_empty()
            || !self.tls.server_key_pem.is_empty()
        {
            if self.tls.server_cert_pem.is_empty() {
                return Err("tls.server_cert_pem is required when TLS is enabled".to_string());
            }
            if self.tls.server_key_pem.is_empty() {
                return Err("tls.server_key_pem is required when TLS is enabled".to_string());
            }
            if self.tls.client_cert_required && self.tls.client_ca_pem.is_empty() {
                return Err(
                    "tls.client_ca_pem is required when client_cert_required is set".to_string(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_validates() {
        let cfg = SignalingControlPlaneConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn enabled_config_requires_fields() {
        let mut cfg = SignalingControlPlaneConfig {
            enabled: true,
            grpc: GrpcConfig {
                listen: "127.0.0.1:9090".to_string(),
                ..Default::default()
            },
            store: StoreConfig {
                path: "/tmp/test.db".to_string(),
                ..Default::default()
            },
            registry: RegistryConfig {
                node_identity: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());

        cfg.registry.node_identity = "not-a-uuid".to_string();
        assert!(cfg.validate().is_err());
        cfg.registry.node_identity = "550e8400-e29b-41d4-a716-446655440000".to_string();

        cfg.tls.client_cert_required = true;
        assert!(cfg.validate().is_err());

        cfg.tls.client_ca_pem = "-----BEGIN CERTIFICATE-----\nMIIB...".to_string();
        cfg.tls.server_cert_pem = "-----BEGIN CERTIFICATE-----\nMIIB...".to_string();
        cfg.tls.server_key_pem = "-----BEGIN PRIVATE KEY-----\nMIIB...".to_string();
        assert!(cfg.validate().is_ok());

        // A lone private key should also trigger the TLS consistency check.
        cfg.tls.client_cert_required = false;
        cfg.tls.client_ca_pem.clear();
        cfg.tls.server_cert_pem.clear();
        assert!(cfg.validate().is_err());

        // Zero message limits are rejected.
        let mut cfg = SignalingControlPlaneConfig {
            enabled: true,
            grpc: GrpcConfig {
                listen: "127.0.0.1:9090".to_string(),
                ..Default::default()
            },
            store: StoreConfig {
                path: "/tmp/test.db".to_string(),
                ..Default::default()
            },
            registry: RegistryConfig {
                node_identity: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.grpc.message_limits.max_inbound_size = 0;
        assert!(cfg.validate().is_err());
        cfg.grpc.message_limits.max_inbound_size = 4 * 1024 * 1024;
        cfg.grpc.message_limits.max_outbound_size = 0;
        assert!(cfg.validate().is_err());
    }
}
