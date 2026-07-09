//! RTMP Client Connection Fuzzing
//!
//! 对 RTMP 客户端连接的接收数据处理进行模糊测试。

#![no_main]

use cheetah_rtmp_core::{CoreInput, RtmpCore};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut core = RtmpCore::new_client();
    let _ = core.handle_input(CoreInput::Bytes(data.to_vec().into()));
});
