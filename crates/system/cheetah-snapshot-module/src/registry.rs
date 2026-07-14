//! In-memory snapshot registry.
//!
//! 内存中的截图注册表。

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cheetah_media_api::ids::{MediaKey, SnapshotId};
use cheetah_media_api::model::{SnapshotHandle, SnapshotInfo};
use parking_lot::Mutex;

/// In-memory registry of snapshot metadata.
///
/// 截图元数据内存注册表。
#[derive(Clone)]
pub struct SnapshotRegistry {
    inner: Arc<Mutex<Vec<SnapshotInfo>>>,
    max_per_key: Arc<AtomicUsize>,
    max_total: Arc<AtomicUsize>,
}

impl Default for SnapshotRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            max_per_key: Arc::new(AtomicUsize::new(0)),
            max_total: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl SnapshotRegistry {
    /// Create a new registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of snapshots kept per media key. `0` means unlimited.
    pub fn set_max_per_key(&self, max: u32) {
        self.max_per_key.store(max as usize, Ordering::SeqCst);
    }

    /// Set the global maximum number of snapshot entries. `0` means unlimited.
    pub fn set_max_total(&self, max: u32) {
        self.max_total.store(max as usize, Ordering::SeqCst);
    }

    /// Insert or replace a snapshot entry by id.
    pub fn upsert(&self, info: SnapshotInfo) {
        let mut guard = self.inner.lock();
        if let Some(existing) = guard.iter_mut().find(|i| i.snapshot_id == info.snapshot_id) {
            *existing = info.clone();
        } else {
            guard.push(info.clone());
        }
        self.evict_oldest_per_key(&mut guard, &info.media_key);
        self.evict_global_oldest(&mut guard);
    }

    fn evict_oldest_per_key(&self, guard: &mut Vec<SnapshotInfo>, key: &MediaKey) {
        let max = self.max_per_key.load(Ordering::SeqCst);
        if max == 0 {
            return;
        }
        let matching: Vec<_> = guard
            .iter()
            .filter(|i| &i.media_key == key)
            .cloned()
            .collect();
        if matching.len() <= max {
            return;
        }
        let mut sorted = matching;
        sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let to_remove: HashSet<_> = sorted
            .iter()
            .take(sorted.len() - max)
            .map(|i| i.snapshot_id.clone())
            .collect();
        guard.retain(|i| !(i.media_key == *key && to_remove.contains(&i.snapshot_id)));
    }

    fn evict_global_oldest(&self, guard: &mut Vec<SnapshotInfo>) {
        let max = self.max_total.load(Ordering::SeqCst);
        if max == 0 || guard.len() <= max {
            return;
        }
        guard.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        guard.truncate(max);
    }

    /// Retrieve a snapshot by id.
    pub fn get(&self, id: &SnapshotId) -> Option<SnapshotInfo> {
        self.inner
            .lock()
            .iter()
            .find(|i| i.snapshot_id == *id)
            .cloned()
    }

    /// List snapshots, optionally filtered by media key.
    pub fn list(&self, media_key: Option<&MediaKey>) -> Vec<SnapshotInfo> {
        let guard = self.inner.lock();
        guard
            .iter()
            .filter(|i| media_key.is_none_or(|k| &i.media_key == k))
            .cloned()
            .collect()
    }

    /// Delete snapshots matching the media key. Returns the number removed.
    pub fn delete_by_media_key(&self, key: &MediaKey) -> usize {
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|i| &i.media_key != key);
        before - guard.len()
    }

    /// Build a `SnapshotHandle` from an existing info entry.
    pub fn to_handle(info: &SnapshotInfo) -> SnapshotHandle {
        SnapshotHandle {
            snapshot_id: info.snapshot_id.clone(),
            media_key: info.media_key.clone(),
            state: info.state,
            path_handle: info.path_handle.clone(),
            download_url: None,
            created_at: info.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::FileHandle;
    use cheetah_media_api::model::SnapshotState;

    fn sample_info(id: &str, key: &str, state: SnapshotState, created_at: i64) -> SnapshotInfo {
        let media_key = MediaKey::with_default_vhost("live", key, None).unwrap();
        SnapshotInfo {
            snapshot_id: SnapshotId(id.to_string()),
            media_key,
            state,
            path_handle: FileHandle(format!("handle-{id}")),
            created_at,
            size_bytes: Some(42),
            format: "jpg".to_string(),
        }
    }

    #[test]
    fn upsert_and_get() {
        let reg = SnapshotRegistry::new();
        let info = sample_info("a", "s1", SnapshotState::Completed, 1);
        reg.upsert(info.clone());
        assert_eq!(reg.get(&SnapshotId("a".to_string())), Some(info));
    }

    #[test]
    fn list_filters_by_media_key() {
        let reg = SnapshotRegistry::new();
        let key1 = MediaKey::with_default_vhost("live", "s1", None).unwrap();
        let key2 = MediaKey::with_default_vhost("live", "s2", None).unwrap();
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed, 1));

        let found = reg.list(Some(&key1));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].snapshot_id.0, "a");

        assert_eq!(reg.list(Some(&key2)).len(), 1);
        assert_eq!(reg.list(None).len(), 2);
    }

    #[test]
    fn delete_by_media_key() {
        let reg = SnapshotRegistry::new();
        let key1 = MediaKey::with_default_vhost("live", "s1", None).unwrap();
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed, 1));

        assert_eq!(reg.delete_by_media_key(&key1), 1);
        assert_eq!(reg.list(None).len(), 1);
    }

    #[test]
    fn upsert_replaces_existing() {
        let reg = SnapshotRegistry::new();
        let info = sample_info("a", "s1", SnapshotState::Pending, 1);
        reg.upsert(info.clone());
        let completed = sample_info("a", "s1", SnapshotState::Completed, 1);
        reg.upsert(completed.clone());
        assert_eq!(
            reg.get(&SnapshotId("a".to_string())).unwrap().state,
            SnapshotState::Completed
        );
        assert_eq!(reg.list(None).len(), 1);
    }

    #[test]
    fn max_per_key_evicts_oldest() {
        let reg = SnapshotRegistry::new();
        reg.set_max_per_key(2);
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s1", SnapshotState::Completed, 2));
        reg.upsert(sample_info("c", "s1", SnapshotState::Completed, 3));

        let ids: Vec<_> = reg
            .list(None)
            .iter()
            .map(|i| i.snapshot_id.0.clone())
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
    }

    #[test]
    fn max_per_key_zero_is_unlimited() {
        let reg = SnapshotRegistry::new();
        reg.set_max_total(100);
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s1", SnapshotState::Completed, 2));
        reg.upsert(sample_info("c", "s1", SnapshotState::Completed, 3));
        assert_eq!(reg.list(None).len(), 3);
    }

    #[test]
    fn max_per_key_only_affects_same_key() {
        let reg = SnapshotRegistry::new();
        reg.set_max_per_key(1);
        reg.set_max_total(100);
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed, 2));

        assert_eq!(reg.list(None).len(), 2);
        assert_eq!(
            reg.list(Some(
                &MediaKey::with_default_vhost("live", "s1", None).unwrap()
            ))
            .len(),
            1
        );
    }

    #[test]
    fn max_total_evicts_oldest_across_keys() {
        let reg = SnapshotRegistry::new();
        reg.set_max_total(2);
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed, 1));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed, 2));
        reg.upsert(sample_info("c", "s3", SnapshotState::Completed, 3));

        let ids: Vec<_> = reg
            .list(None)
            .iter()
            .map(|i| i.snapshot_id.0.clone())
            .collect();
        assert_eq!(ids, vec!["c", "b"]);
    }
}
