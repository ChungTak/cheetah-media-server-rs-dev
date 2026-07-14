//! In-memory proxy registry with bounded capacity and idempotent creation.
//!
//! 有界容量与幂等创建的内存代理注册表。

use std::sync::Arc;

use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::{ProxyInfo, ProxyKind, ProxyState};
use parking_lot::Mutex;

/// In-memory registry of proxy metadata.
///
/// 代理元数据内存注册表。
#[derive(Clone)]
pub struct ProxyRegistry {
    inner: Arc<Mutex<Vec<ProxyInfo>>>,
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
        if let Some(existing) = guard.iter().find(|i| proxy_key(i) == key) {
            return existing.clone();
        }
        guard.push(info.clone());
        self.evict_oldest_if_needed(&mut guard);
        info
    }

    fn evict_oldest_if_needed(&self, guard: &mut Vec<ProxyInfo>) {
        if guard.len() <= self.max_total {
            return;
        }
        guard.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        let to_evict = guard.len() - self.max_total;
        guard.drain(0..to_evict);
    }

    /// Retrieve a proxy by id.
    pub fn get(&self, id: &ProxyId) -> Option<ProxyInfo> {
        self.inner
            .lock()
            .iter()
            .find(|i| i.proxy_id == *id)
            .cloned()
    }

    /// List proxies, optionally filtered by kind and/or state.
    pub fn list(&self, kind: Option<ProxyKind>, state: Option<ProxyState>) -> Vec<ProxyInfo> {
        let guard = self.inner.lock();
        guard
            .iter()
            .filter(|i| kind.is_none_or(|k| i.kind == k))
            .filter(|i| state.is_none_or(|s| i.state == s))
            .cloned()
            .collect()
    }

    /// Update the state of an existing proxy.
    pub fn update_state(&self, id: &ProxyId, state: ProxyState, last_error: Option<String>) {
        let mut guard = self.inner.lock();
        if let Some(existing) = guard.iter_mut().find(|i| i.proxy_id == *id) {
            existing.state = state;
            existing.last_error = last_error;
            existing.updated_at = wall_clock_ms();
        }
    }

    /// Delete a proxy by id. Returns `true` if it existed.
    pub fn delete(&self, id: &ProxyId) -> bool {
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|i| i.proxy_id != *id);
        before != guard.len()
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
}
