//! Runtime-neutral async semaphore used to cap image-processing concurrency.
//!
//! Only compiled when `media-processing-image` is enabled. Implemented with
//! `futures::channel::oneshot` so it does not depend on any specific runtime.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;

#[derive(Debug)]
struct SemaphoreState {
    permits: usize,
    waiters: VecDeque<oneshot::Sender<()>>,
}

/// Async permit-backed concurrency limiter.
#[derive(Clone, Debug)]
pub struct Semaphore {
    state: Arc<Mutex<SemaphoreState>>,
}

impl Semaphore {
    /// Creates a new semaphore with the given number of initial permits.
    pub fn new(permits: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(SemaphoreState {
                permits,
                waiters: VecDeque::new(),
            })),
        }
    }

    /// Acquires a permit, waiting asynchronously until one is available.
    pub async fn acquire(&self) -> Permit {
        loop {
            let rx = {
                let mut state = self.state.lock().unwrap();
                if state.permits > 0 {
                    state.permits -= 1;
                    return Permit {
                        state: Arc::clone(&self.state),
                    };
                }
                let (tx, rx) = oneshot::channel();
                state.waiters.push_back(tx);
                rx
            };

            // A successful wakeup means a permit was handed directly to us.
            // If the sender was dropped without sending, retry the loop.
            if rx.await.is_ok() {
                return Permit {
                    state: Arc::clone(&self.state),
                };
            }
        }
    }
}

/// A held semaphore permit. Releases the permit on drop.
pub struct Permit {
    state: Arc<Mutex<SemaphoreState>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        // Hand the permit off to the next waiter. If that waiter was canceled
        // (receiver dropped), keep looking for a live waiter or restore the
        // permit count.
        while let Some(tx) = state.waiters.pop_front() {
            if tx.send(()).is_ok() {
                return;
            }
        }
        state.permits += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permits_limit_concurrency_until_released() {
        let sem = Semaphore::new(1);
        let p1 = sem.acquire().await;

        let sem2 = sem.clone();
        let pending = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_millis(50), sem2.acquire()).await
        });

        assert!(pending.await.unwrap().is_err());

        // Once the first permit is dropped, the queued waiter should proceed.
        drop(p1);
        let _p2 = sem.acquire().await;
    }

    #[tokio::test]
    async fn released_permit_is_reused() {
        let sem = Semaphore::new(1);
        {
            let _p = sem.acquire().await;
        }
        let _p = sem.acquire().await;
    }

    #[tokio::test]
    async fn queued_waiter_receives_released_permit() {
        let sem = Semaphore::new(1);
        let p1 = sem.acquire().await;

        let sem2 = sem.clone();
        let pending = tokio::spawn(async move { sem2.acquire().await });

        // Give the spawned task time to reach the wait queue.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        drop(p1);

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), pending).await;
        assert!(
            result.is_ok(),
            "queued waiter should obtain the released permit"
        );
    }
}
