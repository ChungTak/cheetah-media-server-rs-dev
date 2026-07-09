#![no_main]
use libfuzzer_sys::fuzz_target;
use cheetah_rtsp_core::parse_rtcp_xr;

fuzz_target!(|data: &[u8]| {
    let _ = parse_rtcp_xr(data);
});
