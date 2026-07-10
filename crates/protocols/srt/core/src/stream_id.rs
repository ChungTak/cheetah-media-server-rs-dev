use std::collections::BTreeMap;

use crate::config::SrtStreamMode;
use crate::error::{SrtCoreError, SrtCoreResult};

/// `ParsedSrtStreamId` data structure.
/// `ParsedSrtStreamId` 数据结构.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSrtStreamId {
    /// `stream_key` field of type `String`.
    /// `stream_key` 字段，类型为 `String`.
    pub stream_key: String,
    /// `mode` field.
    /// `mode` 字段.
    pub mode: Option<SrtStreamMode>,
    /// `user` field.
    /// `user` 字段.
    pub user: Option<String>,
    /// `host` field.
    /// `host` 字段.
    pub host: Option<String>,
    /// `session` field.
    /// `session` 字段.
    pub session: Option<String>,
    /// `extras` field.
    /// `extras` 字段.
    pub extras: BTreeMap<String, String>,
}

/// Parses `srt_stream_id` from input.
/// 解析 `srt_stream_id` 来自 输入.
pub fn parse_srt_stream_id(input: &str) -> SrtCoreResult<ParsedSrtStreamId> {
    let input = input.trim();
    if input.is_empty() {
        return Err(SrtCoreError::InvalidStreamId(
            "stream id is empty".to_string(),
        ));
    }

    if let Some(rest) = input.strip_prefix("#!::") {
        parse_access_control_stream_id(rest)
    } else {
        let stream_key = normalize_stream_key(input)?;
        Ok(ParsedSrtStreamId {
            stream_key,
            mode: None,
            user: None,
            host: None,
            session: None,
            extras: BTreeMap::new(),
        })
    }
}

fn parse_access_control_stream_id(input: &str) -> SrtCoreResult<ParsedSrtStreamId> {
    let mut fields = BTreeMap::new();
    for pair in input.split(',') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            SrtCoreError::InvalidStreamId(format!("field `{pair}` is missing `=`"))
        })?;
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        fields.insert(key, value);
    }

    let raw_resource = fields
        .remove("r")
        .ok_or_else(|| SrtCoreError::InvalidStreamId("missing `r` resource".to_string()))?;
    let stream_key = normalize_stream_key(&raw_resource)?;
    let mode = match fields.remove("m").as_deref() {
        Some("publish") => Some(SrtStreamMode::Publish),
        Some("request") => Some(SrtStreamMode::Request),
        Some("play") => Some(SrtStreamMode::Play),
        Some(other) => {
            return Err(SrtCoreError::InvalidStreamId(format!(
                "unknown stream mode `{other}`"
            )));
        }
        None => None,
    };

    Ok(ParsedSrtStreamId {
        stream_key,
        mode,
        user: fields.remove("u"),
        host: fields.remove("h"),
        session: fields.remove("s"),
        extras: fields,
    })
}

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

/// `percent_decode` function.
/// `percent_decode` 函数.
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

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
