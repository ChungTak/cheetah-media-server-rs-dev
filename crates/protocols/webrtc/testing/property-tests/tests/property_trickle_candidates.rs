//! Property tests for the trickle-ICE candidate extraction helper.
//!
//! Phase 05 promises:
//!
//! * `extract_trickle_candidates` never panics on arbitrary input.
//! * Every line that survives extraction starts with `candidate:`,
//!   matching the form `str0m::Candidate::from_sdp_string` expects.
//! * Lines that do not begin with `a=candidate:` are silently
//!   dropped — no coercion, no fallback.
//! * Empty `a=candidate:` lines are dropped (they would otherwise
//!   produce an empty `candidate:` token that downstream parsers
//!   reject).

use cheetah_webrtc_module::extract_trickle_candidates;
use proptest::prelude::*;

proptest! {
    #[test]
    fn extract_does_not_panic(input in any::<String>()) {
        let _ = extract_trickle_candidates(&input);
    }

    #[test]
    fn extract_only_yields_candidate_lines(input in any::<String>()) {
        for line in extract_trickle_candidates(&input) {
            prop_assert!(line.starts_with("candidate:"),
                "every extracted line must start with `candidate:`, got {line:?}");
            // Everything after `candidate:` must be non-empty —
            // empty `a=candidate:` source lines are silently dropped.
            prop_assert!(line.len() > "candidate:".len());
        }
    }

    /// Lines without the `a=candidate:` prefix never appear in the
    /// output. We synthesise inputs of "junk" lines and assert the
    /// extractor returns an empty vector.
    #[test]
    fn non_candidate_lines_are_dropped(
        prefix in "[a-zA-Z][a-zA-Z0-9_=:-]{0,20}",
        body in "[a-zA-Z0-9 \\-:]{0,100}",
    ) {
        // Skip the literal `a=candidate:` prefix so we don't
        // accidentally generate a real candidate.
        prop_assume!(!prefix.starts_with("a=candidate:"));
        let line = format!("{prefix}: {body}");
        prop_assert!(extract_trickle_candidates(&line).is_empty());
    }

    /// Round-trip: well-formed candidate body produces exactly one
    /// extracted line for each input line.
    #[test]
    fn well_formed_candidates_count_round_trips(
        n in 1usize..6,
        suffix in "[a-zA-Z0-9][a-zA-Z0-9 ]{0,40}",
    ) {
        let mut body = String::new();
        for _ in 0..n {
            body.push_str("a=candidate:");
            body.push_str(&suffix);
            body.push_str("\r\n");
        }
        let v = extract_trickle_candidates(&body);
        prop_assert_eq!(v.len(), n);
    }
}
