#![no_main]
use libfuzzer_sys::fuzz_target;
use cheetah_rtsp_core::{parse_rtcp_fb, RTCP_PT_RTPFB, RTCP_PT_PSFB};

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let pt = data[0];
    let fmt = data[1];
    let payload = &data[2..];
    // Only fuzz valid PT values
    if pt == RTCP_PT_RTPFB || pt == RTCP_PT_PSFB {
        let _ = parse_rtcp_fb(pt, fmt, payload);
    }
});
