//! Deployment rollout mode and gating helpers for the signaling control plane.
//!
//! 信号控制面部署阶段与放行控制。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::ids::{OperationId, TenantId};

/// Deployment phase of the signaling control plane.
///
/// 信号控制面的部署灰度阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutMode {
    /// Feature is compiled in but the gRPC listener and control plane runtime
    /// are not started.
    #[default]
    RegisterOnly,
    /// gRPC listener is up; register heartbeats and respond to capability/query
    /// calls, but do not drive business mutations or emit events as the
    /// authoritative source.
    ShadowQuery,
    /// Typed mutations are allowed for an allowlisted subset of tenants and
    /// operations.
    Canary,
    /// Full typed control plane.
    Production,
}

impl RolloutMode {
    /// Whether query and capability requests are served.
    pub const fn query_allowed(&self) -> bool {
        matches!(
            self,
            RolloutMode::RegisterOnly
                | RolloutMode::ShadowQuery
                | RolloutMode::Canary
                | RolloutMode::Production
        )
    }

    /// Whether the control plane may emit controlled events as the
    /// authoritative source.
    pub const fn event_allowed(&self) -> bool {
        matches!(
            self,
            RolloutMode::ShadowQuery | RolloutMode::Canary | RolloutMode::Production
        )
    }

    /// Whether typed mutations are allowed at all.
    pub const fn mutation_allowed(&self) -> bool {
        matches!(self, RolloutMode::Canary | RolloutMode::Production)
    }

    /// Whether the control plane is the authoritative owner (no shadow mode).
    pub const fn production(&self) -> bool {
        matches!(self, RolloutMode::Production)
    }
}

/// Gating decisions for control-plane operations.
///
/// 控制面操作灰度开关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutGate {
    mode: RolloutMode,
    canary_tenants: HashSet<TenantId>,
    canary_operations: HashSet<OperationId>,
}

impl RolloutGate {
    /// Create a gate for the given rollout mode.
    pub fn new(mode: RolloutMode) -> Self {
        Self {
            mode,
            canary_tenants: HashSet::new(),
            canary_operations: HashSet::new(),
        }
    }

    /// Allow a specific tenant in `Canary` mode.
    pub fn allow_tenant(mut self, tenant: TenantId) -> Self {
        self.canary_tenants.insert(tenant);
        self
    }

    /// Allow a specific operation in `Canary` mode.
    pub fn allow_operation(mut self, operation: OperationId) -> Self {
        self.canary_operations.insert(operation);
        self
    }

    /// Allow multiple tenants in `Canary` mode.
    pub fn allow_tenants<I: IntoIterator<Item = TenantId>>(mut self, tenants: I) -> Self {
        self.canary_tenants.extend(tenants);
        self
    }

    /// Allow multiple operations in `Canary` mode.
    pub fn allow_operations<I: IntoIterator<Item = OperationId>>(mut self, operations: I) -> Self {
        self.canary_operations.extend(operations);
        self
    }

    /// Whether the current rollout mode permits query/capability requests.
    pub const fn query_allowed(&self) -> bool {
        self.mode.query_allowed()
    }

    /// Whether the current rollout mode permits emitting authoritative events.
    pub const fn event_allowed(&self) -> bool {
        self.mode.event_allowed()
    }

    /// Whether the current rollout mode permits any mutation.
    pub const fn mutation_allowed(&self) -> bool {
        self.mode.mutation_allowed()
    }

    /// Whether a concrete mutation is allowed for the given tenant and operation.
    ///
    /// In `Canary` mode, the tenant and operation must be in the allowlists
    /// (empty allowlist means all are allowed).
    /// In `Production` mode all mutations are allowed.
    pub fn operation_allowed(&self, tenant: &TenantId, operation: &OperationId) -> bool {
        if !self.mode.mutation_allowed() {
            return false;
        }
        if self.mode.production() {
            return true;
        }
        // Canary mode with allowlists.
        let tenant_ok = self.canary_tenants.is_empty() || self.canary_tenants.contains(tenant);
        let operation_ok =
            self.canary_operations.is_empty() || self.canary_operations.contains(operation);
        tenant_ok && operation_ok
    }

    /// Return the current rollout mode.
    pub const fn mode(&self) -> RolloutMode {
        self.mode
    }
}

impl From<RolloutMode> for RolloutGate {
    fn from(mode: RolloutMode) -> Self {
        Self::new(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_only_allows_query_not_mutation() {
        let gate = RolloutGate::new(RolloutMode::RegisterOnly);
        assert!(gate.query_allowed());
        assert!(!gate.event_allowed());
        assert!(!gate.mutation_allowed());
    }

    #[test]
    fn shadow_query_allows_query_and_event_not_mutation() {
        let gate = RolloutGate::new(RolloutMode::ShadowQuery);
        assert!(gate.query_allowed());
        assert!(gate.event_allowed());
        assert!(!gate.mutation_allowed());
    }

    #[test]
    fn canary_respects_allowlists() {
        let t1 = TenantId::new("tenant-1").unwrap();
        let t2 = TenantId::new("tenant-2").unwrap();
        let op1 = OperationId::new("create").unwrap();
        let op2 = OperationId::new("delete").unwrap();

        let gate = RolloutGate::new(RolloutMode::Canary)
            .allow_tenant(t1.clone())
            .allow_operation(op1.clone());

        assert!(gate.mutation_allowed());
        assert!(gate.operation_allowed(&t1, &op1));
        assert!(!gate.operation_allowed(&t2, &op1));
        assert!(!gate.operation_allowed(&t1, &op2));
    }

    #[test]
    fn canary_with_empty_allowlists_allows_all() {
        let t = TenantId::new("tenant-1").unwrap();
        let op = OperationId::new("create").unwrap();
        let gate = RolloutGate::new(RolloutMode::Canary);
        assert!(gate.operation_allowed(&t, &op));
    }

    #[test]
    fn production_allows_all_mutations() {
        let t = TenantId::new("tenant-1").unwrap();
        let op = OperationId::new("create").unwrap();
        let gate = RolloutGate::new(RolloutMode::Production);
        assert!(gate.query_allowed());
        assert!(gate.event_allowed());
        assert!(gate.mutation_allowed());
        assert!(gate.operation_allowed(&t, &op));
    }
}
