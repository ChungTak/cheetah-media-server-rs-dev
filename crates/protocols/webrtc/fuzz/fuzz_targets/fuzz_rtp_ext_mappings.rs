#![no_main]

//! Fuzz target for the RTP extension mapping extractor.
//!
//! Asserts:
//! 1. The extractor never panics on arbitrary UTF-8 inputs.
//! 2. Every returned mapping has a non-zero id and a non-empty URI.
//! 3. The `ext_type` field is consistent with `from_uri(uri)`.

use cheetah_webrtc_core::{extract_rtp_extension_mappings, RtpExtensionType};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let mappings = extract_rtp_extension_mappings(input);
    for m in &mappings {
        assert!(m.id > 0, "extension id must be positive");
        assert!(!m.uri.is_empty(), "extension uri must be non-empty");
        assert_eq!(
            m.ext_type,
            RtpExtensionType::from_uri(&m.uri),
            "ext_type must match from_uri(uri)"
        );
    }
});
