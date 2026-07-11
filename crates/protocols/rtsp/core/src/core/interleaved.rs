use bytes::{Bytes, BytesMut};

use super::{
    encode_interleaved_frame, parse_interleaved_frame, RtspCoreError, RtspInterleavedEncodeError,
};

/// Interleaved binary frame parsed from the TCP byte stream.
///
/// This is an internal intermediate type consumed by the Sans-I/O state machine;
/// the driver never sees it directly.
///
/// 从 TCP 字节流解析出的交错二进制帧。
///
/// 这是 Sans-I/O 状态机消费的内部中间类型；驱动层不会直接看到它。
pub(crate) struct ParsedInterleavedFrame {
    pub(crate) channel: u8,
    pub(crate) payload: Bytes,
}

/// Try to extract one complete interleaved frame from a byte buffer.
///
/// Uses `parse_interleaved_frame` to inspect the 4-byte framing header; if the
/// payload length is within limits and the whole frame has arrived, the bytes
/// are split from the buffer and returned. Otherwise returns `None` to wait for
/// more data.
///
/// 尝试从字节缓冲区中提取一个完整的交错帧。
///
/// 使用 `parse_interleaved_frame` 检查 4 字节帧头；若负载长度在限制内且整帧已到达，
/// 则从缓冲区切分并返回；否则返回 `None` 等待更多数据。
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

/// Encode an RTP/RTCP payload as an interleaved `$` frame.
///
/// Wraps `encode_interleaved_frame` to expose a `Bytes`-based API and translate
/// the encode error into `RtspCoreError`.
///
/// 将 RTP/RTCP 负载编码为交错的 `$` 帧。
///
/// 封装 `encode_interleaved_frame` 以提供基于 `Bytes` 的 API，并将编码错误转换为
/// `RtspCoreError`。
pub(crate) fn encode_frame(channel: u8, payload: Bytes) -> Result<Bytes, RtspCoreError> {
    let encoded = encode_interleaved_frame(channel, payload.as_ref()).map_err(|err| match err {
        RtspInterleavedEncodeError::PayloadTooLarge { actual, .. } => {
            RtspCoreError::InterleavedPayloadTooLarge(actual)
        }
    })?;
    Ok(Bytes::from(encoded))
}
