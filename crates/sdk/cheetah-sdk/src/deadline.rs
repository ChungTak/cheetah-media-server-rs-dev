//! Deadline helpers for media providers, drivers, and adapters.
//!
//! `MediaRequestContext.deadline` carries an absolute Unix timestamp in
//! milliseconds. This module converts it to remaining durations and creates
//! cancellation children that fire when the deadline is reached.
//!
//! media 提供者、driver 与 adapter 使用的 deadline 工具。

use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_codec::MonoTime;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};

use crate::media_api::port::MediaRequestContext;

/// A request deadline as an absolute Unix timestamp in milliseconds.
///
/// 以绝对 Unix 毫秒时间戳表示的请求 deadline。
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    deadline_ms: Option<i64>,
}

impl Deadline {
    pub fn new(deadline_ms: Option<i64>) -> Self {
        Self { deadline_ms }
    }

    pub fn from_context(ctx: &MediaRequestContext) -> Self {
        Self::new(ctx.deadline)
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Remaining milliseconds until the deadline, or `None` if there is no
    /// deadline. Saturates at zero.
    ///
    /// 返回距离 deadline 的剩余毫秒数；无 deadline 时返回 `None`。
    pub fn remaining_ms(&self) -> Option<i64> {
        let deadline = self.deadline_ms?;
        let now = Self::now_ms();
        Some((deadline - now).max(0))
    }

    /// Remaining duration until the deadline, or `None`.
    ///
    /// 返回距离 deadline 的剩余时长；无 deadline 时返回 `None`。
    pub fn remaining(&self) -> Option<std::time::Duration> {
        self.remaining_ms()
            .map(|ms| std::time::Duration::from_millis(ms.max(0) as u64))
    }

    /// Returns true when a deadline exists and has already passed.
    ///
    /// 当存在 deadline 且已过期时返回 `true`。
    pub fn is_expired(&self) -> bool {
        matches!(self.remaining_ms(), Some(0))
    }

    /// Returns true when either no deadline is set or it has not yet expired.
    ///
    /// 无 deadline 或尚未过期时返回 `true`。
    pub fn is_valid(&self) -> bool {
        self.remaining_ms().map(|ms| ms > 0).unwrap_or(true)
    }

    /// Create a child cancellation token that is cancelled when the deadline
    /// expires. If `ctx` has no deadline, the child is only cancelled when the
    /// parent is cancelled. If the deadline is already expired, the child is
    /// cancelled immediately.
    ///
    /// 创建一个子取消 token；到达 deadline 时自动取消。若 ctx 无 deadline，
    /// 则只在父 token 取消时取消。若 deadline 已过期，子 token 立即取消。
    pub fn cancellation_child(
        &self,
        runtime: &dyn RuntimeApi,
        parent: &CancellationToken,
    ) -> CancellationToken {
        let child = parent.child_token();
        let Some(rem_ms) = self.remaining_ms() else {
            return child;
        };
        if rem_ms == 0 {
            child.cancel();
            return child;
        }

        let rem_us = rem_ms as u64 * 1000;
        let now = runtime.now();
        let target = MonoTime::from_micros(now.as_micros() + rem_us);
        let mut timer = runtime.sleep_until(target);
        let child_for_task = child.clone();
        runtime.spawn(Box::pin(async move {
            timer.wait().await;
            child_for_task.cancel();
        }));
        child
    }

    /// Check the deadline before starting an operation and return a
    /// runtime-neutral `DeadlineExceeded` SDK error if it has expired.
    ///
    /// 在操作开始前检查 deadline；已过期时返回 SDK `DeadlineExceeded` 错误。
    pub fn check(&self) -> Result<(), crate::SdkError> {
        if self.is_expired() {
            return Err(crate::SdkError::Unavailable(
                "request deadline exceeded".to_string(),
            ));
        }
        Ok(())
    }
}

/// Convenience function to create a cancellation child from a request context.
///
/// 从请求上下文创建取消子 token 的便捷函数。
pub fn cancellation_child(
    ctx: &MediaRequestContext,
    runtime: &dyn RuntimeApi,
    parent: &CancellationToken,
) -> CancellationToken {
    Deadline::from_context(ctx).cancellation_child(runtime, parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_runtime_api::{CancellationToken, RuntimeApi};
    use cheetah_runtime_tokio::TokioRuntime;

    #[test]
    fn past_deadline_is_expired() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let deadline = Deadline::new(Some(now - 1000));
        assert!(deadline.is_expired());
        assert!(!deadline.is_valid());
        assert!(deadline.check().is_err());
    }

    #[test]
    fn future_deadline_is_not_expired() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let deadline = Deadline::new(Some(now + 60_000));
        assert!(!deadline.is_expired());
        assert!(deadline.is_valid());
        assert!(deadline.check().is_ok());
    }

    #[test]
    fn missing_deadline_is_valid() {
        let deadline = Deadline::new(None);
        assert!(!deadline.is_expired());
        assert!(deadline.is_valid());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_child_fires_at_deadline() {
        let runtime: std::sync::Arc<dyn RuntimeApi> = std::sync::Arc::new(TokioRuntime::new());
        let parent = CancellationToken::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let ctx = MediaRequestContext {
            deadline: Some(now + 50),
            ..Default::default()
        };
        let child = cancellation_child(&ctx, runtime.as_ref(), &parent);
        assert!(!child.is_cancelled());
        child.cancelled().await;
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_deadline_cancels_child_immediately() {
        let runtime: std::sync::Arc<dyn RuntimeApi> = std::sync::Arc::new(TokioRuntime::new());
        let parent = CancellationToken::new();
        let ctx = MediaRequestContext {
            deadline: Some(0),
            ..Default::default()
        };
        let child = cancellation_child(&ctx, runtime.as_ref(), &parent);
        assert!(child.is_cancelled());
    }
}
