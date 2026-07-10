use crate::bytes::BytesReader;
use crate::error::Error;
use crate::message::RtmpMessageStreamId;
use crate::prelude::*;
use crate::timestamp::RtmpTimestamp;

const EVENT_STREAM_BEGIN: u16 = 0;
const EVENT_STREAM_EOF: u16 = 1;
const EVENT_STREAM_DRY: u16 = 2;
const EVENT_SET_BUFFER_LENGTH: u16 = 3;
const EVENT_STREAM_IS_RECORDED: u16 = 4;
const EVENT_PING_REQUEST: u16 = 6;
const EVENT_PING_RESPONSE: u16 = 7;
const EVENT_BUFFER_EMPTY: u16 = 31;
const EVENT_BUFFER_READY: u16 = 32;

/// RTMP user control event carried in a `UserControl` protocol message.
/// RTMP 用户控制事件，由 `UserControl` 协议消息承载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtmpUserControlEvent {
    /// Stream playback begins.
    /// 流开始播放。
    StreamBegin { stream_id: RtmpMessageStreamId },
    /// End of stream.
    /// 流结束。
    StreamEof { stream_id: RtmpMessageStreamId },
    /// Stream is dry (no more data).
    /// 流已干涸（无更多数据）。
    StreamDry { stream_id: RtmpMessageStreamId },
    /// Set the client buffer length for the stream.
    /// 设置客户端流缓冲区长度。
    SetBufferLength {
        stream_id: RtmpMessageStreamId,
        length: u32,
    },
    /// The stream is recorded.
    /// 该流正在录制。
    StreamIsRecorded { stream_id: RtmpMessageStreamId },
    /// Server ping request, client must respond with a `PingResponse`.
    /// 服务端 ping 请求，客户端必须以 `PingResponse` 响应。
    PingRequest { timestamp: RtmpTimestamp },
    /// Response to a `PingRequest`.
    /// 对 `PingRequest` 的响应。
    PingResponse { timestamp: RtmpTimestamp },
    /// Client buffer is empty.
    /// 客户端缓冲区为空。
    BufferEmpty { stream_id: RtmpMessageStreamId },
    /// Client buffer has data ready.
    /// 客户端缓冲区已有数据就绪。
    BufferReady { stream_id: RtmpMessageStreamId },
}

impl RtmpUserControlEvent {
    /// Returns the wire name of the user control event.
    /// 返回用户控制事件的线名称。
    pub fn name(&self) -> &'static str {
        match self {
            Self::StreamBegin { .. } => "StreamBegin",
            Self::StreamEof { .. } => "StreamEof",
            Self::StreamDry { .. } => "StreamDry",
            Self::SetBufferLength { .. } => "SetBufferLength",
            Self::StreamIsRecorded { .. } => "StreamIsRecorded",
            Self::PingRequest { .. } => "PingRequest",
            Self::PingResponse { .. } => "PingResponse",
            Self::BufferEmpty { .. } => "BufferEmpty",
            Self::BufferReady { .. } => "BufferReady",
        }
    }

    /// Encodes the event into the payload buffer for a `UserControl` message.
    /// 将事件编码到 `UserControl` 消息负载缓冲区。
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Self::StreamBegin { stream_id } => {
                buf.extend_from_slice(&EVENT_STREAM_BEGIN.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
            Self::StreamEof { stream_id } => {
                buf.extend_from_slice(&EVENT_STREAM_EOF.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
            Self::StreamDry { stream_id } => {
                buf.extend_from_slice(&EVENT_STREAM_DRY.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
            Self::SetBufferLength { stream_id, length } => {
                buf.extend_from_slice(&EVENT_SET_BUFFER_LENGTH.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
                buf.extend_from_slice(&length.to_be_bytes());
            }
            Self::StreamIsRecorded { stream_id } => {
                buf.extend_from_slice(&EVENT_STREAM_IS_RECORDED.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
            Self::PingRequest { timestamp } => {
                buf.extend_from_slice(&EVENT_PING_REQUEST.to_be_bytes());
                buf.extend_from_slice(&timestamp.as_millis().to_be_bytes());
            }
            Self::PingResponse { timestamp } => {
                buf.extend_from_slice(&EVENT_PING_RESPONSE.to_be_bytes());
                buf.extend_from_slice(&timestamp.as_millis().to_be_bytes());
            }
            Self::BufferEmpty { stream_id } => {
                buf.extend_from_slice(&EVENT_BUFFER_EMPTY.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
            Self::BufferReady { stream_id } => {
                buf.extend_from_slice(&EVENT_BUFFER_READY.to_be_bytes());
                buf.extend_from_slice(&stream_id.get().to_be_bytes());
            }
        }
    }

    /// Decodes a user control event from a `UserControl` payload buffer.
    /// 从 `UserControl` 负载缓冲区中解码用户控制事件。
    pub fn decode(mut buf: &[u8]) -> Result<Self, Error> {
        let event_type = buf.read_u16()?;
        let event = match event_type {
            EVENT_STREAM_BEGIN => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::StreamBegin { stream_id }
            }
            EVENT_STREAM_EOF => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::StreamEof { stream_id }
            }
            EVENT_STREAM_DRY => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::StreamDry { stream_id }
            }
            EVENT_SET_BUFFER_LENGTH => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                let length = buf.read_u32()?;
                Self::SetBufferLength { stream_id, length }
            }
            EVENT_STREAM_IS_RECORDED => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::StreamIsRecorded { stream_id }
            }
            EVENT_PING_REQUEST => {
                let timestamp = RtmpTimestamp::from_millis(buf.read_u32()?);
                Self::PingRequest { timestamp }
            }
            EVENT_PING_RESPONSE => {
                let timestamp = RtmpTimestamp::from_millis(buf.read_u32()?);
                Self::PingResponse { timestamp }
            }
            EVENT_BUFFER_EMPTY => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::BufferEmpty { stream_id }
            }
            EVENT_BUFFER_READY => {
                let stream_id = RtmpMessageStreamId::new(buf.read_u32()?);
                Self::BufferReady { stream_id }
            }
            _ => {
                return Err(Error::invalid_data(format!(
                    "unknown user control event type: {event_type}"
                )));
            }
        };
        Ok(event)
    }
}
