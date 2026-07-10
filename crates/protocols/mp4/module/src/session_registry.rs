//! VOD session registry.
//!
//! VOD 会话注册表。

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_mp4_driver_tokio::VodDriverHandle;
use parking_lot::RwLock;

#[derive(Debug, Clone)]
/// Record stored in the session registry for a VOD session.
///
/// 会话注册表中的 VOD 会话记录。
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

/// In-memory registry of active VOD sessions.
///
/// 活跃 VOD 会话的内存注册表。
pub struct VodSessionRegistry {
    sessions: RwLock<HashMap<String, (VodSessionRecord, Arc<VodDriverHandle>)>>,
    capacity: usize,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
/// Registry-level errors for session management.
///
/// 会话管理的注册表层错误。
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session already exists: {0}")]
    Duplicate(String),
    #[error("registry capacity exceeded ({0})")]
    CapacityExceeded(usize),
}

/// `VodSessionRegistry` constructors and accessors.
///
/// `VodSessionRegistry` 构造与访问。
impl VodSessionRegistry {
    pub fn new(capacity: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

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

    pub fn list(&self) -> Vec<VodSessionRecord> {
        self.sessions
            .read()
            .values()
            .map(|(rec, _)| rec.clone())
            .collect()
    }

    pub fn handle(&self, session_id: &str) -> Option<Arc<VodDriverHandle>> {
        self.sessions.read().get(session_id).map(|(_, h)| h.clone())
    }

    pub fn remove(&self, session_id: &str) -> Result<VodSessionRecord, SessionError> {
        self.sessions
            .write()
            .remove(session_id)
            .map(|(rec, _)| rec)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))
    }
}
