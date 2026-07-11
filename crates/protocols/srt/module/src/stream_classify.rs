/// Result of classifying an SRT stream id.
///
/// SRT stream id 分类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SrtClassifiedStream {
    pub mode: SrtStreamMode,
    pub stream_key: StreamKey,
    pub auth: SrtAuthContext,
}
fn classified_from_forced(
    config: &SrtModuleConfig,
    forced: ForcedSrtMode,
    peer_addr: SocketAddr,
) -> SrtClassifiedStream {
    let vhost = config.default_vhost.clone();
    let app = forced.stream_key.namespace.clone();
    let stream = forced.stream_key.path.clone();
    SrtClassifiedStream {
        mode: forced.mode,
        stream_key: forced.stream_key.clone(),
        auth: SrtAuthContext {
            mode: forced.mode,
            vhost,
            app,
            stream,
            stream_key: forced.stream_key,
            user: None,
            auth_params: BTreeMap::new(),
            peer_addr: Some(peer_addr),
        },
    }
}

/// Build the engine `StreamKey` from vhost, app, and stream.
///
/// 从 vhost、app、stream 构建引擎 `StreamKey`。
fn build_stream_key(vhost_mode: &str, vhost: &str, app: &str, stream: &str) -> StreamKey {
    if vhost_mode.eq_ignore_ascii_case("vhost_prefix") {
        StreamKey::new(format!("{vhost}/{app}"), stream)
    } else {
        StreamKey::new(app, stream)
    }
}

fn parse_default_mode(s: &str) -> Result<SrtStreamMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "publish" => Ok(SrtStreamMode::Publish),
        "request" => Ok(SrtStreamMode::Request),
        "play" => Ok(SrtStreamMode::Play),
        other => Err(format!("unknown default mode `{other}`")),
    }
}

/// Classify the stream id into a mode, stream key, and auth context.
///
/// 将 stream id 分类为模式、流密钥与鉴权上下文。
fn classify_stream(
    config: &SrtModuleConfig,
    stream_id: Option<&str>,
    peer_addr: Option<SocketAddr>,
    peer_version: Option<u32>,
) -> Result<SrtClassifiedStream, String> {
    let input = match stream_id {
        Some(value) if !value.is_empty() => value,
        _ => {
            if config.ingress.default_publish_stream_key.is_empty() {
                return Err("reject:invalid_stream_id: missing stream id".to_string());
            }
            config.ingress.default_publish_stream_key.as_str()
        }
    };

    let opts = StreamIdParseOptions {
        default_vhost: config.default_vhost.clone(),
        strict_prefix: config.stream_id.strict_prefix,
        strict_resource: config.stream_id.strict_resource,
        allow_bare_key: config.stream_id.allow_bare_key,
    };
    let parsed = parse_srt_stream_id_with_options(input, &opts)
        .map_err(|err| format!("reject:invalid_stream_id: {err}"))?;

    let default_mode = parse_default_mode(&config.ingress.default_mode)?;
    let mode = parsed.mode.unwrap_or(default_mode);
    let stream_key = build_stream_key(
        &config.stream_id.stream_key_vhost_mode,
        &parsed.vhost,
        &parsed.app,
        &parsed.stream,
    );

    let auth = SrtAuthContext {
        mode,
        vhost: parsed.vhost.clone(),
        app: parsed.app.clone(),
        stream: parsed.stream.clone(),
        stream_key: stream_key.clone(),
        user: parsed.user.clone(),
        auth_params: parsed.auth_params.clone(),
        peer_addr,
    };

    authorize_stream(config, &auth)?;

    let min_version = parse_srt_version(&config.min_peer_srt_version)
        .map_err(|err| format!("reject:invalid_config: min_peer_srt_version: {err}"))?;
    if let Some(peer) = peer_version {
        if !version_at_least(peer, min_version) {
            return Err(format!(
                "reject:peer_version_too_old: peer {version} < min {min_version_fmt}",
                version = format_srt_version(peer),
                min_version_fmt = format_srt_version(min_version),
            ));
        }
    } else if config.require_peer_version_extension {
        return Err("reject:peer_version_missing".to_string());
    }

    Ok(SrtClassifiedStream {
        mode,
        stream_key,
        auth,
    })
}
