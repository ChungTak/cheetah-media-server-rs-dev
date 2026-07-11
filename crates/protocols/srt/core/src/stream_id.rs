use std::collections::BTreeMap;

use crate::config::SrtStreamMode;
use crate::error::{SrtCoreError, SrtCoreResult};

/// Parse options for `parse_srt_stream_id_with_options`.
///
/// `parse_srt_stream_id_with_options` 的解析选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamIdParseOptions {
    /// Default virtual host when `h` is absent.
    pub default_vhost: String,
    /// Require the `#!::` access-control prefix.
    pub strict_prefix: bool,
    /// Require the `r` resource to contain two non-empty segments (`app/stream`).
    pub strict_resource: bool,
    /// Allow a bare key without `#!::` for legacy clients.
    pub allow_bare_key: bool,
}

impl Default for StreamIdParseOptions {
    fn default() -> Self {
        Self {
            default_vhost: "__defaultVhost__".to_string(),
            strict_prefix: true,
            strict_resource: true,
            allow_bare_key: false,
        }
    }
}

/// Parsed SRT access-control stream id.
///
/// 解析后的 SRT 访问控制流 ID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSrtStreamId {
    pub vhost: String,
    pub app: String,
    pub stream: String,
    /// Normalized `app/stream` resource.
    pub resource: String,
    /// Backward-compatible `app/stream` key.
    pub stream_key: String,
    pub mode: Option<SrtStreamMode>,
    pub user: Option<String>,
    pub session: Option<String>,
    /// All remaining key/value pairs except `h` and `r`, including `m`.
    pub auth_params: BTreeMap<String, String>,
}

/// Parse an SRT stream id using the strict ZLM-compatible defaults.
///
/// 使用严格 ZLM 兼容默认选项解析 SRT stream id。
pub fn parse_srt_stream_id(input: &str) -> SrtCoreResult<ParsedSrtStreamId> {
    parse_srt_stream_id_with_options(input, &StreamIdParseOptions::default())
}

/// Parse an SRT stream id with explicit options.
///
/// 使用显式选项解析 SRT stream id。
pub fn parse_srt_stream_id_with_options(
    input: &str,
    opts: &StreamIdParseOptions,
) -> SrtCoreResult<ParsedSrtStreamId> {
    let input = input.trim();
    if input.is_empty() {
        return Err(SrtCoreError::InvalidStreamId(
            "stream id is empty".to_string(),
        ));
    }

    if let Some(body) = input.strip_prefix("#!::") {
        parse_access_control_body(body, opts)
    } else if opts.allow_bare_key {
        parse_bare_stream_key(input, opts)
    } else {
        Err(SrtCoreError::InvalidStreamId(
            "stream id must start with #!::".to_string(),
        ))
    }
}

fn parse_access_control_body(
    input: &str,
    opts: &StreamIdParseOptions,
) -> SrtCoreResult<ParsedSrtStreamId> {
    let mut fields = BTreeMap::new();
    for pair in input.split(',') {
        if pair.is_empty() {
            continue;
        }
        let Some((key, value)) = pair.split_once('=') else {
            return Err(SrtCoreError::InvalidStreamId(format!(
                "field `{pair}` is missing `=`"
            )));
        };
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        fields.insert(key, value);
    }

    let vhost = fields
        .remove("h")
        .unwrap_or_else(|| opts.default_vhost.clone());
    let raw_resource = fields
        .remove("r")
        .ok_or_else(|| SrtCoreError::InvalidStreamId("missing `r` resource".to_string()))?;

    if raw_resource.is_empty() {
        return Err(SrtCoreError::InvalidStreamId(
            "resource `r` is empty".to_string(),
        ));
    }

    let (app, stream, resource) = if opts.strict_resource {
        parse_strict_resource(&raw_resource)?
    } else {
        let normalized = normalize_stream_key(&raw_resource)?;
        let (app, stream) = split_app_stream(&normalized);
        (app, stream, normalized)
    };

    let stream_key = format!("{app}/{stream}");

    // `m` stays in auth_params for authorization hooks.
    let mode = parse_mode(fields.get("m").map(String::as_str));
    let user = fields.get("u").cloned();
    let session = fields.get("s").cloned();
    let auth_params = fields;

    Ok(ParsedSrtStreamId {
        vhost,
        app,
        stream,
        resource,
        stream_key,
        mode,
        user,
        session,
        auth_params,
    })
}

fn parse_mode(raw: Option<&str>) -> Option<SrtStreamMode> {
    match raw {
        Some("publish") => Some(SrtStreamMode::Publish),
        Some("request") => Some(SrtStreamMode::Request),
        Some("play") => Some(SrtStreamMode::Play),
        Some(_) => Some(SrtStreamMode::Request),
        None => None,
    }
}

fn parse_strict_resource(raw: &str) -> SrtCoreResult<(String, String, String)> {
    let (app, stream) = raw.split_once('/').ok_or_else(|| {
        SrtCoreError::InvalidStreamId(format!("resource `{raw}` must be app/stream"))
    })?;
    validate_resource_segment(app, "app")?;
    validate_resource_segment(stream, "stream")?;
    Ok((app.to_string(), stream.to_string(), raw.to_string()))
}

fn validate_resource_segment(value: &str, name: &str) -> SrtCoreResult<()> {
    if value.is_empty() {
        return Err(SrtCoreError::InvalidStreamId(format!(
            "{name} in `r` is empty"
        )));
    }
    if value.starts_with('/') {
        return Err(SrtCoreError::InvalidStreamId(format!(
            "{name} in `r` must not start with `/`"
        )));
    }
    if value.contains("..") {
        return Err(SrtCoreError::InvalidStreamId(format!(
            "{name} in `r` must not contain `..`"
        )));
    }
    if value.contains("//") {
        return Err(SrtCoreError::InvalidStreamId(format!(
            "{name} in `r` must not contain `//`"
        )));
    }
    if value.chars().any(|ch| ch.is_ascii_control()) {
        return Err(SrtCoreError::InvalidStreamId(format!(
            "{name} in `r` contains control characters"
        )));
    }
    Ok(())
}

fn parse_bare_stream_key(
    input: &str,
    opts: &StreamIdParseOptions,
) -> SrtCoreResult<ParsedSrtStreamId> {
    let normalized = normalize_stream_key(input)?;
    let (app, stream) = split_app_stream(&normalized);
    let resource = normalized;
    let stream_key = format!("{app}/{stream}");
    Ok(ParsedSrtStreamId {
        vhost: opts.default_vhost.clone(),
        app,
        stream,
        resource,
        stream_key,
        mode: None,
        user: None,
        session: None,
        auth_params: BTreeMap::new(),
    })
}

fn split_app_stream(value: &str) -> (String, String) {
    match value.split_once('/') {
        Some((app, stream)) if !app.is_empty() && !stream.is_empty() => {
            (app.to_string(), stream.to_string())
        }
        _ => ("live".to_string(), value.to_string()),
    }
}

/// Normalize a stream key by trimming leading slashes and rejecting dangerous paths.
///
/// 规范流密钥：去除前导斜杠并拒绝危险路径。
fn normalize_stream_key(input: &str) -> SrtCoreResult<String> {
    let stream_key = input.trim().trim_start_matches('/').to_string();
    if stream_key.is_empty() {
        return Err(SrtCoreError::InvalidStreamId(
            "stream key is empty".to_string(),
        ));
    }
    if stream_key.contains("..") {
        return Err(SrtCoreError::InvalidStreamId(
            "stream key must not contain `..`".to_string(),
        ));
    }
    if stream_key.contains("//") {
        return Err(SrtCoreError::InvalidStreamId(
            "stream key must not contain `//`".to_string(),
        ));
    }
    if stream_key.chars().any(|ch| ch.is_ascii_control()) {
        return Err(SrtCoreError::InvalidStreamId(
            "stream key contains control characters".to_string(),
        ));
    }
    Ok(stream_key)
}

/// Percent-decode a UTF-8 string with validation.
///
/// 对 UTF-8 字符串进行百分号解码并校验。
pub(crate) fn percent_decode(input: &str) -> SrtCoreResult<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(SrtCoreError::InvalidStreamId(
                        "invalid percent escape".to_string(),
                    ));
                }
                let hi = hex_value(bytes[i + 1]).ok_or_else(|| {
                    SrtCoreError::InvalidStreamId("invalid percent escape".to_string())
                })?;
                let lo = hex_value(bytes[i + 2]).ok_or_else(|| {
                    SrtCoreError::InvalidStreamId("invalid percent escape".to_string())
                })?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| {
        SrtCoreError::InvalidStreamId("percent-decoded value is not UTF-8".to_string())
    })
}

/// Convert a hex ASCII digit to its numeric value.
///
/// 将十六进制 ASCII 数字转换为数值。
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
