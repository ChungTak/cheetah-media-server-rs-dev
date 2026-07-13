//! Shared helper utilities for HTTP adapters.
//!
//! HTTP adapter 的共享辅助工具。

use crate::error::AdapterError;
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

/// Parse a user-supplied FFmpeg command string into controlled `input_options`
/// and `output_options` vectors.
///
/// The `ffmpeg` binary token is stripped, the `-i` input option and its argument
/// are removed from the option list, and the source URL is returned from
/// `src_url` when present, or from the `-i` argument otherwise. The remaining
/// tokens are split into options before `-i` (`input_options`) and after the
/// input source (`output_options`).
///
/// 将用户提供的 FFmpeg 命令字符串解析为受控的 `input_options` 和 `output_options` 向量。
/// 移除 `ffmpeg` 二进制 token，移除 `-i` 输入选项及其参数，`src_url` 优先作为源地址，
/// 否则从 `-i` 参数获取。剩余 token 按 `-i` 前后分别放入 `input_options` 和 `output_options`。
pub fn parse_ffmpeg_request(
    ffmpeg_cmd: Option<&str>,
    src_url: Option<&str>,
) -> Result<(String, Vec<String>, Vec<String>), AdapterError> {
    let mut args = if let Some(cmd) = ffmpeg_cmd.filter(|c| !c.is_empty()) {
        shlex::split(cmd)
            .ok_or_else(|| AdapterError::InvalidRequest("invalid ffmpeg_cmd".to_string()))?
    } else {
        Vec::new()
    };

    if let Some(first) = args.first() {
        if first.eq_ignore_ascii_case("ffmpeg") || first.ends_with("ffmpeg") {
            args.remove(0);
        }
    }

    let (input_options, source_from_cmd, output_options) =
        if let Some(idx) = args.iter().position(|a| a == "-i") {
            (
                args[..idx].to_vec(),
                args.get(idx + 1).cloned(),
                args.get(idx + 2..).unwrap_or_default().to_vec(),
            )
        } else {
            (args, None, Vec::new())
        };

    let source_url = src_url
        .map(String::from)
        .or(source_from_cmd)
        .ok_or_else(|| {
            AdapterError::InvalidRequest("src_url or ffmpeg_cmd -i is required".to_string())
        })?;

    Ok((source_url, input_options, output_options))
}

/// Reject tokens that are common shell metacharacters or contain command
/// substitution patterns, so they cannot be used to inject shell behavior.
///
/// 拒绝常见 shell 元字符或包含命令替换的 token，避免被注入 shell 行为。
pub fn validate_ffmpeg_options(options: &[String]) -> Result<(), AdapterError> {
    const DENIED: &[&str] = &[
        ";", "|", "&", "&&", "||", ">", ">>", "<", "<<", "$(", "`", "~", "*", "?", "!", "#", "^",
        "$", "(", ")", "[", "]", "{", "}", "=", "\\",
    ];
    for token in options {
        if DENIED.contains(&token.as_str()) {
            return Err(AdapterError::InvalidRequest(format!(
                "unsafe ffmpeg option: {token}"
            )));
        }
        if token.contains("$(") || token.contains('`') {
            return Err(AdapterError::InvalidRequest(format!(
                "unsafe ffmpeg option: {token}"
            )));
        }
    }
    Ok(())
}
