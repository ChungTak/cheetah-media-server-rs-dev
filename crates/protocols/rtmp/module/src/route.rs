use cheetah_sdk::StreamKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpPlayMode {
    Normal,
    Enhanced,
    FastPts,
}

#[derive(Debug, Clone)]
pub struct StreamRoute {
    pub stream_key: StreamKey,
    pub play_mode: RtmpPlayMode,
}

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

pub fn split_stream_path_query(stream_name: &str) -> (&str, &str) {
    if let Some(index) = stream_name.find('?') {
        (&stream_name[..index], &stream_name[index + 1..])
    } else {
        (stream_name, "")
    }
}

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
