use cheetah_sdk::StreamKey;

/// Parses `stream key spec` from input.
/// 从输入解析 `stream key spec`。
pub fn parse_stream_key_spec(spec: &str) -> Option<StreamKey> {
    let trimmed = spec.trim().trim_matches('/');
    let (namespace, path) = trimmed.split_once('/')?;
    let namespace = namespace.trim().trim_matches('/');
    let path = path.trim().trim_matches('/');
    if namespace.is_empty() || path.is_empty() {
        return None;
    }
    Some(StreamKey::new(namespace, path))
}

/// Validates the `pull source URL` and returns errors if invalid.
/// 验证 `pull source URL`，无效时返回错误。
pub fn validate_pull_source_url(source_url: &str) -> bool {
    let trimmed = source_url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("ws");
    if !scheme_ok {
        return false;
    }

    let host = rest.split('/').next().unwrap_or_default().trim();
    if host.is_empty() {
        return false;
    }
    if host.contains(char::is_whitespace) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stream_key_requires_namespace_and_path() {
        assert!(parse_stream_key_spec("live/test").is_some());
        assert!(parse_stream_key_spec("live/").is_none());
        assert!(parse_stream_key_spec("onlyone").is_none());
    }

    #[test]
    fn pull_source_url_accepts_http_and_ws() {
        assert!(validate_pull_source_url("http://localhost/live/1.flv"));
        assert!(validate_pull_source_url("ws://localhost/live/1.flv"));
        assert!(!validate_pull_source_url("https://127.0.0.1/live/2.flv"));
        assert!(!validate_pull_source_url("wss://localhost/live/1.flv"));
        assert!(!validate_pull_source_url("rtmp://localhost/live/test"));
        assert!(!validate_pull_source_url("http:///no-host"));
    }
}
