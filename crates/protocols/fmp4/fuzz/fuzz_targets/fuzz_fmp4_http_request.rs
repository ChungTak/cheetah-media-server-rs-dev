#![no_main]
use libfuzzer_sys::fuzz_target;

use cheetah_fmp4_core::parse_fmp4_request_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_fmp4_request_target(s);
    }
});
