use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_media_api::command::ProxyQuery;
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::{ProxyInfo, ProxyKind, ProxyState};
use cheetah_runtime_api::CancellationToken;
use dashmap::DashMap;

/// In-memory registry of active proxy entries.
///
/// 活跃代理条目的内存注册表。
pub struct ProxyRegistry {
    entries: DashMap<String, ProxyEntry>,
    capacity: usize,
    next_id: AtomicU64,
}

/// A single proxy entry with its metadata and runtime cancellation handle.
///
/// 单个代理条目及其元数据和运行时取消句柄。
#[derive(Clone)]
pub struct ProxyEntry {
    pub info: ProxyInfo,
    pub cancel: Option<CancellationToken>,
}

impl ProxyRegistry {
    /// Create a registry bounded by `capacity`.
    ///
    /// 创建容量为 `capacity` 的注册表。
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: DashMap::new(),
            capacity,
            next_id: AtomicU64::new(1),
        }
    }

    /// Return the number of stored entries.
    ///
    /// 返回已存储条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return whether the registry is empty.
    ///
    /// 返回注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert a new entry. Returns the previous value if the id already existed.
    ///
    /// 插入新条目。若 id 已存在则返回旧值。
    pub fn insert(&self, entry: ProxyEntry) -> Option<ProxyEntry> {
        self.entries.insert(entry.info.proxy_id.0.clone(), entry)
    }

    /// Get a clone of the entry for `id`.
    ///
    /// 获取 `id` 对应条目的克隆。
    pub fn get(&self, id: &ProxyId) -> Option<ProxyEntry> {
        self.entries.get(&id.0).map(|r| r.clone())
    }

    /// Remove and return the entry for `id`.
    ///
    /// 移除并返回 `id` 对应的条目。
    pub fn remove(&self, id: &ProxyId) -> Option<ProxyEntry> {
        self.entries.remove(&id.0).map(|(_, v)| v)
    }

    /// Generate a fresh proxy id.
    ///
    /// 生成新的代理 id。
    pub fn generate_id(&self) -> ProxyId {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        ProxyId(format!("proxy-{n}"))
    }

    /// Update the state of a proxy, returning `true` if it existed.
    ///
    /// 更新代理状态；若存在则返回 `true`。
    pub fn update_state(&self, id: &ProxyId, state: ProxyState) -> bool {
        if let Some(mut entry) = self.entries.get_mut(&id.0) {
            entry.info.state = state;
            entry.info.updated_at = now_unix_millis();
            true
        } else {
            false
        }
    }

    /// Set or clear the last error of a proxy, returning `true` if it existed.
    ///
    /// 设置或清除代理最近错误；若存在则返回 `true`。
    pub fn update_error(&self, id: &ProxyId, error: Option<String>) -> bool {
        if let Some(mut entry) = self.entries.get_mut(&id.0) {
            entry.info.last_error = error;
            entry.info.updated_at = now_unix_millis();
            true
        } else {
            false
        }
    }

    /// Store the cancellation handle for a proxy so that `delete` can stop the
    /// background task. Returns `true` if the entry existed.
    ///
    /// 保存代理的取消句柄，以便删除时停止后台任务。若条目存在则返回 `true`。
    pub fn set_cancel(&self, id: &ProxyId, token: CancellationToken) -> bool {
        if let Some(mut entry) = self.entries.get_mut(&id.0) {
            entry.cancel = Some(token);
            true
        } else {
            false
        }
    }

    /// Cancel the background task for `id` if one exists, returning `true` if
    /// the entry was found.
    ///
    /// 取消 `id` 的后台任务（如有）；若找到条目则返回 `true`。
    pub fn cancel(&self, id: &ProxyId) -> bool {
        if let Some(entry) = self.entries.get(&id.0) {
            if let Some(token) = &entry.cancel {
                token.cancel();
            }
            true
        } else {
            false
        }
    }

    /// Query and paginate entries, filtering by optional kind and state.
    ///
    /// 按可选 kind 和 state 过滤并分页返回条目。
    pub fn query(&self, query: &ProxyQuery) -> (Vec<ProxyInfo>, u64) {
        let mut items: Vec<ProxyInfo> = self
            .entries
            .iter()
            .map(|r| r.value().info.clone())
            .filter(|info| {
                if let Some(kind) = query.kind {
                    if info.kind != kind {
                        return false;
                    }
                }
                if let Some(state) = query.state {
                    if info.state != state {
                        return false;
                    }
                }
                true
            })
            .collect();

        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.proxy_id.0.cmp(&b.proxy_id.0))
        });

        let total = items.len() as u64;
        let page_size = query.page_size.max(1);
        let page = query.page.max(1);
        let start = ((page - 1) * page_size) as usize;
        let end = start + page_size as usize;
        let page_items = if start >= items.len() {
            Vec::new()
        } else {
            items[start..items.len().min(end)].to_vec()
        };

        (page_items, total)
    }

    /// Return the total number of entries matching the optional filters.
    ///
    /// 返回符合可选过滤条件的条目总数。
    pub fn count_matching(&self, kind: Option<ProxyKind>, state: Option<ProxyState>) -> u64 {
        self.entries
            .iter()
            .filter(|r| {
                let info = &r.value().info;
                if let Some(k) = kind {
                    if info.kind != k {
                        return false;
                    }
                }
                if let Some(s) = state {
                    if info.state != s {
                        return false;
                    }
                }
                true
            })
            .count() as u64
    }

    /// Return whether the registry has reached its capacity.
    ///
    /// 返回注册表是否已达容量上限。
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }

    /// Cancel all background tasks.
    ///
    /// 取消所有后台任务。
    pub fn cancel_all(&self) {
        for entry in self.entries.iter() {
            if let Some(token) = &entry.cancel {
                token.cancel();
            }
        }
    }
}

fn now_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
