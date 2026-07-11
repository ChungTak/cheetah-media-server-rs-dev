//! Property-based round-trip tests for RTMP user control events.
//!
//! User control events are protocol control messages (message type 4) used for flow
//! control, stream notifications, and ping/pong. This module verifies that every
//! variant encodes to a binary payload and decodes back to the same value.
//!
//! RTMP 用户控制事件的属性测试往返测试。
//!
//! 用户控制事件是协议控制消息（message type 4），用于流控、流通知以及 ping/pong。
//! 本模块校验每个变体都能编码为二进制负载并解码回相同值。

use cheetah_rtmp_core::{RtmpMessageStreamId, RtmpTimestamp, RtmpUserControlEvent};
use proptest::prelude::*;

/// Generate an arbitrary `RtmpMessageStreamId`.
///
/// 生成任意 `RtmpMessageStreamId`。
fn arb_stream_id() -> impl Strategy<Value = RtmpMessageStreamId> {
    any::<u32>().prop_map(RtmpMessageStreamId::new)
}

/// Generate an arbitrary `RtmpUserControlEvent` covering all defined event types.
///
/// The generator deliberately includes `SetBufferLength`, `PingRequest`,
/// `PingResponse`, and the buffer state events so the encoder is exercised across
/// the full event ID space.
///
/// 生成覆盖所有已定义事件类型的任意 `RtmpUserControlEvent`。
///
/// 生成器特意包含 `SetBufferLength`、`PingRequest`、`PingResponse` 以及缓冲区状态事件，
/// 使编码器在完整事件 ID 空间上都得到测试。
fn arb_event() -> impl Strategy<Value = RtmpUserControlEvent> {
    prop_oneof![
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::StreamBegin { stream_id }),
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::StreamEof { stream_id }),
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::StreamDry { stream_id }),
        (arb_stream_id(), any::<u32>()).prop_map(|(stream_id, length)| {
            RtmpUserControlEvent::SetBufferLength { stream_id, length }
        }),
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::StreamIsRecorded { stream_id }),
        any::<u32>().prop_map(|ms| RtmpUserControlEvent::PingRequest {
            timestamp: RtmpTimestamp::from_millis(ms),
        }),
        any::<u32>().prop_map(|ms| RtmpUserControlEvent::PingResponse {
            timestamp: RtmpTimestamp::from_millis(ms),
        }),
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::BufferEmpty { stream_id }),
        arb_stream_id().prop_map(|stream_id| RtmpUserControlEvent::BufferReady { stream_id }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Verify that every user control event round-trips through encode/decode.
    ///
    /// 校验每个用户控制事件都能通过 encode/decode 往返。
    #[test]
    fn user_control_roundtrip(event in arb_event()) {
        let mut buf = Vec::new();
        event.encode(&mut buf);
        let decoded = RtmpUserControlEvent::decode(&buf).expect("decode should succeed");
        prop_assert_eq!(decoded, event);
    }
}
