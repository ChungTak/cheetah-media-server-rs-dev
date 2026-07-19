//! Runtime-neutral async semaphore used to cap image-processing concurrency.
//!
//! Only compiled when `media-processing-image` is enabled. The limit is stored
//! inside the same mutex that tracks active permits, so hot-reloaded
//! `max_concurrent_jobs` updates and waiter wakeups are atomic and cannot miss
//! a slot that has just become available.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;

use crate::config::MAX_CONCURRENT_JOBS;

struct State {
    max: usize,
    active: usize,
    waiters: VecDeque<oneshot::Sender<()>>,
}

struct SemaphoreInner {
    state: Mutex<State>,
}

/// Async permit-backed concurrency limiter.
#[derive(Clone)]
pub struct Semaphore {
    inner: Arc<SemaphoreInner>,
}

impl std::fmt::Debug for Semaphore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Semaphore").finish()
    }
}

impl Semaphore {
    /// Creates a new semaphore with the given maximum number of concurrent
    /// permits, clamped to [`MAX_CONCURRENT_JOBS`].
    pub fn with_max(max: usize) -> Self {
        Self {
            inner: Arc::new(SemaphoreInner {
                state: Mutex::new(State {
                    max: max.min(MAX_CONCURRENT_JOBS as usize),
                    active: 0,
                    waiters: VecDeque::new(),
                }),
            }),
        }
    }

    /// Updates the limit and wakes all current waiters so they re-evaluate.
    ///
    /// Used when a hot configuration change alters `max_concurrent_jobs`.
    pub fn set_max(&self, max: usize) {
        let max = max.min(MAX_CONCURRENT_JOBS as usize);
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.max = max;
        while let Some(tx) = state.waiters.pop_front() {
            let _ = tx.send(());
        }
    }

    /// Acquires a permit, waiting asynchronously until one is available.
    pub async fn acquire(&self) -> Permit {
        loop {
            let (tx, rx) = oneshot::channel();
            {
                let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.active < state.max {
                    state.active += 1;
                    return Permit {
                        inner: self.inner.clone(),
                    };
                }
                state.waiters.push_back(tx);
            }
            rx.await.ok();
        }
    }
}

/// A held semaphore permit. Releases the permit on drop.
pub struct Permit {
    inner: Arc<SemaphoreInner>,
}

impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit").finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = state.active.saturating_sub(1);
        while let Some(tx) = state.waiters.pop_front() {
            if tx.send(()).is_ok() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn permits_limit_concurrency_until_released() {
        let sem = Semaphore::with_max(1);
        let p1 = sem.acquire().await;

        let sem2 = sem.clone();
        let pending = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_millis(50), sem2.acquire()).await
        });

        assert!(pending.await.unwrap().is_err());

        // Once the first permit is dropped, the queued waiter should proceed.
        drop(p1);
        let _p2 = sem.acquire().await;
    }

    #[tokio::test]
    async fn released_permit_is_reused() {
        let sem = Semaphore::with_max(1);
        {
            let _p = sem.acquire().await;
        }
        let _p = sem.acquire().await;
    }

    #[tokio::test]
    async fn queued_waiter_receives_released_permit() {
        let sem = Semaphore::with_max(1);
        let p1 = sem.acquire().await;

        let sem2 = sem.clone();
        let pending = tokio::spawn(async move { sem2.acquire().await });

        // Give the spawned task time to reach the wait queue.
        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(p1);

        let result = tokio::time::timeout(Duration::from_millis(500), pending).await;
        assert!(
            result.is_ok(),
            "queued waiter should obtain the released permit"
        );
    }

    #[tokio::test]
    async fn canceled_waiter_at_front_does_not_block_live_waiter() {
        let sem = Semaphore::with_max(1);
        let p1 = sem.acquire().await;

        // W1 will time out and be canceled while in the waiter queue.
        let sem2 = sem.clone();
        let w1 = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_millis(50), sem2.acquire()).await
        });

        // W2 is queued behind W1.
        let sem3 = sem.clone();
        let w2 = tokio::spawn(async move { sem3.acquire().await });

        // Let both tasks enter the wait queue.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(w1.await.unwrap().is_err(), "w1 should time out");

        // Releasing should skip the canceled W1 and wake W2.
        drop(p1);
        let result = tokio::time::timeout(Duration::from_millis(500), w2).await;
        assert!(
            result.is_ok(),
            "live waiter behind a canceled waiter should still proceed"
        );
    }

    #[tokio::test]
    async fn canceled_waiter_after_wake_does_not_lose_permit() {
        let sem = Semaphore::with_max(1);
        let _p1 = sem.acquire().await;

        // Start a waiter that will block until a permit is released.
        let sem2 = sem.clone();
        let w1 = tokio::spawn(async move { sem2.acquire().await });

        // Give w1 time to park on the empty channel.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Drop the held permit. w1 will be woken and receive a Permit.
        drop(_p1);

        // Abort w1 before it can use the Permit and wait for the task to drop
        // its Permit. The Permit it received must be returned to the pool on
        // drop, so a new waiter can proceed.
        w1.abort();
        let _ = w1.await;

        let _p2 = tokio::time::timeout(Duration::from_millis(500), sem.acquire())
            .await
            .expect("permit should not be lost after canceled waiter");
    }

    #[tokio::test]
    async fn dynamic_limit_increase_wakes_waiters() {
        let sem = Semaphore::with_max(1);

        // Consume the only permit.
        let p1 = sem.acquire().await;

        // A new waiter should block because the limit is reached.
        let sem2 = sem.clone();
        let pending = tokio::spawn(async move { sem2.acquire().await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Increase the limit and notify; the waiter should proceed.
        sem.set_max(2);

        let result = tokio::time::timeout(Duration::from_millis(500), pending).await;
        assert!(
            result.is_ok(),
            "waiter should be woken when limit increases"
        );

        drop(p1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_acquire_and_release_do_not_hang() {
        let sem = Semaphore::with_max(2);

        let mut handles = Vec::new();
        for _ in 0..20 {
            let sem2 = sem.clone();
            handles.push(tokio::spawn(async move {
                let _p = sem2.acquire().await;
                tokio::time::sleep(Duration::from_micros(100)).await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
    }
}
