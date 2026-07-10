use crate::amf::{AmfValue, AmfVersion};
use crate::bytes::BytesWriter;
use crate::chunk::RtmpChunkEncoder;
use crate::chunk::{RtmpChunk, RtmpChunkStreamId};
use crate::command::TransactionId;
use crate::message::RtmpMessage;
use crate::prelude::*;

use bytes::Bytes;

/// `RtmpMessageEncoder` data structure.
/// `RtmpMessageEncoder` 数据结构.
#[derive(Debug, Default)]
pub struct RtmpMessageEncoder {
    /// `chunk_encoder` field of type `RtmpChunkEncoder`.
    /// `chunk_encoder` 字段，类型为 `RtmpChunkEncoder`.
    chunk_encoder: RtmpChunkEncoder,
}

impl RtmpMessageEncoder {
    /// Sets the `chunk_size` value.
    /// Sets `chunk_size` 值.
    pub fn set_chunk_size(&mut self, size: crate::chunk::RtmpChunkSize) {
        self.chunk_encoder.set_chunk_size(size);
    }

    /// `encode_raw_chunk` function.
    /// `encode_raw_chunk` 函数.
    pub fn encode_raw_chunk(&mut self, buf: &mut Vec<u8>, chunk: &RtmpChunk) {
        self.chunk_encoder.encode(buf, chunk);
    }

    /// `encode` function.
    /// `encode` 函数.
    pub fn encode(
        &mut self,
        buf: &mut Vec<u8>,
        chunk_stream_id: RtmpChunkStreamId,
        message: RtmpMessage,
    ) {
        let new_chunk_size = if let RtmpMessage::SetChunkSize { size, .. } = message {
            // [NOTE]
            // SetChunkSize 会影响消息编码为块的方式，
            // 因此必须在本方法内处理，而非由调用方处理
            //
            // 另外，本 crate 本身不会发出 Abort，
            // 所以没有对应的处理
            // （解码端需要处理，因为对方可能会发送）
            Some(size)
        } else {
            None
        };

        let chunk = self.message_to_chunk(chunk_stream_id, message);
        self.chunk_encoder.encode(buf, &chunk);

        if let Some(size) = new_chunk_size {
            self.chunk_encoder.set_chunk_size(size);
        }
    }

    fn message_to_chunk(
        &self,
        chunk_stream_id: RtmpChunkStreamId,
        message: RtmpMessage,
    ) -> RtmpChunk {
        let header = message.header();
        let message_type = message.message_type();
        let mut payload = Vec::new();
        self.encode_message_payload(&mut payload, message);
        RtmpChunk {
            chunk_stream_id,
            message_stream_id: header.stream_id,
            message_type,
            timestamp: header.timestamp,
            payload: Bytes::from(payload),
        }
    }

    fn encode_message_payload(&self, buf: &mut Vec<u8>, message: RtmpMessage) {
        match message {
            RtmpMessage::SetChunkSize { size, .. } => {
                buf.write_u32(size.get() as u32);
            }
            RtmpMessage::Abort {
                chunk_stream_id, ..
            } => {
                buf.write_u32(chunk_stream_id.get());
            }
            RtmpMessage::Ack {
                sequence_number, ..
            } => {
                buf.write_u32(sequence_number);
            }
            RtmpMessage::WinAckSize { size, .. } => {
                buf.write_u32(size);
            }
            RtmpMessage::SetPeerBandwidth {
                size, limit_type, ..
            } => {
                buf.write_u32(size);
                buf.write_u8(limit_type as u8);
            }
            RtmpMessage::Audio { frame, .. } => {
                crate::flv::encode_audio_frame(buf, &frame);
            }
            RtmpMessage::Video { frame, .. } => {
                crate::flv::encode_video_frame(buf, &frame);
            }
            RtmpMessage::Data { values, .. } => {
                self.encode_data_payload(buf, &values);
            }
            RtmpMessage::UserControl { event, .. } => {
                event.encode(buf);
            }
            RtmpMessage::Command {
                amf_version,
                name,
                transaction_id,
                object,
                args,
                ..
            } => {
                self.encode_command(buf, amf_version, &name, transaction_id, &object, &args);
            }
        }
    }

    fn encode_command(
        &self,
        buf: &mut Vec<u8>,
        amf_version: AmfVersion,
        name: &str,
        transaction_id: TransactionId,
        object: &AmfValue,
        args: &[AmfValue],
    ) {
        AmfValue::from((amf_version, name)).encode(buf);
        AmfValue::from((amf_version, transaction_id.get() as f64)).encode(buf);
        object.encode(buf);
        for arg in args {
            arg.encode(buf);
        }
    }

    fn encode_data_payload(&self, buf: &mut Vec<u8>, values: &[AmfValue]) {
        for value in values {
            value.encode(buf);
        }
    }
}
