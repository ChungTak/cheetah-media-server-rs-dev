use base64::Engine;
use cheetah_rtsp_core::{RtspMethod, RtspResponseMessage};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

type DigestNonceKey = (String, String, String);
const MAX_TRACKED_DIGEST_NONCES: usize = 4096;

#[derive(Default)]
struct DigestNonceCountState {
    counts: HashMap<DigestNonceKey, u32>,
    lru: VecDeque<DigestNonceKey>,
}

static DIGEST_CNONCE_COUNTER: AtomicU64 = AtomicU64::new(1);
static DIGEST_NONCE_COUNT: OnceLock<Mutex<DigestNonceCountState>> = OnceLock::new();

/// `RtspClientCredentials` data structure.
/// `RtspClientCredentials` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtspClientCredentials {
    pub username: String,
    pub password: String,
}

/// `authorization_header_from_response` function.
/// `authorization_header_from_response` 函数。
pub fn authorization_header_from_response(
    response: &RtspResponseMessage,
    method: RtspMethod,
    uri: &str,
    credentials: &RtspClientCredentials,
) -> Option<String> {
    if response.status_code != 401 {
        return None;
    }
    response
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("WWW-Authenticate"))
        .find_map(|header| {
            build_authorization_header(&header.value, method.clone(), uri, credentials)
        })
}

fn build_authorization_header(
    challenge: &str,
    method: RtspMethod,
    uri: &str,
    credentials: &RtspClientCredentials,
) -> Option<String> {
    let trimmed = challenge.trim();
    if trim_scheme_prefix(trimmed, "Basic").is_some() {
        return Some(basic_authorization_header(credentials));
    }
    if let Some(payload) = trim_scheme_prefix(trimmed, "Digest") {
        return digest_authorization_header(payload, method, uri, credentials);
    }
    None
}

fn trim_scheme_prefix<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    let mut parts = value.splitn(2, char::is_whitespace);
    let prefix = parts.next().unwrap_or_default();
    if !prefix.eq_ignore_ascii_case(scheme) {
        return None;
    }
    Some(parts.next().unwrap_or_default().trim())
}

fn basic_authorization_header(credentials: &RtspClientCredentials) -> String {
    let raw = format!("{}:{}", credentials.username, credentials.password);
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

fn digest_authorization_header(
    challenge_payload: &str,
    method: RtspMethod,
    uri: &str,
    credentials: &RtspClientCredentials,
) -> Option<String> {
    let params = parse_digest_params(challenge_payload);
    let realm = required_digest_param(&params, "realm")?;
    let nonce = required_digest_param(&params, "nonce")?;
    let opaque = required_digest_param(&params, "opaque");
    let algorithm = params
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("algorithm"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("MD5");
    let (hash_fn, algo_label): (fn(&str) -> String, &str) = if algorithm.eq_ignore_ascii_case("MD5")
    {
        (md5_hex, "MD5")
    } else if algorithm.eq_ignore_ascii_case("SHA-256") || algorithm.eq_ignore_ascii_case("SHA256")
    {
        (sha256_hex, "SHA-256")
    } else {
        return None;
    };

    let ha1 = hash_fn(&format!(
        "{}:{realm}:{}",
        credentials.username, credentials.password
    ));
    let ha2 = hash_fn(&format!("{}:{uri}", method.as_str()));
    let qop = parse_digest_qop_auth(&params)?;
    let qop_ctx = qop.map(|qop| {
        let nc_value = next_digest_nonce_count(&credentials.username, realm, nonce);
        (
            qop.to_ascii_lowercase(),
            format!("{nc_value:08x}"),
            build_digest_cnonce(credentials, nonce, method, uri),
        )
    });
    let response = if let Some((qop, nc, cnonce)) = &qop_ctx {
        hash_fn(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"))
    } else {
        hash_fn(&format!("{ha1}:{nonce}:{ha2}"))
    };

    let mut header = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\", algorithm={algo_label}",
        credentials.username, realm, nonce, uri, response
    );
    if let Some(opaque) = opaque {
        header.push_str(&format!(", opaque=\"{opaque}\""));
    }
    if let Some((qop_lc, nc, cnonce)) = qop_ctx {
        header.push_str(&format!(", qop={qop_lc}, nc={nc}, cnonce=\"{cnonce}\""));
    }
    Some(header)
}

fn parse_digest_params(value: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in value.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ',' if !in_quote => {
                parse_digest_pair(current.trim(), &mut out);
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parse_digest_pair(current.trim(), &mut out);
    }
    out
}

fn parse_digest_pair(value: &str, out: &mut Vec<(String, String)>) {
    let Some((raw_name, raw_value)) = value.split_once('=') else {
        return;
    };
    let name = raw_name.trim();
    if name.is_empty() {
        return;
    }
    let mut parsed_value = raw_value.trim();
    if parsed_value.starts_with('"') && parsed_value.ends_with('"') && parsed_value.len() >= 2 {
        parsed_value = &parsed_value[1..parsed_value.len() - 1];
    }
    out.push((name.to_string(), parsed_value.to_string()));
}

fn required_digest_param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn parse_digest_qop_auth(params: &[(String, String)]) -> Option<Option<&str>> {
    let Some(raw_qop) = required_digest_param(params, "qop") else {
        return Some(None);
    };
    let has_auth = raw_qop
        .split(',')
        .map(str::trim)
        .any(|token| token.eq_ignore_ascii_case("auth"));
    if has_auth {
        Some(Some("auth"))
    } else {
        None
    }
}

fn next_digest_nonce_count(username: &str, realm: &str, nonce: &str) -> u32 {
    let state = DIGEST_NONCE_COUNT.get_or_init(|| Mutex::new(DigestNonceCountState::default()));
    let mut guard = state.lock();
    let key = (username.to_string(), realm.to_string(), nonce.to_string());
    if let Some(position) = guard.lru.iter().position(|entry| entry == &key) {
        guard.lru.remove(position);
    }
    if !guard.counts.contains_key(&key) && guard.counts.len() >= MAX_TRACKED_DIGEST_NONCES {
        while let Some(evicted_key) = guard.lru.pop_front() {
            if guard.counts.remove(&evicted_key).is_some() {
                break;
            }
        }
    }
    guard.lru.push_back(key.clone());
    let entry = guard.counts.entry(key).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

fn build_digest_cnonce(
    credentials: &RtspClientCredentials,
    nonce: &str,
    method: RtspMethod,
    uri: &str,
) -> String {
    let counter = DIGEST_CNONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    md5_hex(&format!(
        "{}:{}:{}:{}:{}",
        credentials.username,
        nonce,
        method.as_str(),
        uri,
        counter
    ))
}

fn md5_hex(value: &str) -> String {
    format!("{:x}", md5::compute(value.as_bytes()))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_rtsp_core::{RtspHeader, RtspResponseMessage};

    fn response_with_www_authenticate(value: &str) -> RtspResponseMessage {
        response_with_www_authenticate_headers(vec![value])
    }

    fn response_with_www_authenticate_headers(values: Vec<&str>) -> RtspResponseMessage {
        RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 401,
            reason_phrase: "Unauthorized".to_string(),
            headers: values
                .into_iter()
                .map(|value| RtspHeader {
                    name: "WWW-Authenticate".to_string(),
                    value: value.to_string(),
                })
                .collect(),
            body: bytes::Bytes::new(),
        }
    }

    fn required_header_param(value: &str, key: &str) -> String {
        parse_digest_params(value)
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("missing {key}"))
    }

    #[test]
    fn builds_basic_authorization_header_from_401_response() {
        let response = response_with_www_authenticate("Basic");
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("basic header");
        assert_eq!(header, "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn accepts_basic_challenge_with_realm_param() {
        let response = response_with_www_authenticate(r#"Basic realm="live""#);
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        );
        assert_eq!(header.as_deref(), Some("Basic dXNlcjpwYXNz"));
    }

    #[test]
    fn picks_supported_auth_challenge_from_multiple_headers() {
        let response = response_with_www_authenticate_headers(vec![
            r#"Bearer realm="live""#,
            r#"Basic realm="live""#,
        ]);
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        );
        assert_eq!(header.as_deref(), Some("Basic dXNlcjpwYXNz"));
    }

    #[test]
    fn builds_digest_authorization_header_from_401_response() {
        let response =
            response_with_www_authenticate(r#"Digest realm="cheetah", nonce="abc", algorithm=MD5"#);
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("digest header");
        assert!(header.starts_with("Digest username=\"user\""));
        assert!(header.contains("realm=\"cheetah\""));
        assert!(header.contains("nonce=\"abc\""));
        assert!(header.contains("uri=\"rtsp://127.0.0.1/live/test\""));
        assert!(header.contains("response=\"26d238c1b0db16cfff04cbc953857eed\""));
    }

    #[test]
    fn builds_digest_authorization_header_with_qop_auth_fields() {
        let response = response_with_www_authenticate(
            r#"Digest realm="cheetah", nonce="abc-qop-fields", qop="auth", algorithm=MD5"#,
        );
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("digest header with qop");
        let payload = trim_scheme_prefix(&header, "Digest").expect("digest scheme");
        let cnonce = required_header_param(payload, "cnonce");
        let nc = required_header_param(payload, "nc");
        let qop = required_header_param(payload, "qop");
        assert_eq!(qop, "auth");
        assert_eq!(nc, "00000001");
        let expected_response = md5_hex(&format!(
            "{}:{}:{}:{}:{}:{}",
            md5_hex("user:cheetah:pass"),
            "abc-qop-fields",
            nc,
            cnonce,
            "auth",
            md5_hex("DESCRIBE:rtsp://127.0.0.1/live/test")
        ));
        assert!(header.contains("qop=auth"));
        assert!(header.contains("nc=00000001"));
        assert!(header.contains(&format!("cnonce=\"{cnonce}\"")));
        assert!(header.contains(&format!("response=\"{expected_response}\"")));
    }

    #[test]
    fn digest_qop_nonce_count_increments_for_reused_challenge() {
        let response = response_with_www_authenticate(
            r#"Digest realm="cheetah", nonce="abc-nc-monotonic", qop="auth", algorithm=MD5"#,
        );
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let first = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("first digest header");
        let second = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("second digest header");
        let first_payload = trim_scheme_prefix(&first, "Digest").expect("first digest payload");
        let second_payload = trim_scheme_prefix(&second, "Digest").expect("second digest payload");
        assert_eq!(required_header_param(first_payload, "nc"), "00000001");
        assert_eq!(required_header_param(second_payload, "nc"), "00000002");
    }

    #[test]
    fn returns_none_when_not_401_or_unsupported_algorithm() {
        let response = RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 200,
            reason_phrase: "OK".to_string(),
            headers: Vec::new(),
            body: bytes::Bytes::new(),
        };
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        assert!(authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds
        )
        .is_none());

        let unsupported = response_with_www_authenticate(
            r#"Digest realm="cheetah", nonce="abc", algorithm=SHA-512"#,
        );
        assert!(authorization_header_from_response(
            &unsupported,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds
        )
        .is_none());
    }

    #[test]
    fn builds_digest_authorization_header_sha256() {
        let response = response_with_www_authenticate(
            r#"Digest realm="cheetah", nonce="sha256nonce", algorithm=SHA-256"#,
        );
        let creds = RtspClientCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let header = authorization_header_from_response(
            &response,
            RtspMethod::Describe,
            "rtsp://127.0.0.1/live/test",
            &creds,
        )
        .expect("sha256 digest header");
        assert!(header.contains("algorithm=SHA-256"));
        let ha1 = sha256_hex("user:cheetah:pass");
        let ha2 = sha256_hex("DESCRIBE:rtsp://127.0.0.1/live/test");
        let expected_response = sha256_hex(&format!("{ha1}:sha256nonce:{ha2}"));
        assert!(header.contains(&format!("response=\"{expected_response}\"")));
    }
}
