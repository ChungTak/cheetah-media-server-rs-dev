//! In-memory proxy registry with bounded capacity and idempotent creation.
//!
//! 有界容量与幂等创建的内存代理注册表。

use std::sync::Arc;

use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::{ProxyInfo, ProxyKind, ProxyState};
use cheetah_sdk::{CancellationToken, JoinHandle};
use parking_lot::Mutex;

/// Opaque handle to a running proxy task.
///
/// 正在运行的代理任务句柄。
struct ProxyTask {
    cancel: CancellationToken,
    handle: Box<dyn JoinHandle>,
}

/// Internal entry pairing metadata with an optional live task handle.
///
/// 内部条目，将元数据与可选的实时任务句柄配对。
struct ProxySession {
    info: ProxyInfo,
    task: Option<ProxyTask>,
}

/// In-memory registry of proxy metadata and live task handles.
///
/// 代理元数据与实时任务句柄的内存注册表。
#[derive(Clone)]
pub struct ProxyRegistry {
    inner: Arc<Mutex<Vec<ProxySession>>>,
    max_total: usize,
}

impl Default for ProxyRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            max_total: 1_000,
        }
    }
}

impl ProxyRegistry {
    /// Create a new registry with the given global capacity.
    pub fn new(max_total: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            max_total: max_total as usize,
        }
    }

    /// Insert or return an existing proxy with the same logical key.
    pub fn upsert_idempotent(&self, info: ProxyInfo) -> ProxyInfo {
        let mut guard = self.inner.lock();
        let key = proxy_key(&info);
        if let Some(existing) = guard.iter().find(|s| proxy_key(&s.info) == key) {
            return existing.info.clone();
        }
        guard.push(ProxySession {
            info: info.clone(),
            task: None,
        });
        let evicted = self.evict_oldest_if_needed(&mut guard);
        drop(guard);
        self.cancel_sessions(evicted);
        info
    }

    fn evict_oldest_if_needed(&self, guard: &mut Vec<ProxySession>) -> Vec<ProxySession> {
        if guard.len() <= self.max_total {
            return Vec::new();
        }
        guard.sort_by(|a, b| a.info.updated_at.cmp(&b.info.updated_at));
        let to_evict = guard.len() - self.max_total;
        guard.drain(0..to_evict).collect()
    }

    fn cancel_sessions(&self, sessions: Vec<ProxySession>) {
        for session in sessions {
            if let Some(task) = session.task {
                task.cancel.cancel();
                task.handle.abort();
            }
        }
    }

    /// Retrieve a proxy by id.
    pub fn get(&self, id: &ProxyId) -> Option<ProxyInfo> {
        self.inner
            .lock()
            .iter()
            .find(|s| s.info.proxy_id == *id)
            .map(|s| s.info.clone())
    }

    /// List proxies, optionally filtered by kind and/or state.
    pub fn list(&self, kind: Option<ProxyKind>, state: Option<ProxyState>) -> Vec<ProxyInfo> {
        self.inner
            .lock()
            .iter()
            .filter(|s| kind.is_none_or(|k| s.info.kind == k))
            .filter(|s| state.is_none_or(|s2| s.info.state == s2))
            .map(|s| s.info.clone())
            .collect()
    }

    /// Attach a running task to an existing proxy.
    pub fn attach_task(
        &self,
        id: &ProxyId,
        cancel: CancellationToken,
        handle: Box<dyn JoinHandle>,
    ) -> bool {
        let mut guard = self.inner.lock();
        if let Some(session) = guard.iter_mut().find(|s| s.info.proxy_id == *id) {
            if let Some(task) = session.task.take() {
                task.cancel.cancel();
                task.handle.abort();
            }
            session.task = Some(ProxyTask { cancel, handle });
            true
        } else {
            false
        }
    }

    /// Update the state of an existing proxy.
    pub fn update_state(&self, id: &ProxyId, state: ProxyState, last_error: Option<String>) {
        let mut guard = self.inner.lock();
        if let Some(session) = guard.iter_mut().find(|s| s.info.proxy_id == *id) {
            session.info.state = state;
            session.info.last_error = last_error;
            session.info.updated_at = wall_clock_ms();
        }
    }

    /// Cancel the live task for a proxy and update its state to `Stopped`.
    pub fn stop(&self, id: &ProxyId) -> bool {
        let mut guard = self.inner.lock();
        if let Some(session) = guard.iter_mut().find(|s| s.info.proxy_id == *id) {
            if let Some(task) = session.task.take() {
                task.cancel.cancel();
                task.handle.abort();
            }
            session.info.state = ProxyState::Stopped;
            session.info.updated_at = wall_clock_ms();
            true
        } else {
            false
        }
    }

    /// Delete a proxy by id. Cancels any live task. Returns `true` if it existed.
    pub fn delete(&self, id: &ProxyId) -> bool {
        let mut guard = self.inner.lock();
        let mut kept = Vec::new();
        let mut cancelled = Vec::new();
        let mut existed = false;
        for session in guard.drain(..) {
            if session.info.proxy_id == *id {
                existed = true;
                if let Some(task) = session.task {
                    cancelled.push(task);
                }
            } else {
                kept.push(session);
            }
        }
        *guard = kept;
        drop(guard);
        for task in cancelled {
            task.cancel.cancel();
            task.handle.abort();
        }
        existed
    }

    /// Update retry count for an existing proxy.
    pub fn update_retry_count(&self, id: &ProxyId, retry_count: u32) {
        let mut guard = self.inner.lock();
        if let Some(session) = guard.iter_mut().find(|s| s.info.proxy_id == *id) {
            session.info.retry_count = retry_count;
            session.info.updated_at = wall_clock_ms();
        }
    }
}

fn proxy_key(info: &ProxyInfo) -> (ProxyKind, String, String) {
    (
        info.kind,
        info.source.clone(),
        info.destination.to_canonical(),
    )
}

fn wall_clock_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::MediaKey;

    fn sample_info(id: &str, kind: ProxyKind, source: &str, key: &str) -> ProxyInfo {
        ProxyInfo {
            proxy_id: ProxyId(id.to_string()),
            kind,
            source: source.to_string(),
            destination: MediaKey::with_default_vhost("live", key, None).unwrap(),
            state: ProxyState::Created,
            retry_count: 0,
            last_error: None,
            created_at: 1,
            updated_at: 1,
            output_urls: Vec::new(),
        }
    }

    #[test]
    fn upsert_idempotent_returns_existing_for_same_key() {
        let reg = ProxyRegistry::new(10);
        let a = sample_info("a", ProxyKind::Pull, "rtsp://x", "s1");
        let first = reg.upsert_idempotent(a.clone());
        let b = sample_info("b", ProxyKind::Pull, "rtsp://x", "s1");
        let second = reg.upsert_idempotent(b);
        assert_eq!(first.proxy_id, second.proxy_id);
        assert_eq!(reg.list(None, None).len(), 1);
    }

    #[test]
    fn list_filters_by_kind_and_state() {
        let reg = ProxyRegistry::new(10);
        reg.upsert_idempotent(sample_info("a", ProxyKind::Pull, "u", "s1"));
        reg.upsert_idempotent(sample_info("b", ProxyKind::Push, "u", "s2"));

        assert_eq!(reg.list(Some(ProxyKind::Pull), None).len(), 1);
        assert_eq!(reg.list(None, Some(ProxyState::Created)).len(), 2);
    }

    #[test]
    fn delete_removes_entry() {
        let reg = ProxyRegistry::new(10);
        reg.upsert_idempotent(sample_info("a", ProxyKind::Pull, "u", "s1"));
        assert!(reg.delete(&ProxyId("a".to_string())));
        assert!(!reg.delete(&ProxyId("a".to_string())));
    }

    #[test]
    fn max_total_evicts_oldest() {
        let reg = ProxyRegistry::new(2);
        reg.upsert_idempotent(sample_info("a", ProxyKind::Pull, "u1", "s1"));
        reg.upsert_idempotent(sample_info("b", ProxyKind::Pull, "u2", "s2"));
        reg.upsert_idempotent(sample_info("c", ProxyKind::Pull, "u3", "s3"));

        let ids: Vec<_> = reg
            .list(None, None)
            .iter()
            .map(|i| i.proxy_id.0.clone())
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
    }

    #[test]
    fn attach_task_replaces_existing() {
        let reg = ProxyRegistry::new(10);
        let info = sample_info("a", ProxyKind::Pull, "u", "s1");
        reg.upsert_idempotent(info);

        let cancel = CancellationToken::new();
        let handle: Box<dyn JoinHandle> = Box::new(NullHandle);
        assert!(reg.attach_task(&ProxyId("a".to_string()), cancel, handle));
        assert!(reg.stop(&ProxyId("a".to_string())));
        assert_eq!(
            reg.get(&ProxyId("a".to_string())).unwrap().state,
            ProxyState::Stopped
        );
    }

    struct NullHandle;

    impl cheetah_sdk::JoinHandle for NullHandle {
        fn abort(&self) {}
        fn is_finished(&self) -> bool {
            true
        }
        fn wait(
            self: Box<Self>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), cheetah_sdk::TaskJoinError>>
                    + Send
                    + 'static,
            >,
        > {
            Box::pin(async { Ok(()) })
        }
    }
}
