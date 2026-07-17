//! Runtime-neutral async semaphore used to cap image-processing concurrency.
//!
//! Only compiled when `media-processing-image` is enabled. Backed by a
//! bounded `futures::channel::mpsc` channel so that a permit is an owned token
//! returned to the channel on drop, including when a waiting task is cancelled.

use std::sync::Arc;

use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::stream::StreamExt;

/// Async permit-backed concurrency limiter.
#[derive(Clone)]
pub struct Semaphore {
    tx: mpsc::Sender<()>,
    rx: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl std::fmt::Debug for Semaphore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Semaphore").finish()
    }
}

impl Semaphore {
    /// Creates a new semaphore with the given number of initial permits.
    pub fn new(permits: usize) -> Self {
        let (mut tx, rx) = mpsc::channel(permits.max(1));
        for _ in 0..permits {
            // The channel is sized to hold all permits, so seeding cannot fail.
            tx.try_send(())
                .expect("semaphore channel has capacity for initial permits");
        }
        Self {
            tx,
            rx: Arc::new(Mutex::new(rx)),
        }
    }

    /// Acquires a permit, waiting asynchronously until one is available.
    pub async fn acquire(&self) -> Permit {
        let mut rx = self.rx.lock().await;
        rx.next()
            .await
            .expect("semaphore sender is always alive while a Semaphore exists");
        Permit {
            tx: self.tx.clone(),
        }
    }
}

/// A held semaphore permit. Releases the permit on drop.
pub struct Permit {
    tx: mpsc::Sender<()>,
}

impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit").finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        // Return the permit to the pool. If the channel is closed (all
        // `Semaphore` clones dropped) the permit is simply discarded.
        let _ = self.tx.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::future::{select, Either};

    use super::*;

    #[tokio::test]
    async fn permits_limit_concurrency_until_released() {
        let sem = Semaphore::new(1);
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
        let sem = Semaphore::new(1);
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
        let sem = Semaphore::new(1);
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
}
