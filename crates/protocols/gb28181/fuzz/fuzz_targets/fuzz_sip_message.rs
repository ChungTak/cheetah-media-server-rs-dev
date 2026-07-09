#![no_main]

use cheetah_gb28181_core::SipMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = SipMessage::parse(text);
    }
});
