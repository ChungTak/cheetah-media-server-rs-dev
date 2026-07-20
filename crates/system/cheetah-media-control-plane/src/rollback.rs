//! Rollback rules for signaling control-plane migration.
//!
//! 信号控制面迁移回滚规则校验。

use cheetah_media_api::ids::{MediaNodeInstanceEpoch, ResourceGeneration};
use cheetah_media_api::rollout::RolloutMode;

/// Monotonic schema version of the control-plane SQLite store.
///
/// 控制面 SQLite store 的 schema 版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaVersion(pub u64);

/// Current state used to evaluate a rollback request.
///
/// 用于评估回滚请求的当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackContext {
    pub current_rollout: RolloutMode,
    pub current_instance_epoch: MediaNodeInstanceEpoch,
    pub current_capability_generation: ResourceGeneration,
    pub current_schema_version: SchemaVersion,
}

/// A request to roll back or roll forward the control-plane deployment.
///
/// 控制面部署回滚/推进请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackRequest {
    pub target_rollout: RolloutMode,
    pub target_instance_epoch: MediaNodeInstanceEpoch,
    pub target_capability_generation: ResourceGeneration,
    pub target_schema_version: SchemaVersion,
    /// Whether an export/restore path has been verified for binary rollback.
    pub has_export_restore: bool,
    /// Whether the create gate has been closed before the rollback.
    pub create_gate_closed: bool,
}

/// Reason a rollback request was rejected.
///
/// 回滚请求被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackViolation {
    CreateGateMustBeClosed,
    InstanceEpochCannotRegress,
    CapabilityGenerationCannotRegress,
    CapabilityGenerationMustAdvanceOnDowngrade,
    SchemaVersionCannotDowngradeWithoutExportRestore,
}

/// Outcome of a rollback validation.
///
/// 回滚校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackOutcome {
    Allowed,
    Rejected(RollbackViolation),
}

/// Policy controlling rollback/upgrade validation.
///
/// 回滚/升级校验策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RollbackPolicy;

impl RollbackPolicy {
    /// Validate a rollback request against the current context.
    pub fn validate(&self, ctx: &RollbackContext, req: &RollbackRequest) -> RollbackOutcome {
        if !req.create_gate_closed {
            return RollbackOutcome::Rejected(RollbackViolation::CreateGateMustBeClosed);
        }

        if req.target_instance_epoch < ctx.current_instance_epoch {
            return RollbackOutcome::Rejected(RollbackViolation::InstanceEpochCannotRegress);
        }

        if req.target_capability_generation < ctx.current_capability_generation {
            return RollbackOutcome::Rejected(RollbackViolation::CapabilityGenerationCannotRegress);
        }

        // Rolling back to a less-featured mode must advance capability generation
        // so the new code can prove it has observed the previous owner's state.
        let is_downgrade = rollback_rank(req.target_rollout) < rollback_rank(ctx.current_rollout);
        if is_downgrade && req.target_capability_generation == ctx.current_capability_generation {
            return RollbackOutcome::Rejected(
                RollbackViolation::CapabilityGenerationMustAdvanceOnDowngrade,
            );
        }

        if req.target_schema_version < ctx.current_schema_version && !req.has_export_restore {
            return RollbackOutcome::Rejected(
                RollbackViolation::SchemaVersionCannotDowngradeWithoutExportRestore,
            );
        }

        RollbackOutcome::Allowed
    }
}

fn rollback_rank(mode: RolloutMode) -> u8 {
    match mode {
        RolloutMode::RegisterOnly => 0,
        RolloutMode::ShadowQuery => 1,
        RolloutMode::Canary => 2,
        RolloutMode::Production => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RollbackContext {
        RollbackContext {
            current_rollout: RolloutMode::Production,
            current_instance_epoch: MediaNodeInstanceEpoch(7),
            current_capability_generation: ResourceGeneration(3),
            current_schema_version: SchemaVersion(4),
        }
    }

    #[test]
    fn allows_rollback_with_closed_gate_and_monotonic_values() {
        let policy = RollbackPolicy;
        let req = RollbackRequest {
            target_rollout: RolloutMode::Canary,
            target_instance_epoch: MediaNodeInstanceEpoch(8),
            target_capability_generation: ResourceGeneration(4),
            target_schema_version: SchemaVersion(4),
            has_export_restore: false,
            create_gate_closed: true,
        };
        assert_eq!(policy.validate(&ctx(), &req), RollbackOutcome::Allowed);
    }

    #[test]
    fn rejects_open_create_gate() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.create_gate_closed = false;
        assert_eq!(
            policy.validate(&ctx(), &req),
            RollbackOutcome::Rejected(RollbackViolation::CreateGateMustBeClosed)
        );
    }

    #[test]
    fn rejects_regressing_instance_epoch() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.target_instance_epoch = MediaNodeInstanceEpoch(6);
        assert_eq!(
            policy.validate(&ctx(), &req),
            RollbackOutcome::Rejected(RollbackViolation::InstanceEpochCannotRegress)
        );
    }

    #[test]
    fn rejects_regressing_capability_generation() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.target_capability_generation = ResourceGeneration(2);
        assert_eq!(
            policy.validate(&ctx(), &req),
            RollbackOutcome::Rejected(RollbackViolation::CapabilityGenerationCannotRegress)
        );
    }

    #[test]
    fn downgrade_requires_capability_generation_advance() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.target_rollout = RolloutMode::Canary;
        req.target_capability_generation = ResourceGeneration(3);
        assert_eq!(
            policy.validate(&ctx(), &req),
            RollbackOutcome::Rejected(
                RollbackViolation::CapabilityGenerationMustAdvanceOnDowngrade
            )
        );
    }

    #[test]
    fn schema_downgrade_requires_export_restore() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.target_schema_version = SchemaVersion(3);
        req.has_export_restore = false;
        assert_eq!(
            policy.validate(&ctx(), &req),
            RollbackOutcome::Rejected(
                RollbackViolation::SchemaVersionCannotDowngradeWithoutExportRestore
            )
        );
    }

    #[test]
    fn schema_downgrade_allowed_with_export_restore() {
        let policy = RollbackPolicy;
        let mut req = rollback_req();
        req.target_schema_version = SchemaVersion(3);
        req.has_export_restore = true;
        assert_eq!(policy.validate(&ctx(), &req), RollbackOutcome::Allowed);
    }

    fn rollback_req() -> RollbackRequest {
        RollbackRequest {
            target_rollout: RolloutMode::Canary,
            target_instance_epoch: MediaNodeInstanceEpoch(8),
            target_capability_generation: ResourceGeneration(4),
            target_schema_version: SchemaVersion(4),
            has_export_restore: false,
            create_gate_closed: true,
        }
    }
}
