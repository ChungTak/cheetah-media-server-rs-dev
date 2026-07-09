#![no_main]
use libfuzzer_sys::fuzz_target;

use cheetah_fmp4_core::{
    parse_fmp4_request_target, validate_websocket_upgrade, HttpMethod, HttpRequestHead,
};

fuzz_target!(|data: &[u8]| {
    // Fuzz WebSocket upgrade validation with arbitrary header values
    if data.len() < 4 {
        return;
    }
    let split = data[0] as usize % data.len().max(1);
    let (key_part, version_part) = data[1..].split_at(split.min(data.len() - 1));

    let key = String::from_utf8_lossy(key_part).to_string();
    let version = String::from_utf8_lossy(version_part).to_string();

    let head = HttpRequestHead {
        method: HttpMethod::Get,
        method_raw: "GET".to_string(),
        target: "/live/test.mp4".to_string(),
        headers: vec![
            ("Connection".to_string(), "Upgrade".to_string()),
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Sec-WebSocket-Version".to_string(), version),
            ("Sec-WebSocket-Key".to_string(), key),
        ],
    };
    let _ = validate_websocket_upgrade(&head);

    // Also fuzz request target parsing with arbitrary bytes
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_fmp4_request_target(s);
    }
});
