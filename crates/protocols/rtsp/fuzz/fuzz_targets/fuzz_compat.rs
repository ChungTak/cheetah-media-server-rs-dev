#![no_main]
use libfuzzer_sys::fuzz_target;
use cheetah_rtsp_core::{
    default_clock_rate, normalize_range_now, resolve_control_url, strip_sdp_suffix,
};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = strip_sdp_suffix(s);
    let _ = normalize_range_now(s);
    let _ = default_clock_rate(s);
    if let Some((base, control)) = s.split_once('\n') {
        let _ = resolve_control_url(base, control);
    }
});
