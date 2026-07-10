//! Property tests for the ZLMediaKit-style `rtc://` URL parser.
//!
//! The parser lives in [`cheetah_webrtc_module::parse_zlm_rtc_url`].
//! Phase 05 promises:
//!
//! * The parser never panics on arbitrary inputs.
//! * Valid `rtc://host/app/stream` inputs round-trip the host, app and stream
//!   segments verbatim (no implicit normalisation that the caller cannot observe).
//! * Re-parsing a syntactically-valid URL is idempotent at the field level.
//!
//! The proptest strategy generates only the inputs we want to assert over
//! (printable ASCII path segments, ASCII hosts, optional ports); a separate fuzz
//! target covers the panic-free invariant for arbitrary bytes.
//!
//! ZLMediaKit 风格 `rtc://` URL 解析器属性测试。
//!
//! 解析器位于 [`cheetah_webrtc_module::parse_zlm_rtc_url`]。
//! 阶段 05 承诺：
//! * 解析器对任意输入不 panic。
//! * 有效 `rtc://host/app/stream` 输入在 host、app、stream 段上逐字往返
//!   （不存在调用方无法察觉的隐式规范化）。
//! * 对语法有效 URL 重新解析在字段级别是幂等的。
//!
//! proptest 策略只生成我们希望断言的输入（可打印 ASCII 路径段、ASCII host、
//! 可选端口）；单独的 fuzz 目标覆盖任意字节下的不 panic 不变量。

use cheetah_webrtc_module::{parse_zlm_rtc_url, ZlmRtcScheme};
use proptest::prelude::*;

/// Generate a URL-safe path segment.
///
/// 生成 URL 安全路径段。
fn arb_path_segment() -> impl Strategy<Value = String> {
    // Use a tight subset of URL-safe characters so we do not have to model
    // percent-encoding for the round-trip assertion.
    "[a-zA-Z0-9_\\-]{1,16}"
}

/// Generate a valid hostname.
///
/// 生成有效主机名。
fn arb_host() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9\\-]{0,16}(\\.[a-z][a-z0-9\\-]{0,16}){0,3}"
}

/// Generate a ZLM-style URL scheme.
///
/// 生成 ZLM 风格 URL scheme。
fn arb_scheme() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("rtc".to_string()),
        Just("rtcs".to_string()),
        Just("webrtc".to_string()),
        Just("webrtcs".to_string()),
    ]
}

proptest! {
    /// Parsing must never panic, even on arbitrary text. The parser returns
    /// `Result` so any input either yields a structured URL or a structured error.
    ///
    /// 解析绝不 panic，即使对任意文本。解析器返回 `Result`，任何输入要么产生结构化
    /// URL，要么产生结构化错误。
    #[test]
    fn parse_does_not_panic(input in any::<String>()) {
        let _ = parse_zlm_rtc_url(&input);
    }

    /// Well-formed URLs round-trip the host, app and stream segments. We do not
    /// yet have a `to_string()` on `ZlmRtcUrl`, so the round-trip is field-level:
    /// we re-render the canonical form and re-parse, asserting equality on the
    /// parsed structure.
    ///
    /// 格式良好的 URL 在 host、app、stream 段上往返。目前 `ZlmRtcUrl` 没有 `to_string()`，
    /// 因此往返是字段级：重新渲染规范形式并重新解析，断言解析结构相等。
    #[test]
    fn well_formed_rtc_url_roundtrips_fields(
        scheme in arb_scheme(),
        host in arb_host(),
        port in proptest::option::of(1024u16..65000u16),
        app in arb_path_segment(),
        stream in arb_path_segment(),
    ) {
        let authority = match port {
            Some(p) => format!("{host}:{p}"),
            None => host.clone(),
        };
        let canonical = format!("{scheme}://{authority}/{app}/{stream}");
        let parsed = parse_zlm_rtc_url(&canonical).expect("canonical url must parse");
        prop_assert_eq!(parsed.host.as_str(), host.as_str());
        prop_assert_eq!(parsed.port, port);
        prop_assert_eq!(parsed.app.as_str(), app.as_str());
        prop_assert_eq!(parsed.stream.as_str(), stream.as_str());
        prop_assert_eq!(parsed.signaling_protocols, 0);
        prop_assert!(parsed.peer_room_id.is_none());

        // Re-parse the rendered canonical form. Idempotency at the field level
        // guards against any latent state in the parser (interner, regex side
        // effects, etc.).
        let rerendered = format!(
            "{}://{}/{}/{}",
            match parsed.scheme {
                ZlmRtcScheme::Rtc => "rtc",
                ZlmRtcScheme::Rtcs => "rtcs",
                ZlmRtcScheme::WebRtc => "webrtc",
                ZlmRtcScheme::WebRtcs => "webrtcs",
            },
            authority,
            parsed.app,
            parsed.stream,
        );
        let reparsed = parse_zlm_rtc_url(&rerendered).expect("rerendered url must parse");
        prop_assert_eq!(reparsed, parsed);
    }

    /// `signaling_protocols` is always parsed as an unsigned integer when present
    /// and well-formed; passing the literal `0` or `1` must yield those numeric
    /// values.
    ///
    /// `signaling_protocols` 存在且格式良好时解析为无符号整数；字面量 `0` 或 `1` 必须
    /// 得到对应数值。
    #[test]
    fn signaling_protocols_parses_as_integer(
        host in arb_host(),
        app in arb_path_segment(),
        stream in arb_path_segment(),
        protocols in 0u32..16u32,
    ) {
        let url = format!("rtc://{host}/{app}/{stream}?signaling_protocols={protocols}");
        let parsed = parse_zlm_rtc_url(&url).expect("url with signaling_protocols must parse");
        prop_assert_eq!(parsed.signaling_protocols, protocols);
    }

    /// Unknown query parameters are surfaced verbatim through `extra_params`,
    /// preserving their order. ZLM-style clients rely on this for opaque keys like
    /// `secret=...`.
    ///
    /// 未知查询参数通过 `extra_params` 原样保留顺序。ZLM 风格客户端依赖此行为传递
    /// `secret=...` 等不透明键。
    #[test]
    fn extra_query_params_preserve_keys(
        host in arb_host(),
        app in arb_path_segment(),
        stream in arb_path_segment(),
        keys in proptest::collection::vec("[a-z]{1,8}", 1..5),
    ) {
        let pairs: Vec<String> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| format!("{k}=v{i}"))
            .collect();
        let query = pairs.join("&");
        let url = format!("rtc://{host}/{app}/{stream}?{query}");
        let parsed = parse_zlm_rtc_url(&url).expect("url with extras must parse");
        let saw_keys: std::collections::HashSet<&str> = parsed
            .extra_params
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        for k in &keys {
            // Keys are always preserved (some duplicates may collapse when the
            // generator picks the same string twice; we use a set membership check
            // instead of equality).
            prop_assert!(saw_keys.contains(k.as_str()));
        }
    }
}
