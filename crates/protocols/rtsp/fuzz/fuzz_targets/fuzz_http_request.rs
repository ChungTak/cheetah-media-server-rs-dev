#![no_main]

use cheetah_rtsp_core::RtspRequestDecoder;
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_INPUT_BYTES)];
    let mut decoder = RtspRequestDecoder::new();
    let _ = decoder.feed(bounded);
    // 解析失败允许返回错误，只要不会 panic 即可。
    let _ = decoder.decode();
});
