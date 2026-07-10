use cheetah_sdk::StreamKey;

/// Parse a `namespace/stream` stream key specification from user input.
///
/// Trims leading/trailing slashes, splits on the first '/', then trims and
/// validates both parts. Returns `None` if either part is empty.
///
/// 从用户输入解析 `namespace/stream` 格式的流 Key。
///
/// 去掉首尾斜杠，按第一个 `/` 切分，再 trim 并校验两部分。任一部分为空时返回 `None`。
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

/// Validate that a pull source URL uses `http://` or `ws://` and has a host.
///
/// HTTPS and WSS are intentionally rejected by this validator; the pull client
/// does not yet support TLS sources.
///
/// 校验拉流源 URL 是否使用 `http://` 或 `ws://` 并包含主机。
///
/// 此校验器故意拒绝 HTTPS 和 WSS；拉流客户端暂不支持 TLS 源。
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
