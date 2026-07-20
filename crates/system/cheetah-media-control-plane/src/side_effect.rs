//! Side-effect crash windows and recovery actions.
//!
//! Every mutation moves through a bounded set of windows. On restart the
//! control plane must know which window a request stopped in so it can apply
//! the right recovery rule and never create a second resource.
//!
//! 副作用崩溃窗口与恢复动作。每个变更操作都经过一组有界窗口；重启时
//! 控制面根据窗口选择正确的恢复规则，禁止创建第二个资源。

use std::fmt;

/// The crash window a mutation was in when the process stopped.
///
/// 变更操作在进程停止时所在的崩溃窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SideEffectWindow {
    /// No durable record exists yet; the caller can safely retry.
    BeforePrepared,
    /// A `PREPARED` idempotency record exists but the side effect has not run.
    PreparedBeforeEffect,
    /// The side effect ran but its outcome has not been persisted.
    EffectBeforePersistence,
    /// The outcome was persisted but the response was never sent.
    PersistenceBeforeResponse,
    /// The response was sent but the corresponding event was not emitted.
    ResponseBeforeEvent,
}

/// Recovery action selected for a crash window.
///
/// 崩溃窗口对应的恢复动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryAction {
    /// Safe to retry from the beginning.
    Retry,
    /// Continue the in-flight attempt using the prepared record.
    ContinueAttempt,
    /// Query the provider by idempotency/binding/handle to prove whether the
    /// side effect happened.
    QueryProvider,
    /// Replay the persisted outcome to the caller.
    Replay,
    /// Hand the ambiguous record to the reconciler; do not auto-execute.
    Reconcile,
}

impl SideEffectWindow {
    /// Return the default recovery action for this window.
    ///
    /// 返回该窗口的默认恢复动作。
    pub fn recovery_action(&self) -> RecoveryAction {
        match self {
            SideEffectWindow::BeforePrepared => RecoveryAction::Retry,
            SideEffectWindow::PreparedBeforeEffect => RecoveryAction::ContinueAttempt,
            SideEffectWindow::EffectBeforePersistence => RecoveryAction::QueryProvider,
            SideEffectWindow::PersistenceBeforeResponse => RecoveryAction::Replay,
            SideEffectWindow::ResponseBeforeEvent => RecoveryAction::Reconcile,
        }
    }
}

impl fmt::Display for SideEffectWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SideEffectWindow::BeforePrepared => "before_prepared",
            SideEffectWindow::PreparedBeforeEffect => "prepared_before_effect",
            SideEffectWindow::EffectBeforePersistence => "effect_before_persistence",
            SideEffectWindow::PersistenceBeforeResponse => "persistence_before_response",
            SideEffectWindow::ResponseBeforeEvent => "response_before_event",
        };
        write!(f, "{s}")
    }
}

impl fmt::Display for RecoveryAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RecoveryAction::Retry => "retry",
            RecoveryAction::ContinueAttempt => "continue_attempt",
            RecoveryAction::QueryProvider => "query_provider",
            RecoveryAction::Replay => "replay",
            RecoveryAction::Reconcile => "reconcile",
        };
        write!(f, "{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_window_recovery_actions_match_plan() {
        assert_eq!(
            SideEffectWindow::BeforePrepared.recovery_action(),
            RecoveryAction::Retry
        );
        assert_eq!(
            SideEffectWindow::PreparedBeforeEffect.recovery_action(),
            RecoveryAction::ContinueAttempt
        );
        assert_eq!(
            SideEffectWindow::EffectBeforePersistence.recovery_action(),
            RecoveryAction::QueryProvider
        );
        assert_eq!(
            SideEffectWindow::PersistenceBeforeResponse.recovery_action(),
            RecoveryAction::Replay
        );
        assert_eq!(
            SideEffectWindow::ResponseBeforeEvent.recovery_action(),
            RecoveryAction::Reconcile
        );
    }

    #[test]
    fn display_round_trips() {
        for window in [
            SideEffectWindow::BeforePrepared,
            SideEffectWindow::PreparedBeforeEffect,
            SideEffectWindow::EffectBeforePersistence,
            SideEffectWindow::PersistenceBeforeResponse,
            SideEffectWindow::ResponseBeforeEvent,
        ] {
            let s = window.to_string();
            assert!(!s.is_empty());
        }
    }
}
