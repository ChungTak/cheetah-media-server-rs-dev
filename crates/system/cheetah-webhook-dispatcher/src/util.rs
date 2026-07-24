use std::collections::HashMap;

use url::Url;

/// Sign a webhook body with HMAC-SHA256 and return the base64 signature.
///
/// 使用 HMAC-SHA256 对 webhook body 签名并返回 base64 签名值。
pub fn sign_body(body: &[u8], secret: &str) -> Result<String, hmac::digest::InvalidLength> {
    use base64::Engine;
    use hmac::Mac;
    use sha2::Sha256;

    type HmacSha256 = hmac::Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(body);
    let result = mac.finalize().into_bytes();
    Ok(base64::engine::general_purpose::STANDARD.encode(result))
}

/// True if an HTTP status code indicates success.
pub fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// True if an HTTP status code is a client error (no retry).
pub fn is_client_error(status: u16) -> bool {
    (400..500).contains(&status)
}

/// Build the common headers for a webhook POST.
pub fn webhook_headers(event_id: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("X-Event-Id".to_string(), event_id.to_string());
    headers
}

/// True if an HTTP header commonly carries credentials or session secrets.
pub fn is_secret_header(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "authorization" | "x-zlm-secret" | "cookie" | "proxy-authorization"
    )
}

/// True if a URL query parameter key commonly carries secrets.
pub fn is_secret_query_key(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "authorization"
            | "token"
            | "api_key"
            | "apikey"
            | "secret"
            | "password"
            | "passwd"
            | "x-zlm-secret"
    )
}

/// Redact secret values from a raw query string (without the leading `?`).
pub fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|part| {
            if let Some((key, _value)) = part.split_once('=') {
                if is_secret_query_key(key) {
                    return format!("{key}=<redacted>");
                }
            }
            part.to_string()
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Redact a `path?query` string so secrets in the query are not logged.
pub fn redact_path_and_query(path_and_query: &str) -> String {
    if let Some((path, query)) = path_and_query.split_once('?') {
        format!("{}?{}", path, redact_query(query))
    } else {
        path_and_query.to_string()
    }
}

/// Redact a full URL so userinfo, fragments and secret query values are not logged.
pub fn redact_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        let query = parsed
            .query()
            .map(|q| format!("?{}", redact_query(q)))
            .unwrap_or_default();
        return format!(
            "{}://{}{}{}{}",
            parsed.scheme(),
            host,
            port,
            parsed.path(),
            query
        );
    }

    // Fallback for malformed strings: strip a `user:pass@` prefix if present.
    if let Some(pos) = url.find("://") {
        let after_scheme = &url[pos + 3..];
        if let Some(at) = after_scheme.find('@') {
            let path_and_query = &after_scheme[at + 1..];
            let redacted = if let Some((path, query)) = path_and_query.split_once('?') {
                format!("{}?{}", path, redact_query(query))
            } else {
                path_and_query.to_string()
            };
            return format!("{}://{}", &url[..pos], redacted);
        }
    }

    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_strips_userinfo_and_fragment_and_secret_query() {
        let url = "http://user:pass@example.com:8080/hook?token=secret&x=1#frag";
        let out = redact_url(url);
        assert!(!out.contains("user:pass"), "userinfo leaked: {out}");
        assert!(!out.contains("token=secret"), "secret query leaked: {out}");
        assert!(!out.contains("#frag"), "fragment not stripped: {out}");
        assert!(
            out.contains("http://example.com:8080/hook"),
            "host/path missing: {out}"
        );
        assert!(out.contains("x=1"), "non-secret query dropped: {out}");
    }

    #[test]
    fn redact_url_ipv6() {
        let out = redact_url("https://[::1]:8080/hook?api_key=abc");
        assert!(
            out.contains("https://[::1]:8080/hook"),
            "IPv6 host missing: {out}"
        );
        assert!(!out.contains("api_key=abc"), "secret query leaked: {out}");
    }

    #[test]
    fn redact_url_fallback_strips_userinfo() {
        let out = redact_url("http://user:pass@example.com/hook");
        assert!(
            !out.contains("user:pass"),
            "fallback userinfo leaked: {out}"
        );
        assert!(
            out.starts_with("http://example.com/hook"),
            "fallback host missing: {out}"
        );
    }
}
