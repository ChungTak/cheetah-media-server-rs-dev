//! User Control Event Fuzzing
//!
//! RTMP 规范 Section 6.2: User Control Messages
//!
//! User Control 消息通过 message type 4 发送。
//! 它由 Event type（2 字节）+ Event data（可变长度）组成。
//!
//! Event types:
//! - 0: StreamBegin
//! - 1: StreamEof
//! - 2: StreamDry
//! - 3: SetBufferLength
//! - 4: StreamIsRecorded
//! - 6: PingRequest
//! - 7: PingResponse
//! - 31: BufferEmpty
//! - 32: BufferReady

#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::RtmpUserControlEvent;

fuzz_target!(|data: &[u8]| {
    // 尝试解码 User Control Event
    if let Ok(event) = RtmpUserControlEvent::decode(data) {
        // 如果解码成功，也尝试重新编码
        let mut encoded = Vec::new();
        event.encode(&mut encoded);
    }
});
