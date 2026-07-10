use cheetah_sdk::StreamKey;

/// `RtmpPlayMode` enumeration.
/// `RtmpPlayMode` 枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpPlayMode {
    /// `Normal` variant.
    /// `Normal` 变体.
    Normal,
    /// `Enhanced` variant.
    /// `Enhanced` 变体.
    Enhanced,
    /// `FastPts` variant.
    /// `FastPts` 变体.
    FastPts,
}

/// `StreamRoute` data structure.
/// `StreamRoute` 数据结构.
#[derive(Debug, Clone)]
pub struct StreamRoute {
    /// `stream_key` field of type `StreamKey`.
    /// `stream_key` 字段，类型为 `StreamKey`.
    pub stream_key: StreamKey,
    /// `play_mode` field of type `RtmpPlayMode`.
    /// `play_mode` 字段，类型为 `RtmpPlayMode`.
    pub play_mode: RtmpPlayMode,
}

/// Parses `stream_route` from input.
/// 解析 `stream_route` 来自 输入.
pub fn parse_stream_route(app: &str, stream_name: &str) -> StreamRoute {
    let app = app.trim_matches('/');
    let (app, _) = split_stream_path_query(app);
    let (path, query) = split_stream_path_query(stream_name);
    let play_mode = parse_play_mode_from_query(query);
    StreamRoute {
        stream_key: StreamKey::new(app, path.trim_matches('/')),
        play_mode,
    }
}

/// `split_stream_path_query` function.
/// `split_stream_path_query` 函数.
pub fn split_stream_path_query(stream_name: &str) -> (&str, &str) {
    if let Some(index) = stream_name.find('?') {
        (&stream_name[..index], &stream_name[index + 1..])
    } else {
        (stream_name, "")
    }
}

/// Parses `play_mode_from_query` from input.
/// 解析 `play_mode_from_query` 来自 输入.
pub fn parse_play_mode_from_query(query: &str) -> RtmpPlayMode {
    for part in query.split('&') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or_default();
        let value = kv.next().unwrap_or_default();
        if key.eq_ignore_ascii_case("type") {
            if value.eq_ignore_ascii_case("enhanced") {
                return RtmpPlayMode::Enhanced;
            }
            if value.eq_ignore_ascii_case("fastPts") {
                return RtmpPlayMode::FastPts;
            }
        }
    }
    RtmpPlayMode::Normal
}

/// Parses `stream_key_spec` from input.
/// 解析 `stream_key_spec` 来自 输入.
pub fn parse_stream_key_spec(spec: &str) -> Option<StreamKey> {
    let trimmed = spec.trim().trim_matches('/');
    let (namespace, path) = trimmed.split_once('/')?;
    let namespace = namespace.trim().trim_matches('/');
    let path = path.trim().trim_matches('/');
    if namespace.is_empty() || path.is_empty() {
        return None;
    }
    Some(StreamKey::new(namespace, path))
}

/// Extract the `token` query parameter from a stream name.
pub fn extract_token_from_stream_name(stream_name: &str) -> Option<&str> {
    let (_, query) = split_stream_path_query(stream_name);
    for part in query.split('&') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or_default();
        let value = kv.next().unwrap_or_default();
        if key.eq_ignore_ascii_case("token") && !value.is_empty() {
            return Some(value);
        }
    }
    None
}
