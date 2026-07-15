//! Shared helper utilities for HTTP adapters.
//!
//! HTTP adapter 的共享辅助工具。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Map;

static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Percent-decode a string, preserving encoded slashes (`%2F` / `%2f`) so that
/// `%2F` does not accidentally become a path separator and fail downstream
/// `MediaKey` validators.
///
/// 百分比解码字符串，保留编码后的斜杠（`%2F` / `%2f`），避免其被误转为路径分隔符
/// 导致下游 `MediaKey` 校验失败。
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1] as char;
            let h2 = bytes[i + 2] as char;
            let hex = format!("{h1}{h2}");
            if hex.eq_ignore_ascii_case("2F") {
                // Keep the encoded slash literal.
                out.extend_from_slice(b"%2F");
            } else if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b);
            } else {
                out.push(bytes[i]);
                out.push(bytes[i + 1]);
                out.push(bytes[i + 2]);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a URL query string into a `serde_json::Value` object.
///
/// `+` is interpreted as a space, and percent-encoded values are decoded.
///
/// 将 URL query 字符串解析为 `serde_json::Value` 对象。
///
/// `+` 被解释为空格，百分号编码的值会被解码。
pub fn query_to_json(query: Option<&str>) -> serde_json::Value {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return serde_json::Value::Null;
    };
    let mut map = Map::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").replace('+', " ");
        let value = parts.next().unwrap_or("").replace('+', " ");
        let key = percent_decode(&key);
        let value = percent_decode(&value);
        map.insert(key, serde_json::Value::String(value));
    }
    serde_json::Value::Object(map)
}

/// Parse a JSON value that may be an unsigned integer or a numeric string.
///
/// 解析可能是无符号整数或数字字符串的 JSON 值。
pub fn parse_json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Parse a JSON value that may be a boolean, a numeric flag (non-zero = true),
/// or a string like `"true"` / `"1"` / `"yes"` / `"on"`.
///
/// 解析可能是布尔值、数字标志或字符串形式的 JSON 值。
pub fn parse_json_bool(value: &serde_json::Value) -> Option<bool> {
    if let Some(b) = value.as_bool() {
        return Some(b);
    }
    if let Some(n) = parse_json_u64(value) {
        return Some(n != 0);
    }
    value.as_str().map(|s| {
        matches!(
            s.trim().to_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_keeps_encoded_slashes() {
        assert_eq!(percent_decode("live%2Ftest"), "live%2Ftest");
        assert_eq!(percent_decode("live%2ftest"), "live%2Ftest");
        assert_eq!(percent_decode("a%20b"), "a b");
    }

    #[test]
    fn query_to_json_decodes_plus_and_percent() {
        let value = query_to_json(Some("vhost=__defaultVhost__&app=live&stream=a+b%2F1"));
        let map = value.as_object().unwrap();
        assert_eq!(map["vhost"], "__defaultVhost__");
        assert_eq!(map["app"], "live");
        assert_eq!(map["stream"], "a b%2F1");
    }

    #[test]
    fn parse_json_u64_handles_number_and_string() {
        assert_eq!(parse_json_u64(&serde_json::json!(42)), Some(42));
        assert_eq!(parse_json_u64(&serde_json::json!("123")), Some(123));
        assert_eq!(parse_json_u64(&serde_json::json!("abc")), None);
    }

    #[test]
    fn parse_json_bool_handles_variants() {
        assert_eq!(parse_json_bool(&serde_json::json!(true)), Some(true));
        assert_eq!(parse_json_bool(&serde_json::json!(false)), Some(false));
        assert_eq!(parse_json_bool(&serde_json::json!(1)), Some(true));
        assert_eq!(parse_json_bool(&serde_json::json!(0)), Some(false));
        assert_eq!(parse_json_bool(&serde_json::json!("yes")), Some(true));
        assert_eq!(parse_json_bool(&serde_json::json!("off")), Some(false));
    }

    #[test]
    fn generate_request_id_is_unique_per_call() {
        let a = generate_request_id();
        let b = generate_request_id();
        assert_ne!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn request_deadline_caps_client_value_and_returns_none_by_default() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let far_future = before + 600_000; // 10 minutes
        let d = request_deadline(Some(far_future), 30_000).unwrap();
        assert!(d <= before + 30_000 && d >= before);
        assert_eq!(request_deadline(None, 30_000), None);
    }

    #[test]
    fn set_request_id_header_replaces_existing() {
        use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
        let mut req = HttpRequest {
            method: HttpMethod::Get,
            path: "/test".to_string(),
            query: None,
            headers: vec![HttpHeader {
                name: "x-request-id".to_string(),
                value: "old".to_string(),
            }],
            body: bytes::Bytes::new(),
        };
        set_request_id_header(&mut req, "new");
        assert_eq!(req.headers.len(), 1);
        assert_eq!(req.headers[0].value, "new");
    }

    #[test]
    fn set_response_request_id_header_replaces_existing() {
        use cheetah_sdk::{HttpHeader, HttpResponse};
        let mut resp = HttpResponse {
            status: 200,
            headers: vec![HttpHeader {
                name: "x-request-id".to_string(),
                value: "old".to_string(),
            }],
            body: bytes::Bytes::new(),
        };
        set_response_request_id_header(&mut resp, "new");
        assert_eq!(resp.headers.len(), 1);
        assert_eq!(resp.headers[0].value, "new");
    }
}

/// Generate a process-unique request ID.
///
/// 生成进程内唯一的 request ID。
pub fn generate_request_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let n = REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{now:016x}-{n:08x}")
}

/// Compute a request deadline from an optional client deadline header.
///
/// 根据可选的客户端 deadline 头计算请求 deadline。
///
/// When a client deadline is provided it is clamped to `now + default_timeout_ms`
/// so callers cannot request unbounded server-side wait times. When no header is
/// present `None` is returned and the route-level timeout is used instead.
pub fn request_deadline(client_deadline_ms: Option<i64>, default_timeout_ms: i64) -> Option<i64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let max_deadline = now + default_timeout_ms;
    client_deadline_ms.map(|d| d.min(max_deadline))
}

/// Set or overwrite the `x-request-id` header on an incoming request.
pub fn set_request_id_header(req: &mut cheetah_sdk::HttpRequest, request_id: &str) {
    use cheetah_sdk::HttpHeader;
    req.headers
        .retain(|h| !h.name.eq_ignore_ascii_case("x-request-id"));
    req.headers.push(HttpHeader {
        name: "x-request-id".to_string(),
        value: request_id.to_string(),
    });
}

/// Set or overwrite the `x-request-id` header on an outgoing response.
pub fn set_response_request_id_header(resp: &mut cheetah_sdk::HttpResponse, request_id: &str) {
    use cheetah_sdk::HttpHeader;
    resp.headers
        .retain(|h| !h.name.eq_ignore_ascii_case("x-request-id"));
    resp.headers.push(HttpHeader {
        name: "x-request-id".to_string(),
        value: request_id.to_string(),
    });
}
