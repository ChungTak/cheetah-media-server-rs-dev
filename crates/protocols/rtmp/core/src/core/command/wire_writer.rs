use alloc::vec::Vec;

use bytes::{BufMut, Bytes, BytesMut};

use crate::chunk::{RtmpChunk, RtmpChunkStreamId};
use crate::message::{RtmpMessage, RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

use super::super::{CoreOutput, RtmpCore, RtmpCoreError};

impl RtmpCore {
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

    pub(crate) fn send_rtmp_message(
        &mut self,
        csid: u32,
        message: RtmpMessage,
        out: &mut Vec<CoreOutput>,
    ) {
        let chunk_stream_id = RtmpChunkStreamId::new(csid).expect("valid RTMP chunk stream id");
        let mut wire = Vec::new();
        self.encoder.encode(&mut wire, chunk_stream_id, message);
        out.push(CoreOutput::Write(Bytes::from(wire)));
    }
}
