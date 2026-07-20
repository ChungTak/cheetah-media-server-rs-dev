//! Health handle that wraps `tonic-health` without exposing it in the public API.
//!
//! 健康状态句柄，避免在公开 API 中暴露 tonic-health 类型。

/// gRPC health serving status exposed by the adapter.
///
/// 由 adapter 公开的 gRPC 健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcServingStatus {
    /// The service status is unknown.
    Unknown,
    /// The service is serving requests.
    Serving,
    /// The service is not serving requests.
    NotServing,
}

impl From<GrpcServingStatus> for tonic_health::ServingStatus {
    fn from(status: GrpcServingStatus) -> Self {
        match status {
            GrpcServingStatus::Unknown => tonic_health::ServingStatus::Unknown,
            GrpcServingStatus::Serving => tonic_health::ServingStatus::Serving,
            GrpcServingStatus::NotServing => tonic_health::ServingStatus::NotServing,
        }
    }
}

/// Named health subsystem reported by the signaling control plane.
///
/// 信号控制面报告的命名健康子系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthCategory {
    /// Contract descriptor compatibility.
    Contract,
    /// Persistent SQLite store.
    Store,
    /// gRPC listener readiness.
    GrpcListener,
    /// Registry lease/heartbeat.
    RegistryLease,
    /// Capability/preflight readiness.
    CapabilityPreflight,
    /// Capacity overload state.
    Capacity,
    /// Event journal/replay.
    EventJournal,
    /// SecretExchange credential exchange.
    CredentialExchange,
    /// Reconciliation engine.
    Reconciliation,
}

impl HealthCategory {
    /// gRPC health service name for this subsystem.
    pub const fn as_str(&self) -> &'static str {
        match self {
            HealthCategory::Contract => "cheetah.signaling.contract",
            HealthCategory::Store => "cheetah.signaling.store",
            HealthCategory::GrpcListener => "cheetah.signaling.grpc_listener",
            HealthCategory::RegistryLease => "cheetah.signaling.registry_lease",
            HealthCategory::CapabilityPreflight => "cheetah.signaling.capability_preflight",
            HealthCategory::Capacity => "cheetah.signaling.capacity",
            HealthCategory::EventJournal => "cheetah.signaling.event_journal",
            HealthCategory::CredentialExchange => "cheetah.signaling.credential_exchange",
            HealthCategory::Reconciliation => "cheetah.signaling.reconciliation",
        }
    }
}

/// Handle for updating the gRPC health status served by the adapter.
///
/// `HealthReporter` is already `Clone` and backed by an internal watch channel,
/// so callers that need concurrent updates can clone this handle.
///
/// 用于更新 adapter 提供的 gRPC 健康状态的句柄。
#[derive(Clone, Debug)]
pub struct GrpcHealthHandle {
    reporter: tonic_health::server::HealthReporter,
}

impl GrpcHealthHandle {
    pub(crate) fn new(reporter: tonic_health::server::HealthReporter) -> Self {
        Self { reporter }
    }

    /// Set the overall (empty service name) health status.
    ///
    /// 设置整体（空 service 名）健康状态。
    pub async fn set_overall(&mut self, status: GrpcServingStatus) {
        self.reporter.set_service_status("", status.into()).await;
    }

    /// Clear the overall health status.
    ///
    /// 清除整体健康状态。
    pub async fn clear_overall(&mut self) {
        self.reporter.clear_service_status("").await;
    }

    /// Set the health status for a named service.
    ///
    /// 设置指定 service 的健康状态。
    pub async fn set_service(&mut self, name: impl AsRef<str>, status: GrpcServingStatus) {
        self.reporter
            .set_service_status(name.as_ref(), status.into())
            .await;
    }

    /// Clear the health status for a named service.
    ///
    /// 清除指定 service 的健康状态。
    pub async fn clear_service(&mut self, name: impl AsRef<str>) {
        self.reporter.clear_service_status(name.as_ref()).await;
    }

    /// Set the health status for a signaling subsystem category.
    ///
    /// 设置某个信号控制面子系统的健康状态。
    pub async fn set_category(&mut self, category: HealthCategory, status: GrpcServingStatus) {
        self.set_service(category.as_str(), status).await;
    }

    /// Clear the health status for a signaling subsystem category.
    ///
    /// 清除某个信号控制面子系统的健康状态。
    pub async fn clear_category(&mut self, category: HealthCategory) {
        self.clear_service(category.as_str()).await;
    }
}
