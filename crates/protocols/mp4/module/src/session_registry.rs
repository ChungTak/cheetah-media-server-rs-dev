//! VOD session registry.

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_mp4_driver_tokio::VodDriverHandle;
use parking_lot::RwLock;

/// `VodSessionRecord` data structure.
/// `VodSessionRecord` 数据结构。
#[derive(Debug, Clone)]
pub struct VodSessionRecord {
    pub session_id: String,
    pub source_uri: String,
    pub stream_key: String,
    pub paused: bool,
    pub scale: f32,
    pub state: String,
    /// ABL `on_rtsp_replay`-style audit fields. Populated by the
    /// protocol layer when a peer attaches; left empty for
    /// programmatically-loaded sessions. Bounded to a small set of
    /// scalar fields so the registry stays cheap to clone.
    pub reader_count: u32,
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    pub network_type: Option<String>,
    pub params: Option<String>,
}

/// `VodSessionRegistry` data structure.
/// `VodSessionRegistry` 数据结构。
pub struct VodSessionRegistry {
    sessions: RwLock<HashMap<String, (VodSessionRecord, Arc<VodDriverHandle>)>>,
    capacity: usize,
}

/// Error returned by `Session` operations.
/// `Session` 操作返回的错误。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session already exists: {0}")]
    Duplicate(String),
    #[error("registry capacity exceeded ({0})")]
    CapacityExceeded(usize),
}

impl VodSessionRegistry {
    /// Creates a new `VodSessionRegistry` instance.
    /// 创建新的 `VodSessionRegistry` 实例。
    pub fn new(capacity: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    /// `capacity` function of `VodSessionRegistry`.
    /// `VodSessionRegistry` 的 `capacity` 函数。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// `count` function of `VodSessionRegistry`.
    /// `VodSessionRegistry` 的 `count` 函数。
    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

    /// Inserts the value into the collection.
    /// 将值插入集合。
    pub fn insert(
        &self,
        record: VodSessionRecord,
        handle: Arc<VodDriverHandle>,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write();
        if sessions.contains_key(&record.session_id) {
            return Err(SessionError::Duplicate(record.session_id.clone()));
        }
        if sessions.len() >= self.capacity {
            return Err(SessionError::CapacityExceeded(self.capacity));
        }
        sessions.insert(record.session_id.clone(), (record, handle));
        Ok(())
    }

    /// `list` function of `VodSessionRegistry`.
    /// `VodSessionRegistry` 的 `list` 函数。
    pub fn list(&self) -> Vec<VodSessionRecord> {
        self.sessions
            .read()
            .values()
            .map(|(rec, _)| rec.clone())
            .collect()
    }

    /// Handles the event or request and updates internal state.
    /// 处理事件或请求并更新内部状态。
    pub fn handle(&self, session_id: &str) -> Option<Arc<VodDriverHandle>> {
        self.sessions.read().get(session_id).map(|(_, h)| h.clone())
    }

    /// Removes the value from the collection.
    /// 从集合中移除值。
    pub fn remove(&self, session_id: &str) -> Result<VodSessionRecord, SessionError> {
        self.sessions
            .write()
            .remove(session_id)
            .map(|(rec, _)| rec)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))
    }
}
