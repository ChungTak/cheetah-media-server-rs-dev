#![no_main]

use cheetah_rtsp_core::Sdp;
use libfuzzer_sys::fuzz_target;

const MAX_TEXT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    let bounded = &data[..data.len().min(MAX_TEXT_BYTES)];
    if let Ok(text) = std::str::from_utf8(bounded) {
        if let Ok(sdp) = Sdp::parse(text) {
            let _ = sdp.to_string();
        }
    }
});
