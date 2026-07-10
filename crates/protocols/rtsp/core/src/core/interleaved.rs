use bytes::{Bytes, BytesMut};

use super::{
    encode_interleaved_frame, parse_interleaved_frame, RtspCoreError, RtspInterleavedEncodeError,
};

/// `ParsedInterleavedFrame` data structure.
/// `ParsedInterleavedFrame` 数据结构.
pub(crate) struct ParsedInterleavedFrame {
    /// `channel` field of type `u8`.
    /// `channel` 字段，类型为 `u8`.
    pub(crate) channel: u8,
    /// `payload` field of type `Bytes`.
    /// `payload` 字段，类型为 `Bytes`.
    pub(crate) payload: Bytes,
}

/// `try_parse_frame` function.
/// `try_parse_frame` 函数.
pub(crate) fn try_parse_frame(
    buffer: &mut BytesMut,
    max_frame_size: usize,
) -> Result<Option<ParsedInterleavedFrame>, RtspCoreError> {
    let Some(frame_header) = parse_interleaved_frame(buffer.as_ref()) else {
        return Ok(None);
    };
    let payload_len = usize::from(frame_header.payload_len);
    if payload_len > max_frame_size {
        return Err(RtspCoreError::InterleavedFrameSizeLimitExceeded {
            max: max_frame_size,
            actual: payload_len,
        });
    }
    let packet_len = frame_header.total_len;
    if buffer.len() < packet_len {
        return Ok(None);
    }

    let packet = buffer.split_to(packet_len).freeze();
    Ok(Some(ParsedInterleavedFrame {
        channel: packet[1],
        payload: packet.slice(4..),
    }))
}

/// `encode_frame` function.
/// `encode_frame` 函数.
pub(crate) fn encode_frame(channel: u8, payload: Bytes) -> Result<Bytes, RtspCoreError> {
    let encoded = encode_interleaved_frame(channel, payload.as_ref()).map_err(|err| match err {
        RtspInterleavedEncodeError::PayloadTooLarge { actual, .. } => {
            RtspCoreError::InterleavedPayloadTooLarge(actual)
        }
    })?;
    Ok(Bytes::from(encoded))
}
