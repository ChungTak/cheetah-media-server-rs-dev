#![no_main]

use libfuzzer_sys::fuzz_target;
use cheetah_rtmp_core::RtmpChunkDecoder;

fuzz_target!(|data: &[u8]| {
    let mut decoder = RtmpChunkDecoder::default();
    let mut remaining = data;

    // 持续解码多个 chunk
    while !remaining.is_empty() {
        match decoder.decode(remaining) {
            Ok((size, _maybe_chunk)) => {
                if size == 0 {
                    break;
                }
                remaining = &remaining[size..];
            }
            Err(_) => break,
        }
    }
});
