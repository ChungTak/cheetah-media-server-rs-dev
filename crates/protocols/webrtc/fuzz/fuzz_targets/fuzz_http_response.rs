#![no_main]

//! Fuzz target for the WHIP/WHEP HTTP/1.1 response parser.
//!
//! The contract: `fuzz_parse_http_response_for_testing` must never
//! panic for arbitrary input bytes. The parser is bounded by
//! `max_body` so we capped that at a small constant per the WHIP/WHEP
//! HTTP client default (64 KiB), which keeps the corpus realistic
//! without exhausting the fuzzer's memory budget.
//!
//! Successful parses must satisfy:
//! * The status code is in the standard HTTP range (we permit any
//!   `u16` since the parser does not normalize, but a panic would be
//!   a real bug).
//! * The body is no larger than the max we passed in.
//!
//! `Ok(None)` (i.e., incomplete framing) and any structured
//! `Err(HttpClientError)` are equally valid outcomes; the fuzzer is
//! only interested in the absence of panics, infinite loops, or
//! buffer overruns.

use cheetah_webrtc_module::http_client::fuzz_parse_http_response_for_testing;
use libfuzzer_sys::fuzz_target;

const FUZZ_MAX_BODY: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(resp)) = fuzz_parse_http_response_for_testing(data, FUZZ_MAX_BODY) {
        assert!(
            resp.body.len() <= FUZZ_MAX_BODY,
            "body must respect the configured max"
        );
        // Touch the headers vector to force the fuzzer to materialise
        // the response into memory; otherwise dead-code elimination
        // could mask real bugs.
        let _ = resp.headers.len();
        let _ = resp.status;
    }
});
