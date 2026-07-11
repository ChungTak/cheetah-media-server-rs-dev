use cheetah_srt_core::{
    parse_srt_stream_id, parse_srt_stream_id_with_options, parse_srt_url, SrtKeyLength, SrtRole,
    SrtStreamMode, StreamIdParseOptions,
};

#[test]
fn zlm_publish_with_vhost() {
    let parsed = parse_srt_stream_id("#!::h=zlmediakit.com,r=live/test,m=publish").unwrap();
    assert_eq!(parsed.vhost, "zlmediakit.com");
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Publish));
    assert_eq!(
        parsed.auth_params.get("m").map(String::as_str),
        Some("publish")
    );
    assert_eq!(parsed.stream_key, "live/test");
}

#[test]
fn zlm_play_default_no_m() {
    let parsed = parse_srt_stream_id("#!::r=live/test").unwrap();
    assert_eq!(parsed.vhost, "__defaultVhost__");
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, None);
    assert_eq!(parsed.stream_key, "live/test");
}

#[test]
fn zlm_request() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=request").unwrap();
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Request));
    assert_eq!(
        parsed.auth_params.get("m").map(String::as_str),
        Some("request")
    );
}

#[test]
fn zlm_play() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=play,token=abc").unwrap();
    assert_eq!(parsed.mode, Some(SrtStreamMode::Play));
    assert_eq!(
        parsed.auth_params.get("m").map(String::as_str),
        Some("play")
    );
    assert_eq!(
        parsed.auth_params.get("token").map(String::as_str),
        Some("abc")
    );
    assert!(!parsed.auth_params.contains_key("h"));
    assert!(!parsed.auth_params.contains_key("r"));
}

#[test]
fn zlm_token_and_custom() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=publish,token=t,foo=bar").unwrap();
    assert_eq!(
        parsed.auth_params.get("m").map(String::as_str),
        Some("publish")
    );
    assert_eq!(
        parsed.auth_params.get("token").map(String::as_str),
        Some("t")
    );
    assert_eq!(
        parsed.auth_params.get("foo").map(String::as_str),
        Some("bar")
    );
    assert!(!parsed.auth_params.contains_key("h"));
    assert!(!parsed.auth_params.contains_key("r"));
}

#[test]
fn missing_r_fails() {
    assert!(parse_srt_stream_id("#!::m=publish").is_err());
}

#[test]
fn single_segment_r_fails_strict() {
    assert!(parse_srt_stream_id("#!::r=live").is_err());
}

#[test]
fn bare_rejected_when_strict() {
    assert!(parse_srt_stream_id("live/test").is_err());
}

#[test]
fn bare_ok_when_allowed() {
    let opts = StreamIdParseOptions {
        allow_bare_key: true,
        ..Default::default()
    };
    let parsed = parse_srt_stream_id_with_options("live/test", &opts).unwrap();
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, None);

    let single = parse_srt_stream_id_with_options("mycam", &opts).unwrap();
    assert_eq!(single.app, "live");
    assert_eq!(single.stream, "mycam");
}

#[test]
fn strict_prefix_false_allows_bare() {
    let opts = StreamIdParseOptions {
        strict_prefix: false,
        allow_bare_key: false,
        ..Default::default()
    };
    let parsed = parse_srt_stream_id_with_options("live/test", &opts).unwrap();
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, None);
}

#[test]
fn percent_encoded_r_parses() {
    let parsed = parse_srt_stream_id("#!::r=live%2Ftest,m=play").unwrap();
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "test");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Play));
}

#[test]
fn plus_is_literal_not_space() {
    let parsed = parse_srt_stream_id("#!::r=live/camera+1,m=publish,token=a+b").unwrap();
    assert_eq!(parsed.app, "live");
    assert_eq!(parsed.stream, "camera+1");
    assert_eq!(parsed.stream_key, "live/camera+1");
    assert_eq!(
        parsed.auth_params.get("token").map(String::as_str),
        Some("a+b")
    );
}

#[test]
fn invalid_stream_ids_are_rejected() {
    assert!(parse_srt_stream_id("#!::m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=../secret,m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=live//test,m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=live%2Ftest").is_ok());
    assert!(parse_srt_stream_id("#!::r=live/%").is_err());
    assert!(parse_srt_stream_id("#!::r=live/%A").is_err());
    assert!(parse_srt_stream_id("").is_err());
}

#[test]
fn unknown_mode_treated_as_play() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=unknown").unwrap();
    assert_eq!(parsed.mode, Some(SrtStreamMode::Request));
    assert_eq!(
        parsed.auth_params.get("m").map(String::as_str),
        Some("unknown")
    );
}

#[test]
fn caller_url_with_stream_id_parses() {
    let parsed =
        parse_srt_url("srt://127.0.0.1:9000?mode=caller&streamid=#!::r=live/test,m=publish")
            .unwrap();
    assert_eq!(parsed.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(parsed.port, 9000);
    assert_eq!(parsed.mode, Some(SrtRole::Caller));
    assert_eq!(
        parsed.stream_id.as_deref(),
        Some("#!::r=live/test,m=publish")
    );
}

#[test]
fn listener_url_with_empty_host_parses() {
    let parsed = parse_srt_url("srt://:9000?mode=listener").unwrap();
    assert_eq!(parsed.host.as_deref(), None);
    assert_eq!(parsed.port, 9000);
    assert_eq!(parsed.mode, Some(SrtRole::Listener));
}

#[test]
fn url_crypto_options_parse() {
    let parsed = parse_srt_url(
        "srt://example.com:9000?mode=caller&latency=120&passphrase=secret&pbkeylen=32",
    )
    .unwrap();
    assert_eq!(parsed.latency_ms, Some(120));
    assert_eq!(parsed.passphrase.as_deref(), Some("secret"));
    assert_eq!(parsed.key_length, Some(SrtKeyLength::Aes256));
}

#[test]
fn url_query_plus_is_literal_not_space() {
    let parsed = parse_srt_url(
        "srt://example.com:9000?mode=caller&streamid=#!::r=live/camera+1,m=publish&passphrase=a+b",
    )
    .unwrap();

    assert_eq!(
        parsed.stream_id.as_deref(),
        Some("#!::r=live/camera+1,m=publish")
    );
    assert_eq!(parsed.passphrase.as_deref(), Some("a+b"));
}

#[test]
fn malformed_url_percent_escapes_are_rejected() {
    assert!(parse_srt_url("srt://example.com:9000?streamid=live/%").is_err());
    assert!(parse_srt_url("srt://example.com:9000?streamid=live/%A").is_err());
}
