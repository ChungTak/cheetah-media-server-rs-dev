//! Best-effort redaction helpers for debug/logging output.
//!
//! 用于调试/日志输出的尽力脱敏辅助函数。

/// Query parameter keys that commonly carry credentials or session secrets.
const SECRET_QUERY_KEYS: &[&str] = &[
    "authorization",
    "token",
    "access_token",
    "refresh_token",
    "api_key",
    "apikey",
    "key",
    "secret",
    "signature",
    "sign",
    "auth",
    "ticket",
    "password",
    "passwd",
    "x-api-key",
    "x_zlm_secret",
    "x-zlm-secret",
    "cookie",
    "proxy-authorization",
    "passphrase",
];

fn is_secret_query_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_QUERY_KEYS.iter().any(|k| lower == *k)
}

/// Best-effort URL redaction for `Debug` implementations.
///
/// Strips `user:pass@` userinfo and redacts known secret query keys. Falls back
/// to the original string when no scheme is present, so opaque paths are not
/// corrupted.
pub fn redact_url_secrets_for_debug(url: &str) -> String {
    let mut s = url.to_string();
    if let Some(scheme_end) = s.find("://") {
        let after = &s[scheme_end + 3..];
        if let Some(at) = after.find('@') {
            s = format!("{}://{}", &s[..scheme_end], &after[at + 1..]);
        }
    }

    if let Some((path, query)) = s.split_once('?') {
        let redacted = query
            .split('&')
            .map(|part| {
                if let Some((key, _)) = part.split_once('=') {
                    if is_secret_query_key(key) {
                        return format!("{key}=<redacted>");
                    }
                }
                part.to_string()
            })
            .collect::<Vec<_>>()
            .join("&");
        format!("{path}?{redacted}")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_userinfo_and_secret_query_keys() {
        let url = "http://user:pass@host/path?token=secret&api_key=sk&other=ok";
        let out = redact_url_secrets_for_debug(url);
        assert!(!out.contains("user:pass"), "{out}");
        assert!(!out.contains("token=secret"), "{out}");
        assert!(!out.contains("api_key=sk"), "{out}");
        assert!(out.contains("other=ok"), "{out}");
    }

    #[test]
    fn leaves_url_without_query_unchanged() {
        let url = "http://host/path";
        assert_eq!(redact_url_secrets_for_debug(url), url);
    }
}
