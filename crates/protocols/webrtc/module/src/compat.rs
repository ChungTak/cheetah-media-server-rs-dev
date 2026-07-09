//! SMS-style request/response compatibility helpers.

use serde_json::Value;

/// Pull `appName`/`app` and `streamName`/`stream`/`recvStream` aliases out
/// of an SMS-style JSON body.
///
/// ZLM-style requests may also carry a `url` field of the form
/// `rtc://vhost/app/stream?...`. When the explicit `app` / `stream`
/// fields are missing, we fall back to parsing the URL via
/// [`parse_zlm_rtc_url`] and use its `app` / `stream` segments. The
/// caller can still override by setting the explicit fields.
pub fn extract_app_stream_aliases(body: &Value) -> (String, Option<String>) {
    let explicit_app = body
        .get("appName")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("app").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let explicit_stream = body
        .get("streamName")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("stream").and_then(|v| v.as_str()))
        .or_else(|| body.get("recvStream").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    if let (Some(app), Some(stream)) = (explicit_app.as_ref(), explicit_stream.as_ref()) {
        return (app.clone(), Some(stream.clone()));
    }

    // Fall back to parsing a ZLM-style `url` field.
    let url_parsed = body
        .get("url")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_zlm_rtc_url(s).ok());

    let app = explicit_app
        .or_else(|| url_parsed.as_ref().map(|p| p.app.clone()))
        .unwrap_or_else(|| "live".to_string());
    let stream = explicit_stream.or_else(|| url_parsed.as_ref().map(|p| p.stream.clone()));
    (app, stream)
}

/// Recognized ABL-style WHEP path patterns.
///
/// ABL clients use paths like `/rtc/v1/whep/` or `/rtc/v1/whep` (with or
/// without trailing slash) and the shorter `/whep` form. All of these
/// should map to the WHEP play handler. This function returns `true` when
/// the given *relative* path (as seen by the module's `handle` method)
/// matches one of the known WHEP alias patterns.
///
/// The canonical path `/whep` is included so callers can use a single
/// check for all WHEP-eligible paths.
pub fn is_abl_whep_path(path: &str) -> bool {
    let normalized = path.trim_end_matches('/');
    matches!(normalized, "/whep" | "/rtc/v1/whep")
}

/// Validate that the required `app` and `stream` query parameters are
/// present for an ABL-style WHEP request. Returns `Ok((app, stream))` on
/// success or an `Err` with a human-readable error message when either
/// parameter is missing.
///
/// When `app` is absent, it defaults to `"live"` (matching ABL behaviour).
/// `stream` is always required.
pub fn validate_abl_whep_query(query: Option<&str>) -> Result<(String, String), AblWhepQueryError> {
    let (app, stream) = extract_app_stream_from_query(query);
    let app = app.unwrap_or_else(|| "live".to_string());
    let stream = stream.ok_or(AblWhepQueryError::MissingStream)?;
    if stream.is_empty() {
        return Err(AblWhepQueryError::MissingStream);
    }
    Ok((app, stream))
}

/// Error returned when ABL-style WHEP query parameters are invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AblWhepQueryError {
    #[error("missing required query parameter: stream (or streamName)")]
    MissingStream,
}

/// OME-style WebRTC URL direction.
///
/// OvenMediaEngine uses `/App/Stream?direction=whip` for HTTP WHIP
/// ingest, `/App/Stream?direction=send` for WebSocket ingest, and an
/// absent direction for playback WebSocket URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmeDirection {
    /// Publish over OME's custom WebSocket signalling.
    Send,
    /// Publish over HTTP WHIP.
    Whip,
    /// Playback. OME WebSocket playback commonly omits the direction
    /// parameter, so this is the parser default.
    Play,
}

/// OME `transport` query compatibility mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmeTransportMode {
    /// UDP ICE candidates only.
    Udp,
    /// Direct ICE-TCP candidates only.
    Tcp,
    /// TURN relay candidates only.
    Relay,
    /// OME default: UDP plus direct TCP.
    UdpTcp,
    /// All available candidate families.
    All,
}

/// Parsed OME-compatible WebRTC request target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmeWebRtcRequest {
    pub app: String,
    pub stream: String,
    pub playlist: Option<String>,
    pub direction: OmeDirection,
    pub transport: OmeTransportMode,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OmeWebRtcUrlError {
    #[error("missing app segment")]
    MissingApp,
    #[error("missing stream segment")]
    MissingStream,
    #[error("too many path segments")]
    TooManyPathSegments,
    #[error("invalid direction `{0}`")]
    InvalidDirection(String),
    #[error("invalid transport `{0}`")]
    InvalidTransport(String),
}

/// Parse an OME-compatible WebRTC path and query pair.
///
/// The path is relative to the WebRTC module mount and uses the OME
/// shape `/<app>/<stream>[/<playlist>]`. Query parameters are decoded
/// with the same forgiving decoder used by the SMS/ZLM compatibility
/// layer.
pub fn parse_ome_webrtc_path_query(
    path: &str,
    query: Option<&str>,
) -> Result<OmeWebRtcRequest, OmeWebRtcUrlError> {
    parse_ome_webrtc_path_query_with_default_transport(path, query, OmeTransportMode::UdpTcp)
}

pub fn parse_ome_webrtc_path_query_with_default_transport(
    path: &str,
    query: Option<&str>,
    default_transport: OmeTransportMode,
) -> Result<OmeWebRtcRequest, OmeWebRtcUrlError> {
    let segments: Vec<String> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(url_decode_lossy)
        .collect();
    let (app, stream, playlist) = match segments.as_slice() {
        [] => return Err(OmeWebRtcUrlError::MissingApp),
        [_app] => return Err(OmeWebRtcUrlError::MissingStream),
        [app, stream] => (app.clone(), stream.clone(), None),
        [app, stream, playlist] => (app.clone(), stream.clone(), Some(playlist.clone())),
        _ => return Err(OmeWebRtcUrlError::TooManyPathSegments),
    };
    if app.is_empty() {
        return Err(OmeWebRtcUrlError::MissingApp);
    }
    if stream.is_empty() {
        return Err(OmeWebRtcUrlError::MissingStream);
    }

    let mut direction = OmeDirection::Play;
    let mut transport = default_transport;
    if let Some(q) = query {
        for kv in q.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = match kv.split_once('=') {
                Some(pair) => pair,
                None => (kv, ""),
            };
            let key = url_decode_lossy(k).to_ascii_lowercase();
            let value = url_decode_lossy(v);
            match key.as_str() {
                "direction" => direction = parse_ome_direction(&value)?,
                "transport" => transport = parse_ome_transport_mode(&value)?,
                _ => {}
            }
        }
    }

    Ok(OmeWebRtcRequest {
        app,
        stream,
        playlist,
        direction,
        transport,
    })
}

fn parse_ome_direction(input: &str) -> Result<OmeDirection, OmeWebRtcUrlError> {
    let s = input.trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "play" | "recv" | "receive" => Ok(OmeDirection::Play),
        "send" => Ok(OmeDirection::Send),
        "whip" => Ok(OmeDirection::Whip),
        other => Err(OmeWebRtcUrlError::InvalidDirection(other.to_string())),
    }
}

pub fn parse_ome_transport_mode(input: &str) -> Result<OmeTransportMode, OmeWebRtcUrlError> {
    let s = input.trim().to_ascii_lowercase();
    match s.as_str() {
        "" | "udptcp" | "udp_tcp" | "udp-tcp" => Ok(OmeTransportMode::UdpTcp),
        "udp" => Ok(OmeTransportMode::Udp),
        "tcp" => Ok(OmeTransportMode::Tcp),
        "relay" | "turn" => Ok(OmeTransportMode::Relay),
        "all" => Ok(OmeTransportMode::All),
        other => Err(OmeWebRtcUrlError::InvalidTransport(other.to_string())),
    }
}

/// Extract `app` and `stream` values from a query string (WHIP/WHEP path
/// alternatives).
pub fn extract_app_stream_from_query(query: Option<&str>) -> (Option<String>, Option<String>) {
    let q = match query {
        Some(q) => q,
        None => return (None, None),
    };
    let mut app: Option<String> = None;
    let mut stream: Option<String> = None;
    for kv in q.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            let v = url_decode_lossy(v);
            match k {
                "appName" | "app" => app = Some(v),
                "streamName" | "stream" => stream = Some(v),
                _ => {}
            }
        }
    }
    (app, stream)
}

/// Decode a URL-encoded string, replacing invalid UTF-8 with the
/// replacement character. Limited to a small subset of percent-encodings;
/// good enough for the query parameters we expect from WHIP/WHEP clients.
pub(crate) fn url_decode_lossy(input: &str) -> String {
    let mut buf: Vec<u8> = Vec::with_capacity(input.len());
    let mut bytes = input.bytes();
    while let Some(b) = bytes.next() {
        if b == b'+' {
            buf.push(b' ');
            continue;
        }
        if b == b'%' {
            let h = bytes.next();
            let l = bytes.next();
            if let (Some(h), Some(l)) = (h, l) {
                if let (Some(hi), Some(lo)) = (hex(h), hex(l)) {
                    buf.push((hi << 4) | lo);
                    continue;
                }
            }
            buf.push(b'%');
            continue;
        }
        buf.push(b);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_alias_pairs_from_json() {
        let body = serde_json::json!({
            "appName": "live",
            "streamName": "demo",
        });
        let (a, s) = extract_app_stream_aliases(&body);
        assert_eq!(a, "live");
        assert_eq!(s.as_deref(), Some("demo"));
    }

    #[test]
    fn falls_back_to_short_aliases() {
        let body = serde_json::json!({
            "app": "live",
            "stream": "demo",
        });
        let (a, s) = extract_app_stream_aliases(&body);
        assert_eq!(a, "live");
        assert_eq!(s.as_deref(), Some("demo"));
    }

    #[test]
    fn falls_back_to_default_app_when_missing() {
        let body = serde_json::json!({"stream": "demo"});
        let (a, _) = extract_app_stream_aliases(&body);
        assert_eq!(a, "live");
    }

    #[test]
    fn extracts_query_pairs() {
        let (a, s) = extract_app_stream_from_query(Some("appName=live&streamName=demo"));
        assert_eq!(a.as_deref(), Some("live"));
        assert_eq!(s.as_deref(), Some("demo"));
    }

    #[test]
    fn url_decode_handles_percent_encoded() {
        let (a, s) = extract_app_stream_from_query(Some("app=l%69ve&stream=d%65mo"));
        assert_eq!(a.as_deref(), Some("live"));
        assert_eq!(s.as_deref(), Some("demo"));
    }

    #[test]
    fn extracts_app_stream_from_zlm_url_field() {
        // ZLM-style request bodies often only carry a `url`. The
        // module is expected to extract `app` and `stream` from the
        // URL when explicit fields are missing.
        let body = serde_json::json!({"url": "rtc://example.com/live/demo"});
        let (a, s) = extract_app_stream_aliases(&body);
        assert_eq!(a, "live");
        assert_eq!(s.as_deref(), Some("demo"));
    }

    #[test]
    fn explicit_fields_take_precedence_over_zlm_url() {
        let body = serde_json::json!({
            "url": "rtc://example.com/live/demo",
            "app": "vod",
            "stream": "movie",
        });
        let (a, s) = extract_app_stream_aliases(&body);
        assert_eq!(a, "vod");
        assert_eq!(s.as_deref(), Some("movie"));
    }

    #[test]
    fn invalid_zlm_url_falls_back_to_default_app_with_no_stream() {
        let body = serde_json::json!({"url": "ftp://nope"});
        let (a, s) = extract_app_stream_aliases(&body);
        assert_eq!(a, "live");
        assert_eq!(s, None);
    }

    // --- ABL WHEP URL alias tests ---

    #[test]
    fn abl_whep_path_with_trailing_slash() {
        assert!(is_abl_whep_path("/rtc/v1/whep/"));
    }

    #[test]
    fn abl_whep_path_without_trailing_slash() {
        assert!(is_abl_whep_path("/rtc/v1/whep"));
    }

    #[test]
    fn abl_whep_short_path() {
        assert!(is_abl_whep_path("/whep"));
    }

    #[test]
    fn abl_whep_short_path_with_trailing_slash() {
        assert!(is_abl_whep_path("/whep/"));
    }

    #[test]
    fn abl_whep_rejects_unrelated_paths() {
        assert!(!is_abl_whep_path("/rtc/v1/whip"));
        assert!(!is_abl_whep_path("/rtc/v2/whep"));
        assert!(!is_abl_whep_path("/api/v1/rtc/whep"));
        assert!(!is_abl_whep_path("/publish"));
        assert!(!is_abl_whep_path("/play"));
    }

    #[test]
    fn abl_whep_url_alias_maps_to_play_request_with_query() {
        // Simulates: /rtc/v1/whep/?app=live&stream=camera01
        assert!(is_abl_whep_path("/rtc/v1/whep/"));
        let result = validate_abl_whep_query(Some("app=live&stream=camera01"));
        assert_eq!(result, Ok(("live".to_string(), "camera01".to_string())));
    }

    #[test]
    fn abl_whep_url_alias_no_trailing_slash_maps_to_play_request() {
        // Simulates: /rtc/v1/whep?app=live&stream=camera01
        assert!(is_abl_whep_path("/rtc/v1/whep"));
        let result = validate_abl_whep_query(Some("app=live&stream=camera01"));
        assert_eq!(result, Ok(("live".to_string(), "camera01".to_string())));
    }

    #[test]
    fn abl_whep_short_url_maps_to_play_request() {
        // Simulates: /whep?app=live&stream=camera01
        assert!(is_abl_whep_path("/whep"));
        let result = validate_abl_whep_query(Some("app=live&stream=camera01"));
        assert_eq!(result, Ok(("live".to_string(), "camera01".to_string())));
    }

    #[test]
    fn abl_whep_missing_stream_returns_error() {
        let result = validate_abl_whep_query(Some("app=live"));
        assert_eq!(result, Err(AblWhepQueryError::MissingStream));
    }

    #[test]
    fn abl_whep_missing_app_defaults_to_live() {
        let result = validate_abl_whep_query(Some("stream=camera01"));
        assert_eq!(result, Ok(("live".to_string(), "camera01".to_string())));
    }

    #[test]
    fn abl_whep_empty_query_returns_error() {
        let result = validate_abl_whep_query(None);
        assert_eq!(result, Err(AblWhepQueryError::MissingStream));
    }

    #[test]
    fn abl_whep_empty_stream_value_returns_error() {
        let result = validate_abl_whep_query(Some("app=live&stream="));
        assert_eq!(result, Err(AblWhepQueryError::MissingStream));
    }

    #[test]
    fn abl_whep_uses_stream_name_alias() {
        // ABL clients may use `streamName` instead of `stream`.
        let result = validate_abl_whep_query(Some("app=live&streamName=camera01"));
        assert_eq!(result, Ok(("live".to_string(), "camera01".to_string())));
    }

    #[test]
    fn abl_whep_uses_app_name_alias() {
        // ABL clients may use `appName` instead of `app`.
        let result = validate_abl_whep_query(Some("appName=myapp&stream=camera01"));
        assert_eq!(result, Ok(("myapp".to_string(), "camera01".to_string())));
    }

    #[test]
    fn ome_url_whip_direction_maps_path_app_stream() {
        let parsed = parse_ome_webrtc_path_query("/live/camera01", Some("direction=whip")).unwrap();
        assert_eq!(parsed.app, "live");
        assert_eq!(parsed.stream, "camera01");
        assert_eq!(parsed.playlist, None);
        assert_eq!(parsed.direction, OmeDirection::Whip);
        assert_eq!(parsed.transport, OmeTransportMode::UdpTcp);
    }

    #[test]
    fn ome_url_send_direction_maps_publish() {
        let parsed =
            parse_ome_webrtc_path_query("/live/camera01", Some("direction=send&transport=relay"))
                .unwrap();
        assert_eq!(parsed.direction, OmeDirection::Send);
        assert_eq!(parsed.transport, OmeTransportMode::Relay);
    }

    #[test]
    fn ome_url_default_direction_is_play_with_playlist() {
        let parsed = parse_ome_webrtc_path_query("/live/camera01/abr", None).unwrap();
        assert_eq!(parsed.app, "live");
        assert_eq!(parsed.stream, "camera01");
        assert_eq!(parsed.playlist.as_deref(), Some("abr"));
        assert_eq!(parsed.direction, OmeDirection::Play);
        assert_eq!(parsed.transport, OmeTransportMode::UdpTcp);
    }

    #[test]
    fn ome_transport_values_parse_case_insensitively() {
        let cases = [
            ("udp", OmeTransportMode::Udp),
            ("TCP", OmeTransportMode::Tcp),
            ("relay", OmeTransportMode::Relay),
            ("udptcp", OmeTransportMode::UdpTcp),
            ("all", OmeTransportMode::All),
        ];
        for (value, expected) in cases {
            let query = format!("direction=whip&transport={value}");
            let parsed = parse_ome_webrtc_path_query("/live/camera01", Some(&query)).unwrap();
            assert_eq!(parsed.transport, expected);
        }
    }

    #[test]
    fn ome_transport_unknown_rejected() {
        let err = parse_ome_webrtc_path_query(
            "/live/camera01",
            Some("direction=whip&transport=sideways"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            OmeWebRtcUrlError::InvalidTransport("sideways".to_string())
        );
    }

    #[test]
    fn ome_empty_app_or_stream_rejected() {
        let err = parse_ome_webrtc_path_query("/live/", Some("direction=whip")).unwrap_err();
        assert_eq!(err, OmeWebRtcUrlError::MissingStream);
    }
}

/// Parsed ZLMediaKit-style `rtc://` / `webrtc://` URL.
///
/// ZLM clients address streams via URLs of the shape:
///
/// ```text
/// rtc://vhost/app/stream?signaling_protocols=0
/// webrtc://signaling-host:port/app/stream?signaling_protocols=1&peer_room_id=room
/// rtcs://vhost/app/stream
/// ```
///
/// The parser is deliberately lenient: missing `vhost` falls back to
/// `__defaultVhost__` (matching ZLM behaviour), missing `app` falls
/// back to `live`, and unknown query parameters are surfaced via
/// [`ZlmRtcUrl::extra_params`] so the caller can decide whether to
/// reject or pass them through to driver / signalling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZlmRtcUrl {
    /// Scheme — `rtc`, `rtcs`, `webrtc`, or `webrtcs`.
    pub scheme: ZlmRtcScheme,
    /// Host as it appeared in the authority. For `rtc://` URLs this is
    /// usually the ZLM vhost; for `webrtc://` it's the signalling
    /// server. Drivers may map the value to a configured vhost via
    /// `extra_params` but the URL parser keeps the input verbatim.
    pub host: String,
    /// Optional port from the authority component.
    pub port: Option<u16>,
    /// `app` segment from the path (e.g. `live`).
    pub app: String,
    /// `stream` segment from the path. Required.
    pub stream: String,
    /// `signaling_protocols` query value: `0` for HTTP WHIP/WHEP, `1`
    /// for WebSocket P2P.
    pub signaling_protocols: u32,
    /// `peer_room_id` from the query (P2P only).
    pub peer_room_id: Option<String>,
    /// All other query parameters in the order they appeared.
    pub extra_params: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZlmRtcScheme {
    /// Plain RTC (HTTP signalling, UDP/TCP transport).
    Rtc,
    /// TLS RTC (HTTPS signalling).
    Rtcs,
    /// `webrtc://` — ZLM extended scheme used for WebSocket P2P
    /// signalling and matched by ZLMRTCClient.
    WebRtc,
    /// `webrtcs://` — TLS variant.
    WebRtcs,
}

impl ZlmRtcScheme {
    pub fn is_secure(self) -> bool {
        matches!(self, Self::Rtcs | Self::WebRtcs)
    }

    /// Map a scheme literal (case-insensitive) to a [`ZlmRtcScheme`].
    /// Named distinctly from `std::str::FromStr::from_str` to avoid
    /// the trait-confusion lint and to signal that this is a small
    /// inherent helper rather than a full URL parse.
    pub fn from_scheme_literal(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "rtc" => Self::Rtc,
            "rtcs" => Self::Rtcs,
            "webrtc" => Self::WebRtc,
            "webrtcs" => Self::WebRtcs,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZlmRtcUrlError {
    #[error("invalid scheme — expected rtc/rtcs/webrtc/webrtcs")]
    InvalidScheme,
    #[error("missing authority (host)")]
    MissingHost,
    #[error("invalid port `{port}`: {message}")]
    InvalidPort { port: String, message: String },
    #[error("missing path — URL must have at least /<app>/<stream>")]
    MissingPath,
    #[error("missing stream segment")]
    MissingStream,
    #[error("invalid signaling_protocols `{value}` — expected an integer")]
    InvalidSignalingProtocols { value: String },
}

/// Parse a ZLM-style `rtc://`/`webrtc://` URL.
pub fn parse_zlm_rtc_url(input: &str) -> Result<ZlmRtcUrl, ZlmRtcUrlError> {
    // Split scheme.
    let (scheme_str, rest) = match input.split_once("://") {
        Some(pair) => pair,
        None => return Err(ZlmRtcUrlError::InvalidScheme),
    };
    let scheme =
        ZlmRtcScheme::from_scheme_literal(scheme_str).ok_or(ZlmRtcUrlError::InvalidScheme)?;

    // Authority is everything before the first `/`. Path is the
    // remainder. Query (if any) is split off from the path.
    let (authority, path_and_query) = match rest.split_once('/') {
        Some((a, p)) => (a, p),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(ZlmRtcUrlError::MissingHost);
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 =
                p.parse()
                    .map_err(|e: std::num::ParseIntError| ZlmRtcUrlError::InvalidPort {
                        port: p.to_string(),
                        message: e.to_string(),
                    })?;
            (h.to_string(), Some(port))
        }
        None => (authority.to_string(), None),
    };
    // Reject `:port` style authorities where the host part is empty.
    // ZLM-style URLs always carry an explicit host or vhost; an empty
    // string would otherwise cascade into downstream code that expects
    // a real DNS name.
    if host.is_empty() {
        return Err(ZlmRtcUrlError::MissingHost);
    }

    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_and_query, None),
    };

    if path.is_empty() {
        // Trailing slash with no segments — `rtc://h/` — semantically
        // indicates a missing stream rather than a missing path. The
        // path-and-query split above only treats `path_and_query == ""`
        // (no leading slash at all) as truly path-less.
        if rest.contains('/') {
            return Err(ZlmRtcUrlError::MissingStream);
        }
        return Err(ZlmRtcUrlError::MissingPath);
    }

    // Path is `/app/stream` after the leading slash got consumed by
    // `split_once('/')` above. We require at least one segment that
    // becomes `stream`. ZLM accepts both `/<stream>` (in which case
    // app defaults to `live`) and `/<app>/<stream>`.
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (app, stream) = match segments.as_slice() {
        [stream] => ("live".to_string(), (*stream).to_string()),
        [app, stream] => ((*app).to_string(), (*stream).to_string()),
        // Three-segment paths `/<vhost>/<app>/<stream>` — uncommon
        // in ZLM URLs but Janus and some proxies do it. The first
        // segment is treated as the vhost and overrides the host
        // value at the caller's discretion via `extra_params`.
        [vhost, app, stream] => {
            let extra = vec![("vhost".to_string(), (*vhost).to_string())];
            return finish_parse(scheme, host, port, app, stream, query, extra);
        }
        [] => return Err(ZlmRtcUrlError::MissingStream),
        _ => {
            // 4+ segments: keep the last two as app/stream and the
            // rest as extra path noise. We surface the trailing
            // path via `extra_params` under `path_extra` so the
            // caller can reject if it cares.
            let stream = segments.last().unwrap();
            let app = segments[segments.len() - 2];
            let extra = vec![(
                "path_extra".to_string(),
                segments[..segments.len() - 2].join("/"),
            )];
            return finish_parse(scheme, host, port, app, stream, query, extra);
        }
    };

    finish_parse(scheme, host, port, &app, &stream, query, Vec::new())
}

#[allow(clippy::too_many_arguments)]
fn finish_parse(
    scheme: ZlmRtcScheme,
    host: String,
    port: Option<u16>,
    app: &str,
    stream: &str,
    query: Option<&str>,
    initial_extra: Vec<(String, String)>,
) -> Result<ZlmRtcUrl, ZlmRtcUrlError> {
    let mut signaling_protocols: u32 = 0;
    let mut peer_room_id: Option<String> = None;
    let mut extra_params: Vec<(String, String)> = initial_extra;

    if let Some(q) = query {
        for kv in q.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = match kv.split_once('=') {
                Some(pair) => pair,
                None => (kv, ""),
            };
            let key = k.to_string();
            let val = url_decode_lossy(v);
            match key.as_str() {
                "signaling_protocols" => {
                    signaling_protocols = val.parse::<u32>().map_err(|_| {
                        ZlmRtcUrlError::InvalidSignalingProtocols { value: val.clone() }
                    })?;
                }
                "peer_room_id" => peer_room_id = Some(val),
                _ => extra_params.push((key, val)),
            }
        }
    }

    Ok(ZlmRtcUrl {
        scheme,
        host,
        port,
        app: app.to_string(),
        stream: stream.to_string(),
        signaling_protocols,
        peer_room_id,
        extra_params,
    })
}

#[cfg(test)]
mod zlm_url_tests {
    use super::*;

    #[test]
    fn parses_basic_rtc_url() {
        let parsed = parse_zlm_rtc_url("rtc://example.com/live/demo").unwrap();
        assert_eq!(parsed.scheme, ZlmRtcScheme::Rtc);
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, None);
        assert_eq!(parsed.app, "live");
        assert_eq!(parsed.stream, "demo");
        assert_eq!(parsed.signaling_protocols, 0);
        assert!(parsed.peer_room_id.is_none());
        assert!(parsed.extra_params.is_empty());
        assert!(!parsed.scheme.is_secure());
    }

    #[test]
    fn parses_secure_scheme() {
        let parsed = parse_zlm_rtc_url("rtcs://example.com/live/demo").unwrap();
        assert_eq!(parsed.scheme, ZlmRtcScheme::Rtcs);
        assert!(parsed.scheme.is_secure());
    }

    #[test]
    fn parses_webrtc_p2p_url() {
        let parsed =
            parse_zlm_rtc_url("webrtc://signaling.example.com:8443/app/stream?signaling_protocols=1&peer_room_id=room42")
                .unwrap();
        assert_eq!(parsed.scheme, ZlmRtcScheme::WebRtc);
        assert_eq!(parsed.host, "signaling.example.com");
        assert_eq!(parsed.port, Some(8443));
        assert_eq!(parsed.app, "app");
        assert_eq!(parsed.stream, "stream");
        assert_eq!(parsed.signaling_protocols, 1);
        assert_eq!(parsed.peer_room_id.as_deref(), Some("room42"));
    }

    #[test]
    fn parses_short_path_with_default_app() {
        let parsed = parse_zlm_rtc_url("rtc://h.example/onlystream").unwrap();
        assert_eq!(parsed.app, "live");
        assert_eq!(parsed.stream, "onlystream");
    }

    #[test]
    fn parses_three_segment_path_as_vhost_app_stream() {
        let parsed = parse_zlm_rtc_url("rtc://h.example/vh/app/stream").unwrap();
        assert_eq!(parsed.app, "app");
        assert_eq!(parsed.stream, "stream");
        // The vhost surfaces via extra params so the caller can map
        // it to a configured vhost.
        assert!(parsed
            .extra_params
            .iter()
            .any(|(k, v)| k == "vhost" && v == "vh"));
    }

    #[test]
    fn surfaces_unknown_query_params_in_extras() {
        let parsed = parse_zlm_rtc_url("rtc://h.example/live/demo?foo=bar&secret=shh").unwrap();
        assert!(parsed
            .extra_params
            .iter()
            .any(|(k, v)| k == "foo" && v == "bar"));
        assert!(parsed
            .extra_params
            .iter()
            .any(|(k, v)| k == "secret" && v == "shh"));
    }

    #[test]
    fn rejects_invalid_scheme() {
        let err = parse_zlm_rtc_url("rtmp://h/live/demo").unwrap_err();
        assert_eq!(err, ZlmRtcUrlError::InvalidScheme);
    }

    #[test]
    fn rejects_missing_host() {
        let err = parse_zlm_rtc_url("rtc:///live/demo").unwrap_err();
        assert_eq!(err, ZlmRtcUrlError::MissingHost);
    }

    /// Regression: the fuzzer surfaced `rtc://:77/...` where the
    /// authority is `:77`. The parser previously accepted an empty
    /// host string with a numeric port. The check belongs after the
    /// `rsplit_once(':')` so the host emptiness is detected for both
    /// `rtc:///x/y` and `rtc://:77/x/y` shapes.
    #[test]
    fn rejects_empty_host_with_port() {
        let err = parse_zlm_rtc_url("rtc://:77/live/demo").unwrap_err();
        assert_eq!(err, ZlmRtcUrlError::MissingHost);
    }

    #[test]
    fn rejects_invalid_port() {
        let err = parse_zlm_rtc_url("rtc://h:not-a-port/live/demo").unwrap_err();
        assert!(matches!(err, ZlmRtcUrlError::InvalidPort { .. }));
    }

    #[test]
    fn rejects_missing_path() {
        let err = parse_zlm_rtc_url("rtc://h.example").unwrap_err();
        assert_eq!(err, ZlmRtcUrlError::MissingPath);
    }

    #[test]
    fn rejects_missing_stream() {
        let err = parse_zlm_rtc_url("rtc://h.example/").unwrap_err();
        assert_eq!(err, ZlmRtcUrlError::MissingStream);
    }

    #[test]
    fn rejects_non_integer_signaling_protocols() {
        let err = parse_zlm_rtc_url("rtc://h/live/demo?signaling_protocols=abc").unwrap_err();
        assert!(matches!(
            err,
            ZlmRtcUrlError::InvalidSignalingProtocols { .. }
        ));
    }

    #[test]
    fn percent_decodes_query_values() {
        let parsed = parse_zlm_rtc_url("rtc://h/live/demo?foo=bar%20baz").unwrap();
        assert!(parsed
            .extra_params
            .iter()
            .any(|(k, v)| k == "foo" && v == "bar baz"));
    }

    #[test]
    fn handles_long_path_by_keeping_last_two_segments() {
        let parsed = parse_zlm_rtc_url("rtc://h/proxy/route/app/stream").unwrap();
        assert_eq!(parsed.app, "app");
        assert_eq!(parsed.stream, "stream");
        assert!(parsed
            .extra_params
            .iter()
            .any(|(k, v)| k == "path_extra" && v == "proxy/route"));
    }
}

/// Extract candidate lines from a trickle-ICE SDP fragment body.
///
/// WHIP / WHEP PATCH bodies carry the
/// `application/trickle-ice-sdpfrag` content type. Each `a=candidate:`
/// line is converted to the `candidate:...` form expected by
/// `str0m::Candidate::from_sdp_string`. Other lines are ignored.
///
/// The function never panics; callers reject the request when the
/// returned vector is empty.
pub fn extract_trickle_candidates(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=candidate:") {
            // Defensive: empty `a=candidate:` lines are useless.
            if rest.is_empty() {
                continue;
            }
            out.push(format!("candidate:{rest}"));
        }
    }
    out
}

/// Detect an ICE-restart signal in a trickle-ICE SDP fragment.
///
/// When a WHIP / WHEP client wants to rotate ICE credentials it
/// usually sends a PATCH whose body contains both `a=ice-ufrag:` and
/// `a=ice-pwd:` lines (RFC 8839 §5.4 / WHIP spec §4.6). We mirror
/// `parse_trickle_creds_for_restart` from SMS by returning `Some` only
/// when both fields are present and non-empty so we never trigger an
/// ICE restart on a half-formed PATCH.
///
/// Returns `(ufrag, pwd)` from the body. Both strings are trimmed.
pub fn extract_trickle_ice_restart_creds(body: &str) -> Option<(String, String)> {
    let mut ufrag: Option<String> = None;
    let mut pwd: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=ice-ufrag:") {
            let value = rest.trim().to_string();
            if !value.is_empty() {
                ufrag = Some(value);
            }
        } else if let Some(rest) = line.strip_prefix("a=ice-pwd:") {
            let value = rest.trim().to_string();
            if !value.is_empty() {
                pwd = Some(value);
            }
        }
    }
    match (ufrag, pwd) {
        (Some(u), Some(p)) => Some((u, p)),
        _ => None,
    }
}

#[cfg(test)]
mod trickle_candidate_tests {
    use super::*;

    #[test]
    fn extracts_single_candidate() {
        let body = "a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n";
        let v = extract_trickle_candidates(body);
        assert_eq!(v.len(), 1);
        assert!(v[0].starts_with("candidate:0 1 UDP"));
    }

    #[test]
    fn extracts_multiple_candidates_in_order() {
        let body = "a=candidate:0 1 UDP 2122252543 1.1.1.1 50000 typ host\r\n\
                    a=candidate:1 1 UDP 1685987071 2.2.2.2 50001 typ srflx\r\n";
        let v = extract_trickle_candidates(body);
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("1.1.1.1"));
        assert!(v[1].contains("2.2.2.2"));
    }

    #[test]
    fn ignores_other_attribute_lines() {
        let body = "a=ice-ufrag:abc\r\na=ice-pwd:xyz\r\nv=0\r\n";
        let v = extract_trickle_candidates(body);
        assert!(v.is_empty());
    }

    #[test]
    fn ignores_empty_candidate_lines() {
        let body = "a=candidate:\r\na=candidate:0 1 UDP 2122252543 1.1.1.1 50000 typ host\r\n";
        let v = extract_trickle_candidates(body);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn does_not_panic_on_arbitrary_inputs() {
        // Property: parser must be total. We exercise a few edge
        // shapes manually; the property-test suite covers random
        // bytes.
        assert!(extract_trickle_candidates("").is_empty());
        assert!(extract_trickle_candidates("\0\0\0").is_empty());
        assert!(extract_trickle_candidates("a=candidate:").is_empty());
        let _ = extract_trickle_candidates(&"a=candidate:".repeat(100));
    }

    #[test]
    fn ice_restart_creds_present_when_both_lines_exist() {
        let body = "a=ice-ufrag:abc\r\na=ice-pwd:longpassword\r\n";
        let creds = extract_trickle_ice_restart_creds(body).expect("creds");
        assert_eq!(creds.0, "abc");
        assert_eq!(creds.1, "longpassword");
    }

    #[test]
    fn ice_restart_creds_absent_when_pwd_missing() {
        // ufrag without pwd is not a valid ICE restart trigger.
        let body = "a=ice-ufrag:abc\r\n";
        assert!(extract_trickle_ice_restart_creds(body).is_none());
    }

    #[test]
    fn ice_restart_creds_absent_when_ufrag_missing() {
        let body = "a=ice-pwd:longpassword\r\n";
        assert!(extract_trickle_ice_restart_creds(body).is_none());
    }

    #[test]
    fn ice_restart_creds_ignore_empty_values() {
        let body = "a=ice-ufrag:\r\na=ice-pwd:\r\n";
        assert!(extract_trickle_ice_restart_creds(body).is_none());
    }

    #[test]
    fn ice_restart_creds_ignored_inside_candidate_only_body() {
        let body = "a=candidate:0 1 UDP 2122252543 1.1.1.1 50000 typ host\r\n";
        assert!(extract_trickle_ice_restart_creds(body).is_none());
    }
}

/// Base64-decode a string (standard alphabet, padding tolerated).
///
/// Used by the DataChannel send HTTP endpoint to transport binary
/// payloads. We re-export the workspace base64 engine here so callers
/// keep a stable `crate::compat` import surface.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.decode(input.as_bytes())
}

#[cfg(test)]
mod base64_tests {
    use super::*;

    #[test]
    fn base64_decode_roundtrips_ascii_payload() {
        // "hello" base64 → "aGVsbG8="
        let decoded = base64_decode("aGVsbG8=").expect("decode");
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn base64_decode_rejects_invalid_input() {
        assert!(base64_decode("!!not_valid_base64!!").is_err());
    }
}

/// Rewrite `a=msid:` lines in an echo answer SDP to use a unique stream
/// id, preventing Chrome from silently discarding remote tracks whose
/// `msid` matches the local track's `msid`.
///
/// ZLMediaKit's `WebRtcEchoTest` performs the same rewrite: the answer
/// SDP's `msid` is replaced with a server-generated value so the browser
/// treats the echoed media as a distinct remote stream.
///
/// `session_label` should be a unique per-session string (e.g.
/// `"echo-<session_id>"`).
pub fn rewrite_echo_msid(sdp: &str, session_label: &str) -> String {
    let mut result = String::with_capacity(sdp.len());
    for line in sdp.lines() {
        let trimmed = line.trim();
        if let Some(after_prefix) = trimmed.strip_prefix("a=msid:") {
            // Format: a=msid:<stream-id> <track-id>
            // We replace the stream-id but keep the track-id.
            if let Some(space_pos) = after_prefix.find(' ') {
                let track_id = &after_prefix[space_pos + 1..];
                result.push_str(&format!("a=msid:{session_label} {track_id}"));
            } else {
                // No track-id, just replace the stream-id
                result.push_str(&format!("a=msid:{session_label}"));
            }
        } else {
            result.push_str(line);
        }
        result.push_str("\r\n");
    }
    result
}

#[cfg(test)]
mod echo_msid_tests {
    use super::*;

    #[test]
    fn rewrites_msid_stream_id_preserving_track_id() {
        let sdp = "v=0\r\na=msid:original-stream track-123\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n";
        let rewritten = rewrite_echo_msid(sdp, "echo-42");
        assert!(rewritten.contains("a=msid:echo-42 track-123\r\n"));
        assert!(!rewritten.contains("original-stream"));
    }

    #[test]
    fn rewrites_multiple_msid_lines() {
        let sdp = concat!(
            "v=0\r\n",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            "a=msid:stream1 audio-track\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=msid:stream1 video-track\r\n",
        );
        let rewritten = rewrite_echo_msid(sdp, "echo-99");
        assert!(rewritten.contains("a=msid:echo-99 audio-track\r\n"));
        assert!(rewritten.contains("a=msid:echo-99 video-track\r\n"));
        assert!(!rewritten.contains("stream1"));
    }

    #[test]
    fn preserves_non_msid_lines() {
        let sdp = "v=0\r\na=rtcp-mux\r\na=msid:s t\r\na=mid:0\r\n";
        let rewritten = rewrite_echo_msid(sdp, "echo-1");
        assert!(rewritten.contains("v=0\r\n"));
        assert!(rewritten.contains("a=rtcp-mux\r\n"));
        assert!(rewritten.contains("a=mid:0\r\n"));
    }

    #[test]
    fn handles_msid_without_track_id() {
        let sdp = "a=msid:stream-only\r\n";
        let rewritten = rewrite_echo_msid(sdp, "echo-5");
        assert!(rewritten.contains("a=msid:echo-5\r\n"));
    }
}
