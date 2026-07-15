use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Simple per-target circuit breaker.
///
/// 简单的单目标熔断器。
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    threshold: u32,
    open_duration: Duration,
    failures: Arc<AtomicU32>,
    last_failure: Arc<parking_lot::Mutex<Option<Instant>>>,
    last_success: Arc<parking_lot::Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, open_duration: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            open_duration,
            failures: Arc::new(AtomicU32::new(0)),
            last_failure: Arc::new(parking_lot::Mutex::new(None)),
            last_success: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    pub fn state(&self) -> CircuitState {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures >= self.threshold {
            let last_failure = *self.last_failure.lock();
            if let Some(inst) = last_failure {
                if inst.elapsed() < self.open_duration {
                    CircuitState::Open
                } else {
                    CircuitState::HalfOpen
                }
            } else {
                // Failure count recorded but no timestamp; allow a probe.
                CircuitState::HalfOpen
            }
        } else {
            CircuitState::Closed
        }
    }

    /// Returns true when the caller is allowed to attempt a request.
    pub fn allow(&self) -> bool {
        matches!(self.state(), CircuitState::Closed | CircuitState::HalfOpen)
    }

    pub fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        *self.last_success.lock() = Some(Instant::now());
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        *self.last_failure.lock() = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_starts_closed_and_allows() {
        let cb = CircuitBreaker::new(3, Duration::from_millis(100));
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow());
    }

    #[test]
    fn circuit_opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(100));
        cb.record_failure();
        assert!(cb.allow());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow());
    }

    #[test]
    fn circuit_transitions_to_half_open_after_timeout() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(10));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        assert!(cb.allow());
    }

    #[test]
    fn circuit_closes_on_success() {
        let cb = CircuitBreaker::new(2, Duration::from_millis(100));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }
}
