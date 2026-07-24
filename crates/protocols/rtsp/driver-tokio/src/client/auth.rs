use std::collections::{HashMap, VecDeque};
use std::fmt;

use base64::Engine;
use cheetah_rtsp_core::{RtspMethod, RtspResponseMessage};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

type DigestNonceKey = (String, String, String);
const MAX_TRACKED_DIGEST_NONCES: usize = 4096;

/// State for tracking per-(username, realm, nonce) digest nonce counts.
///
/// Keeps an LRU of recently used challenge keys so that `nc` values remain
/// monotonic per challenge and bounded in memory.
///
/// 用于追踪每个 (username, realm, nonce) 的 digest nonce 计数状态。
///
/// 保留最近使用挑战键的 LRU，使每个挑战的 `nc` 值保持单调且内存占用有界。
#[derive(Default)]
struct DigestNonceCountState {
    counts: HashMap<DigestNonceKey, u32>,
    lru: VecDeque<DigestNonceKey>,
}

/// Global counter for producing unique `cnonce` values.
///
/// 用于生成唯一 `cnonce` 值的全局计数器。
static DIGEST_CNONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Global nonce-count state protected by a mutex.
///
/// 受互斥锁保护的全局 nonce 计数状态。
static DIGEST_NONCE_COUNT: OnceLock<Mutex<DigestNonceCountState>> = OnceLock::new();

/// Credentials used by the RTSP client for Basic or Digest authentication.
///
/// RTSP 客户端用于 Basic 或 Digest 认证的凭据。
#[derive(Clone, PartialEq, Eq)]
pub struct RtspClientCredentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for RtspClientCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtspClientCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Inspect a 401 response and build an `Authorization` header if a supported scheme is found.
///
/// Iterates over `WWW-Authenticate` headers and picks the first supported challenge.
/// Basic is handled by Base64 encoding `username:password`. Digest follows RFC 7616/2617
/// with `qop=auth` when advertised, using either MD5 or SHA-256 as the hash algorithm.
///
/// 检查 401 响应，若发现支持的认证方案则构建 `Authorization` 头。
///
/// 遍历 `WWW-Authenticate` 头并选择第一个支持的挑战。Basic 通过 Base64 编码
/// `username:password` 处理。Digest 遵循 RFC 7616/2617，当提供 `qop=auth` 时使用
/// MD5 或 SHA-256 作为哈希算法。
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

/// Build the authorization header for a single `WWW-Authenticate` challenge.
///
/// 为单个 `WWW-Authenticate` 挑战构建认证头。
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

/// Strip the authentication scheme prefix and return the remaining challenge payload.
///
/// 去除认证方案前缀并返回剩余挑战负载。
fn trim_scheme_prefix<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    let mut parts = value.splitn(2, char::is_whitespace);
    let prefix = parts.next().unwrap_or_default();
    if !prefix.eq_ignore_ascii_case(scheme) {
        return None;
    }
    Some(parts.next().unwrap_or_default().trim())
}

/// Build a Basic `Authorization` header.
///
/// Encodes `username:password` with Base64 and prefixes the scheme.
///
/// 构建 Basic `Authorization` 头。
///
/// 使用 Base64 编码 `username:password` 并加上方案前缀。
fn basic_authorization_header(credentials: &RtspClientCredentials) -> String {
    let raw = format!("{}:{}", credentials.username, credentials.password);
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

/// Build a Digest `Authorization` header from a challenge payload.
///
/// Parses comma-separated `key=value` parameters, computes `HA1` and `HA2`, and
/// generates the response digest. When `qop=auth` is present, it includes `nc`,
/// `cnonce`, and `qop` fields. Nonce counts are tracked per (username, realm, nonce)
/// so the same challenge can be reused across requests with monotonically increasing `nc`.
///
/// 从挑战负载构建 Digest `Authorization` 头。
///
/// 解析逗号分隔的 `key=value` 参数，计算 `HA1` 与 `HA2`，并生成响应摘要。
/// 若存在 `qop=auth`，则包含 `nc`、`cnonce`、`qop` 字段。按 (username, realm, nonce)
/// 追踪 nonce 计数，使同一挑战可在多个请求中复用且 `nc` 单调递增。
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

/// Parse comma-separated digest challenge parameters while respecting quoted values.
///
/// State machine toggles `in_quote` on double quotes so commas inside values are not
/// treated as separators. Outer quotes are stripped from the value.
///
/// 解析逗号分隔的 digest 挑战参数，同时尊重带引号的值。
///
/// 状态机在双引号上切换 `in_quote`，因此值内的逗号不会被当作分隔符。外层引号从值中剥离。
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

/// Parse a single `name=value` pair and append it to the output vector.
///
/// 解析单个 `name=value` 对并追加到输出向量。
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

/// Return a required digest parameter by case-insensitive name.
///
/// 按不区分大小写的名称返回必需的 digest 参数。
fn required_digest_param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

/// Parse the `qop` parameter and confirm that `auth` is supported.
///
/// Returns `Some(None)` if `qop` is absent, `Some(Some("auth"))` if `auth` is present,
/// and `None` if `qop` is present but does not contain `auth`.
///
/// 解析 `qop` 参数并确认是否支持 `auth`。
///
/// 若 `qop` 缺失返回 `Some(None)`；若包含 `auth` 返回 `Some(Some("auth"))`；
/// 若 `qop` 存在但不包含 `auth` 返回 `None`。
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

/// Get the next nonce count for a given challenge and bump the LRU.
///
/// Per-key counts are incremented and stored in an LRU bounded by
/// `MAX_TRACKED_DIGEST_NONCES` to avoid unbounded memory growth.
///
/// 获取给定挑战的下一个 nonce 计数并更新 LRU。
///
/// 按键递增计数并存储在受 `MAX_TRACKED_DIGEST_NONCES` 限制的 LRU 中，防止内存无限增长。
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

/// Build a unique client nonce for the digest response.
///
/// Combines username, nonce, method, URI, and a global counter into an MD5 hex digest.
///
/// 为 digest 响应构建唯一客户端 nonce。
///
/// 将用户名、nonce、方法、URI 和全局计数器组合为 MD5 十六进制摘要。
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

/// Compute the MD5 hex digest of a UTF-8 string.
///
/// 计算 UTF-8 字符串的 MD5 十六进制摘要。
fn md5_hex(value: &str) -> String {
    format!("{:x}", md5::compute(value.as_bytes()))
}

/// Compute the SHA-256 hex digest of a UTF-8 string.
///
/// 计算 UTF-8 字符串的 SHA-256 十六进制摘要。
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

    #[test]
    fn client_credentials_debug_redacts_password() {
        let creds = RtspClientCredentials {
            username: "alice".to_string(),
            password: "wonderland".to_string(),
        };
        let out = format!("{creds:?}");
        assert!(out.contains("alice"), "username missing: {out}");
        assert!(!out.contains("wonderland"), "password leaked: {out}");
    }
}
