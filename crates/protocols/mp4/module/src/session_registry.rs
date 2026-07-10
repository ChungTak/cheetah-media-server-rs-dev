//! VOD session registry.

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_mp4_driver_tokio::VodDriverHandle;
use parking_lot::RwLock;

/// `VodSessionRecord` data structure.
/// `VodSessionRecord` 数据结构.
#[derive(Debug, Clone)]
pub struct VodSessionRecord {
    /// `session_id` field of type `String`.
    /// `session_id` 字段，类型为 `String`.
    pub session_id: String,
    /// `source_uri` field of type `String`.
    /// `source_uri` 字段，类型为 `String`.
    pub source_uri: String,
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `paused` field of type `bool`.
    /// `paused` 字段，类型为 `bool`.
    pub paused: bool,
    /// `scale` field of type `f32`.
    /// `scale` 字段，类型为 `f32`.
    pub scale: f32,
    /// `state` field of type `String`.
    /// `state` 字段，类型为 `String`.
    pub state: String,
    /// ABL `on_rtsp_replay`-style audit fields. Populated by the
    /// protocol layer when a peer attaches; left empty for
    /// programmatically-loaded sessions. Bounded to a small set of
    /// scalar fields so the registry stays cheap to clone.
    pub reader_count: u32,
    /// `remote_ip` field.
    /// `remote_ip` 字段.
    pub remote_ip: Option<String>,
    /// `remote_port` field.
    /// `remote_port` 字段.
    pub remote_port: Option<u16>,
    /// `network_type` field.
    /// `network_type` 字段.
    pub network_type: Option<String>,
    /// `params` field.
    /// `params` 字段.
    pub params: Option<String>,
}

/// `VodSessionRegistry` data structure.
/// `VodSessionRegistry` 数据结构.
pub struct VodSessionRegistry {
    /// `sessions` field.
    /// `sessions` 字段.
    sessions: RwLock<HashMap<String, (VodSessionRecord, Arc<VodDriverHandle>)>>,
    /// `capacity` field of type `usize`.
    /// `capacity` 字段，类型为 `usize`.
    capacity: usize,
}

/// `SessionError` enumeration.
/// `SessionError` 枚举.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// `NotFound` variant.
    /// `NotFound` 变体.
    #[error("session not found: {0}")]
    NotFound(String),
    /// `Duplicate` variant.
    /// `Duplicate` 变体.
    #[error("session already exists: {0}")]
    Duplicate(String),
    /// `CapacityExceeded` variant.
    /// `CapacityExceeded` 变体.
    #[error("registry capacity exceeded ({0})")]
    CapacityExceeded(usize),
}

impl VodSessionRegistry {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(capacity: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    /// `capacity` function.
    /// `capacity` 函数.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// `count` function.
    /// `count` 函数.
    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

    /// `insert` function.
    /// `insert` 函数.
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

    /// `list` function.
    /// `list` 函数.
    pub fn list(&self) -> Vec<VodSessionRecord> {
        self.sessions
            .read()
            .values()
            .map(|(rec, _)| rec.clone())
            .collect()
    }

    /// `handle` function.
    /// `handle` 函数.
    pub fn handle(&self, session_id: &str) -> Option<Arc<VodDriverHandle>> {
        self.sessions.read().get(session_id).map(|(_, h)| h.clone())
    }

    /// `remove` function.
    /// `remove` 函数.
    pub fn remove(&self, session_id: &str) -> Result<VodSessionRecord, SessionError> {
        self.sessions
            .write()
            .remove(session_id)
            .map(|(rec, _)| rec)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))
    }
}
