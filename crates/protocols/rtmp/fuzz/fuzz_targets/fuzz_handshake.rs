//! RTMP Handshake Fuzzing
//!
//! RTMP 规范 Section 5.2: Handshake
//!
//! 握手由 C0/S0（1 byte）+ C1/S1（1536 bytes）+ C2/S2（1536 bytes）组成。
//! 这个 fuzz target 用于测试服务端和客户端对错误握手数据的鲁棒性。

#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::{RtmpClientHandshake, RtmpServerHandshake};

fuzz_target!(|data: &[u8]| {
    // 对服务端握手进行模糊测试
    {
        let mut server = RtmpServerHandshake::new();
        let _ = server.feed_recv_buf(data);
    }

    // 对客户端握手进行模糊测试
    {
        let mut client = RtmpClientHandshake::new();
        let _ = client.feed_recv_buf(data);
    }
});
