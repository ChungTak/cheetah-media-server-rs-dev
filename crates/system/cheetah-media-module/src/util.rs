//! Shared helper utilities for HTTP adapters.
//!
//! HTTP adapter 的共享辅助工具。

use serde_json::Map;

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
