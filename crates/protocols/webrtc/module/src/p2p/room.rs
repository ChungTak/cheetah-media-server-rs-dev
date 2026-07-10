//! P2P room keeper registry.
//!
//! Mirrors the C API surface from `api/include/mk_webrtc.h`:
//!
//! | C API                       | Method on [`P2pRoomKeeperRegistry`]   |
//! |-----------------------------|---------------------------------------|
//! | `mk_webrtc_add_room_keeper` | [`P2pRoomKeeperRegistry::add`]        |
//! | `mk_webrtc_del_room_keeper` | [`P2pRoomKeeperRegistry::remove`]     |
//! | `mk_webrtc_list_room_keepers` | [`P2pRoomKeeperRegistry::list`]     |
//! | `mk_webrtc_list_rooms`      | [`P2pRoomKeeperRegistry::list_rooms`] |
//!
//! The registry only stores configuration and bookkeeping. Actual
//! signaling I/O happens in the future `client.rs` module — keepers
//! are spawned separately and report back through a status update
//! channel that the registry exposes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use thiserror::Error;

use super::message::P2P_MAX_FIELD_BYTES;

/// Hard cap on the number of concurrent keepers per registry.
pub const P2P_DEFAULT_MAX_KEEPERS: usize = 1024;

/// Registry-side errors.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum P2pRoomKeeperError {
    /// `InvalidRoomId` variant.
    /// `InvalidRoomId` 变体.
    #[error("invalid room id: {0}")]
    InvalidRoomId(String),
    /// `InvalidHost` variant.
    /// `InvalidHost` 变体.
    #[error("invalid signaling host: {0}")]
    InvalidHost(String),
    /// `InvalidPort` variant.
    /// `InvalidPort` 变体.
    #[error("invalid signaling port: {0}")]
    InvalidPort(u16),
    /// `LimitReached` variant.
    /// `LimitReached` 变体.
    #[error("keeper limit reached ({0})")]
    LimitReached(usize),
    /// `NotFound` variant.
    /// `NotFound` 变体.
    #[error("keeper not found: {0:?}")]
    NotFound(P2pRoomKeeperKey),
}

/// Configuration for a single keeper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pRoomKeeperConfig {
    /// `server_host` field of type `String`.
    /// `server_host` 字段，类型为 `String`.
    pub server_host: String,
    /// `server_port` field of type `u16`.
    /// `server_port` 字段，类型为 `u16`.
    pub server_port: u16,
    /// Room id this keeper holds open on the remote signaling server.
    pub room_id: String,
    /// Optional vhost / app / stream tuple advertised in `check_in`.
    pub vhost: Option<String>,
    /// `app` field.
    /// `app` 字段.
    pub app: Option<String>,
    /// `stream` field.
    /// `stream` 字段.
    pub stream: Option<String>,
    /// Whether to use `wss://` for the signaling WebSocket.
    pub ssl: bool,
}

impl P2pRoomKeeperConfig {
    /// `validate` function.
    /// `validate` 函数.
    pub fn validate(&self) -> Result<(), P2pRoomKeeperError> {
        if self.room_id.is_empty() || self.room_id.len() > P2P_MAX_FIELD_BYTES {
            return Err(P2pRoomKeeperError::InvalidRoomId(self.room_id.clone()));
        }
        if self.server_host.is_empty() || self.server_host.len() > 253 {
            return Err(P2pRoomKeeperError::InvalidHost(self.server_host.clone()));
        }
        if self.server_port == 0 {
            return Err(P2pRoomKeeperError::InvalidPort(self.server_port));
        }
        Ok(())
    }
}

/// Identifies a keeper inside the registry. Stable across reconnect
/// attempts so callers can correlate `add` / `remove` / `list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct P2pRoomKeeperKey(u64);

impl std::fmt::Display for P2pRoomKeeperKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "keeper-{}", self.0)
    }
}

/// Lifecycle state of a keeper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum P2pKeeperState {
    /// Created but not yet wired to a transport.
    #[default]
    Pending,
    /// `Connecting` variant.
    /// `Connecting` 变体.
    Connecting,
    /// `Registered` variant.
    /// `Registered` 变体.
    Registered,
    /// `Reconnecting` variant.
    /// `Reconnecting` 变体.
    Reconnecting,
    /// `Stopped` variant.
    /// `Stopped` 变体.
    Stopped,
    /// `Failed` variant.
    /// `Failed` 变体.
    Failed,
}

impl P2pKeeperState {
    /// `as_str` function.
    /// `as_str` 函数.
    pub fn as_str(self) -> &'static str {
        match self {
            P2pKeeperState::Pending => "pending",
            P2pKeeperState::Connecting => "connecting",
            P2pKeeperState::Registered => "registered",
            P2pKeeperState::Reconnecting => "reconnecting",
            P2pKeeperState::Stopped => "stopped",
            P2pKeeperState::Failed => "failed",
        }
    }
}

/// Live status of a keeper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct P2pKeeperStatus {
    /// `state` field of type `P2pKeeperState`.
    /// `state` 字段，类型为 `P2pKeeperState`.
    pub state: P2pKeeperState,
    /// `last_error` field.
    /// `last_error` 字段.
    pub last_error: Option<String>,
    /// `reconnect_attempts` field of type `u32`.
    /// `reconnect_attempts` 字段，类型为 `u32`.
    pub reconnect_attempts: u32,
}

/// Snapshot returned by [`P2pRoomKeeperRegistry::list`]. Suitable for
/// HTTP / Prometheus exporters: cheap to clone, no internal locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pRoomKeeperSnapshot {
    /// `key` field of type `P2pRoomKeeperKey`.
    /// `key` 字段，类型为 `P2pRoomKeeperKey`.
    pub key: P2pRoomKeeperKey,
    /// `config` field of type `P2pRoomKeeperConfig`.
    /// `config` 字段，类型为 `P2pRoomKeeperConfig`.
    pub config: P2pRoomKeeperConfig,
    /// `status` field of type `P2pKeeperStatus`.
    /// `status` 字段，类型为 `P2pKeeperStatus`.
    pub status: P2pKeeperStatus,
}

#[derive(Debug, Default)]
struct RegistryInner {
    keepers: HashMap<P2pRoomKeeperKey, P2pRoomKeeperSnapshot>,
}

/// Bounded, mutex-protected registry of P2P room keepers.
///
/// The registry never blocks on I/O. State changes (`Registered`,
/// `Reconnecting`, etc.) come from the keeper task as it makes
/// progress. Removing a keeper is best-effort: the caller is expected
/// to also signal the keeper task to stop.
#[derive(Debug)]
pub struct P2pRoomKeeperRegistry {
    /// `inner` field.
    /// `inner` 字段.
    inner: Mutex<RegistryInner>,
    /// `next_key` field of type `AtomicU64`.
    /// `next_key` 字段，类型为 `AtomicU64`.
    next_key: AtomicU64,
    /// `max_keepers` field of type `usize`.
    /// `max_keepers` 字段，类型为 `usize`.
    max_keepers: usize,
}

impl Default for P2pRoomKeeperRegistry {
    fn default() -> Self {
        Self::with_capacity(P2P_DEFAULT_MAX_KEEPERS)
    }
}

impl P2pRoomKeeperRegistry {
    /// Returns a copy with `capacity` set.
    /// 返回 一个 copy 带有 `capacity` 设置.
    pub fn with_capacity(max_keepers: usize) -> Self {
        Self {
            inner: Mutex::new(RegistryInner::default()),
            next_key: AtomicU64::new(1),
            max_keepers: max_keepers.max(1),
        }
    }

    /// Add a new keeper. Returns the assigned key.
    pub fn add(&self, config: P2pRoomKeeperConfig) -> Result<P2pRoomKeeperKey, P2pRoomKeeperError> {
        config.validate()?;
        let mut guard = self.inner.lock();
        if guard.keepers.len() >= self.max_keepers {
            return Err(P2pRoomKeeperError::LimitReached(self.max_keepers));
        }
        let key = P2pRoomKeeperKey(self.next_key.fetch_add(1, Ordering::Relaxed));
        guard.keepers.insert(
            key,
            P2pRoomKeeperSnapshot {
                key,
                config,
                status: P2pKeeperStatus::default(),
            },
        );
        Ok(key)
    }

    /// Remove a keeper. The caller must separately stop the running
    /// keeper task; this method only removes bookkeeping.
    pub fn remove(
        &self,
        key: P2pRoomKeeperKey,
    ) -> Result<P2pRoomKeeperSnapshot, P2pRoomKeeperError> {
        self.inner
            .lock()
            .keepers
            .remove(&key)
            .ok_or(P2pRoomKeeperError::NotFound(key))
    }

    /// Update the status of a keeper. Called by the keeper task.
    pub fn set_status(&self, key: P2pRoomKeeperKey, status: P2pKeeperStatus) -> bool {
        let mut guard = self.inner.lock();
        match guard.keepers.get_mut(&key) {
            Some(entry) => {
                entry.status = status;
                true
            }
            None => false,
        }
    }

    /// List all known keepers.
    pub fn list(&self) -> Vec<P2pRoomKeeperSnapshot> {
        self.inner.lock().keepers.values().cloned().collect()
    }

    /// List the room ids registered locally. Mirrors
    /// `mk_webrtc_list_rooms`.
    pub fn list_rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = self
            .inner
            .lock()
            .keepers
            .values()
            .map(|snap| snap.config.room_id.clone())
            .collect();
        rooms.sort();
        rooms.dedup();
        rooms
    }

    /// Number of registered keepers.
    pub fn len(&self) -> usize {
        self.inner.lock().keepers.len()
    }

    /// Returns `true` if `empty` is true.
    /// 返回 `真` 如果 `empty` is 真.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(room: &str) -> P2pRoomKeeperConfig {
        P2pRoomKeeperConfig {
            server_host: "signaling.example.com".into(),
            server_port: 8443,
            room_id: room.into(),
            vhost: None,
            app: Some("live".into()),
            stream: Some("demo".into()),
            ssl: true,
        }
    }

    #[test]
    fn add_and_list_round_trip() {
        let reg = P2pRoomKeeperRegistry::default();
        let key = reg.add(cfg("room42")).unwrap();
        let listed = reg.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, key);
        assert_eq!(listed[0].config.room_id, "room42");
        assert_eq!(listed[0].status.state, P2pKeeperState::Pending);
        assert_eq!(reg.list_rooms(), vec!["room42"]);
    }

    #[test]
    fn remove_returns_snapshot() {
        let reg = P2pRoomKeeperRegistry::default();
        let key = reg.add(cfg("room1")).unwrap();
        let removed = reg.remove(key).unwrap();
        assert_eq!(removed.config.room_id, "room1");
        assert_eq!(reg.list().len(), 0);
        // Removing twice errors.
        let err = reg.remove(key).unwrap_err();
        assert!(matches!(err, P2pRoomKeeperError::NotFound(_)));
    }

    #[test]
    fn set_status_round_trip() {
        let reg = P2pRoomKeeperRegistry::default();
        let key = reg.add(cfg("room1")).unwrap();
        let updated = reg.set_status(
            key,
            P2pKeeperStatus {
                state: P2pKeeperState::Registered,
                last_error: None,
                reconnect_attempts: 0,
            },
        );
        assert!(updated);
        let listed = reg.list();
        assert_eq!(listed[0].status.state, P2pKeeperState::Registered);
    }

    #[test]
    fn rejects_invalid_config() {
        let reg = P2pRoomKeeperRegistry::default();
        let mut bad = cfg("");
        bad.room_id = "".into();
        let err = reg.add(bad).unwrap_err();
        assert!(matches!(err, P2pRoomKeeperError::InvalidRoomId(_)));

        let mut bad = cfg("room1");
        bad.server_host = "".into();
        let err = reg.add(bad).unwrap_err();
        assert!(matches!(err, P2pRoomKeeperError::InvalidHost(_)));

        let mut bad = cfg("room1");
        bad.server_port = 0;
        let err = reg.add(bad).unwrap_err();
        assert!(matches!(err, P2pRoomKeeperError::InvalidPort(0)));
    }

    #[test]
    fn enforces_capacity_limit() {
        let reg = P2pRoomKeeperRegistry::with_capacity(2);
        reg.add(cfg("a")).unwrap();
        reg.add(cfg("b")).unwrap();
        let err = reg.add(cfg("c")).unwrap_err();
        assert!(matches!(err, P2pRoomKeeperError::LimitReached(2)));
    }

    #[test]
    fn list_rooms_dedupes_duplicate_room_ids() {
        let reg = P2pRoomKeeperRegistry::default();
        reg.add(cfg("room42")).unwrap();
        reg.add(P2pRoomKeeperConfig {
            // Same room_id, different host — the room only counts once
            // in `list_rooms`.
            server_host: "other.example".into(),
            ..cfg("room42")
        })
        .unwrap();
        let rooms = reg.list_rooms();
        assert_eq!(rooms, vec!["room42"]);
    }
}
