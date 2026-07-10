/// Module for `decoder`.
/// `decoder` 相关模块。
pub mod decoder;
/// Module for `encoder`.
/// `encoder` 相关模块。
pub mod encoder;

pub use decoder::RtmpChunkDecoder;
pub use encoder::RtmpChunkEncoder;

use bytes::Bytes;

use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

/// Identifier for `RTMP Chunk Stream`.
/// `RTMP Chunk Stream` 的标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkStreamId(u32);

impl RtmpChunkStreamId {
    /// Creates a new `RtmpChunkStreamId` instance.
    /// 创建新的 `RtmpChunkStreamId` 实例。
    pub fn new(id: u32) -> Option<Self> {
        (2..=65599).contains(&id).then_some(Self(id))
    }

    /// Creates `message stream ID` from input.
    /// 从输入创建 `message stream ID`。
    pub fn from_message_stream_id(message_stream_id: RtmpMessageStreamId) -> Self {
        let raw = message_stream_id.get().saturating_add(2);
        Self(raw.min(65599))
    }

    /// Returns the value.
    /// 返回值。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// `MessageHeaderFormat` enumeration.
/// `MessageHeaderFormat` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageHeaderFormat {
    F0 = 0,
    F1,
    F2,
    F3,
}

/// `RtmpChunkSize` data structure.
/// `RtmpChunkSize` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkSize(usize);

impl RtmpChunkSize {
    pub const MIN: Self = Self(1);
    pub const MAX: Self = Self(65536);

    /// Creates a new `RtmpChunkSize` instance.
    /// 创建新的 `RtmpChunkSize` 实例。
    pub fn new(size: usize) -> Option<Self> {
        let this = Self(size);
        (Self::MIN..=Self::MAX).contains(&this).then_some(this)
    }

    /// `saturating_new` function of `RtmpChunkSize`.
    /// `RtmpChunkSize` 的 `saturating_new` 函数。
    pub fn saturating_new(size: usize) -> Self {
        Self(size).clamp(Self::MIN, Self::MAX)
    }

    /// Returns the value.
    /// 返回值。
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for RtmpChunkSize {
    fn default() -> Self {
        Self(128)
    }
}

/// `RtmpChunk` data structure.
/// `RtmpChunk` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpChunk {
    pub chunk_stream_id: RtmpChunkStreamId,
    pub message_stream_id: RtmpMessageStreamId,
    pub message_type: RtmpMessageType,
    pub timestamp: RtmpTimestamp,
    pub payload: Bytes,
}
