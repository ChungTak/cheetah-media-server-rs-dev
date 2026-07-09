#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::RtmpMessageDecoder;

fuzz_target!(|data: &[u8]| {
    let mut decoder = RtmpMessageDecoder::default();

    // 尝试解码消息
    decoder.feed_buf(data);
    loop {
        match decoder.decode() {
            Ok(Some(_message)) => {}
            Ok(None) => break,
            Err(_) => break,
        }
    }
});
