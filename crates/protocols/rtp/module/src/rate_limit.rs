//! Simple per-principal request rate limiter for the RTP media API.
//!
//! RTP 媒体 API 的每 principal 请求速率限制器。

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_sdk::media_api::error::{MediaError, Result};
use parking_lot::Mutex;

/// Default upper bound on distinct principals tracked at once. The map is bounded
/// to avoid unbounded growth under a flood of unique source identifiers.
///
/// 默认同时跟踪的 principal 数量上限，防止在大量唯一源标识下无限制增长。
const DEFAULT_MAX_PRINCIPALS: usize = 10_000;

struct Entry {
    timestamps: Vec<MonoTime>,
    last_seen: MonoTime,
}

/// Per-principal sliding-window rate limiter.
///
/// 按 principal 滑窗速率限制器。
#[derive(Clone)]
pub struct RateLimiter {
    /// Window size in microseconds.
    window_us: u64,
    /// Maximum number of requests allowed in the window.
    limit: usize,
    /// Maximum number of distinct principals retained at once.
    max_principals: usize,
    /// Request timestamps keyed by principal identity.
    state: Arc<Mutex<HashMap<String, Entry>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given window in microseconds and request limit.
    /// A limit of 0 disables rate limiting.
    ///
    /// 使用给定的窗口（微秒）与请求上限创建新的速率限制器。上限为 0 时禁用限流。
    pub fn new(window_us: u64, limit: usize) -> Self {
        Self::with_max_principals(window_us, limit, DEFAULT_MAX_PRINCIPALS)
    }

    /// Create a new rate limiter with an explicit bound on tracked principals.
    ///
    /// 使用显式 principal 数量上限创建新的速率限制器。
    pub fn with_max_principals(window_us: u64, limit: usize, max_principals: usize) -> Self {
        Self {
            window_us,
            limit,
            max_principals,
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
        let is_new = !state.contains_key(principal);

        // If we are at capacity and this is a new principal, evict the least
        // recently seen principal before growing the map.
        if is_new && state.len() >= self.max_principals {
            let oldest = state
                .iter()
                .min_by_key(|(_, e)| e.last_seen)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                state.remove(&oldest);
            }
        }

        let cutoff = now.as_micros().saturating_sub(self.window_us);
        let entry = state.entry(principal.to_string()).or_insert(Entry {
            timestamps: Vec::new(),
            last_seen: now,
        });

        entry.timestamps.retain(|t| t.as_micros() >= cutoff);
        entry.last_seen = now;

        if entry.timestamps.len() >= self.limit {
            return Err(MediaError::rate_limited(format!(
                "rate limit exceeded for {principal}: {} requests per {} us",
                self.limit, self.window_us
            )));
        }

        entry.timestamps.push(now);

        // Remove stale empty entries to keep the map bounded and tidy.
        if entry.timestamps.is_empty() {
            state.remove(principal);
        }

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

    #[test]
    fn rate_limiter_evicts_oldest_principal_when_full() {
        let limiter = RateLimiter::with_max_principals(60_000_000, 1, 2);
        limiter.check("alice", MonoTime::from_micros(0)).unwrap();
        limiter.check("bob", MonoTime::from_micros(1)).unwrap();
        // Capacity reached; adding "charlie" evicts the least recently seen principal ("alice").
        limiter.check("charlie", MonoTime::from_micros(2)).unwrap();
        // "alice" has been evicted, so her next request is treated as a new principal.
        limiter.check("alice", MonoTime::from_micros(3)).unwrap();
    }
}
