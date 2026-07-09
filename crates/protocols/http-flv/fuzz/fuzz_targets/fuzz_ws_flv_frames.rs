#![no_main]

use cheetah_http_flv_module::pull::fuzz_decode_ws_frames;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fuzz_decode_ws_frames(data, 1024 * 1024);
});
