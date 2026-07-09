use bytes::{Bytes, BytesMut};

use super::{
    encode_interleaved_frame, parse_interleaved_frame, RtspCoreError, RtspInterleavedEncodeError,
};

pub(crate) struct ParsedInterleavedFrame {
    pub(crate) channel: u8,
    pub(crate) payload: Bytes,
}

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

pub(crate) fn encode_frame(channel: u8, payload: Bytes) -> Result<Bytes, RtspCoreError> {
    let encoded = encode_interleaved_frame(channel, payload.as_ref()).map_err(|err| match err {
        RtspInterleavedEncodeError::PayloadTooLarge { actual, .. } => {
            RtspCoreError::InterleavedPayloadTooLarge(actual)
        }
    })?;
    Ok(Bytes::from(encoded))
}
