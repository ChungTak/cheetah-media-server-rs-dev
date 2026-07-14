//! In-memory snapshot registry.
//!
//! 内存中的截图注册表。

use std::sync::Arc;

use cheetah_media_api::ids::{MediaKey, SnapshotId};
use cheetah_media_api::model::{SnapshotHandle, SnapshotInfo};
use parking_lot::Mutex;

/// In-memory registry of snapshot metadata.
///
/// 截图元数据内存注册表。
#[derive(Default, Clone)]
pub struct SnapshotRegistry {
    inner: Arc<Mutex<Vec<SnapshotInfo>>>,
}

impl SnapshotRegistry {
    /// Create a new registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a snapshot entry by id.
    pub fn upsert(&self, info: SnapshotInfo) {
        let mut guard = self.inner.lock();
        if let Some(existing) = guard.iter_mut().find(|i| i.snapshot_id == info.snapshot_id) {
            *existing = info;
        } else {
            guard.push(info);
        }
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

    fn sample_info(id: &str, key: &str, state: SnapshotState) -> SnapshotInfo {
        let media_key = MediaKey::with_default_vhost("live", key, None).unwrap();
        SnapshotInfo {
            snapshot_id: SnapshotId(id.to_string()),
            media_key,
            state,
            path_handle: FileHandle(format!("handle-{id}")),
            created_at: 1,
            size_bytes: Some(42),
            format: "jpg".to_string(),
        }
    }

    #[test]
    fn upsert_and_get() {
        let reg = SnapshotRegistry::new();
        let info = sample_info("a", "s1", SnapshotState::Completed);
        reg.upsert(info.clone());
        assert_eq!(reg.get(&SnapshotId("a".to_string())), Some(info));
    }

    #[test]
    fn list_filters_by_media_key() {
        let reg = SnapshotRegistry::new();
        let key1 = MediaKey::with_default_vhost("live", "s1", None).unwrap();
        let key2 = MediaKey::with_default_vhost("live", "s2", None).unwrap();
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed));

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
        reg.upsert(sample_info("a", "s1", SnapshotState::Completed));
        reg.upsert(sample_info("b", "s2", SnapshotState::Completed));

        assert_eq!(reg.delete_by_media_key(&key1), 1);
        assert_eq!(reg.list(None).len(), 1);
    }

    #[test]
    fn upsert_replaces_existing() {
        let reg = SnapshotRegistry::new();
        let mut info = sample_info("a", "s1", SnapshotState::Pending);
        reg.upsert(info.clone());
        info.state = SnapshotState::Completed;
        reg.upsert(info.clone());
        assert_eq!(
            reg.get(&SnapshotId("a".to_string())).unwrap().state,
            SnapshotState::Completed
        );
        assert_eq!(reg.list(None).len(), 1);
    }
}
