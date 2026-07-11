//! Property tests for the trickle-ICE candidate extraction helper.
//!
//! Phase 05 promises:
//!
//! * `extract_trickle_candidates` never panics on arbitrary input.
//! * Every line that survives extraction starts with `candidate:`, matching the
//!   form `str0m::Candidate::from_sdp_string` expects.
//! * Lines that do not begin with `a=candidate:` are silently dropped — no
//!   coercion, no fallback.
//! * Empty `a=candidate:` lines are dropped (they would otherwise produce an
//!   empty `candidate:` token that downstream parsers reject).
//!
//! trickle-ICE candidate 提取 helper 的属性测试。
//!
//! 阶段 05 承诺：
//! * `extract_trickle_candidates` 对任意输入不 panic。
//! * 保留下来的每行都以 `candidate:` 开头，符合 `str0m::Candidate::from_sdp_string`
//!   期望的形式。
//! * 不以 `a=candidate:` 开头的行被静默丢弃，不做强制转换或回退。
//! * 空的 `a=candidate:` 行被丢弃（否则会产生下游解析器拒绝的空 `candidate:` token）。

use cheetah_webrtc_module::extract_trickle_candidates;
use proptest::prelude::*;

proptest! {
    /// Extraction never panics on arbitrary input.
    ///
    /// 提取对任意输入不 panic。
    #[test]
    fn extract_does_not_panic(input in any::<String>()) {
        let _ = extract_trickle_candidates(&input);
    }

    /// Only candidate lines are yielded; empty candidate lines are dropped.
    ///
    /// 只输出 candidate 行；空 candidate 行被丢弃。
    #[test]
    fn extract_only_yields_candidate_lines(input in any::<String>()) {
        for line in extract_trickle_candidates(&input) {
            prop_assert!(line.starts_with("candidate:"),
                "every extracted line must start with `candidate:`, got {line:?}");
            // Everything after `candidate:` must be non-empty.
            prop_assert!(line.len() > "candidate:".len());
        }
    }

    /// Lines without the `a=candidate:` prefix never appear in the output.
    ///
    /// 没有 `a=candidate:` 前缀的行不会出现在输出中。
    #[test]
    fn non_candidate_lines_are_dropped(
        prefix in "[a-zA-Z][a-zA-Z0-9_=:-]{0,20}",
        body in "[a-zA-Z0-9 \\-:]{0,100}",
    ) {
        // Skip the literal `a=candidate:` prefix so we don't accidentally
        // generate a real candidate.
        prop_assume!(!prefix.starts_with("a=candidate:"));
        let line = format!("{prefix}: {body}");
        prop_assert!(extract_trickle_candidates(&line).is_empty());
    }

    /// Well-formed candidate bodies produce exactly one extracted line per
    /// input line.
    ///
    /// 格式良好的 candidate body 每输入一行产生恰好一条提取行。
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
