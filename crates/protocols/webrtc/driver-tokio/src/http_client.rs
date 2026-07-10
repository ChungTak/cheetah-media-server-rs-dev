//! Minimal WHIP/WHEP HTTP/1.1 client used by client pull/push jobs.
//!
//! Scope is intentionally narrow: WebRTC client jobs only need a small
//! subset of HTTP semantics — `POST` an SDP offer to receive an SDP
//! answer, and `DELETE` the resource on stop. We hand-roll an
//! HTTP/1.1 client over `tokio::net::TcpStream` (with optional
//! `tokio_rustls` for HTTPS) to avoid pulling in `hyper` or `reqwest`.
//!
//! Bounded behaviours:
//! * Connect/read/write timeouts are externally provided, capped to
//!   60 s by the caller.
//! * Response body is bounded by `max_response_bytes` to prevent runaway
//!   allocations from a hostile or buggy server.
//! * No HTTP redirects; the WHIP/WHEP `Location` header is surfaced to
//!   the caller via [`HttpClientResponse`] but not auto-followed.
//! * No proxy, no compression, no chunked-trailer parsing beyond the
//!   minimum needed for chunked bodies (servers that wrap an SDP answer
//!   in `Transfer-Encoding: chunked` are accepted).
//!
//! Security:
//! * HTTPS uses the same `webpki_roots`-backed `rustls::ClientConfig`
//!   as the rest of the workspace.
//! * SSRF protection lives in the caller: `WebRtcModuleConfig` exposes
//!   the host/scheme allowlists used before the URL is handed over.
//!
//! 客户端拉/推作业使用的最小 WHIP/WHEP HTTP/1.1 客户端。
//!
//! 范围故意缩小：WebRTC 客户端作业只需要 HTTP 语义的一小部分 - `POST` 和 SDP offer 来接收 SDP answer 和 `DELETE` 停止时的资源。
//! 我们在 `tokio::net::TcpStream` 上手动滚动 HTTP/1.1 客户端（对于 HTTPS 具有可选的 `tokio_rustls`），以避免引入 `hyper` 或 `reqwest`。
//!
//! 行为有界：
//! * 连接/读/写超时由外部提供，由调用者限制为 60 秒。
//! * 响应正文以 `max_response_bytes` 为界，以防止来自敌对或有问题的服务器的失控分配。
//! * 没有 HTTP 重定向；
//!   WHIP/WHEP `Location` 标头通过 [`HttpClientResponse`] 呈现给调用者，但不会自动跟随。
//! * 无代理，无压缩，无超出分块主体所需最低限度的分块尾部解析（接受将 SDP answer 包装在 `Transfer-Encoding: chunked` 中的服务器）。
//!
//! 安全：
//! * HTTPS 使用与工作空间的其余部分相同的 `webpki_roots` 支持的 `rustls::ClientConfig`。
//! * SSRF 保护存在于调用者中：`WebRtcModuleConfig` 公开在移交 URL 之前使用的主机/方案允许列表。

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
/// HTTP verbs used by the WHIP/WHEP client.
///
/// WHIP/WHEP 客户端使用的 HTTP 动词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// POST a resource or SDP offer/answer.
    ///
    /// POST 资源或 SDP offer/answer。
    Post,
    /// DELETE a resource, used for WHIP/WHEP session teardown.
    ///
    /// DELETE 资源，用于 WHIP/WHEP 会话拆卸。
    Delete,
    /// PATCH a resource when the server supports partial updates.
    ///
    /// PATCH 当服务器支持部分更新时的资源。
    Patch,
}

impl HttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Post => "POST",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
        }
    }
}
/// Errors returned by the HTTP client at connect, TLS, or response-parse time.
///
/// HTTP 客户端在连接、TLS 或响应解析时返回的错误。
#[derive(Debug, Error)]
pub enum HttpClientError {
    /// The request URL could not be parsed.
    ///
    /// 无法解析请求 URL。
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    /// The URL scheme is not in the caller allow list.
    ///
    /// URL 方案不在调用者允许列表中。
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    /// The remote host name could not be resolved.
    ///
    /// 无法解析远程主机名。
    #[error("network address resolution failed: {0}")]
    DnsFailure(String),
    /// The resolved address is private or otherwise blocked by policy.
    ///
    /// 解析的地址是私有的或被策略阻止。
    #[error("network address blocked by policy: {0}")]
    AddressBlocked(String),
    /// The TCP/TLS handshake exceeded the configured timeout.
    ///
    /// TCP/TLS 握手超出了配置的超时时间。
    #[error("connect timed out")]
    ConnectTimeout,
    /// A low-level socket or stream error occurred.
    ///
    /// 发生低级套接字或流错误。
    #[error("io error: {0}")]
    Io(String),
    /// The TLS handshake or certificate validation failed.
    ///
    /// TLS 握手或证书验证失败。
    #[error("tls error: {0}")]
    Tls(String),
    /// The server returned an HTTP response the parser could not consume.
    ///
    /// 服务器返回解析器无法使用的 HTTP 响应。
    #[error("invalid http response: {0}")]
    BadResponse(String),
    /// The response body exceeded the configured maximum size.
    ///
    /// 响应正文超出了配置的最大大小。
    #[error("response body exceeds {0} bytes")]
    BodyTooLarge(usize),
    /// The complete request/response exchange exceeded the timeout.
    ///
    /// 完整的请求/响应交换超出了超时时间。
    #[error("request timed out")]
    RequestTimeout,
}

impl From<io::Error> for HttpClientError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
/// Parsed HTTP response returned by the WHIP/WHEP client.
///
/// 已解析的 WHIP/WHEP 客户端返回的 HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpClientResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl HttpClientResponse {
    /// Look up a response header case-insensitively.
    ///
    /// 查找响应标头时不区分大小写。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Configuration for an outbound HTTP request.
///
/// 出站 HTTP 请求的配置。
#[derive(Debug, Clone)]
pub struct HttpClientRequest {
    pub url: String,
    pub method: HttpMethod,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub timeout: Duration,
    pub max_response_bytes: usize,
    pub allow_private_ips: bool,
    pub allowed_schemes: Vec<&'static str>,
}

impl HttpClientRequest {
    /// Build a POST request carrying an SDP body with WHIP/WHEP content types.
    ///
    /// 构建一个 POST 请求，携带 SDP 主体和 WHIP/WHEP 内容类型。
    pub fn new_post_sdp(url: impl Into<String>, sdp: impl Into<Bytes>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Post,
            headers: vec![
                ("content-type".into(), "application/sdp".into()),
                ("accept".into(), "application/sdp".into()),
            ],
            body: sdp.into(),
            timeout: Duration::from_secs(10),
            max_response_bytes: 64 * 1024,
            allow_private_ips: false,
            allowed_schemes: vec!["http", "https"],
        }
    }

    /// Build a DELETE request with no body and a short timeout.
    ///
    /// 构建一个没有正文且超时时间很短的 DELETE 请求。
    pub fn new_delete(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Delete,
            headers: Vec::new(),
            body: Bytes::new(),
            timeout: Duration::from_secs(5),
            max_response_bytes: 4 * 1024,
            allow_private_ips: false,
            allowed_schemes: vec!["http", "https"],
        }
    }
}

/// Reusable HTTP client. Holds a shared rustls config so we do not
/// rebuild the trust store per request.
///
/// 可重复使用的 HTTP 客户端。
/// 保存共享的 rustls 配置，因此我们不会根据请求重建信任存储。
#[derive(Clone)]
pub struct WhipWhepHttpClient {
    tls_config: Arc<ClientConfig>,
}

impl WhipWhepHttpClient {
    /// Create a client with a rustls trust store and process-default crypto provider.
    ///
    /// 创建一个具有 rustls 信任存储和进程默认加密提供程序的客户端。
    pub fn new() -> Self {
        // Install the process-default rustls crypto provider lazily.
        // Production deployments call this from `main.rs` before any
        // TLS work; tests construct multiple clients per process and
        // we don't want to require manual setup for every test.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Self {
            tls_config: Arc::new(config),
        }
    }

    /// Send a single request and return the parsed response.
    ///
    /// 发送单个请求并返回解析后的响应。
    pub async fn send(
        &self,
        req: HttpClientRequest,
    ) -> Result<HttpClientResponse, HttpClientError> {
        let parsed = ParsedUrl::parse(&req.url)?;
        if !req
            .allowed_schemes
            .iter()
            .any(|s| s == &parsed.scheme.as_str())
        {
            return Err(HttpClientError::UnsupportedScheme(parsed.scheme.into()));
        }

        let timeout = req.timeout.min(Duration::from_secs(60));

        let send_future = async {
            let stream = self.connect(&parsed, &req).await?;
            let response = self.send_over_stream(stream, &parsed, &req).await?;
            Ok(response)
        };

        match tokio::time::timeout(timeout, send_future).await {
            Ok(res) => res,
            Err(_) => Err(HttpClientError::RequestTimeout),
        }
    }

    async fn connect(
        &self,
        url: &ParsedUrl,
        req: &HttpClientRequest,
    ) -> Result<TransportStream, HttpClientError> {
        let host = url.host.clone();
        let port = url.effective_port();
        let addrs = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| HttpClientError::DnsFailure(e.to_string()))?
            .collect::<Vec<_>>();
        if addrs.is_empty() {
            return Err(HttpClientError::DnsFailure(format!("no addrs for {host}")));
        }
        if !req.allow_private_ips {
            for addr in &addrs {
                if is_private_ip(addr.ip()) {
                    return Err(HttpClientError::AddressBlocked(format!(
                        "{} resolves to private/loopback ip {}",
                        host,
                        addr.ip()
                    )));
                }
            }
        }
        let target = addrs[0];
        let tcp = TcpStream::connect(target)
            .await
            .map_err(|e| HttpClientError::Io(e.to_string()))?;
        if url.scheme.is_https() {
            let server_name = ServerName::try_from(host.clone())
                .map_err(|e| HttpClientError::Tls(format!("invalid sni {host}: {e}")))?;
            let connector = TlsConnector::from(self.tls_config.clone());
            let tls = connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| HttpClientError::Tls(e.to_string()))?;
            Ok(TransportStream::Tls(Box::new(tls)))
        } else {
            Ok(TransportStream::Plain(tcp))
        }
    }

    async fn send_over_stream(
        &self,
        mut stream: TransportStream,
        url: &ParsedUrl,
        req: &HttpClientRequest,
    ) -> Result<HttpClientResponse, HttpClientError> {
        // Build the request line + headers.
        let mut head = String::with_capacity(256 + req.body.len());
        head.push_str(req.method.as_str());
        head.push(' ');
        head.push_str(&url.request_target());
        head.push_str(" HTTP/1.1\r\n");
        head.push_str("host: ");
        head.push_str(&url.host_header());
        head.push_str("\r\n");
        head.push_str("user-agent: cheetah-webrtc/0.1\r\n");
        head.push_str("connection: close\r\n");
        head.push_str("accept-encoding: identity\r\n");
        let mut have_content_length = false;
        let mut have_content_type = false;
        for (k, v) in &req.headers {
            let lower = k.to_ascii_lowercase();
            if lower == "host"
                || lower == "user-agent"
                || lower == "connection"
                || lower == "accept-encoding"
            {
                continue;
            }
            if lower == "content-length" {
                have_content_length = true;
            }
            if lower == "content-type" {
                have_content_type = true;
            }
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        if !have_content_length {
            head.push_str(&format!("content-length: {}\r\n", req.body.len()));
        }
        if matches!(req.method, HttpMethod::Post | HttpMethod::Patch) && !have_content_type {
            head.push_str("content-type: application/sdp\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        if !req.body.is_empty() {
            stream.write_all(&req.body).await?;
        }
        stream.flush().await?;

        // Read the response into a bounded buffer.
        let mut buf = Vec::with_capacity(2048);
        let mut tmp = [0u8; 4096];
        loop {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            if buf.len() + n > req.max_response_bytes + 4096 {
                return Err(HttpClientError::BodyTooLarge(req.max_response_bytes));
            }
            buf.extend_from_slice(&tmp[..n]);
            // Heuristic: if the response is complete (we have a full
            // body framed by content-length and the buffer holds it),
            // break early.
            if let Some(pair) = parse_complete_response(&buf, req.max_response_bytes)? {
                return Ok(pair);
            }
        }
        match parse_complete_response(&buf, req.max_response_bytes)? {
            Some(resp) => Ok(resp),
            None => Err(HttpClientError::BadResponse(
                "incomplete response (connection closed)".into(),
            )),
        }
    }
}

impl Default for WhipWhepHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

enum TransportStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl TransportStream {
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            TransportStream::Plain(s) => s.write_all(buf).await,
            TransportStream::Tls(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        match self {
            TransportStream::Plain(s) => s.flush().await,
            TransportStream::Tls(s) => s.flush().await,
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            TransportStream::Plain(s) => s.read(buf).await,
            TransportStream::Tls(s) => s.read(buf).await,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpScheme {
    Http,
    Https,
}

impl HttpScheme {
    fn as_str(self) -> &'static str {
        match self {
            HttpScheme::Http => "http",
            HttpScheme::Https => "https",
        }
    }
    fn is_https(self) -> bool {
        matches!(self, HttpScheme::Https)
    }
}

impl From<HttpScheme> for String {
    fn from(value: HttpScheme) -> Self {
        value.as_str().to_string()
    }
}

#[derive(Debug, Clone)]
struct ParsedUrl {
    scheme: HttpScheme,
    host: String,
    port: Option<u16>,
    path_and_query: String,
}

impl ParsedUrl {
    fn parse(input: &str) -> Result<Self, HttpClientError> {
        let (scheme, rest) = if let Some(rest) = input.strip_prefix("https://") {
            (HttpScheme::Https, rest)
        } else if let Some(rest) = input.strip_prefix("http://") {
            (HttpScheme::Http, rest)
        } else {
            return Err(HttpClientError::InvalidUrl(format!(
                "missing http(s) scheme in {input}"
            )));
        };
        // Authority ends at the first `/`, `?`, or `#` after the
        // scheme. Path and query MUST NOT be scanned for `@` because
        // application-level URLs may carry `@` in query parameters
        // (e.g. `?email=user@host.com`).
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let path = if authority_end == rest.len() {
            "/"
        } else {
            &rest[authority_end..]
        };
        if authority.is_empty() {
            return Err(HttpClientError::InvalidUrl(format!(
                "empty authority in {input}"
            )));
        }
        // Reject userinfo (`user:pass@host`) — we don't support
        // basic-auth in URLs to avoid accidental credential leakage.
        // Scoped to the authority so query strings containing `@`
        // are not falsely rejected.
        if authority.contains('@') {
            return Err(HttpClientError::InvalidUrl(
                "userinfo in URL is not supported".into(),
            ));
        }
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            // IPv6 bracketed literal: `[::1]` or `[::1]:443`.
            let (literal, after) = rest
                .split_once(']')
                .ok_or_else(|| HttpClientError::InvalidUrl(format!("unclosed `[` in {input}")))?;
            let port = match after {
                "" => None,
                rest => match rest.strip_prefix(':') {
                    Some(p) => Some(p.parse::<u16>().map_err(|_| {
                        HttpClientError::InvalidUrl(format!("bad port `{p}` in {input}"))
                    })?),
                    None => {
                        return Err(HttpClientError::InvalidUrl(format!(
                            "trailing data after IPv6 host in {input}"
                        )))
                    }
                },
            };
            (literal.to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => {
                    let port: u16 = p.parse().map_err(|_| {
                        HttpClientError::InvalidUrl(format!("bad port `{p}` in {input}"))
                    })?;
                    (h.to_string(), Some(port))
                }
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(HttpClientError::InvalidUrl(format!(
                "empty host in {input}"
            )));
        }
        Ok(Self {
            scheme,
            host,
            port,
            path_and_query: path.to_string(),
        })
    }

    fn effective_port(&self) -> u16 {
        self.port
            .unwrap_or(if self.scheme.is_https() { 443 } else { 80 })
    }

    fn request_target(&self) -> String {
        if self.path_and_query.is_empty() {
            "/".to_string()
        } else {
            self.path_and_query.clone()
        }
    }

    fn host_header(&self) -> String {
        match self.port {
            Some(p) if !is_default_port(self.scheme, p) => {
                if self.host.contains(':') {
                    format!("[{}]:{}", self.host, p)
                } else {
                    format!("{}:{}", self.host, p)
                }
            }
            _ => {
                if self.host.contains(':') {
                    format!("[{}]", self.host)
                } else {
                    self.host.clone()
                }
            }
        }
    }
}

fn is_default_port(scheme: HttpScheme, port: u16) -> bool {
    match scheme {
        HttpScheme::Http => port == 80,
        HttpScheme::Https => port == 443,
    }
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.segments()[0] == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

fn parse_complete_response(
    buf: &[u8],
    max_body: usize,
) -> Result<Option<HttpClientResponse>, HttpClientError> {
    // Find header/body split.
    let header_end = match find_double_crlf(buf) {
        Some(idx) => idx,
        None => return Ok(None),
    };
    let header_bytes = &buf[..header_end];
    let body_start = header_end + 4;

    let mut lines = header_bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty());
    let status_line = lines
        .next()
        .ok_or_else(|| HttpClientError::BadResponse("missing status line".into()))?;
    let status_line = std::str::from_utf8(status_line)
        .map_err(|_| HttpClientError::BadResponse("non-utf8 status line".into()))?
        .trim_end_matches('\r');
    let status_parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if status_parts.len() < 2 || !status_parts[0].starts_with("HTTP/") {
        return Err(HttpClientError::BadResponse(format!(
            "malformed status line: {status_line:?}"
        )));
    }
    let status: u16 = status_parts[1]
        .parse()
        .map_err(|_| HttpClientError::BadResponse("bad status code".into()))?;

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let line = std::str::from_utf8(line)
            .map_err(|_| HttpClientError::BadResponse("non-utf8 header line".into()))?
            .trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (name, value) = match line.split_once(':') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => continue,
        };
        let name_lower = name.to_ascii_lowercase();
        if name_lower == "content-length" {
            content_length = value.parse().ok();
        } else if name_lower == "transfer-encoding" && value.eq_ignore_ascii_case("chunked") {
            chunked = true;
        }
        headers.push((name, value));
    }

    let body_remaining = &buf[body_start..];
    let body = if chunked {
        match parse_chunked_body(body_remaining, max_body)? {
            Some(b) => b,
            None => return Ok(None),
        }
    } else if let Some(len) = content_length {
        if len > max_body {
            return Err(HttpClientError::BodyTooLarge(max_body));
        }
        if body_remaining.len() < len {
            return Ok(None);
        }
        Bytes::copy_from_slice(&body_remaining[..len])
    } else {
        // No content-length, no chunked: body ends at connection close.
        // We can only return once the caller signals EOF; return None
        // here so the read loop continues.
        return Ok(None);
    };

    Ok(Some(HttpClientResponse {
        status,
        headers,
        body,
    }))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    (0..buf.len().saturating_sub(3)).find(|&i| &buf[i..i + 4] == b"\r\n\r\n")
}

fn parse_chunked_body(buf: &[u8], max: usize) -> Result<Option<Bytes>, HttpClientError> {
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < buf.len() {
        // Find chunk size line.
        let line_end = match find_crlf(&buf[idx..]) {
            Some(p) => idx + p,
            None => return Ok(None),
        };
        let size_line = std::str::from_utf8(&buf[idx..line_end])
            .map_err(|_| HttpClientError::BadResponse("non-utf8 chunk size".into()))?;
        let size_str = size_line.split(';').next().unwrap_or(size_line).trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| HttpClientError::BadResponse(format!("bad chunk size {size_str:?}")))?;
        idx = line_end + 2;
        if size == 0 {
            // Skip optional trailers up to final CRLF.
            return Ok(Some(Bytes::from(out)));
        }
        if out.len() + size > max {
            return Err(HttpClientError::BodyTooLarge(max));
        }
        if idx + size + 2 > buf.len() {
            return Ok(None);
        }
        out.extend_from_slice(&buf[idx..idx + size]);
        idx += size;
        if &buf[idx..idx + 2] != b"\r\n" {
            return Err(HttpClientError::BadResponse(
                "missing CRLF after chunk body".into(),
            ));
        }
        idx += 2;
    }
    Ok(None)
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    (0..buf.len().saturating_sub(1)).find(|&i| buf[i] == b'\r' && buf[i + 1] == b'\n')
}

/// Result of [`fuzz_parse_url_for_testing`], exposed for the fuzz
/// harness only.
///
/// The fields mirror the private `ParsedUrl` but we deliberately do
/// not expose the internal scheme enum: fuzzers should treat this as
/// an opaque structure whose only contract is that the parser never
/// panics on arbitrary inputs.
///
/// [`fuzz_parse_url_for_testing`] 的结果，仅针对模糊线束公开。
///
/// 这些字段镜像私有 `ParsedUrl` 但我们故意不公开内部方案枚举：模糊器应该将其视为不透明的结构，其唯一的契约是解析器永远不会对任意输入感到恐慌。
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ParsedUrlForTesting {
    pub scheme_https: bool,
    pub host: String,
    pub port: Option<u16>,
    pub effective_port: u16,
    pub path_and_query: String,
}

/// Public wrapper around the internal URL parser, intended for the
/// `cheetah-webrtc-fuzz::fuzz_url_parse` target. **Not** part of the
/// stable API; production callers should use `WhipWhepHttpClient`.
///
/// 围绕内部 URL 解析器的公共包装器，用于 `cheetah-webrtc-fuzz::fuzz_url_parse` 目标。
/// **不是**稳定 API 的一部分；
/// 生产调用者应使用 `WhipWhepHttpClient`。
#[doc(hidden)]
pub fn fuzz_parse_url_for_testing(input: &str) -> Result<ParsedUrlForTesting, HttpClientError> {
    let parsed = ParsedUrl::parse(input)?;
    Ok(ParsedUrlForTesting {
        scheme_https: parsed.scheme.is_https(),
        host: parsed.host.clone(),
        port: parsed.port,
        effective_port: parsed.effective_port(),
        path_and_query: parsed.path_and_query.clone(),
    })
}

/// Public wrapper around the internal HTTP response parser, intended
/// for the `cheetah-webrtc-fuzz::fuzz_http_response` target. **Not**
/// part of the stable API.
///
/// 围绕内部 HTTP 响应解析器的公共包装器，用于 `cheetah-webrtc-fuzz::fuzz_http_response` 目标。
/// **不是**稳定 API 的一部分。
#[doc(hidden)]
pub fn fuzz_parse_http_response_for_testing(
    buf: &[u8],
    max_body: usize,
) -> Result<Option<HttpClientResponse>, HttpClientError> {
    parse_complete_response(buf, max_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_http_default_port() {
        let p = ParsedUrl::parse("http://example.com/whip/live/demo").unwrap();
        assert_eq!(p.scheme, HttpScheme::Http);
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, None);
        assert_eq!(p.effective_port(), 80);
        assert_eq!(p.request_target(), "/whip/live/demo");
        assert_eq!(p.host_header(), "example.com");
    }

    #[test]
    fn parse_url_https_with_port() {
        let p = ParsedUrl::parse("https://signaling.example.com:8443/whep").unwrap();
        assert_eq!(p.scheme, HttpScheme::Https);
        assert_eq!(p.port, Some(8443));
        assert_eq!(p.effective_port(), 8443);
        assert_eq!(p.host_header(), "signaling.example.com:8443");
    }

    #[test]
    fn parse_url_strips_default_port_in_host_header() {
        let p = ParsedUrl::parse("https://example.com:443/x").unwrap();
        assert_eq!(p.host_header(), "example.com");
    }

    #[test]
    fn parse_url_rejects_userinfo() {
        let err = ParsedUrl::parse("https://user:pass@example.com/x").unwrap_err();
        assert!(matches!(err, HttpClientError::InvalidUrl(_)));
    }

    #[test]
    fn parse_url_allows_at_in_query_and_path() {
        // `@` in path or query must be accepted; we only forbid it in
        // the authority (RFC 3986 userinfo).
        let p = ParsedUrl::parse("https://example.com/whip?email=user@host.com").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.request_target(), "/whip?email=user@host.com");

        let p2 = ParsedUrl::parse("http://example.com/p/u@ser/x").unwrap();
        assert_eq!(p2.request_target(), "/p/u@ser/x");
    }

    #[test]
    fn parse_url_rejects_unknown_scheme() {
        let err = ParsedUrl::parse("ftp://example.com/").unwrap_err();
        assert!(matches!(err, HttpClientError::InvalidUrl(_)));
    }

    #[test]
    fn parse_url_handles_ipv6_literal() {
        let p = ParsedUrl::parse("http://[::1]:8080/x").unwrap();
        assert_eq!(p.host, "::1");
        assert_eq!(p.port, Some(8080));
        assert_eq!(p.host_header(), "[::1]:8080");
    }

    #[test]
    fn private_ip_blocking_is_strict_by_default() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
        assert!(is_private_ip("::1".parse().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn parse_complete_response_handles_content_length() {
        let raw = b"HTTP/1.1 201 Created\r\ncontent-length: 5\r\nlocation: /x\r\n\r\nv=0\r\n";
        let resp = parse_complete_response(raw, 64 * 1024)
            .expect("parse")
            .expect("complete");
        assert_eq!(resp.status, 201);
        assert_eq!(resp.body.as_ref(), b"v=0\r\n");
        assert_eq!(resp.header("location"), Some("/x"));
    }

    #[test]
    fn parse_complete_response_handles_chunked() {
        let raw = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let resp = parse_complete_response(raw, 64 * 1024)
            .expect("parse")
            .expect("complete");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_ref(), b"hello");
    }

    #[test]
    fn parse_complete_response_partial_returns_none() {
        let raw = b"HTTP/1.1 200 OK\r\n";
        let resp = parse_complete_response(raw, 64 * 1024).expect("parse");
        assert!(resp.is_none());
    }

    #[test]
    fn parse_complete_response_rejects_oversized_body() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n";
        let err = parse_complete_response(raw, 8).unwrap_err();
        assert!(matches!(err, HttpClientError::BodyTooLarge(_)));
    }
}
