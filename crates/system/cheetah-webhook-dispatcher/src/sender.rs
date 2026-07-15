use cheetah_codec::MonoTime;
use cheetah_runtime_api::RuntimeApi;
use futures::future::FutureExt;
use futures::select_biased;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use crate::security::{ParsedUrl, WebhookUrlVerdict};

/// HTTP request to be sent by a `WebhookSender`.
///
/// `WebhookSender` 要发送的 HTTP 请求。
#[derive(Debug, Clone)]
pub struct WebhookHttpRequest {
    pub verdict: WebhookUrlVerdict,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

/// Result of a webhook HTTP POST.
///
/// webhook HTTP POST 结果。
#[derive(Debug, Clone)]
pub struct WebhookResponse {
    pub status: u16,
    pub body: String,
    pub duration_ms: u64,
}

/// HTTP transport abstraction used by the dispatcher.
///
/// 分发器使用的 HTTP 传输抽象。
#[async_trait::async_trait]
pub trait WebhookSender: Send + Sync {
    async fn send(&self, request: WebhookHttpRequest) -> Result<WebhookResponse, WebhookSendError>;
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookSendError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("timeout")]
    Timeout,
    #[error("invalid response")]
    InvalidResponse,
    #[error("denied by policy: {0}")]
    Policy(String),
}

/// Runtime-backed HTTP/1.1 client.
///
/// 基于 Runtime 的 HTTP/1.1 客户端。
pub struct RuntimeHttpClient {
    runtime_api: Arc<dyn RuntimeApi>,
}

impl RuntimeHttpClient {
    pub fn new(runtime_api: Arc<dyn RuntimeApi>) -> Self {
        Self { runtime_api }
    }
}

#[async_trait::async_trait]
impl WebhookSender for RuntimeHttpClient {
    async fn send(&self, request: WebhookHttpRequest) -> Result<WebhookResponse, WebhookSendError> {
        let (addr, parsed) = match request.verdict {
            WebhookUrlVerdict::Allow(addr, parsed) => (addr, parsed),
            WebhookUrlVerdict::Deny(ref reason) => {
                return Err(WebhookSendError::Policy(reason.clone()));
            }
        };

        let start = std::time::Instant::now();
        let deadline = self
            .runtime_api
            .now()
            .as_micros()
            .saturating_add(request.timeout.as_micros() as u64);

        let mut stream = if parsed.scheme == "https" {
            self.runtime_api.connect_tls(addr, &parsed.host).await?
        } else {
            self.runtime_api.connect_tcp(addr)?
        };

        let req = build_request(&parsed, &request.headers, &request.body);

        let io = async move {
            stream.write_all(&req).await?;

            let mut buf = Vec::with_capacity(4096);
            let mut tmp = [0u8; 4096];
            let mut parse = HttpParser::new();

            loop {
                let n = stream.read(&mut tmp).await?;
                if n == 0 {
                    if let Some((status, body)) = parse.finish_with_eof(&buf) {
                        return Ok(WebhookResponse {
                            status,
                            body: String::from_utf8_lossy(body).to_string(),
                            duration_ms: start.elapsed().as_millis() as u64,
                        });
                    }
                    return Err(WebhookSendError::InvalidResponse);
                }

                buf.extend_from_slice(&tmp[..n]);

                if let Some((status, body)) = parse.try_parse(&buf) {
                    return Ok(WebhookResponse {
                        status,
                        body: String::from_utf8_lossy(body).to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                }

                if buf.len() > 1_048_576 {
                    return Err(WebhookSendError::InvalidResponse);
                }
            }
        };

        let mut timer = self
            .runtime_api
            .sleep_until(MonoTime::from_micros(deadline));
        let timeout = async move {
            timer.wait().await;
        };

        let mut io_fut = io.boxed().fuse();
        let mut timeout_fut = timeout.boxed().fuse();

        select_biased! {
            _ = timeout_fut => Err(WebhookSendError::Timeout),
            result = io_fut => result,
        }
    }
}

fn build_request(parsed: &ParsedUrl, headers: &HashMap<String, String>, body: &[u8]) -> Vec<u8> {
    let mut req = Vec::new();
    req.extend_from_slice(format!("POST {} HTTP/1.1\r\n", parsed.path_and_query).as_bytes());
    req.extend_from_slice(
        format!("Host: {}\r\n", host_header(&parsed.host, parsed.port)).as_bytes(),
    );
    req.extend_from_slice(b"Connection: close\r\n");
    for (k, v) in headers {
        req.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    req.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body);
    req
}

fn host_header(host: &str, port: u16) -> String {
    let default_port = if port == 443 { 443 } else { 80 };
    if port == default_port {
        host.to_string()
    } else {
        format!("{}:{}", host, port)
    }
}

struct HttpParser {
    header_end: Option<usize>,
    content_length: Option<usize>,
    chunked: bool,
    status: Option<u16>,
}

impl HttpParser {
    fn new() -> Self {
        Self {
            header_end: None,
            content_length: None,
            chunked: false,
            status: None,
        }
    }

    fn try_parse<'a>(&mut self, buf: &'a [u8]) -> Option<(u16, &'a [u8])> {
        let header_end = self.header_end(buf)?;
        if self.chunked {
            return None;
        }
        if let Some(len) = self.content_length {
            let body_start = header_end + 4;
            if buf.len() >= body_start + len {
                let status = self.status?;
                return Some((status, &buf[body_start..body_start + len]));
            }
            return None;
        }
        // No Content-Length: wait until EOF.
        None
    }

    fn finish_with_eof<'a>(&mut self, buf: &'a [u8]) -> Option<(u16, &'a [u8])> {
        let _ = self.header_end(buf)?;
        let header_end = self.header_end?;
        let status = self.status?;
        if self.chunked || self.content_length.is_some() {
            return self.try_parse(buf);
        }
        // No explicit body length: treat everything after headers as body.
        let body_start = header_end + 4;
        if body_start <= buf.len() {
            return Some((status, &buf[body_start..]));
        }
        None
    }

    fn header_end(&mut self, buf: &[u8]) -> Option<usize> {
        if let Some(end) = self.header_end {
            return Some(end);
        }
        if let Some(idx) = find_substring(buf, b"\r\n\r\n") {
            self.header_end = Some(idx);
            let header_block = &buf[..idx];
            self.parse_status(header_block)?;
            self.parse_headers(header_block);
            return Some(idx);
        }
        None
    }

    fn parse_status(&mut self, header_block: &[u8]) -> Option<()> {
        let line_end = find_substring(header_block, b"\r\n")?;
        let line = std::str::from_utf8(&header_block[..line_end]).ok()?;
        let mut parts = line.split_whitespace();
        parts.next()?; // HTTP/1.x
        let code = parts.next()?;
        self.status = Some(code.parse().ok()?);
        Some(())
    }

    fn parse_headers(&mut self, header_block: &[u8]) {
        for line in header_block.split(|&b| b == b'\n') {
            let line = if line.ends_with(b"\r") {
                &line[..line.len() - 1]
            } else {
                line
            };
            let mut it = line.splitn(2, |&b| b == b':');
            let name = match it.next() {
                Some(n) => n,
                None => continue,
            };
            let value = match it.next() {
                Some(v) => v,
                None => continue,
            };
            let name = std::str::from_utf8(name)
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let value = std::str::from_utf8(value).unwrap_or("").trim();
            if name == "content-length" {
                if let Ok(len) = value.parse::<usize>() {
                    self.content_length = Some(len);
                }
            } else if name == "transfer-encoding" && value.to_lowercase() == "chunked" {
                self.chunked = true;
            }
        }
    }
}

fn find_substring(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
