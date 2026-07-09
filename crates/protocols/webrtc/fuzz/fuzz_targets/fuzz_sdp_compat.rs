#![no_main]

//! Fuzz target for the SDP compatibility preprocessor.
//!
//! Asserts the same invariants the property-test suite checks but
//! drives them with `libfuzzer-sys`'s coverage-guided generator:
//! 1. The preprocessor never panics on arbitrary inputs.
//! 2. The preprocessor is idempotent: running it twice produces the
//!    same output and an empty diagnostic on the second pass.
//! 3. Output of any non-empty input always ends with `\r\n`.

use cheetah_webrtc_core::{preprocess_remote_sdp, SdpCompatReport};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let (first, _) = preprocess_remote_sdp(input);
    let (second, second_report) = preprocess_remote_sdp(&first);
    assert_eq!(first, second, "preprocess_remote_sdp must be idempotent");
    assert_eq!(
        second_report,
        SdpCompatReport::default(),
        "second pass must report no further changes"
    );
    if !first.is_empty() {
        assert!(
            first.ends_with("\r\n"),
            "non-empty preprocessor output must end with CRLF"
        );
    }
});
