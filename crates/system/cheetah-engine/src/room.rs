use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_sdk::{RoomId, RoomParticipant, RoomServiceApi, RoomSnapshot, SdkError, StreamKey};
use dashmap::DashMap;
use parking_lot::RwLock;

/// In-memory room state with participants and bound streams.
///
/// 内存房间状态，包含参与者与绑定的流。
struct RoomEntry {
    name: String,
    participants: RwLock<Vec<RoomParticipant>>,
    streams: RwLock<Vec<StreamKey>>,
}

/// In-memory room manager with participant/stream bindings.
///
/// 内存房间管理器，支持参与者/流绑定。
#[derive(Default)]
pub struct RoomService {
    next_id: AtomicU64,
    rooms: DashMap<RoomId, RoomEntry>,
}

impl RoomService {
    /// Build a public `RoomSnapshot` from a room entry.
    ///
    /// 从房间条目构建公共 `RoomSnapshot`。
    fn snapshot_from(room_id: RoomId, entry: &RoomEntry) -> RoomSnapshot {
        RoomSnapshot {
            room_id,
            name: entry.name.clone(),
            participants: entry.participants.read().clone(),
            bound_streams: entry.streams.read().clone(),
        }
    }
}

/// `RoomServiceApi` implementation backed by `DashMap`.
///
/// 由 `DashMap` 支持的 `RoomServiceApi` 实现。
impl RoomServiceApi for RoomService {
    fn create_room(&self, name: &str) -> Result<RoomId, SdkError> {
        let id = RoomId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        self.rooms.insert(
            id,
            RoomEntry {
                name: name.to_string(),
                participants: RwLock::new(Vec::new()),
                streams: RwLock::new(Vec::new()),
            },
        );
        Ok(id)
    }

    fn delete_room(&self, room_id: RoomId) -> Result<(), SdkError> {
        self.rooms
            .remove(&room_id)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("room {}", room_id.0)))
    }

    fn join_room(&self, room_id: RoomId, participant_id: &str) -> Result<(), SdkError> {
        let room = self
            .rooms
            .get(&room_id)
            .ok_or_else(|| SdkError::NotFound(format!("room {}", room_id.0)))?;
        let mut participants = room.participants.write();
        if participants.iter().any(|p| p.id == participant_id) {
            return Ok(());
        }
        participants.push(RoomParticipant {
            id: participant_id.to_string(),
        });
        Ok(())
    }

    fn leave_room(&self, room_id: RoomId, participant_id: &str) -> Result<(), SdkError> {
        let room = self
            .rooms
            .get(&room_id)
            .ok_or_else(|| SdkError::NotFound(format!("room {}", room_id.0)))?;
        room.participants.write().retain(|p| p.id != participant_id);
        Ok(())
    }

    fn bind_stream(&self, room_id: RoomId, stream_key: StreamKey) -> Result<(), SdkError> {
        let room = self
            .rooms
            .get(&room_id)
            .ok_or_else(|| SdkError::NotFound(format!("room {}", room_id.0)))?;
        let mut streams = room.streams.write();
        if !streams.contains(&stream_key) {
            streams.push(stream_key);
        }
        Ok(())
    }

    fn unbind_stream(&self, room_id: RoomId, stream_key: &StreamKey) -> Result<(), SdkError> {
        let room = self
            .rooms
            .get(&room_id)
            .ok_or_else(|| SdkError::NotFound(format!("room {}", room_id.0)))?;
        room.streams.write().retain(|v| v != stream_key);
        Ok(())
    }

    fn get_room(&self, room_id: RoomId) -> Result<Option<RoomSnapshot>, SdkError> {
        Ok(self
            .rooms
            .get(&room_id)
            .map(|entry| Self::snapshot_from(room_id, entry.value())))
    }

    fn snapshot(&self) -> Vec<RoomSnapshot> {
        let mut out: Vec<_> = self
            .rooms
            .iter()
            .map(|entry| Self::snapshot_from(*entry.key(), entry.value()))
            .collect();
        out.sort_by_key(|snapshot| snapshot.room_id);
        out
    }
}
