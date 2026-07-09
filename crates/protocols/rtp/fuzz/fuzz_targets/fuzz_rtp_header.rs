#![no_main]

use cheetah_codec::RtpPacket;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 限制输入长度，防止极度膨胀
    if data.len() > 65536 {
        return;
    }
    // 喂给 Sans-I/O 协议解析器，确保不 panic
    let _ = RtpPacket::parse(data);
});
