#![no_main]

use cheetah_http_flv_core::{
    parse_play_request_target, validate_websocket_upgrade, HttpMethod, HttpRequestHead,
};
use cheetah_http_flv_module::pull::fuzz_http_response_head;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let target = text.as_ref();
    let _ = parse_play_request_target(target);

    let head = HttpRequestHead {
        method: HttpMethod::Get,
        method_raw: "GET".to_string(),
        target: "/live/stream.flv".to_string(),
        headers: vec![
            ("Connection".to_string(), "Upgrade".to_string()),
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Sec-WebSocket-Version".to_string(), "13".to_string()),
            ("Sec-WebSocket-Key".to_string(), target.to_string()),
        ],
    };
    let _ = validate_websocket_upgrade(&head);
    let _ = fuzz_http_response_head(data, 32 * 1024);
});
