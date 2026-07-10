/// `decoder` module.
/// `decoder` 模块.
pub mod decoder;
/// `encoder` module.
/// `encoder` 模块.
pub mod encoder;

pub use decoder::RtmpChunkDecoder;
pub use encoder::RtmpChunkEncoder;

use bytes::Bytes;

use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

/// `RtmpChunkStreamId` data structure.
/// `RtmpChunkStreamId` 数据结构.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkStreamId(u32);

impl RtmpChunkStreamId {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(id: u32) -> Option<Self> {
        (2..=65599).contains(&id).then_some(Self(id))
    }

    /// Creates `message_stream_id` from input.
    /// 创建 `message_stream_id` 来自 输入.
    pub fn from_message_stream_id(message_stream_id: RtmpMessageStreamId) -> Self {
        let raw = message_stream_id.get().saturating_add(2);
        Self(raw.min(65599))
    }

    /// `get` function.
    /// `get` 函数.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// `MessageHeaderFormat` enumeration.
/// `MessageHeaderFormat` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageHeaderFormat {
    /// `F0` variant.
    /// `F0` 变体.
    F0 = 0,
    /// `F1` variant.
    /// `F1` 变体.
    F1,
    /// `F2` variant.
    /// `F2` 变体.
    F2,
    /// `F3` variant.
    /// `F3` 变体.
    F3,
}

/// `RtmpChunkSize` data structure.
/// `RtmpChunkSize` 数据结构.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkSize(usize);

impl RtmpChunkSize {
    pub const MIN: Self = Self(1);
    pub const MAX: Self = Self(65536);

    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(size: usize) -> Option<Self> {
        let this = Self(size);
        (Self::MIN..=Self::MAX).contains(&this).then_some(this)
    }

    /// `saturating_new` function.
    /// `saturating_new` 函数.
    pub fn saturating_new(size: usize) -> Self {
        Self(size).clamp(Self::MIN, Self::MAX)
    }

    /// `get` function.
    /// `get` 函数.
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
/// `RtmpChunk` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpChunk {
    /// `chunk_stream_id` field of type `RtmpChunkStreamId`.
    /// `chunk_stream_id` 字段，类型为 `RtmpChunkStreamId`.
    pub chunk_stream_id: RtmpChunkStreamId,
    /// `message_stream_id` field of type `RtmpMessageStreamId`.
    /// `message_stream_id` 字段，类型为 `RtmpMessageStreamId`.
    pub message_stream_id: RtmpMessageStreamId,
    /// `message_type` field of type `RtmpMessageType`.
    /// `message_type` 字段，类型为 `RtmpMessageType`.
    pub message_type: RtmpMessageType,
    /// `timestamp` field of type `RtmpTimestamp`.
    /// `timestamp` 字段，类型为 `RtmpTimestamp`.
    pub timestamp: RtmpTimestamp,
    /// `payload` field of type `Bytes`.
    /// `payload` 字段，类型为 `Bytes`.
    pub payload: Bytes,
}
