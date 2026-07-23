//! Driver-wide load and resource limits.
//!
//! Tracks active sessions, active TCP connections and the rolling incoming byte rate
//! so the driver can reject new work before it reaches the core or the runtime.
//!
//! 驱动级负载与资源限制。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_runtime_api::RuntimeApi;
use tracing::warn;

/// Driver-side resource limits.
///
/// A value of `0` means the corresponding resource is unlimited.
/// `max_incoming_bytes_per_second` is a per-second cap; the rolling measurement
/// window is `bytes_rate_window_ms` (default 1000 ms) and the per-window budget
/// is scaled proportionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverLimits {
    pub max_sessions: usize,
    pub max_tcp_connections: usize,
    pub max_incoming_bytes_per_second: u64,
    pub bytes_rate_window_ms: u64,
}

impl Default for DriverLimits {
    fn default() -> Self {
        Self {
            max_sessions: 0,
            max_tcp_connections: 0,
            max_incoming_bytes_per_second: 0,
            bytes_rate_window_ms: 1_000,
        }
    }
}

/// Atomic resource tracker used by the driver loop and I/O tasks.
#[derive(Clone)]
pub(crate) struct LoadLimiter {
    runtime: std::sync::Arc<dyn RuntimeApi>,
    limits: DriverLimits,
    active_sessions: Arc<AtomicU64>,
    active_tcp_connections: Arc<AtomicU64>,
    bytes_window_start_ms: Arc<AtomicU64>,
    bytes_in_window: Arc<AtomicU64>,
    /// Last window start (ms) for which a byte-rate warning was emitted.
    last_warn_window_ms: Arc<AtomicU64>,
}

impl LoadLimiter {
    pub(crate) fn new(runtime: std::sync::Arc<dyn RuntimeApi>, limits: DriverLimits) -> Self {
        Self {
            runtime,
            limits,
            active_sessions: Arc::new(AtomicU64::new(0)),
            active_tcp_connections: Arc::new(AtomicU64::new(0)),
            bytes_window_start_ms: Arc::new(AtomicU64::new(0)),
            bytes_in_window: Arc::new(AtomicU64::new(0)),
            last_warn_window_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn session_created(&self) {
        self.active_sessions.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn session_closed(&self) {
        self.active_sessions.fetch_sub(1, Ordering::SeqCst);
    }

    /// Try to acquire a slot for a new TCP connection. Returns `false` if the
    /// connection limit is exceeded.
    pub(crate) fn try_new_tcp_connection(&self) -> bool {
        if self.limits.max_tcp_connections == 0 {
            return true;
        }
        let prev = self.active_tcp_connections.fetch_add(1, Ordering::SeqCst);
        if prev >= self.limits.max_tcp_connections as u64 {
            self.active_tcp_connections.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    pub(crate) fn release_tcp_connection(&self) {
        self.active_tcp_connections.fetch_sub(1, Ordering::SeqCst);
    }

    /// RAII guard that releases a TCP connection slot when dropped.
    pub(crate) fn tcp_connection_guard(&self) -> TcpConnectionGuard {
        TcpConnectionGuard(self.clone())
    }

    /// Per-window byte budget derived from `max_incoming_bytes_per_second`
    /// and `bytes_rate_window_ms`. A window of 1000 ms is the identity.
    fn byte_budget(&self) -> u64 {
        let window_ms = self.limits.bytes_rate_window_ms.max(1);
        self.limits
            .max_incoming_bytes_per_second
            .saturating_mul(window_ms)
            / 1000
    }

    /// Record `n` incoming bytes and return `true` if the current measurement
    /// window is within budget.
    pub(crate) fn try_consume_bytes(&self, n: usize) -> bool {
        if self.limits.max_incoming_bytes_per_second == 0 {
            return true;
        }

        let window_ms = self.limits.bytes_rate_window_ms.max(1);
        let now_ms = self.runtime.now().as_micros() / 1000;

        let mut start = self.bytes_window_start_ms.load(Ordering::Relaxed);
        let mut attempts = 0;
        loop {
            if now_ms.saturating_sub(start) >= window_ms {
                // Try to advance the window to `now_ms` and reset the counter.
                let swapped = self
                    .bytes_window_start_ms
                    .compare_exchange(start, now_ms, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok();
                if swapped {
                    self.bytes_in_window.store(0, Ordering::SeqCst);
                }
                start = self.bytes_window_start_ms.load(Ordering::Relaxed);
            }

            let current = self.bytes_in_window.load(Ordering::Relaxed);
            if now_ms.saturating_sub(start) >= window_ms {
                // Another thread reset the window; retry.
                attempts += 1;
                if attempts > 8 {
                    // Avoid infinite loop under extreme contention; allow the packet.
                    return true;
                }
                continue;
            }

            let budget = self.byte_budget();
            if current.saturating_add(n as u64) > budget {
                // Rate-limit the warning to one per measurement window so an overload
                // does not generate per-packet log traffic.
                if self
                    .last_warn_window_ms
                    .compare_exchange(start, start, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    warn!("incoming byte rate limit exceeded: {current} + {n} > {budget}");
                }
                return false;
            }

            if self
                .bytes_in_window
                .compare_exchange(
                    current,
                    current + n as u64,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return true;
            }
            // Retry on contention.
        }
    }

    /// Check whether a new session would currently violate limits without
    /// actually acquiring a slot.
    pub(crate) fn allow_new_session(&self) -> bool {
        let session_ok = self.limits.max_sessions == 0
            || self.active_sessions.load(Ordering::SeqCst) < self.limits.max_sessions as u64;

        let byte_rate_ok = if self.limits.max_incoming_bytes_per_second == 0 {
            true
        } else {
            let window_ms = self.limits.bytes_rate_window_ms.max(1);
            let now_ms = self.runtime.now().as_micros() / 1000;
            let start = self.bytes_window_start_ms.load(Ordering::Relaxed);
            if now_ms.saturating_sub(start) >= window_ms {
                true
            } else {
                let bytes = self.bytes_in_window.load(Ordering::Relaxed);
                bytes < self.byte_budget()
            }
        };

        session_ok && byte_rate_ok
    }
}

pub(crate) struct TcpConnectionGuard(LoadLimiter);

impl Drop for TcpConnectionGuard {
    fn drop(&mut self) {
        self.0.release_tcp_connection();
    }
}
