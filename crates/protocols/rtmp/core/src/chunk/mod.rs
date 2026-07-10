pub mod decoder;
pub mod encoder;

pub use decoder::RtmpChunkDecoder;
pub use encoder::RtmpChunkEncoder;

use bytes::Bytes;

use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkStreamId(u32);

impl RtmpChunkStreamId {
    pub fn new(id: u32) -> Option<Self> {
        (2..=65599).contains(&id).then_some(Self(id))
    }

    pub fn from_message_stream_id(message_stream_id: RtmpMessageStreamId) -> Self {
        let raw = message_stream_id.get().saturating_add(2);
        Self(raw.min(65599))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageHeaderFormat {
    F0 = 0,
    F1,
    F2,
    F3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkSize(usize);

impl RtmpChunkSize {
    pub const MIN: Self = Self(1);
    pub const MAX: Self = Self(65536);

    pub fn new(size: usize) -> Option<Self> {
        let this = Self(size);
        (Self::MIN..=Self::MAX).contains(&this).then_some(this)
    }

    pub fn saturating_new(size: usize) -> Self {
        Self(size).clamp(Self::MIN, Self::MAX)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for RtmpChunkSize {
    fn default() -> Self {
        Self(128)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpChunk {
    pub chunk_stream_id: RtmpChunkStreamId,
    pub message_stream_id: RtmpMessageStreamId,
    pub message_type: RtmpMessageType,
    pub timestamp: RtmpTimestamp,
    pub payload: Bytes,
}
