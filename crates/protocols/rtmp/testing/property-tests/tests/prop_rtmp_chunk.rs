//! Property-based round-trip tests for the RTMP chunk layer.
//!
//! RTMP chunks consist of a basic header (chunk stream id), a message header
//! (timestamp, length, type, message stream id), and a payload. This module tests
//! chunk encoding/decoding, extended timestamps, chunk stream id encoding sizes,
//! and the incremental state maintained by the decoder across consecutive chunks.
//!
//! RTMP chunk 层的属性测试往返测试。
//!
//! RTMP chunk 由 basic header（chunk stream id）、message header（时间戳、长度、类型、
//! message stream id）以及 payload 组成。本模块测试 chunk 编码/解码、扩展时间戳、chunk stream id
//! 编码大小，以及解码器在相邻 chunk 之间维护的增量状态。

use cheetah_rtmp_core::{
    RtmpChunk, RtmpChunkDecoder, RtmpChunkEncoder, RtmpChunkStreamId, RtmpMessageStreamId,
    RtmpMessageType, RtmpTimestamp,
};
use proptest::prelude::*;

use bytes::Bytes;

/// Generate a valid `RtmpChunkStreamId` across all encoding sizes.
///
/// Valid ids are 2..=65599. The basic header uses 1 byte for ids 2-63, 2 bytes for
/// ids 64-319, and 3 bytes for ids 320-65599.
///
/// 生成覆盖所有编码大小的有效 `RtmpChunkStreamId`。
///
/// 有效 id 范围为 2..=65599。basic header 对 2-63 使用 1 字节，64-319 使用 2 字节，320-65599 使用 3 字节。
fn arb_chunk_stream_id() -> impl Strategy<Value = RtmpChunkStreamId> {
    prop_oneof![
        (2u32..=63).prop_map(|id| RtmpChunkStreamId::new(id).unwrap()),
        (64u32..=319).prop_map(|id| RtmpChunkStreamId::new(id).unwrap()),
        (320u32..=65599).prop_map(|id| RtmpChunkStreamId::new(id).unwrap()),
    ]
}

/// Generate boundary values of `RtmpChunkStreamId`.
///
/// 生成 `RtmpChunkStreamId` 的边界值。
fn arb_chunk_stream_id_boundary() -> impl Strategy<Value = RtmpChunkStreamId> {
    prop_oneof![
        Just(RtmpChunkStreamId::new(2).unwrap()),
        Just(RtmpChunkStreamId::new(63).unwrap()),
        Just(RtmpChunkStreamId::new(64).unwrap()),
        Just(RtmpChunkStreamId::new(319).unwrap()),
        Just(RtmpChunkStreamId::new(320).unwrap()),
        Just(RtmpChunkStreamId::new(65599).unwrap()),
    ]
}

/// Generate a `RtmpMessageType`.
///
/// 生成 `RtmpMessageType`。
fn arb_message_type() -> impl Strategy<Value = RtmpMessageType> {
    prop_oneof![
        Just(RtmpMessageType::SetChunkSize),
        Just(RtmpMessageType::Abort),
        Just(RtmpMessageType::Ack),
        Just(RtmpMessageType::UserControl),
        Just(RtmpMessageType::WinAckSize),
        Just(RtmpMessageType::SetPeerBandwidth),
        Just(RtmpMessageType::Audio),
        Just(RtmpMessageType::Video),
        Just(RtmpMessageType::DataAmf3),
        Just(RtmpMessageType::CommandAmf3),
        Just(RtmpMessageType::DataAmf0),
        Just(RtmpMessageType::CommandAmf0),
    ]
}

/// Generate a timestamp in milliseconds, with extra density around the extended threshold.
///
/// 生成毫秒时间戳，在扩展阈值附近增加密度。
fn arb_timestamp() -> impl Strategy<Value = RtmpTimestamp> {
    prop_oneof![
        (0u32..=0xFFFFFE).prop_map(RtmpTimestamp::from_millis),
        (0xFFFFFFu32..=0x1FFFFFFu32).prop_map(RtmpTimestamp::from_millis),
    ]
}

/// Generate boundary timestamps around the 24-bit extended timestamp threshold.
///
/// 生成 24 位扩展时间戳阈值附近的边界时间戳。
fn arb_timestamp_boundary() -> impl Strategy<Value = RtmpTimestamp> {
    prop_oneof![
        Just(RtmpTimestamp::ZERO),
        Just(RtmpTimestamp::from_millis(0xFFFFFE)),
        Just(RtmpTimestamp::from_millis(0xFFFFFF)),
        Just(RtmpTimestamp::from_millis(0x1000000)),
        Just(RtmpTimestamp::from_millis(0x12345678)),
    ]
}

/// Generate a payload, including empty and common length buckets.
///
/// 生成 payload，包含空以及常见长度区间。
fn arb_payload() -> impl Strategy<Value = Bytes> {
    prop_oneof![
        Just(Bytes::new()),
        prop::collection::vec(any::<u8>(), 1..=127).prop_map(Bytes::from),
        prop::collection::vec(any::<u8>(), 128..=256).prop_map(Bytes::from),
        prop::collection::vec(any::<u8>(), 512..=1024).prop_map(Bytes::from),
    ]
}

/// Generate an arbitrary `RtmpChunk`.
///
/// 生成任意 `RtmpChunk`。
fn arb_chunk() -> impl Strategy<Value = RtmpChunk> {
    (
        arb_chunk_stream_id(),
        any::<u32>(),
        arb_message_type(),
        arb_timestamp(),
        arb_payload(),
    )
        .prop_map(
            |(chunk_stream_id, message_stream_id, message_type, timestamp, payload)| RtmpChunk {
                chunk_stream_id,
                message_stream_id: RtmpMessageStreamId::new(message_stream_id),
                message_type,
                timestamp,
                payload,
            },
        )
}

/// Generate a `RtmpChunk` biased toward boundary values.
///
/// 生成偏向边界值的 `RtmpChunk`。
fn arb_chunk_boundary() -> impl Strategy<Value = RtmpChunk> {
    (
        arb_chunk_stream_id_boundary(),
        any::<u32>(),
        arb_message_type(),
        arb_timestamp_boundary(),
        arb_payload(),
    )
        .prop_map(
            |(chunk_stream_id, message_stream_id, message_type, timestamp, payload)| RtmpChunk {
                chunk_stream_id,
                message_stream_id: RtmpMessageStreamId::new(message_stream_id),
                message_type,
                timestamp,
                payload,
            },
        )
}

/// Feed a byte buffer into a decoder until one complete chunk is produced.
///
/// This helper models segmented TCP delivery and returns the total consumed bytes
/// together with the decoded chunk, or `None` if the buffer was fully drained without
/// completing a chunk.
///
/// 将字节缓冲持续喂入解码器，直到产生一条完整 chunk。
///
/// 该 helper 模拟分段 TCP 交付，返回总消费字节数与解码出的 chunk；
/// 若缓冲被耗尽仍未完成 chunk，则返回 `None`。
fn decode_one_message(decoder: &mut RtmpChunkDecoder, buf: &[u8]) -> (usize, Option<RtmpChunk>) {
    let mut total_consumed = 0;
    let mut remaining = buf;

    loop {
        let (size, maybe_chunk) = decoder.decode(remaining).expect("decode should succeed");
        total_consumed += size;
        remaining = &remaining[size..];

        if let Some(chunk) = maybe_chunk {
            return (total_consumed, Some(chunk));
        }

        if remaining.is_empty() {
            return (total_consumed, None);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Round-trip: encoding and decoding a single chunk preserves all fields.
    ///
    /// 往返：编码并解码单个 chunk 应保留所有字段。
    #[test]
    fn chunk_roundtrip(chunk in arb_chunk()) {
        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk);

        let mut decoder = RtmpChunkDecoder::default();
        let (size, decoded) = decode_one_message(&mut decoder, &buf);

        prop_assert!(decoded.is_some(), "decoded chunk should not be None");
        let decoded = decoded.unwrap();

        prop_assert_eq!(&decoded, &chunk, "roundtrip should preserve chunk data");
        prop_assert_eq!(size, buf.len(), "decoder should consume all bytes");
    }

    /// Round-trip with boundary values for chunk stream id and timestamp.
    ///
    /// 使用 chunk stream id 与时间戳的边界值进行往返测试。
    #[test]
    fn chunk_boundary_roundtrip(chunk in arb_chunk_boundary()) {
        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk);

        let mut decoder = RtmpChunkDecoder::default();
        let (size, decoded) = decode_one_message(&mut decoder, &buf);

        prop_assert!(decoded.is_some(), "decoded chunk should not be None");
        let decoded = decoded.unwrap();

        prop_assert_eq!(&decoded, &chunk, "roundtrip should preserve chunk data");
        prop_assert_eq!(size, buf.len(), "decoder should consume all bytes");
    }

    /// Verify that the chunk stream id is encoded with the correct basic header size.
    ///
    /// 校验 chunk stream id 按正确 basic header 大小编码。
    #[test]
    fn chunk_stream_id_encoding_size(id in 2u32..=65599u32) {
        let chunk_stream_id = RtmpChunkStreamId::new(id).unwrap();
        let chunk = RtmpChunk {
            chunk_stream_id,
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::Ack,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::new(),
        };

        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk);

        let expected_basic_header_size = if id < 64 {
            1
        } else if id < 320 {
            2
        } else {
            3
        };

        let expected_total_size = expected_basic_header_size + 11;
        prop_assert_eq!(buf.len(), expected_total_size,
            "chunk stream id {} should use {} byte basic header",
            id, expected_basic_header_size);
    }

    /// Verify that timestamps at or above 0xFFFFFF emit an extended timestamp field.
    ///
    /// 校验大于等于 0xFFFFFF 的时间戳会输出扩展时间戳字段。
    #[test]
    fn extended_timestamp_encoding(timestamp_ms in 0u32..=0x2000000u32) {
        let timestamp = RtmpTimestamp::from_millis(timestamp_ms);
        let chunk = RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(4).unwrap(),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::Ack,
            timestamp,
            payload: Bytes::new(),
        };

        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk);

        let uses_extended = timestamp_ms >= 0xFFFFFF;
        let expected_size = if uses_extended {
            1 + 11 + 4
        } else {
            1 + 11
        };

        prop_assert_eq!(buf.len(), expected_size,
            "timestamp {} should {}use extended timestamp",
            timestamp_ms, if uses_extended { "" } else { "not " });
    }

    /// Verify that two consecutive chunks on the same chunk stream round-trip.
    ///
    /// The second chunk should use the compact message header format because the
    /// decoder already knows the message stream id, type, and length from the first chunk.
    ///
    /// 校验同一条 chunk stream 上的两个连续 chunk 往返。
    ///
    /// 第二个 chunk 应使用紧凑 message header 格式，因为解码器已从第一个 chunk 获知 message stream id、
    /// 类型与长度。
    #[test]
    fn consecutive_chunks_roundtrip(
        chunk1 in arb_chunk(),
        chunk2_timestamp_delta in 0u32..1000u32,
        chunk2_payload in arb_payload(),
    ) {
        let chunk2 = RtmpChunk {
            chunk_stream_id: chunk1.chunk_stream_id,
            message_stream_id: chunk1.message_stream_id,
            message_type: chunk1.message_type,
            timestamp: chunk1
                .timestamp
                .wrapping_add(RtmpTimestamp::from_millis(chunk2_timestamp_delta)),
            payload: chunk2_payload,
        };

        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk1);
        encoder.encode(&mut buf, &chunk2);

        let mut decoder = RtmpChunkDecoder::default();
        let mut decoded_chunks = Vec::new();
        let mut remaining = buf.as_slice();

        while !remaining.is_empty() {
            let (size, maybe_chunk) = decoder.decode(remaining).expect("decode should succeed");
            remaining = &remaining[size..];
            if let Some(chunk) = maybe_chunk {
                decoded_chunks.push(chunk);
            }
        }

        prop_assert_eq!(decoded_chunks.len(), 2, "should decode exactly 2 chunks");
        prop_assert_eq!(&decoded_chunks[0], &chunk1);
        prop_assert_eq!(&decoded_chunks[1], &chunk2);
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;

    /// Verify the minimum valid chunk stream id.
    ///
    /// 校验最小有效 chunk stream id。
    #[test]
    fn chunk_stream_id_min() {
        let id = RtmpChunkStreamId::new(2).unwrap();
        assert_eq!(id.get(), 2);
    }

    /// Verify the maximum valid chunk stream id.
    ///
    /// 校验最大有效 chunk stream id。
    #[test]
    fn chunk_stream_id_max() {
        let id = RtmpChunkStreamId::new(65599).unwrap();
        assert_eq!(id.get(), 65599);
    }

    /// Verify that out-of-range chunk stream ids are rejected.
    ///
    /// 校验超出范围的 chunk stream id 被拒绝。
    #[test]
    fn chunk_stream_id_out_of_range() {
        assert!(RtmpChunkStreamId::new(1).is_none());
        assert!(RtmpChunkStreamId::new(65600).is_none());
    }

    /// Verify an empty payload round-trip.
    ///
    /// 校验空 payload 的往返。
    #[test]
    fn empty_payload_roundtrip() {
        let chunk = RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(4).unwrap(),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::Ack,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::new(),
        };

        let mut encoder = RtmpChunkEncoder::default();
        let mut buf = Vec::new();
        encoder.encode(&mut buf, &chunk);

        let mut decoder = RtmpChunkDecoder::default();
        let (_, decoded) = decoder.decode(&buf).unwrap();
        assert_eq!(decoded.unwrap(), chunk);
    }
}
