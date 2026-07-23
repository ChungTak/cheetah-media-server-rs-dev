//! Simple per-principal request rate limiter for the RTP media API.
//!
//! RTP 媒体 API 的每 principal 请求速率限制器。

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_sdk::media_api::error::{MediaError, Result};
use parking_lot::Mutex;

/// Per-principal sliding-window rate limiter.
///
/// 按 principal 滑窗速率限制器。
#[derive(Clone)]
pub struct RateLimiter {
    /// Window size in microseconds.
    window_us: u64,
    /// Maximum number of requests allowed in the window.
    limit: usize,
    /// Request timestamps keyed by principal identity.
    state: Arc<Mutex<HashMap<String, Vec<MonoTime>>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given window in microseconds and request limit.
    /// A limit of 0 disables rate limiting.
    ///
    /// 使用给定的窗口（微秒）与请求上限创建新的速率限制器。上限为 0 时禁用限流。
    pub fn new(window_us: u64, limit: usize) -> Self {
        Self {
            window_us,
            limit,
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check whether the request for `principal` is within the configured rate limit.
    ///
    /// 检查 `principal` 的请求是否超过配置速率。
    pub fn check(&self, principal: &str, now: MonoTime) -> Result<()> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut state = self.state.lock();
        let timestamps = state.entry(principal.to_string()).or_default();
        let cutoff = now.as_micros().saturating_sub(self.window_us);
        timestamps.retain(|t| t.as_micros() >= cutoff);
        if timestamps.len() >= self.limit {
            return Err(MediaError::rate_limited(format!(
                "rate limit exceeded for {principal}: {} requests per {} us",
                self.limit, self.window_us
            )));
        }
        timestamps.push(now);
        Ok(())
    }
}

/// Extract a rate-limit key from a `MediaRequestContext`.
///
/// 从 `MediaRequestContext` 中提取限流键。
pub fn rate_limit_key(ctx: &cheetah_sdk::media_api::port::MediaRequestContext) -> String {
    if let Some(principal) = &ctx.principal {
        format!("{}:{}", ctx.source_adapter, principal.identity)
    } else {
        format!("{}:anonymous", ctx.source_adapter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_requests_under_limit_and_blocks_excess() {
        let limiter = RateLimiter::new(60_000_000, 2);
        let t0 = MonoTime::from_micros(0);
        limiter.check("alice", t0).unwrap();
        limiter.check("alice", MonoTime::from_micros(1000)).unwrap();
        assert!(limiter.check("alice", MonoTime::from_micros(2000)).is_err());
        // Different principal is unaffected.
        limiter.check("bob", MonoTime::from_micros(3000)).unwrap();
    }

    #[test]
    fn rate_limiter_sliding_window_refreshes() {
        let limiter = RateLimiter::new(60_000_000, 1);
        limiter.check("alice", MonoTime::from_micros(0)).unwrap();
        assert!(limiter
            .check("alice", MonoTime::from_micros(1_000_000))
            .is_err());
        assert!(limiter
            .check("alice", MonoTime::from_micros(60_001_000))
            .is_ok());
    }
}
