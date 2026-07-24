use alloc::vec::Vec;

use bytes::{BufMut, Bytes, BytesMut};

use crate::chunk::{RtmpChunk, RtmpChunkStreamId};
use crate::message::{RtmpMessage, RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

use super::super::{CoreOutput, RtmpCore, RtmpCoreError};

impl RtmpCore {
    /// Encodes a user control event of the given type and value.
    /// 编码指定类型与值的用户控制事件。
    pub(crate) fn send_user_control(
        &mut self,
        event_type: u16,
        value: u32,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let mut payload = BytesMut::with_capacity(6);
        payload.put_u16(event_type);
        payload.put_u32(value);
        self.send_message(2, 0, 4, 0, payload.freeze(), out)
    }

    /// Encodes a raw payload as a `RtmpMessage` with a fixed chunk stream ID.
    /// 将原始负载编码为指定 chunk 流 ID 的 `RtmpMessage`。
    pub(crate) fn send_message(
        &mut self,
        csid: u32,
        timestamp: u32,
        message_type: u8,
        message_stream_id: u32,
        payload: Bytes,
        out: &mut Vec<CoreOutput>,
    ) -> Result<(), RtmpCoreError> {
        let chunk_stream_id = RtmpChunkStreamId::new(csid)
            .ok_or_else(|| RtmpCoreError::Chunk(format!("invalid outbound csid: {csid}")))?;
        let message_type = RtmpMessageType::from_type_id(message_type)?;
        let chunk = RtmpChunk {
            chunk_stream_id,
            message_stream_id: RtmpMessageStreamId::new(message_stream_id),
            message_type,
            timestamp: RtmpTimestamp::from_millis(timestamp),
            payload,
        };
        let mut wire = Vec::new();
        self.encoder.encode_raw_chunk(&mut wire, &chunk);
        out.push(CoreOutput::Write(Bytes::from(wire)));
        Ok(())
    }

    /// Encodes a fully formed `RtmpMessage` onto the chunk stream with the given ID.
    /// 将完整的 `RtmpMessage` 编码到指定 ID 的 chunk 流。
    pub(crate) fn send_rtmp_message(
        &mut self,
        csid: u32,
        message: RtmpMessage,
        out: &mut Vec<CoreOutput>,
    ) {
        let chunk_stream_id = RtmpChunkStreamId::new(csid).unwrap_or_else(|| unreachable!());
        let mut wire = Vec::new();
        self.encoder.encode(&mut wire, chunk_stream_id, message);
        out.push(CoreOutput::Write(Bytes::from(wire)));
    }
}
