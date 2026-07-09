#![no_main]

use cheetah_codec::parse_tcp_rtp_frame;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    // 验证 TCP 分包解析的健壮性
    let _ = parse_tcp_rtp_frame(data);
});
