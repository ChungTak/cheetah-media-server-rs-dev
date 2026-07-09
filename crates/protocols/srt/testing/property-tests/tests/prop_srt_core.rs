use cheetah_srt_core::{parse_srt_stream_id, parse_srt_url};
use proptest::prelude::*;

proptest! {
    #[test]
    fn stream_id_parser_never_panics(input in ".*") {
        let _ = parse_srt_stream_id(&input);
    }

    #[test]
    fn srt_url_parser_never_panics(input in ".*") {
        let candidate = format!("srt://example.com:9000?{input}");
        let _ = parse_srt_url(&candidate);
    }

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

    #[test]
    fn stream_id_unknown_fields_are_preserved(
        value in "[a-zA-Z0-9_-]{1,16}"
    ) {
        let input = format!("#!::r=live/test,m=publish,x-vendor={value}");
        let parsed = parse_srt_stream_id(&input).expect("stream id should parse");

        prop_assert_eq!(parsed.extras.get("x-vendor").map(String::as_str), Some(value.as_str()));
    }

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
