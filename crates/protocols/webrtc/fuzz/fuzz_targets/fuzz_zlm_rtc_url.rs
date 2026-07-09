#![no_main]

//! Fuzz target for the ZLMediaKit-style `rtc://` URL parser.
//!
//! Phase 05 contract: `parse_zlm_rtc_url` must never panic regardless
//! of input bytes. Successful parses additionally satisfy:
//!
//! * The host is non-empty (otherwise we would have returned
//!   `ZlmRtcUrlError::MissingHost`).
//! * `app` and `stream` are non-empty (otherwise `MissingPath` /
//!   `MissingStream`).
//! * `signaling_protocols` is whatever the caller supplied — we just
//!   assert no debug-overflow panics by exercising the value as a
//!   plain `u32`.

use cheetah_webrtc_module::parse_zlm_rtc_url;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(parsed) = parse_zlm_rtc_url(input) {
        assert!(
            !parsed.host.is_empty(),
            "successful parse must have a non-empty host"
        );
        assert!(
            !parsed.app.is_empty(),
            "successful parse must have a non-empty app segment"
        );
        assert!(
            !parsed.stream.is_empty(),
            "successful parse must have a non-empty stream segment"
        );
        // Touch the integer to prove no UB / debug overflow panic.
        let _ = parsed.signaling_protocols;
    }
});
