//! Deterministic fault injection hooks for control-plane mutation tests.
//!
//! 控制面变更测试的确定性故障注入点。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The points in a mutation pipeline where a fault can be injected.
///
/// 故障可注入的变更流程节点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    BeforeIdempotencyPrepare,
    AfterIdempotencyPrepare,
    BeforeCapacity,
    AfterCapacity,
    BeforeSideEffect,
    AfterSideEffect,
    BeforeResultCommit,
    AfterResultCommit,
    ResponseLoss,
    EventAppendLoss,
    EventSendLoss,
    SqliteBusy,
    SqliteFull,
    SqliteCorrupt,
    RegistryTimeout,
    LeaseExpiry,
    InstanceReplacement,
    WorkerPanic,
    ModuleRestart,
    EngineShutdown,
    SlowRpc,
    SlowSubscriber,
    TlsRotate,
}

/// The action a fault injection should take at a given point.
///
/// 故障注入动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAction {
    /// Inject a deterministic error once, then clear.
    FailOnce,
    /// Inject a deterministic error until explicitly reset.
    FailUntilReset,
    /// Delay the step by the specified milliseconds.
    Delay(u64),
    /// Panic the worker thread/task.
    Panic,
    /// Stall the step until the fault is cleared.
    Stall,
    /// Drop the message/response as if lost.
    Drop,
    /// Skip the step without side effects.
    Skip,
    /// Succeed normally.
    Succeed,
}

impl FaultAction {
    /// Return true for actions that survive `FaultInjector::reset`.
    const fn is_persistent(self) -> bool {
        !matches!(self, FaultAction::FailUntilReset | FaultAction::Stall)
    }
}

/// Trait for deterministic fault injection.
///
/// 确定性故障注入 trait。
pub trait FaultInjector: Send + Sync {
    /// Return the action to take at `point`, if any.
    fn inject(&self, point: FaultPoint) -> Option<FaultAction>;

    /// Reset `FailUntilReset` and `Stall` faults to normal.
    ///
    /// One-shot actions (`FailOnce`, `Delay`, `Panic`, `Drop`, `Skip`) persist.
    fn reset(&self);
}

/// A no-op fault injector that never triggers.
///
/// 从不触发故障的空实现。
#[derive(Debug, Default)]
pub struct NullFaultInjector;

impl FaultInjector for NullFaultInjector {
    fn inject(&self, _point: FaultPoint) -> Option<FaultAction> {
        None
    }

    fn reset(&self) {}
}

/// In-memory deterministic fault injector keyed by `FaultPoint`.
///
/// 基于内存的确定性故障注入器。
#[derive(Debug, Default, Clone)]
pub struct DeterministicFaultInjector {
    rules: Arc<Mutex<HashMap<FaultPoint, FaultAction>>>,
}

impl DeterministicFaultInjector {
    /// Register a fault rule.
    pub fn set(&self, point: FaultPoint, action: FaultAction) {
        self.rules.lock().unwrap().insert(point, action);
    }

    /// Remove a fault rule.
    pub fn clear(&self, point: FaultPoint) {
        self.rules.lock().unwrap().remove(&point);
    }
}

impl FaultInjector for DeterministicFaultInjector {
    fn inject(&self, point: FaultPoint) -> Option<FaultAction> {
        let mut rules = self.rules.lock().unwrap();
        rules.get(&point).copied().map(|action| match action {
            FaultAction::FailOnce => {
                rules.remove(&point);
                FaultAction::FailOnce
            }
            other => other,
        })
    }

    fn reset(&self) {
        let mut rules = self.rules.lock().unwrap();
        rules.retain(|_, action| action.is_persistent());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_injector_never_triggers() {
        let injector = NullFaultInjector;
        assert!(injector.inject(FaultPoint::BeforeCapacity).is_none());
    }

    #[test]
    fn deterministic_injector_fails_once() {
        let injector = DeterministicFaultInjector::default();
        injector.set(FaultPoint::BeforeCapacity, FaultAction::FailOnce);
        assert_eq!(
            injector.inject(FaultPoint::BeforeCapacity),
            Some(FaultAction::FailOnce)
        );
        assert!(injector.inject(FaultPoint::BeforeCapacity).is_none());
    }

    #[test]
    fn reset_clears_stall_and_fail_until_reset_but_keeps_persistent() {
        let injector = DeterministicFaultInjector::default();
        injector.set(FaultPoint::SqliteBusy, FaultAction::Stall);
        injector.set(FaultPoint::SlowRpc, FaultAction::Delay(42));
        injector.reset();
        assert!(injector.inject(FaultPoint::SqliteBusy).is_none());
        assert_eq!(
            injector.inject(FaultPoint::SlowRpc),
            Some(FaultAction::Delay(42))
        );
    }
}
