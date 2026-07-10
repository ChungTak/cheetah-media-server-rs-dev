use crate::amf::{AmfValue, AmfVersion};
use crate::bytes::{Buf, BytesReader};
use crate::chunk::RtmpChunkDecoder;
use crate::chunk::{RtmpChunk, RtmpChunkSize, RtmpChunkStreamId};
use crate::command::TransactionId;
use crate::error::{Error, ErrorKind};
use crate::message::{RtmpMessage, RtmpMessageHeader, RtmpMessageType, SetPeerBandwidthLimitType};
use crate::prelude::*;
use crate::user_control::RtmpUserControlEvent;

/// Decodes complete chunks into typed `RtmpMessage`s and updates chunk size on `SetChunkSize`.
/// 将完整的 chunk 解码为类型化的 `RtmpMessage`，并在 `SetChunkSize` 时更新 chunk 大小。
#[derive(Debug, Default)]
pub struct RtmpMessageDecoder {
    chunk_decoder: RtmpChunkDecoder,
    buf: Buf,
}

impl RtmpMessageDecoder {
    /// Appends raw bytes to the internal decode buffer.
    /// 将原始字节追加到内部解码缓冲区。
    pub fn feed_buf(&mut self, buf: &[u8]) {
        self.buf.feed(buf);
    }

    /// Iteratively decodes chunks until a complete message is assembled or more bytes are needed.
    /// 循环解码 chunk，直到组装出完整消息或需要更多字节。
    pub fn decode(&mut self) -> Result<Option<RtmpMessage>, Error> {
        loop {
            let chunk = match self.chunk_decoder.decode(self.buf.get()) {
                Ok((size, maybe_chunk)) => {
                    self.buf.advance(size);

                    if let Some(chunk) = maybe_chunk {
                        chunk
                    } else {
                        continue;
                    }
                }
                Err(e) if e.kind == ErrorKind::InsufficientBuffer => {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };

            let message = decode_rtmp_chunk_to_message(chunk)?;
            match &message {
                RtmpMessage::SetChunkSize { size, .. } => self.chunk_decoder.set_chunk_size(*size),
                RtmpMessage::Abort {
                    chunk_stream_id, ..
                } => self.chunk_decoder.reset_chunk_stream(*chunk_stream_id),
                _ => {}
            }

            return Ok(Some(message));
        }
    }
}

/// Converts a fully reassembled chunk into the corresponding `RtmpMessage` based on its type.
/// 根据类型将完整重组的 chunk 转换为对应的 `RtmpMessage`。
pub fn decode_rtmp_chunk_to_message(chunk: RtmpChunk) -> Result<RtmpMessage, Error> {
    let header = RtmpMessageHeader {
        stream_id: chunk.message_stream_id,
        timestamp: chunk.timestamp,
    };

    let mut payload: &[u8] = &chunk.payload;

    let message = match chunk.message_type {
        RtmpMessageType::SetChunkSize => {
            let size = payload.read_u32()? as usize;
            let size = RtmpChunkSize::saturating_new(size);
            RtmpMessage::SetChunkSize { header, size }
        }
        RtmpMessageType::Abort => {
            let chunk_stream_id = RtmpChunkStreamId::new(payload.read_u32()?)
                .ok_or_else(|| Error::invalid_data("invalid chunk stream ID"))?;
            RtmpMessage::Abort {
                header,
                chunk_stream_id,
            }
        }
        RtmpMessageType::Ack => {
            let sequence_number = payload.read_u32()?;
            RtmpMessage::Ack {
                header,
                sequence_number,
            }
        }
        RtmpMessageType::WinAckSize => {
            let size = payload.read_u32()?;
            RtmpMessage::WinAckSize { header, size }
        }
        RtmpMessageType::SetPeerBandwidth => {
            let size = payload.read_u32()?;
            let limit_type = match payload.read_u8()? {
                0 => SetPeerBandwidthLimitType::Hard,
                1 => SetPeerBandwidthLimitType::Soft,
                2 => SetPeerBandwidthLimitType::Dynamic,
                t => {
                    return Err(Error::invalid_data(format!("invalid limit type: {t}")));
                }
            };
            RtmpMessage::SetPeerBandwidth {
                header,
                size,
                limit_type,
            }
        }
        RtmpMessageType::UserControl => {
            let event = RtmpUserControlEvent::decode(payload)?;
            RtmpMessage::UserControl { header, event }
        }
        RtmpMessageType::Audio => {
            let frame = crate::flv::decode_audio_frame(&chunk.payload, header.timestamp)?;
            RtmpMessage::Audio {
                header,
                frame,
                payload: chunk.payload,
            }
        }
        RtmpMessageType::Video => {
            let frame = crate::flv::decode_video_frame(&chunk.payload, header.timestamp)?;
            RtmpMessage::Video {
                header,
                frame,
                payload: chunk.payload,
            }
        }
        RtmpMessageType::DataAmf0 => {
            let values = decode_amf_values(AmfVersion::Amf0, payload)?;
            RtmpMessage::Data {
                header,
                amf_version: AmfVersion::Amf0,
                values,
            }
        }
        RtmpMessageType::DataAmf3 => {
            let values = decode_amf_values(AmfVersion::Amf3, payload)?;
            RtmpMessage::Data {
                header,
                amf_version: AmfVersion::Amf3,
                values,
            }
        }
        RtmpMessageType::CommandAmf0 => decode_command(AmfVersion::Amf0, header, &chunk.payload)?,
        RtmpMessageType::CommandAmf3 => decode_command(AmfVersion::Amf3, header, &chunk.payload)?,
        RtmpMessageType::Aggregate => {
            // Aggregate messages are handled at the core level by splitting into sub-messages.
            // This path should not be reached since on_message handles Aggregate before decoding.
            RtmpMessage::Data {
                header,
                amf_version: AmfVersion::Amf0,
                values: Vec::new(),
            }
        }
    };

    Ok(message)
}

/// Parses an AMF command payload: name, transaction ID, object, and optional arguments.
/// 解析 AMF 命令负载：名称、事务 ID、对象与可选参数。
fn decode_command(
    mut amf_version: AmfVersion,
    header: RtmpMessageHeader,
    payload: &[u8],
) -> Result<RtmpMessage, Error> {
    let mut buf = payload;

    if amf_version == AmfVersion::Amf3 && buf.first() == Some(&0) {
        buf = &buf[1..];
        amf_version = AmfVersion::Amf0;
    }

    let (size, name) = AmfValue::decode(buf, amf_version)?;
    let name = name.expect_str()?.to_owned();
    buf = &buf[size..];

    let (size, transaction_id) = AmfValue::decode(buf, amf_version)?;
    let transaction_id = TransactionId::from_f64(transaction_id.expect_number()?);
    buf = &buf[size..];

    let (size, object) = AmfValue::decode(buf, amf_version)?;
    buf = &buf[size..];

    let args = decode_amf_values(amf_version, buf)?;

    Ok(RtmpMessage::Command {
        header,
        amf_version,
        name,
        transaction_id,
        object,
        args,
    })
}

/// Decodes all remaining AMF values from the payload until it is exhausted.
/// 从负载中解码所有剩余的 AMF 值，直到负载耗尽。
fn decode_amf_values(amf_version: AmfVersion, mut buf: &[u8]) -> Result<Vec<AmfValue>, Error> {
    let mut values = Vec::new();

    while !buf.is_empty() {
        let (size, value) = AmfValue::decode(buf, amf_version)?;
        buf = &buf[size..];
        values.push(value);
    }

    Ok(values)
}
