#![no_main]

use cheetah_srt_core::parse_srt_url;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let query = String::from_utf8_lossy(data);
    let input = format!("srt://example.com:9000?{query}");
    let _ = parse_srt_url(&input);
});

