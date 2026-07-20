//! Health handle that wraps `tonic-health` without exposing it in the public API.
//!
//! 健康状态句柄，避免在公开 API 中暴露 tonic-health 类型。

use std::sync::Arc;

use tokio::sync::Mutex;

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

/// Handle for updating the gRPC health status served by the adapter.
///
/// 用于更新 adapter 提供的 gRPC 健康状态的句柄。
#[derive(Clone, Debug)]
pub struct GrpcHealthHandle {
    reporter: Arc<Mutex<tonic_health::server::HealthReporter>>,
}

impl GrpcHealthHandle {
    pub(crate) fn new(reporter: tonic_health::server::HealthReporter) -> Self {
        Self {
            reporter: Arc::new(Mutex::new(reporter)),
        }
    }

    /// Set the overall (empty service name) health status.
    ///
    /// 设置整体（空 service 名）健康状态。
    pub async fn set_overall(&self, status: GrpcServingStatus) {
        let mut reporter = self.reporter.lock().await;
        reporter.set_service_status("", status.into()).await;
    }

    /// Clear the overall health status.
    ///
    /// 清除整体健康状态。
    pub async fn clear_overall(&self) {
        let mut reporter = self.reporter.lock().await;
        reporter.clear_service_status("").await;
    }

    /// Set the health status for a named service.
    ///
    /// 设置指定 service 的健康状态。
    pub async fn set_service(&self, name: impl AsRef<str>, status: GrpcServingStatus) {
        let mut reporter = self.reporter.lock().await;
        reporter
            .set_service_status(name.as_ref(), status.into())
            .await;
    }

    /// Clear the health status for a named service.
    ///
    /// 清除指定 service 的健康状态。
    pub async fn clear_service(&self, name: impl AsRef<str>) {
        let mut reporter = self.reporter.lock().await;
        reporter.clear_service_status(name.as_ref()).await;
    }
}
