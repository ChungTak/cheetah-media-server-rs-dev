//! Runtime-neutral async semaphore used to cap image-processing concurrency.
//!
//! Only compiled when `media-processing-image` is enabled. Supports a fixed
//! permit count for tests and a dynamic count backed by the shared module
//! configuration, so hot-reloaded `max_concurrent_jobs` takes effect without a
//! module restart.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;

use crate::config::{MediaProcessingModuleConfig, MAX_CONCURRENT_JOBS};

enum Max {
    Config(Arc<Mutex<MediaProcessingModuleConfig>>),
}

struct SemaphoreInner {
    max: Max,
    active: Mutex<usize>,
    waiters: Mutex<VecDeque<oneshot::Sender<()>>>,
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
    /// Creates a new semaphore whose permit limit tracks `max_concurrent_jobs`
    /// in the shared module configuration.
    pub fn with_config(config: Arc<Mutex<MediaProcessingModuleConfig>>) -> Self {
        Self {
            inner: Arc::new(SemaphoreInner {
                max: Max::Config(config),
                active: Mutex::new(0),
                waiters: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Acquires a permit, waiting asynchronously until one is available.
    pub async fn acquire(&self) -> Permit {
        loop {
            let max = self.permit_limit();
            if let Some(permit) = self.try_acquire(max) {
                return permit;
            }

            let (tx, rx) = oneshot::channel();
            self.inner
                .waiters
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(tx);
            rx.await.ok();
        }
    }

    fn permit_limit(&self) -> usize {
        match &self.inner.max {
            Max::Config(cfg) => {
                cfg.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .max_concurrent_jobs as usize
            }
        }
        .min(MAX_CONCURRENT_JOBS as usize)
    }

    fn try_acquire(&self, max: usize) -> Option<Permit> {
        let mut active = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        if *active < max {
            *active += 1;
            Some(Permit {
                inner: self.inner.clone(),
            })
        } else {
            None
        }
    }

    /// Wakes all current waiters so they re-evaluate the current limit.
    ///
    /// Used after a hot configuration update increases `max_concurrent_jobs`.
    pub fn notify_waiters(&self) {
        let mut waiters = self.inner.waiters.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(tx) = waiters.pop_front() {
            let _ = tx.send(());
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
        let mut active = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = active.saturating_sub(1);
        let mut waiters = self.inner.waiters.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(tx) = waiters.pop_front() {
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

    fn cfg(max: u32) -> Arc<Mutex<MediaProcessingModuleConfig>> {
        let mut c = MediaProcessingModuleConfig::default();
        c.max_concurrent_jobs = max;
        Arc::new(Mutex::new(c))
    }

    #[tokio::test]
    async fn permits_limit_concurrency_until_released() {
        let sem = Semaphore::with_config(cfg(1));
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
        let sem = Semaphore::with_config(cfg(1));
        {
            let _p = sem.acquire().await;
        }
        let _p = sem.acquire().await;
    }

    #[tokio::test]
    async fn queued_waiter_receives_released_permit() {
        let sem = Semaphore::with_config(cfg(1));
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
        let sem = Semaphore::with_config(cfg(1));
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
        let sem = Semaphore::with_config(cfg(1));
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
        let config = cfg(1);
        let sem = Semaphore::with_config(config.clone());

        // Consume the only permit.
        let p1 = sem.acquire().await;

        // A new waiter should block because the limit is reached.
        let sem2 = sem.clone();
        let pending = tokio::spawn(async move { sem2.acquire().await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Increase the limit and notify; the waiter should proceed.
        config.lock().unwrap().max_concurrent_jobs = 2;
        sem.notify_waiters();

        let result = tokio::time::timeout(Duration::from_millis(500), pending).await;
        assert!(
            result.is_ok(),
            "waiter should be woken when limit increases"
        );

        drop(p1);
    }
}
