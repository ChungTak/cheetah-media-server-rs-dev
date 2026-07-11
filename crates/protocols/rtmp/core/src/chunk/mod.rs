/// Chunk decoding logic: reassembles fragmented RTMP messages into complete chunks.
/// 块解码逻辑：将分段的 RTMP 消息重组为完整的 chunk。
pub mod decoder;
/// Chunk encoding logic: splits RTMP messages into chunks with compact headers.
/// 块编码逻辑：将 RTMP 消息拆分为带紧凑头部的 chunk。
pub mod encoder;

pub use decoder::RtmpChunkDecoder;
pub use encoder::RtmpChunkEncoder;

use bytes::Bytes;

use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

/// RTMP chunk stream identifier (CSID), validated to the 2..=65599 range.
/// RTMP 块流标识符（CSID），校验范围固定为 2..=65599。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkStreamId(u32);

impl RtmpChunkStreamId {
    /// Creates a valid chunk stream ID from a raw `u32`, rejecting protocol-reserved values.
    /// 从原始 `u32` 创建一个合法的块流 ID，拒绝协议保留值。
    pub fn new(id: u32) -> Option<Self> {
        (2..=65599).contains(&id).then_some(Self(id))
    }

    /// Derives a chunk stream ID from a message stream ID using a fixed offset mapping.
    /// 使用固定偏移映射，从消息流 ID 派生出块流 ID。
    pub fn from_message_stream_id(message_stream_id: RtmpMessageStreamId) -> Self {
        let raw = message_stream_id.get().saturating_add(2);
        Self(raw.min(65599))
    }

    /// Returns the raw chunk stream ID value.
    /// 返回原始块流 ID 值。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Chunk header format (F0..F3) that controls which message fields are repeated.
/// 块头部格式（F0..F3），控制哪些消息字段被重复发送。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageHeaderFormat {
    /// Full 11-byte header carrying timestamp, size, type and stream ID.
    /// 完整 11 字节头部，包含时间戳、大小、类型和流 ID。
    F0 = 0,
    /// Header omits stream ID; reuses the one from the previous chunk on this stream.
    /// 头部省略流 ID；复用当前流上一 chunk 的流 ID。
    F1,
    /// Header carries only the timestamp delta; size and type are reused.
    /// 头部仅携带时间戳增量；大小和类型被复用。
    F2,
    /// Empty header; everything is reused from the previous chunk on this stream.
    /// 空头部；当前流上一 chunk 的所有字段都被复用。
    F3,
}

/// Negotiated chunk payload size, clamped to the 1..=65536 range.
/// 协商的块负载大小，被限制在 1..=65536 范围内。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RtmpChunkSize(usize);

impl RtmpChunkSize {
    /// Minimum chunk payload size allowed by the protocol.
    /// 协议允许的最小块负载大小。
    pub const MIN: Self = Self(1);
    /// Maximum chunk payload size allowed by the protocol.
    /// 协议允许的最大块负载大小。
    pub const MAX: Self = Self(65536);

    /// Constructs a chunk size if it is within the protocol limits.
    /// 在协议限制范围内构造 chunk 大小。
    pub fn new(size: usize) -> Option<Self> {
        let this = Self(size);
        (Self::MIN..=Self::MAX).contains(&this).then_some(this)
    }

    /// Constructs a chunk size, clamping any out-of-range value to the valid interval.
    /// 构造 chunk 大小，并将越界值裁剪到合法区间。
    pub fn saturating_new(size: usize) -> Self {
        Self(size).clamp(Self::MIN, Self::MAX)
    }

    /// Returns the raw chunk payload size in bytes.
    /// 返回原始块负载大小（字节）。
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for RtmpChunkSize {
    fn default() -> Self {
        Self(128)
    }
}

/// A complete RTMP chunk, either newly decoded or ready to encode.
/// 一个完整的 RTMP chunk，可以是刚解码出来的或准备编码的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpChunk {
    pub chunk_stream_id: RtmpChunkStreamId,
    pub message_stream_id: RtmpMessageStreamId,
    pub message_type: RtmpMessageType,
    pub timestamp: RtmpTimestamp,
    pub payload: Bytes,
}
