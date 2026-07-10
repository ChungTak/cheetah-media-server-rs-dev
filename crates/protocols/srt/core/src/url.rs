use std::collections::BTreeMap;

use crate::config::{SrtKeyLength, SrtRole};
use crate::error::{SrtCoreError, SrtCoreResult};
use crate::stream_id::percent_decode;

/// `ParsedSrtUrl` data structure.
/// `ParsedSrtUrl` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSrtUrl {
    pub host: Option<String>,
    pub port: u16,
    pub mode: Option<SrtRole>,
    pub stream_id: Option<String>,
    pub latency_ms: Option<u64>,
    pub passphrase: Option<String>,
    pub key_length: Option<SrtKeyLength>,
    pub extras: BTreeMap<String, String>,
}

/// Parses `SRT URL` from input.
/// 从输入解析 `SRT URL`。
pub fn parse_srt_url(input: &str) -> SrtCoreResult<ParsedSrtUrl> {
    let rest = input
        .strip_prefix("srt://")
        .ok_or_else(|| SrtCoreError::InvalidUrl("URL must start with `srt://`".to_string()))?;
    let (authority, query) = rest.split_once('?').unwrap_or((rest, ""));
    let authority = authority.trim_end_matches('/');
    let (host_part, port_part) = authority.rsplit_once(':').ok_or_else(|| {
        SrtCoreError::InvalidUrl("SRT URL authority must include a port".to_string())
    })?;
    let port = port_part
        .parse::<u16>()
        .map_err(|err| SrtCoreError::InvalidUrl(format!("invalid port: {err}")))?;
    let host = if host_part.is_empty() {
        None
    } else {
        Some(host_part.trim_matches(['[', ']']).to_string())
    };

    let mut fields = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.insert(percent_decode_url(key)?, percent_decode_url(value)?);
    }

    let mode = match fields.remove("mode").as_deref() {
        Some("caller") => Some(SrtRole::Caller),
        Some("listener") => Some(SrtRole::Listener),
        Some(other) => {
            return Err(SrtCoreError::InvalidUrl(format!("unknown mode `{other}`")));
        }
        None => None,
    };

    let stream_id = fields
        .remove("streamid")
        .or_else(|| fields.remove("streamId"));
    let latency_ms = match fields.remove("latency") {
        Some(value) => Some(value.parse::<u64>().map_err(|err| {
            SrtCoreError::InvalidUrl(format!("invalid latency value `{value}`: {err}"))
        })?),
        None => None,
    };
    let passphrase = fields.remove("passphrase");
    let key_length = match fields.remove("pbkeylen") {
        Some(value) => Some(match value.as_str() {
            "16" => SrtKeyLength::Aes128,
            "32" => SrtKeyLength::Aes256,
            _ => {
                return Err(SrtCoreError::InvalidConfig(format!(
                    "unsupported pbkeylen `{value}`"
                )));
            }
        }),
        None => None,
    };

    Ok(ParsedSrtUrl {
        host,
        port,
        mode,
        stream_id,
        latency_ms,
        passphrase,
        key_length,
        extras: fields,
    })
}

fn percent_decode_url(input: &str) -> SrtCoreResult<String> {
    percent_decode(input).map_err(|err| SrtCoreError::InvalidUrl(err.to_string()))
}
