//! Property-based tests for SRT URL and stream-id parsing.
//!
//! These tests verify that `parse_srt_stream_id` and `parse_srt_url` never panic,
//! that well-known query parameters round-trip regardless of order, and that
//! unknown vendor fields are preserved.
//!
//! SRT URL 与 stream-id 解析属性测试。
//!
//! 这些测试验证 `parse_srt_stream_id` 与 `parse_srt_url` 永不 panic、
//! 已知查询参数不受顺序影响而往返一致，并保留未知厂商字段。

use cheetah_srt_core::{parse_srt_stream_id, parse_srt_url};
use proptest::prelude::*;

proptest! {
    /// The stream-id parser never panics on arbitrary input.
    ///
    /// stream-id 解析器对任意输入不 panic。
    #[test]
    fn stream_id_parser_never_panics(input in ".*") {
        let _ = parse_srt_stream_id(&input);
    }

    /// The URL parser never panics on arbitrary query strings.
    ///
    /// URL 解析器对任意查询字符串不 panic。
    #[test]
    fn srt_url_parser_never_panics(input in ".*") {
        let candidate = format!("srt://example.com:9000?{input}");
        let _ = parse_srt_url(&candidate);
    }

    /// Reordering the query does not change the parsed known fields.
    ///
    /// 查询参数顺序改变不影响已识别字段的解析结果。
    #[test]
    fn url_query_order_does_not_change_known_fields(
        latency in 0_u64..10_000,
        stream_name in "[a-z][a-z0-9_]{0,12}"
    ) {
        let first = format!(
            "srt://example.com:9000?mode=caller&latency={latency}&streamid=#!::r=live/{stream_name},m=publish&token=secret"
        );
        let second = format!(
            "srt://example.com:9000?token=secret&streamid=#!::r=live/{stream_name},m=publish&latency={latency}&mode=caller"
        );

        let first = parse_srt_url(&first).expect("first URL should parse");
        let second = parse_srt_url(&second).expect("second URL should parse");

        prop_assert_eq!(first.host, second.host);
        prop_assert_eq!(first.port, second.port);
        prop_assert_eq!(first.mode, second.mode);
        prop_assert_eq!(first.stream_id, second.stream_id);
        prop_assert_eq!(first.latency_ms, second.latency_ms);
        prop_assert_eq!(first.extras.get("token"), second.extras.get("token"));
    }

    /// Unknown stream-id fields are preserved in the `extras` map.
    ///
    /// 未知的 stream-id 字段保留在 `extras` 映射中。
    #[test]
    fn stream_id_unknown_fields_are_preserved(
        value in "[a-zA-Z0-9_-]{1,16}"
    ) {
        let input = format!("#!::r=live/test,m=publish,x-vendor={value}");
        let parsed = parse_srt_stream_id(&input).expect("stream id should parse");

        prop_assert_eq!(parsed.extras.get("x-vendor").map(String::as_str), Some(value.as_str()));
    }

    /// Re-serializing a valid stream-id and re-parsing it is stable.
    ///
    /// 对有效 stream-id 重新序列化并再解析结果稳定。
    #[test]
    fn valid_stream_id_parse_normalize_parse_is_stable(
        stream_name in "[a-z][a-z0-9_]{0,12}",
        user in "[a-z][a-z0-9_]{0,12}",
        token in "[a-zA-Z0-9_-]{1,16}"
    ) {
        let input = format!("#!::r=live/{stream_name},m=publish,u={user},token={token}");
        let parsed = parse_srt_stream_id(&input).expect("stream id should parse");
        let normalized = format!(
            "#!::r={},m=publish,u={},token={}",
            parsed.stream_key,
            parsed.user.as_deref().unwrap_or_default(),
            parsed.extras.get("token").map(String::as_str).unwrap_or_default()
        );
        let reparsed = parse_srt_stream_id(&normalized).expect("normalized stream id should parse");

        prop_assert_eq!(parsed.stream_key, reparsed.stream_key);
        prop_assert_eq!(parsed.mode, reparsed.mode);
        prop_assert_eq!(parsed.user, reparsed.user);
        prop_assert_eq!(parsed.extras.get("token"), reparsed.extras.get("token"));
    }

    /// Unknown URL query fields are preserved in the `extras` map.
    ///
    /// 未知的 URL 查询字段保留在 `extras` 映射中。
    #[test]
    fn url_unknown_fields_are_preserved(
        field in "x-[a-z]{1,8}",
        value in "[a-zA-Z0-9_-]{1,16}"
    ) {
        let input = format!(
            "srt://example.com:9000?mode=caller&streamid=#!::r=live/test,m=publish&{field}={value}"
        );
        let parsed = parse_srt_url(&input).expect("URL should parse");

        prop_assert_eq!(parsed.extras.get(&field).map(String::as_str), Some(value.as_str()));
    }
}
