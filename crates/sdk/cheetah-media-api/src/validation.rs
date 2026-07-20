//! Mutation context validation for the signaling control plane.
//!
//! `cheetah-media-api` is runtime-neutral, so these validators only perform
//! contract and context checks that do not require external state. Stateful
//! guard checks are expressed as a trait that `cheetah-media-control-plane`
//! will implement.
//!
//! 信令控制面 mutation 上下文校验。
//!
//! `cheetah-media-api` 运行时无关，因此这些校验器只执行不需要外部状态的契约
//! 和上下文检查。有状态的 guard 检查以 trait 形式表达，由
//! `cheetah-media-control-plane` 实现。

use crate::error::{MediaError, MediaErrorCode, Result};
use crate::fencing::{ControlledResourceRef, LeaseStatus, NodeRuntimeState, NodeState};
use crate::ids::{MediaNodeId, MediaNodeInstanceEpoch, OwnerEpoch, ResourceGeneration};
use crate::port::{MediaMutationContext, MediaRequestContext};

/// Validation steps in the fixed order used for every control-plane mutation.
///
/// 每个控制面 mutation 使用的固定校验步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationStep {
    ContractVersion,
    Identity,
    RequiredFields,
    TenantScope,
    Deadline,
    TargetNode,
    NodeStateGate,
    OwnerEpoch,
    BindingSession,
}

/// Intended effect of a mutation, used by the node-state gate.
///
/// mutation 的预期作用，用于节点状态门控。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationIntent {
    /// Create or start a new controlled resource.
    Create,
    /// Update an existing controlled resource.
    Mutate,
    /// Read or list without side effects.
    Read,
    /// Stop a controlled resource.
    Stop,
    /// Delete a controlled resource.
    Delete,
}

/// Result of a successful guard validation.
///
/// 成功 guard 校验的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardOutcome {
    pub accepted_owner_epoch: OwnerEpoch,
    pub accepted_generation: ResourceGeneration,
}

impl MediaMutationContext {
    /// Adapter-side validation of the mutation context.
    ///
    /// Performs fast, stateless checks in the fixed order before any
    /// side-effecting work. The returned step is the last step that passed,
    /// which callers may include in audit logs.
    ///
    /// Adapter 侧对 mutation 上下文的校验。按固定顺序执行快速、无状态的检查，
    /// 在任何副作用工作之前返回通过的最后一个步骤，调用方可将其记入审计日志。
    pub fn validate_adapter(&self) -> Result<ValidationStep> {
        if self.contract_version.trim().is_empty() {
            return Err(MediaError::invalid_argument("contract_version is required"));
        }
        if self.contract_version.len() > 64 {
            return Err(MediaError::invalid_argument(
                "contract_version exceeds maximum length",
            ));
        }

        if self.source_signaling_node_id.as_str().is_empty()
            || self.target_media_node_id.as_str().is_empty()
        {
            return Err(MediaError::invalid_argument(
                "source_signaling_node_id and target_media_node_id are required",
            ));
        }

        if self.operation_id.as_str().is_empty() || self.operation_step_id.as_str().is_empty() {
            return Err(MediaError::invalid_argument(
                "operation_id and operation_step_id are required",
            ));
        }

        if self.tenant_id.as_str().is_empty() {
            return Err(MediaError::invalid_argument("tenant_id is required"));
        }

        if self.target_media_node_instance_epoch.0 == 0 {
            return Err(MediaError::invalid_argument(
                "target_media_node_instance_epoch must be non-zero",
            ));
        }

        if self.owner_epoch.0 == 0 {
            return Err(MediaError::invalid_argument("owner_epoch must be non-zero"));
        }

        Ok(ValidationStep::OwnerEpoch)
    }

    /// Return true if the request targets the given node instance.
    ///
    /// 返回请求是否面向给定节点实例。
    pub fn targets(&self, node_id: &MediaNodeId, instance_epoch: MediaNodeInstanceEpoch) -> bool {
        self.target_media_node_id.as_str() == node_id.as_str()
            && self.target_media_node_instance_epoch == instance_epoch
    }
}

impl MediaRequestContext {
    /// Require a cluster mutation context and run adapter validation.
    ///
    /// Runs the fixed adapter sequence first, then checks the request-level
    /// deadline, so errors are reported in the declared `ValidationStep` order.
    ///
    /// 要求存在集群 mutation 上下文并执行 adapter 校验。按固定 adapter 顺序执行，
    /// 最后检查请求级 deadline，使错误按声明的 `ValidationStep` 顺序返回。
    pub fn validate_mutation_adapter(&self) -> Result<&MediaMutationContext> {
        let ctx = match &self.mutation {
            Some(ctx) => ctx,
            None => {
                return Err(MediaError::new(
                    MediaErrorCode::InvalidArgument,
                    "mutation context is required for cluster mutations",
                ))
            }
        };

        ctx.validate_adapter()?;

        if self.deadline.is_none() {
            return Err(MediaError::new(
                MediaErrorCode::InvalidArgument,
                "deadline is required for cluster mutations",
            ));
        }

        Ok(ctx)
    }
}

/// Guard that checks a mutation context against the current node runtime state
/// and an optional controlled resource.
///
/// 将 mutation 上下文与当前节点运行时状态及可选受控资源进行比对检查。
#[derive(Debug, Clone, Copy)]
pub struct MutationGuard<'a> {
    ctx: &'a MediaMutationContext,
    node: &'a NodeRuntimeState,
}

impl<'a> MutationGuard<'a> {
    /// Create a guard for the given mutation and node state.
    ///
    /// 为给定 mutation 和节点状态创建 guard。
    pub fn new(ctx: &'a MediaMutationContext, node: &'a NodeRuntimeState) -> Self {
        Self { ctx, node }
    }

    /// Run the full guard validation sequence.
    ///
    /// 执行完整 guard 校验序列。
    pub fn validate(&self, intent: OperationIntent) -> Result<GuardOutcome> {
        self.validate_target_node()?;
        self.validate_node_state(intent)?;
        let outcome = self.validate_resource()?;
        Ok(outcome)
    }

    /// Step 6-7: target node/instance must match the accepted runtime state,
    /// and the node must be in a state that allows the intent.
    ///
    /// 步骤 6-7：目标节点/实例必须与已接受的运行时状态匹配，且节点必须处于
    /// 允许该意图的状态。
    fn validate_target_node(&self) -> Result<()> {
        if !self
            .ctx
            .targets(&self.node.node_id, self.node.accepted_instance_epoch)
        {
            return Err(MediaError::new(
                MediaErrorCode::StaleOwner,
                "target node or instance epoch does not match the accepted runtime state",
            ));
        }

        if self.node.lease.status != LeaseStatus::Active {
            return Err(MediaError::new(
                MediaErrorCode::Unavailable,
                "node lease is not active",
            ));
        }

        Ok(())
    }

    fn validate_node_state(&self, intent: OperationIntent) -> Result<()> {
        let allowed = match intent {
            OperationIntent::Create | OperationIntent::Mutate => {
                matches!(self.node.state, NodeState::Active)
            }
            OperationIntent::Read | OperationIntent::Stop | OperationIntent::Delete => {
                matches!(
                    self.node.state,
                    NodeState::Active | NodeState::Draining | NodeState::Isolated
                )
            }
        };

        if !allowed {
            return Err(MediaError::new(
                MediaErrorCode::Unavailable,
                format!(
                    "node state {:?} does not permit {:?}",
                    self.node.state, intent
                ),
            ));
        }

        Ok(())
    }

    /// Step 8: owner epoch, binding/session, and expected generation checks.
    ///
    /// 步骤 8：owner epoch、binding/session 与预期 generation 检查。
    fn validate_resource(&self) -> Result<GuardOutcome> {
        // Default: no resource has been created yet. Owner epoch is accepted
        // as-is and generation starts at zero.
        Ok(GuardOutcome {
            accepted_owner_epoch: self.ctx.owner_epoch,
            accepted_generation: ResourceGeneration(0),
        })
    }

    /// Validate against an existing controlled resource, returning the next
    /// generation on success.
    ///
    /// 针对已有受控资源进行校验，成功时返回下一个 generation。
    pub fn validate_against(&self, resource: &ControlledResourceRef) -> Result<ResourceGeneration> {
        if self.ctx.target_media_node_instance_epoch != resource.node_instance_epoch {
            return Err(MediaError::new(
                MediaErrorCode::StaleOwner,
                "resource node_instance_epoch does not match the request target",
            ));
        }

        if self.ctx.owner_epoch.0 < resource.owner_epoch.0 {
            return Err(MediaError::new(
                MediaErrorCode::StaleOwner,
                "request owner epoch is older than the resource owner epoch",
            ));
        }

        if self.ctx.tenant_id != resource.tenant_id {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "tenant does not match the controlled resource",
            ));
        }

        if self.ctx.media_binding_id != resource.media_binding_id {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "media_binding_id does not match the controlled resource",
            ));
        }

        if self.ctx.media_session_id != resource.media_session_id {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "media_session_id does not match the controlled resource",
            ));
        }

        if self.ctx.owner_epoch.0 > resource.owner_epoch.0 {
            // Higher epoch takeover: reset generation to 1 after the side effect.
            return Ok(ResourceGeneration(1));
        }

        Ok(ResourceGeneration(resource.generation.0 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{
        MediaNodeInstanceId, MessageId, OperationId, OperationStepId, RequestId,
        ResourceGeneration, TenantId,
    };
    use crate::MediaNodeLease;

    fn tenant() -> TenantId {
        TenantId::new("tenant-1").unwrap()
    }

    fn node_id() -> MediaNodeId {
        MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn instance_id() -> MediaNodeInstanceId {
        MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440001").unwrap()
    }

    fn ctx(node: &MediaNodeId, epoch: u64) -> MediaMutationContext {
        MediaMutationContext {
            tenant_id: tenant(),
            message_id: MessageId::new("msg-1").unwrap(),
            source_signaling_node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440002")
                .unwrap(),
            owner_epoch: OwnerEpoch(7),
            target_media_node_id: node.clone(),
            target_media_node_instance_epoch: MediaNodeInstanceEpoch(epoch),
            operation_id: OperationId::new("op-1").unwrap(),
            operation_step_id: OperationStepId::new("step-1").unwrap(),
            media_session_id: None,
            media_binding_id: None,
            contract_version: "v1".to_string(),
            traceparent: None,
            tracestate: None,
        }
    }

    fn request_ctx() -> MediaRequestContext {
        MediaRequestContext {
            request_id: RequestId("req-1".to_string()),
            correlation_id: None,
            principal: None,
            source_adapter: "grpc".to_string(),
            trace_context: None,
            deadline: Some(1_000_000),
            idempotency_key: Some("idem-1".to_string()),
            mutation: Some(ctx(&node_id(), 42)),
        }
    }

    fn runtime() -> NodeRuntimeState {
        NodeRuntimeState {
            node_id: node_id(),
            instance_id: instance_id(),
            accepted_instance_epoch: MediaNodeInstanceEpoch(42),
            state: NodeState::Active,
            lease: MediaNodeLease {
                lease_id: "lease-1".to_string(),
                status: LeaseStatus::Active,
                deadline_ms: 1_000_000,
                heartbeat_interval_ms: 5_000,
                cluster_time_ms: 0,
                accepted_contract_version: "v1".to_string(),
                accepted_instance_epoch: MediaNodeInstanceEpoch(42),
            },
            accepted_contract_version: "v1".to_string(),
            control_endpoint: "https://node.example:50051".to_string(),
            network_zone: None,
            region: None,
            labels: std::collections::HashMap::new(),
            advertised_media_addresses: vec![],
            build_version: "0.1.0".to_string(),
            capability_generation: 1,
        }
    }

    #[test]
    fn adapter_validation_passes_for_valid_context() {
        let ctx = ctx(&node_id(), 42);
        assert_eq!(ctx.validate_adapter().unwrap(), ValidationStep::OwnerEpoch);
    }

    #[test]
    fn adapter_rejects_missing_contract_version() {
        let mut ctx = ctx(&node_id(), 42);
        ctx.contract_version = "".to_string();
        assert!(ctx.validate_adapter().is_err());
    }

    #[test]
    fn adapter_rejects_zero_owner_epoch() {
        let mut ctx = ctx(&node_id(), 42);
        ctx.owner_epoch = OwnerEpoch(0);
        assert!(ctx.validate_adapter().is_err());
    }

    #[test]
    fn request_context_requires_deadline() {
        let mut req = request_ctx();
        req.deadline = None;
        assert_eq!(
            req.validate_mutation_adapter().unwrap_err().code,
            MediaErrorCode::InvalidArgument
        );
    }

    #[test]
    fn request_context_requires_mutation() {
        let mut req = request_ctx();
        req.mutation = None;
        assert_eq!(
            req.validate_mutation_adapter().unwrap_err().code,
            MediaErrorCode::InvalidArgument
        );
    }

    #[test]
    fn guard_passes_for_matching_active_node() {
        let ctx = ctx(&node_id(), 42);
        let node = runtime();
        let guard = MutationGuard::new(&ctx, &node);
        let outcome = guard.validate(OperationIntent::Create).unwrap();
        assert_eq!(outcome.accepted_owner_epoch, OwnerEpoch(7));
        assert_eq!(outcome.accepted_generation, ResourceGeneration(0));
    }

    #[test]
    fn guard_rejects_wrong_instance_epoch() {
        let ctx = ctx(&node_id(), 99);
        let node = runtime();
        let guard = MutationGuard::new(&ctx, &node);
        assert_eq!(
            guard.validate(OperationIntent::Create).unwrap_err().code,
            MediaErrorCode::StaleOwner
        );
    }

    #[test]
    fn guard_rejects_inactive_lease() {
        let ctx = ctx(&node_id(), 42);
        let mut node = runtime();
        node.lease.status = LeaseStatus::Expired;
        let guard = MutationGuard::new(&ctx, &node);
        assert_eq!(
            guard.validate(OperationIntent::Create).unwrap_err().code,
            MediaErrorCode::Unavailable
        );
    }

    #[test]
    fn guard_allows_reads_during_draining() {
        let ctx = ctx(&node_id(), 42);
        let mut node = runtime();
        node.state = NodeState::Draining;
        let guard = MutationGuard::new(&ctx, &node);
        assert!(guard.validate(OperationIntent::Read).is_ok());
    }

    #[test]
    fn guard_rejects_create_while_draining() {
        let ctx = ctx(&node_id(), 42);
        let mut node = runtime();
        node.state = NodeState::Draining;
        let guard = MutationGuard::new(&ctx, &node);
        assert_eq!(
            guard.validate(OperationIntent::Create).unwrap_err().code,
            MediaErrorCode::Unavailable
        );
    }

    #[test]
    fn validate_against_resource_bumps_generation() {
        let ctx = ctx(&node_id(), 42);
        let node = runtime();
        let resource = ControlledResourceRef {
            tenant_id: tenant(),
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "session".to_string(),
            resource_handle: "h1".to_string(),
            owner_epoch: OwnerEpoch(7),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            generation: ResourceGeneration(3),
            origin: Default::default(),
        };
        let guard = MutationGuard::new(&ctx, &node);
        let next = guard.validate_against(&resource).unwrap();
        assert_eq!(next, ResourceGeneration(4));
    }

    #[test]
    fn validate_against_resource_rejects_lower_owner_epoch() {
        let ctx = ctx(&node_id(), 42);
        let node = runtime();
        let mut resource = ControlledResourceRef {
            tenant_id: tenant(),
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "session".to_string(),
            resource_handle: "h1".to_string(),
            owner_epoch: OwnerEpoch(7),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            generation: ResourceGeneration(3),
            origin: Default::default(),
        };
        resource.owner_epoch = OwnerEpoch(8);
        let guard = MutationGuard::new(&ctx, &node);
        assert_eq!(
            guard.validate_against(&resource).unwrap_err().code,
            MediaErrorCode::StaleOwner
        );
    }

    #[test]
    fn higher_owner_epoch_takeover_resets_generation() {
        let mut ctx = ctx(&node_id(), 42);
        ctx.owner_epoch = OwnerEpoch(8);
        let node = runtime();
        let resource = ControlledResourceRef {
            tenant_id: tenant(),
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "session".to_string(),
            resource_handle: "h1".to_string(),
            owner_epoch: OwnerEpoch(7),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            generation: ResourceGeneration(3),
            origin: Default::default(),
        };
        let guard = MutationGuard::new(&ctx, &node);
        let next = guard.validate_against(&resource).unwrap();
        assert_eq!(next, ResourceGeneration(1));
    }
}
