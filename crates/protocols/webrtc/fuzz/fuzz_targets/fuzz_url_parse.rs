#![no_main]

//! Fuzz target for the WHIP/WHEP HTTP client URL parser.
//!
//! The contract: `fuzz_parse_url_for_testing` must never panic
//! regardless of input bytes. Successful parses additionally satisfy:
//!
//! * `host` is non-empty (otherwise we would have returned an
//!   `InvalidUrl` error).
//! * `effective_port` is one of the configured defaults (80 for
//!   `http://`, 443 for `https://`) or whatever the explicit port
//!   parsed to.
//! * `path_and_query` either starts with `/` or is exactly empty;
//!   the parser normalizes a missing path to `/` so we expect the
//!   former in practice.
//!
//! The fuzzer drives the parser with arbitrary UTF-8 strings; non-UTF-8
//! bytes are dropped early because URLs in this project are required
//! to be UTF-8.

use cheetah_webrtc_module::http_client::fuzz_parse_url_for_testing;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(parsed) = fuzz_parse_url_for_testing(input) {
        assert!(
            !parsed.host.is_empty(),
            "successful URL parse must have a non-empty host: {input:?}"
        );
        // Either explicit port OR the scheme-default port.
        if let Some(p) = parsed.port {
            assert_eq!(p, parsed.effective_port);
        } else {
            assert!(
                parsed.effective_port == 80 || parsed.effective_port == 443,
                "unexpected default port {} for {input:?}",
                parsed.effective_port
            );
        }
        // Path normalization invariant: every successful parse
        // produces a request target the HTTP/1.1 layer can drop into
        // the request line directly.
        assert!(
            parsed.path_and_query.is_empty() || parsed.path_and_query.starts_with('/'),
            "request target must start with / or be empty: {:?}",
            parsed.path_and_query
        );
    }
});
