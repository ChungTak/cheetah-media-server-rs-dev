#![no_main]

use cheetah_gb28181_core::GbSdp;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }

    // Fuzz GbSdp parsing
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = GbSdp::parse(text);
    }

    // Fuzz generic JSON parsing which is widely used in GB28181 REST APIs
    let _ = serde_json::from_slice::<serde_json::Value>(data);
});
