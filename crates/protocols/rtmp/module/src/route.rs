use cheetah_sdk::StreamKey;

/// RTMP play mode selected through the `type` query parameter.
///
/// - `Normal`: standard RTMP/FLV playback.
/// - `Enhanced`: enhanced RTMP with fourcc headers and negative composition time.
/// - `FastPts`: keeps PTS as the timestamp instead of DTS.
///
/// RTMP 播放模式，通过 `type` 查询参数选择。
///
/// - `Normal`：标准 RTMP/FLV 播放。
/// - `Enhanced`：增强 RTMP，使用 fourcc 头与负合成时间。
/// - `FastPts`：使用 PTS 而非 DTS 作为时间戳。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpPlayMode {
    Normal,
    Enhanced,
    FastPts,
}

/// Parsed RTMP stream route for a `connect`/`publish`/`play` request.
///
/// The namespace comes from the RTMP application; the path comes from the stream
/// name with query stripped. The `play_mode` controls the downstream presentation.
///
/// RTMP 连接/发布/播放请求解析后的流路由。
///
/// 命名空间来自 RTMP 应用名；路径来自剥离查询参数的流名。
/// `play_mode` 控制下游播放表现。
#[derive(Debug, Clone)]
pub struct StreamRoute {
    pub stream_key: StreamKey,
    pub play_mode: RtmpPlayMode,
}

/// Parses an RTMP application and stream name into a `StreamRoute`.
///
/// Trims leading/trailing slashes, removes query parameters, and derives the
/// play mode from the `type` query key.
///
/// 将 RTMP 应用名与流名解析为 `StreamRoute`。
///
/// 去除前后斜杠、移除查询参数，并从 `type` 查询键推导播放模式。
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

/// Splits a stream name/path at the first `?` into the path and query.
///
/// Returns the whole string as the path if there is no query.
///
/// 在第一个 `?` 处将流名/路径拆分为路径与查询。
///
/// 没有查询参数时返回整个字符串作为路径。
pub fn split_stream_path_query(stream_name: &str) -> (&str, &str) {
    if let Some(index) = stream_name.find('?') {
        (&stream_name[..index], &stream_name[index + 1..])
    } else {
        (stream_name, "")
    }
}

/// Parses the `type` query parameter into a `RtmpPlayMode`.
///
/// Recognized values are `enhanced` and `fastPts`. Defaults to `Normal`.
///
/// 将 `type` 查询参数解析为 `RtmpPlayMode`。
///
/// 识别 `enhanced` 与 `fastPts`，默认返回 `Normal`。
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

/// Parses a `namespace/path` stream key spec into a `StreamKey`.
///
/// Both namespace and path must be non-empty after trimming. The leading slash is
/// optional and may be specified as `/namespace/path`.
///
/// 将 `namespace/path` 流标识符解析为 `StreamKey`。
///
/// 命名空间与路径去除空白后必须非空，前导斜杠可选。
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

/// Extracts the `token` query parameter from a stream name for authentication.
///
/// Only returns a non-empty token value. The token is matched against the configured
/// publish/play token in the module.
///
/// 从流名中提取 `token` 查询参数用于鉴权。
///
/// 仅返回非空 token 值；模块中将其与配置的发布/播放 token 匹配。
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
