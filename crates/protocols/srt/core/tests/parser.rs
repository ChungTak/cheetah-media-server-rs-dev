use cheetah_srt_core::{parse_srt_stream_id, parse_srt_url, SrtKeyLength, SrtRole, SrtStreamMode};

#[test]
fn access_control_publish_stream_id_parses() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=publish,u=alice").unwrap();
    assert_eq!(parsed.stream_key, "live/test");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Publish));
    assert_eq!(parsed.user.as_deref(), Some("alice"));
}

#[test]
fn access_control_request_stream_id_parses() {
    let parsed = parse_srt_stream_id("#!::r=live/test,m=request").unwrap();
    assert_eq!(parsed.stream_key, "live/test");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Request));
}

#[test]
fn bare_stream_key_parses() {
    let parsed = parse_srt_stream_id("/live/test").unwrap();
    assert_eq!(parsed.stream_key, "live/test");
    assert_eq!(parsed.mode, None);
}

#[test]
fn percent_encoded_resource_parses() {
    let parsed = parse_srt_stream_id("#!::r=live%2Fencoded,m=play").unwrap();
    assert_eq!(parsed.stream_key, "live/encoded");
    assert_eq!(parsed.mode, Some(SrtStreamMode::Play));
}

#[test]
fn stream_id_plus_is_literal_not_space() {
    let parsed = parse_srt_stream_id("#!::r=live/camera+1,m=publish,token=a+b").unwrap();
    assert_eq!(parsed.stream_key, "live/camera+1");
    assert_eq!(parsed.extras.get("token").map(String::as_str), Some("a+b"));
}

#[test]
fn invalid_stream_ids_are_rejected() {
    assert!(parse_srt_stream_id("#!::m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=../secret,m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=live//test,m=publish").is_err());
    assert!(parse_srt_stream_id("#!::r=live/test,m=unknown").is_err());
    assert!(parse_srt_stream_id("#!::r=live/%").is_err());
    assert!(parse_srt_stream_id("#!::r=live/%A").is_err());
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
