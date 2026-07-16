use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_media_api::command::SnapshotQuery;
use cheetah_media_api::ids::SnapshotId;
use cheetah_media_api::model::SnapshotInfo;
use dashmap::DashMap;
use parking_lot::Mutex;

/// In-memory registry of completed snapshots.
///
/// 已完成快照的内存注册表。
pub struct SnapshotRegistry {
    entries: DashMap<String, SnapshotInfo>,
    order: Mutex<Vec<String>>,
    capacity: usize,
    next_id: AtomicU64,
}

impl SnapshotRegistry {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: DashMap::new(),
            order: Mutex::new(Vec::new()),
            capacity,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn generate_id(&self) -> SnapshotId {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        SnapshotId(format!("snap-{n}"))
    }

    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    pub fn insert(&self, info: SnapshotInfo) {
        let id = info.snapshot_id.0.clone();
        self.entries.insert(id.clone(), info);
        let mut order = self.order.lock();
        order.push(id);
        while order.len() > self.capacity {
            if let Some(old) = order.first().cloned() {
                order.remove(0);
                self.entries.remove(&old);
            } else {
                break;
            }
        }
    }

    pub fn query(&self, query: &SnapshotQuery) -> (Vec<SnapshotInfo>, u64) {
        let mut items: Vec<SnapshotInfo> = self
            .entries
            .iter()
            .map(|e| e.value().clone())
            .filter(|info| {
                if let Some(v) = &query.vhost {
                    if &info.media_key.vhost.0 != v {
                        return false;
                    }
                }
                if let Some(a) = &query.app {
                    if &info.media_key.app.0 != a {
                        return false;
                    }
                }
                if let Some(s) = &query.stream {
                    if &info.media_key.stream.0 != s {
                        return false;
                    }
                }
                if let Some(id) = &query.snapshot_id {
                    if &info.snapshot_id.0 != id {
                        return false;
                    }
                }
                true
            })
            .collect();
        items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.snapshot_id.0.cmp(&b.snapshot_id.0))
        });
        let total = items.len() as u64;
        let page_size = query.page_size.max(1) as usize;
        let page = query.page.max(1) as usize;
        let start = (page - 1).saturating_mul(page_size);
        let end = start.saturating_add(page_size);
        let page_items = if start >= items.len() {
            Vec::new()
        } else {
            items[start..items.len().min(end)].to_vec()
        };
        (page_items, total)
    }

    pub fn find_by_media_key(
        &self,
        media_key: &cheetah_media_api::ids::MediaKey,
    ) -> Vec<SnapshotInfo> {
        self.entries
            .iter()
            .filter(|e| &e.value().media_key == media_key)
            .map(|e| e.value().clone())
            .collect()
    }

    pub fn remove(&self, snapshot_id: &str) -> Option<SnapshotInfo> {
        let removed = self.entries.remove(snapshot_id).map(|(_, v)| v);
        if removed.is_some() {
            let mut order = self.order.lock();
            order.retain(|x| x != snapshot_id);
        }
        removed
    }

    pub fn delete_matching(&self, media_key: &cheetah_media_api::ids::MediaKey) -> u64 {
        let ids: Vec<String> = self
            .entries
            .iter()
            .filter(|e| &e.value().media_key == media_key)
            .map(|e| e.key().clone())
            .collect();
        let mut removed = 0u64;
        for id in ids {
            if self.entries.remove(&id).is_some() {
                removed += 1;
            }
            let mut order = self.order.lock();
            order.retain(|x| x != &id);
        }
        removed
    }
}
