use bytes::Bytes;

/// HTTP headers used for the GET half of the tunnel.
///
/// The client opens the GET channel first and expects the server to stream
/// RTSP responses and interleaved RTP frames through it.
///
/// 隧道 GET 半侧使用的 HTTP 头。
///
/// 客户端首先打开 GET 通道，并期望服务器通过该通道流式返回 RTSP 响应与交错 RTP 帧。
const HTTP_TUNNEL_GET_HEADERS: &str =
    "Accept: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n";

/// HTTP headers used for the POST half of the tunnel.
///
/// The client encodes RTSP requests as Base64 and writes them into the POST body.
///
/// 隧道 POST 半侧使用的 HTTP 头。
///
/// 客户端将 RTSP 请求进行 Base64 编码后写入 POST 请求体。
const HTTP_TUNNEL_POST_HEADERS: &str =
    "Content-Type: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n";

/// Normalize a tunnel path so it always starts with a leading slash.
///
/// Empty or root paths are collapsed to `/` to avoid malformed request lines.
///
/// 规范化隧道路径，使其始终以斜杠开头。
///
/// 空路径或根路径被折叠为 `/`，避免生成非法的请求行。
pub(super) fn normalize_http_tunnel_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Build the GET request used to open the downstream half of the HTTP tunnel.
///
/// 构建用于打开 HTTP 隧道下行半侧的 GET 请求。
pub(super) fn build_http_tunnel_get_request(path: &str, session_cookie: &str) -> Bytes {
    Bytes::from(format!(
        "GET {path} HTTP/1.0\r\nx-sessioncookie: {session_cookie}\r\n{HTTP_TUNNEL_GET_HEADERS}\r\n"
    ))
}

/// Build the POST request used to open the upstream half of the HTTP tunnel.
///
/// 构建用于打开 HTTP 隧道上行半侧的 POST 请求。
pub(super) fn build_http_tunnel_post_request(path: &str, session_cookie: &str) -> Bytes {
    Bytes::from(format!(
        "POST {path} HTTP/1.0\r\nx-sessioncookie: {session_cookie}\r\n{HTTP_TUNNEL_POST_HEADERS}\r\n"
    ))
}
