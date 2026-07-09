use bytes::Bytes;

const HTTP_TUNNEL_GET_HEADERS: &str =
    "Accept: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n";
const HTTP_TUNNEL_POST_HEADERS: &str =
    "Content-Type: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n";

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

pub(super) fn build_http_tunnel_get_request(path: &str, session_cookie: &str) -> Bytes {
    Bytes::from(format!(
        "GET {path} HTTP/1.0\r\nx-sessioncookie: {session_cookie}\r\n{HTTP_TUNNEL_GET_HEADERS}\r\n"
    ))
}

pub(super) fn build_http_tunnel_post_request(path: &str, session_cookie: &str) -> Bytes {
    Bytes::from(format!(
        "POST {path} HTTP/1.0\r\nx-sessioncookie: {session_cookie}\r\n{HTTP_TUNNEL_POST_HEADERS}\r\n"
    ))
}
