#![no_main]

use cheetah_srt_core::parse_srt_stream_id;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = parse_srt_stream_id(&input);
});

