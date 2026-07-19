//! Idempotency repository for media operations.
//!
//! Keys are `principal identity + operation + idempotency-key`. Each record
//! stores the canonical SHA-256 fingerprint, the resulting resource id, the
//! terminal creation result and an expiry. Concurrent callers with the same key
//! and fingerprint either receive the cached result or wait for the in-flight
//! operation. Callers with a different fingerprint receive `Conflict`.
//!
//! 媒体操作的幂等仓库。键为 principal identity + operation + idempotency-key，
//! 值包含 canonical SHA-256 指纹、资源 id、终态结果与过期时间。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_runtime_api::{oneshot_channel, OneShotSender};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Composite key for an idempotent operation.
///
/// 幂等操作的复合键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub principal_identity: String,
    pub operation: String,
    pub idempotency_key: String,
}

impl IdempotencyKey {
    pub fn new(
        principal: impl Into<String>,
        operation: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            principal_identity: principal.into(),
            operation: operation.into(),
            idempotency_key: key.into(),
        }
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.principal_identity, self.operation, self.idempotency_key
        )
    }
}

/// 32-byte SHA-256 fingerprint of the canonical request.
///
/// canonical 请求的 32 字节 SHA-256 指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdempotencyFingerprint([u8; 32]);

impl IdempotencyFingerprint {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for IdempotencyFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// Compute a canonical SHA-256 fingerprint.
///
/// 计算 canonical SHA-256 指纹。
pub fn canonical_hash(input: impl AsRef<[u8]>) -> IdempotencyFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(input.as_ref());
    IdempotencyFingerprint::from_bytes(hasher.finalize().into())
}

/// Terminal outcome of an idempotent operation.
///
/// 幂等操作的终态结果。
#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyOutcome {
    Success { resource_id: String },
    Error { message: String },
}

/// Errors returned by the idempotency repository.
///
/// 幂等仓库返回的错误。
#[derive(Debug, Error, Clone, PartialEq)]
pub enum IdempotencyError {
    #[error("idempotency conflict for {key}: {message}")]
    Conflict {
        key: String,
        resource_id: Option<String>,
        message: String,
    },
    #[error("idempotency operation is still in progress")]
    InProgress,
    #[error("idempotency operation failed: {0}")]
    OperationFailed(String),
    #[error("idempotency operation retryable: {0}")]
    Retryable(String),
}

impl IdempotencyError {
    fn conflict(
        key: &IdempotencyKey,
        resource_id: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Conflict {
            key: key.to_string(),
            resource_id,
            message: message.into(),
        }
    }
}

struct StoredOutcome {
    value: Arc<dyn Any + Send + Sync>,
    resource_id: String,
}

enum Entry {
    InProgress {
        fingerprint: IdempotencyFingerprint,
        resource_id: Option<String>,
        waiters: Vec<OneShotSender>,
    },
    Completed {
        fingerprint: IdempotencyFingerprint,
        outcome: StoredOutcome,
        expiry: i64,
    },
    Error {
        fingerprint: IdempotencyFingerprint,
        message: String,
        expiry: i64,
    },
}

impl Entry {
    fn fingerprint(&self) -> IdempotencyFingerprint {
        match self {
            Entry::InProgress { fingerprint, .. } => *fingerprint,
            Entry::Completed { fingerprint, .. } => *fingerprint,
            Entry::Error { fingerprint, .. } => *fingerprint,
        }
    }

    fn resource_id(&self) -> Option<String> {
        match self {
            Entry::InProgress { resource_id, .. } => resource_id.clone(),
            Entry::Completed { outcome, .. } => Some(outcome.resource_id.clone()),
            Entry::Error { .. } => None,
        }
    }

    fn is_expired(&self, now: i64) -> bool {
        match self {
            Entry::Completed { expiry, .. } | Entry::Error { expiry, .. } => *expiry <= now,
            Entry::InProgress { .. } => false,
        }
    }
}

struct InProgressGuard {
    inner: Option<Arc<Mutex<HashMap<IdempotencyKey, Entry>>>>,
    key: Option<IdempotencyKey>,
}

impl InProgressGuard {
    fn new(inner: Arc<Mutex<HashMap<IdempotencyKey, Entry>>>, key: IdempotencyKey) -> Self {
        Self {
            inner: Some(inner),
            key: Some(key),
        }
    }

    fn defuse(&mut self) {
        self.inner = None;
        self.key = None;
    }
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        let (inner, key) = match (self.inner.take(), self.key.take()) {
            (Some(i), Some(k)) => (i, k),
            _ => return,
        };
        let mut map = inner.lock().unwrap();
        if let Some(Entry::InProgress { waiters, .. }) = map.remove(&key) {
            drop(map);
            for w in waiters {
                let _ = w.send();
            }
        }
    }
}

/// In-memory idempotency repository.
///
/// 内存中的幂等仓库。
#[derive(Clone)]
pub struct InMemoryIdempotencyRepository {
    inner: Arc<Mutex<HashMap<IdempotencyKey, Entry>>>,
}

impl Default for InMemoryIdempotencyRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIdempotencyRepository {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn check_existing<T: Clone + Send + Sync + 'static>(
        key: &IdempotencyKey,
        fingerprint: IdempotencyFingerprint,
        entry: &Entry,
    ) -> Option<Result<T, IdempotencyError>> {
        if entry.fingerprint() != fingerprint {
            return Some(Err(IdempotencyError::conflict(
                key,
                entry.resource_id(),
                "idempotency key already used with a different fingerprint",
            )));
        }
        match entry {
            Entry::Completed { outcome, .. } => {
                let arc = match outcome.value.clone().downcast::<T>() {
                    Ok(a) => a,
                    Err(_) => {
                        return Some(Err(IdempotencyError::OperationFailed(
                            "cached idempotency value has unexpected type".to_string(),
                        )))
                    }
                };
                Some(Ok((*arc).clone()))
            }
            Entry::Error { message, .. } => {
                Some(Err(IdempotencyError::OperationFailed(message.clone())))
            }
            Entry::InProgress { .. } => None,
        }
    }

    /// Execute `f` idempotently. If an operation with the same key and
    /// fingerprint already succeeded, the cached result is returned. If it is
    /// in progress, the caller waits for the existing result. A different
    /// fingerprint produces `IdempotencyError::Conflict`.
    ///
    /// `ttl_ms` controls how long completed records are retained.
    ///
    /// 幂等地执行 `f`。同键同指纹已成功的操作返回缓存结果；进行中的操作等待完成；
    /// 不同指纹返回 `Conflict`。`ttl_ms` 为完成记录的保留时长。
    pub async fn execute<F, Fut, T>(
        &self,
        key: IdempotencyKey,
        fingerprint: IdempotencyFingerprint,
        ttl_ms: i64,
        f: F,
    ) -> Result<T, IdempotencyError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<(T, Option<String>), IdempotencyError>> + Send,
        T: Clone + Send + Sync + 'static,
    {
        // Acquire an in-progress slot or a waiter. All sync work is done in
        // a block so the guard is dropped before any await point.
        loop {
            let waiter_rx = {
                let mut map = self.inner.lock().unwrap();
                let now = Self::now_ms();
                if let Some(entry) = map.get_mut(&key) {
                    if entry.is_expired(now) {
                        map.remove(&key);
                    } else if let Some(result) = Self::check_existing::<T>(&key, fingerprint, entry)
                    {
                        return result;
                    }
                }

                match map.get_mut(&key) {
                    Some(Entry::InProgress { waiters, .. }) => {
                        let (tx, rx) = oneshot_channel();
                        waiters.push(tx);
                        Some(rx)
                    }
                    Some(_) => unreachable!("check_existing handled non-in-progress entries"),
                    None => {
                        map.insert(
                            key.clone(),
                            Entry::InProgress {
                                fingerprint,
                                resource_id: None,
                                waiters: Vec::new(),
                            },
                        );
                        None
                    }
                }
            };

            if let Some(mut rx) = waiter_rx {
                rx.recv().await.map_err(|_| {
                    IdempotencyError::OperationFailed(
                        "idempotency waiter dropped before completion".to_string(),
                    )
                })?;
            } else {
                break;
            }
        }

        // Install a guard that removes the in-progress entry if the executor is
        // cancelled before completing.
        let mut guard = InProgressGuard::new(self.inner.clone(), key.clone());

        // Run the actual operation outside of the lock.
        let result = f().await;

        // Retryable errors are not persisted; dropping the guard removes the
        // in-progress entry so the next call can retry.
        if let Err(e) = &result {
            if matches!(e, IdempotencyError::Retryable(_)) {
                return Err(e.clone());
            }
        }

        // The operation has finished (success or error). Defuse the guard: we
        // will now commit a terminal outcome ourselves.
        guard.defuse();

        // Commit the terminal outcome, wake waiters and return.
        let waiters = {
            let mut map = self.inner.lock().unwrap();
            let entry = map.get_mut(&key);
            match (&result, entry) {
                (Ok((value, resource_id)), Some(Entry::InProgress { waiters, .. })) => {
                    let waiters = std::mem::take(waiters);
                    let resource_id = resource_id.clone().unwrap_or_default();
                    let outcome = StoredOutcome {
                        value: Arc::new(value.clone()),
                        resource_id: resource_id.clone(),
                    };
                    let expiry = Self::now_ms() + ttl_ms;
                    map.insert(
                        key,
                        Entry::Completed {
                            fingerprint,
                            outcome,
                            expiry,
                        },
                    );
                    Some(waiters)
                }
                (Err(e), Some(Entry::InProgress { waiters, .. })) => {
                    let waiters = std::mem::take(waiters);
                    let message = match e {
                        IdempotencyError::OperationFailed(msg) => msg.clone(),
                        ref other => other.to_string(),
                    };
                    let expiry = Self::now_ms() + ttl_ms;
                    map.insert(
                        key,
                        Entry::Error {
                            fingerprint,
                            message,
                            expiry,
                        },
                    );
                    Some(waiters)
                }
                // Entry missing or in an unexpected state means it expired or
                // was cleared during the operation. Return the raw result.
                _ => None,
            }
        };
        if let Some(waiters) = waiters {
            for w in waiters {
                let _ = w.send();
            }
        }
        match result {
            Ok((value, _)) => Ok(value),
            Err(e) => Err(e),
        }
    }

    /// Return the current outcome for a key, if any.
    ///
    /// 返回指定键的当前结果（如有）。
    pub fn outcome(&self, key: &IdempotencyKey) -> Option<IdempotencyOutcome> {
        let map = self.inner.lock().unwrap();
        match map.get(key) {
            Some(Entry::Completed { outcome, .. }) => Some(IdempotencyOutcome::Success {
                resource_id: outcome.resource_id.clone(),
            }),
            Some(Entry::Error { message, .. }) => Some(IdempotencyOutcome::Error {
                message: message.clone(),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn canonical_hash_is_stable() {
        let a = canonical_hash(b"hello");
        let b = canonical_hash("hello");
        assert_eq!(a, b);
    }

    #[test]
    fn canonical_hash_differs_for_different_inputs() {
        let a = canonical_hash(b"foo");
        let b = canonical_hash(b"bar");
        assert_ne!(a, b);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn same_key_and_fingerprint_returns_cached_value() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp = canonical_hash(b"req1");
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = counter.clone();
            let result = repo
                .execute(key.clone(), fp, 60_000, move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(("resource-1".to_string(), Some("res-1".to_string())))
                })
                .await;
            assert_eq!(result.unwrap(), "resource-1");
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn different_fingerprint_for_same_key_is_conflict() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp1 = canonical_hash(b"req1");
        let fp2 = canonical_hash(b"req2");

        let _ = repo
            .execute(key.clone(), fp1, 60_000, || async move {
                Ok(("r1".to_string(), Some("r1".to_string())))
            })
            .await
            .unwrap();

        let err = repo
            .execute(key.clone(), fp2, 60_000, || async move {
                Ok(("r2".to_string(), Some("r2".to_string())))
            })
            .await
            .unwrap_err();
        assert!(matches!(err, IdempotencyError::Conflict { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_waiters_share_single_execution() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp = canonical_hash(b"req1");
        let counter = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::with_capacity(5);
        for _ in 0..5 {
            let repo = repo.clone();
            let key = key.clone();
            let counter = counter.clone();
            handles.push(tokio::spawn(async move {
                repo.execute(key, fp, 60_000, move || async move {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok((42u32, Some("res".to_string())))
                })
                .await
            }));
        }

        let mut all_ok = true;
        for h in handles {
            let r = h.await.unwrap();
            if r.as_ref() != Ok(&42) {
                all_ok = false;
            }
        }
        assert!(all_ok);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_operation_is_cached_and_replayed() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp = canonical_hash(b"req1");
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let c = counter.clone();
            let err = repo
                .execute(key.clone(), fp, 60_000, move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<(String, Option<String>), IdempotencyError>(
                        IdempotencyError::OperationFailed("boom".to_string()),
                    )
                })
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), "idempotency operation failed: boom");
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_records_are_not_reused() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp = canonical_hash(b"req1");
        let counter = Arc::new(AtomicUsize::new(0));

        let c = counter.clone();
        repo.execute(key.clone(), fp, 1, move || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok((1u32, Some("r".to_string())))
        })
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let c = counter.clone();
        repo.execute(key.clone(), fp, 60_000, move || async move {
            c.fetch_add(1, Ordering::SeqCst);
            Ok((2u32, Some("r2".to_string())))
        })
        .await
        .unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_operation_is_cleaned_up_and_retried() {
        let repo = InMemoryIdempotencyRepository::new();
        let key = IdempotencyKey::new("alice", "create_stream", "k1");
        let fp = canonical_hash(b"req1");
        let counter = Arc::new(AtomicUsize::new(0));

        let repo2 = repo.clone();
        let key2 = key.clone();
        let c = counter.clone();
        let handle = tokio::spawn(async move {
            repo2
                .execute(key2, fp, 60_000, move || async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(("never".to_string(), None))
                })
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        handle.abort();
        let _ = handle.await;

        let c = counter.clone();
        let result = repo
            .execute(key, fp, 60_000, move || async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(("ok".to_string(), Some("r".to_string())))
            })
            .await
            .unwrap();
        assert_eq!(result, "ok");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
