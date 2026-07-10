use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::{Bytes, BytesMut};
use cheetah_rtsp_core::{
    encode_interleaved_frame, encode_rtsp_request, parse_interleaved_frame, RtspMessageLimits,
    RtspResponseDecoder,
};
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use super::command::RtspClientCommand;
use super::http_tunnel::{build_http_tunnel_get_request, build_http_tunnel_post_request};
use super::{RtspClientConfig, RtspClientEvent};

/// Context required to open and run an HTTP-tunnelled RTSP client connection.
///
/// RTSP-over-HTTP uses two parallel TCP streams: a GET stream for downstream data and
/// a POST stream for upstream data. Both streams share the same session cookie.
///
/// 打开并运行 HTTP 隧道 RTSP 客户端连接所需的上下文。
///
/// RTSP-over-HTTP 使用两条并行 TCP 流：GET 流用于下行数据，POST 流用于上行数据。
/// 两条流共享同一个会话 cookie。
pub(super) struct HttpTunnelClientContext {
    pub(super) peer: SocketAddr,
    pub(super) path: String,
    pub(super) session_cookie: String,
    pub(super) event_tx: mpsc::Sender<RtspClientEvent>,
    pub(super) cancel: CancellationToken,
    pub(super) config: RtspClientConfig,
}

/// Run the RTSP client over a plain TCP or TLS stream.
///
/// The connection task multiplexes three sources: a command channel for outbound requests,
/// a stream read for inbound responses and interleaved frames, and a cancellation token.
/// Writes are queued and flushed one at a time; when the queue is non-empty, `try_recv`
/// and a zero-timeout read are used to keep the pipeline moving without blocking.
///
/// 在普通 TCP 或 TLS 流上运行 RTSP 客户端。
///
/// 连接任务复用三个来源：出站请求的命令通道、入站响应与交错帧的流读取以及取消令牌。
/// 写入操作排队并逐个刷新；当队列非空时，使用 `try_recv` 和零超时读取保持流水线
/// 运转而不阻塞。
pub(super) async fn run_tcp_client_connection(
    mut stream: Box<dyn AsyncTcpStream>,
    mut cmd_rx: mpsc::Receiver<RtspClientCommand>,
    event_tx: mpsc::Sender<RtspClientEvent>,
    cancel: CancellationToken,
    config: RtspClientConfig,
) {
    let mut pending_writes = VecDeque::<Bytes>::new();
    let max_write_queue = config.write_queue_capacity.max(8);
    let mut read_buf = vec![0_u8; config.read_buffer_size.max(1024)];
    let mut close_requested = false;
    let mut parse_buf = BytesMut::new();
    let response_limits = RtspMessageLimits::default();
    let mut response_decoder = RtspResponseDecoder::with_limits(response_limits.clone());

    let reason = loop {
        if close_requested && pending_writes.is_empty() {
            break "closed by command".to_string();
        }
        if let Some(bytes) = pending_writes.front().cloned() {
            if let Err(err) = write_pending_bytes(stream.as_mut(), &bytes, &cancel).await {
                break err;
            }
            pending_writes.pop_front();
            if !close_requested {
                match cmd_rx.try_recv() {
                    Ok(command) => match queue_client_command(
                        command,
                        &mut pending_writes,
                        max_write_queue,
                        &mut close_requested,
                    ) {
                        Ok(()) => {}
                        Err(reason) => break reason,
                    },
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => break "command channel closed".to_string(),
                }
                if let Ok(read_res) = tokio::time::timeout(
                    std::time::Duration::from_millis(0),
                    stream.read(&mut read_buf),
                )
                .await
                {
                    match handle_tcp_read(
                        read_res,
                        &mut parse_buf,
                        &mut response_decoder,
                        &response_limits,
                        &event_tx,
                        &read_buf,
                    )
                    .await
                    {
                        Ok(Some(reason)) => break reason,
                        Ok(None) => {}
                        Err(reason) => break reason,
                    }
                }
            }
        } else {
            tokio::select! {
                _ = cancel.cancelled() => {
                    break "cancelled".to_string();
                }
                maybe_cmd = cmd_rx.recv(), if !close_requested => {
                    match maybe_cmd {
                        Some(command) => match queue_client_command(
                            command,
                            &mut pending_writes,
                            max_write_queue,
                            &mut close_requested,
                        ) {
                            Ok(()) => {}
                            Err(reason) => break reason,
                        },
                        None => break "command channel closed".to_string(),
                    }
                }
                read_res = stream.read(&mut read_buf), if !close_requested => {
                    match handle_tcp_read(
                        read_res,
                        &mut parse_buf,
                        &mut response_decoder,
                        &response_limits,
                        &event_tx,
                        &read_buf,
                    )
                    .await
                    {
                        Ok(Some(reason)) => break reason,
                        Ok(None) => {}
                        Err(reason) => break reason,
                    }
                }
            }
        }
    };

    let _ = stream.shutdown().await;
    let _ = event_tx.send(RtspClientEvent::Closed { reason }).await;
}

/// Run the RTSP client over an HTTP tunnel (GET/POST pair).
///
/// First opens the tunnel by sending GET and POST HTTP requests and validating 200
/// responses. After that, the GET stream is used for reading RTSP responses and
/// interleaved frames, while the POST stream is used for Base64-encoded outbound traffic.
///
/// 通过 HTTP 隧道（GET/POST 对）运行 RTSP 客户端。
///
/// 首先发送 GET 和 POST HTTP 请求并验证 200 响应以打开隧道。之后，GET 流用于读取
/// RTSP 响应与交错帧，POST 流用于发送 Base64 编码的出站流量。
pub(super) async fn run_http_tunnel_client_connection(
    mut get_stream: Box<dyn AsyncTcpStream>,
    mut post_stream: Box<dyn AsyncTcpStream>,
    mut cmd_rx: mpsc::Receiver<RtspClientCommand>,
    ctx: HttpTunnelClientContext,
) {
    let HttpTunnelClientContext {
        peer,
        path,
        session_cookie,
        event_tx,
        cancel,
        config,
    } = ctx;
    let mut pending_writes = VecDeque::<Bytes>::new();
    let max_write_queue = config.write_queue_capacity.max(8);
    let mut read_buf = vec![0_u8; config.read_buffer_size.max(1024)];
    let mut close_requested = false;
    let mut parse_buf = BytesMut::new();
    let response_limits = RtspMessageLimits::default();
    let mut response_decoder = RtspResponseDecoder::with_limits(response_limits.clone());
    let header_limit = config.http_tunnel_header_limit.max(1024);

    let reason = match open_http_tunnel_pair(
        get_stream.as_mut(),
        post_stream.as_mut(),
        &path,
        &session_cookie,
        &cancel,
        header_limit,
    )
    .await
    {
        Ok(initial_payload) => {
            if !initial_payload.is_empty() {
                if let Err(reason) = validate_parse_buffer_growth(
                    &parse_buf,
                    initial_payload.len(),
                    &response_limits,
                ) {
                    finish_http_tunnel(
                        get_stream.as_mut(),
                        post_stream.as_mut(),
                        &event_tx,
                        reason,
                    )
                    .await;
                    return;
                }
                parse_buf.extend_from_slice(initial_payload.as_ref());
                if let Err(reason) = flush_parse_buffer(
                    &mut parse_buf,
                    &mut response_decoder,
                    &response_limits,
                    &event_tx,
                )
                .await
                {
                    finish_http_tunnel(
                        get_stream.as_mut(),
                        post_stream.as_mut(),
                        &event_tx,
                        reason,
                    )
                    .await;
                    return;
                }
            }
            let _ = event_tx.send(RtspClientEvent::Connected { peer }).await;

            loop {
                if close_requested && pending_writes.is_empty() {
                    break "closed by command".to_string();
                }
                if let Some(bytes) = pending_writes.front().cloned() {
                    let encoded = Bytes::from(STANDARD.encode(bytes.as_ref()));
                    if let Err(err) =
                        write_pending_bytes(post_stream.as_mut(), &encoded, &cancel).await
                    {
                        break err;
                    }
                    pending_writes.pop_front();
                    if !close_requested {
                        match cmd_rx.try_recv() {
                            Ok(command) => match queue_client_command(
                                command,
                                &mut pending_writes,
                                max_write_queue,
                                &mut close_requested,
                            ) {
                                Ok(()) => {}
                                Err(reason) => break reason,
                            },
                            Err(TryRecvError::Empty) => {}
                            Err(TryRecvError::Disconnected) => {
                                break "command channel closed".to_string();
                            }
                        }
                    }
                } else {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            break "cancelled".to_string();
                        }
                        maybe_cmd = cmd_rx.recv(), if !close_requested => {
                            match maybe_cmd {
                                Some(command) => match queue_client_command(
                                    command,
                                    &mut pending_writes,
                                    max_write_queue,
                                    &mut close_requested,
                                ) {
                                    Ok(()) => {}
                                    Err(reason) => break reason,
                                },
                                None => break "command channel closed".to_string(),
                            }
                        }
                        read_res = get_stream.read(&mut read_buf), if !close_requested => {
                            match handle_tcp_read(
                                read_res,
                                &mut parse_buf,
                                &mut response_decoder,
                                &response_limits,
                                &event_tx,
                                &read_buf,
                            ).await {
                                Ok(Some(reason)) => break reason,
                                Ok(None) => {}
                                Err(reason) => break reason,
                            }
                        }
                    }
                }
            }
        }
        Err(reason) => reason,
    };

    finish_http_tunnel(get_stream.as_mut(), post_stream.as_mut(), &event_tx, reason).await;
}

/// Write a queued byte slice to the stream, aborting if the cancellation token fires.
///
/// 将队列中的字节切片写入流，若取消令牌触发则中止。
async fn write_pending_bytes(
    stream: &mut dyn AsyncTcpStream,
    bytes: &[u8],
    cancel: &CancellationToken,
) -> Result<(), String> {
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".to_string()),
        write_res = stream.write_all(bytes) => {
            write_res.map_err(|err| format!("write failed: {err}"))?;
            Ok(())
        }
    }
}

/// Open the HTTP tunnel by performing the GET and POST handshake.
///
/// Sends both HTTP requests, reads the HTTP response headers, and returns any bytes that
/// arrived after the GET response header (which may already contain the first RTSP bytes).
///
/// 通过 GET 与 POST 握手打开 HTTP 隧道。
///
/// 发送两条 HTTP 请求，读取 HTTP 响应头，并返回 GET 响应头之后收到的所有字节
///（可能已包含首个 RTSP 字节）。
async fn open_http_tunnel_pair(
    get_stream: &mut dyn AsyncTcpStream,
    post_stream: &mut dyn AsyncTcpStream,
    path: &str,
    session_cookie: &str,
    cancel: &CancellationToken,
    header_limit: usize,
) -> Result<Bytes, String> {
    let get_request = build_http_tunnel_get_request(path, session_cookie);
    write_pending_bytes(get_stream, get_request.as_ref(), cancel)
        .await
        .map_err(|err| format!("send GET tunnel open request failed: {err}"))?;
    let get_open = read_http_response_header(get_stream, cancel, header_limit, "GET").await?;
    if get_open.status_code != 200 {
        return Err(format!(
            "GET tunnel open failed: unexpected status code {}",
            get_open.status_code
        ));
    }

    let post_request = build_http_tunnel_post_request(path, session_cookie);
    write_pending_bytes(post_stream, post_request.as_ref(), cancel)
        .await
        .map_err(|err| format!("send POST tunnel open request failed: {err}"))?;
    let post_open = read_http_response_header(post_stream, cancel, header_limit, "POST").await?;
    if post_open.status_code != 200 {
        return Err(format!(
            "POST tunnel open failed: unexpected status code {}",
            post_open.status_code
        ));
    }

    Ok(get_open.remaining)
}

/// Parsed HTTP response header including any body bytes that already arrived.
///
/// 已解析的 HTTP 响应头，包含已提前到达的体字节。
struct HttpResponseHeader {
    status_code: u16,
    remaining: Bytes,
}

/// Read an HTTP response header from a tunnel stream.
///
/// Accumulates bytes until `\r\n\r\n` is found, then parses the status line and returns the
/// trailing bytes as `remaining`. Enforces a configurable header size limit.
///
/// 从隧道流读取 HTTP 响应头。
///
/// 累积字节直到找到 `\r\n\r\n`，然后解析状态行并将剩余字节作为 `remaining` 返回。
/// 强制执行可配置的响应头大小限制。
async fn read_http_response_header(
    stream: &mut dyn AsyncTcpStream,
    cancel: &CancellationToken,
    header_limit: usize,
    stage: &str,
) -> Result<HttpResponseHeader, String> {
    let mut raw = Vec::<u8>::with_capacity(512);
    loop {
        if let Some(header_end) = find_header_end(raw.as_ref()) {
            let header = &raw[..header_end];
            let text = std::str::from_utf8(header)
                .map_err(|_| format!("{stage} tunnel open failed: invalid http header utf8"))?;
            let status_code = parse_http_status_code(text)
                .map_err(|err| format!("{stage} tunnel open failed: {err}"))?;
            let body_start = header_end + 4;
            let remaining = if body_start < raw.len() {
                Bytes::copy_from_slice(&raw[body_start..])
            } else {
                Bytes::new()
            };
            return Ok(HttpResponseHeader {
                status_code,
                remaining,
            });
        }
        if raw.len() >= header_limit {
            return Err(format!(
                "{stage} tunnel open failed: http header exceeds {header_limit} bytes"
            ));
        }
        let mut buf = vec![0_u8; 4096];
        let read_res: io::Result<usize> = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled".to_string()),
            read_res = stream.read(&mut buf) => read_res,
        };
        match read_res {
            Ok(0) => {
                return Err(format!(
                    "{stage} tunnel open failed: peer closed before full header"
                ))
            }
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(err) => {
                return Err(format!(
                    "{stage} tunnel open failed: read header failed: {err}"
                ))
            }
        }
    }
}

/// Parse the status code from the first line of an HTTP response.
///
/// 从 HTTP 响应的第一行解析状态码。
fn parse_http_status_code(header: &str) -> Result<u16, &'static str> {
    let Some(line) = header.lines().next() else {
        return Err("missing status line");
    };
    let mut parts = line.split_whitespace();
    let Some(version) = parts.next() else {
        return Err("invalid status line");
    };
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Err("unsupported http version");
    }
    let Some(code_raw) = parts.next() else {
        return Err("missing status code");
    };
    let code = code_raw.parse::<u16>().map_err(|_| "invalid status code")?;
    Ok(code)
}

/// Find the index of the first `\r\n\r\n` sequence in the byte slice.
///
/// 返回字节切片中首个 `\r\n\r\n` 序列的索引。
fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Shut down both tunnel streams and emit a `Closed` event.
///
/// 关闭两条隧道流并发出 `Closed` 事件。
async fn finish_http_tunnel(
    get_stream: &mut dyn AsyncTcpStream,
    post_stream: &mut dyn AsyncTcpStream,
    event_tx: &mpsc::Sender<RtspClientEvent>,
    reason: String,
) {
    let _ = get_stream.shutdown().await;
    let _ = post_stream.shutdown().await;
    let _ = event_tx.send(RtspClientEvent::Closed { reason }).await;
}

/// Convert a command into bytes and append it to the pending write queue.
///
/// `SendRequest` is encoded as an RTSP request. `SendInterleaved` is encoded as an
/// interleaved `$` frame. `Close` sets `close_requested` so the loop drains existing writes.
///
/// 将命令转换为字节并追加到待写队列。
///
/// `SendRequest` 编码为 RTSP 请求。`SendInterleaved` 编码为交错 `$` 帧。`Close` 设置
/// `close_requested`，使循环刷新完现有写入后再退出。
fn queue_client_command(
    command: RtspClientCommand,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
    close_requested: &mut bool,
) -> Result<(), String> {
    match command {
        RtspClientCommand::SendRequest(request) => {
            if pending_writes.len() >= max_write_queue {
                return Err("write queue overflow".to_string());
            }
            let bytes = encode_rtsp_request(&request)
                .map_err(|err| format!("encode request failed: {err}"))?;
            pending_writes.push_back(bytes);
        }
        RtspClientCommand::SendInterleaved { channel, payload } => {
            if pending_writes.len() >= max_write_queue {
                return Err("write queue overflow".to_string());
            }
            let frame = encode_interleaved_frame(channel, payload.as_ref())
                .map_err(|err| format!("encode interleaved frame failed: {err}"))?;
            pending_writes.push_back(Bytes::from(frame));
        }
        RtspClientCommand::Close => {
            *close_requested = true;
        }
    }
    Ok(())
}

/// Handle a result from `AsyncTcpStream::read`.
///
/// EOF is treated as a closed peer. Successful reads are appended to the parse buffer and
/// flushed, which may produce `Response` or `InterleavedFrame` events.
///
/// 处理 `AsyncTcpStream::read` 的结果。
///
/// EOF 视为对端关闭。成功读取的字节追加到解析缓冲区并刷新，可能产生 `Response` 或
/// `InterleavedFrame` 事件。
async fn handle_tcp_read(
    read_res: Result<usize, std::io::Error>,
    parse_buf: &mut BytesMut,
    response_decoder: &mut RtspResponseDecoder,
    response_limits: &RtspMessageLimits,
    event_tx: &mpsc::Sender<RtspClientEvent>,
    read_buf: &[u8],
) -> Result<Option<String>, String> {
    match read_res {
        Ok(0) => Ok(Some("peer closed".to_string())),
        Ok(n) => {
            validate_parse_buffer_growth(parse_buf, n, response_limits)?;
            parse_buf.extend_from_slice(&read_buf[..n]);
            flush_parse_buffer(parse_buf, response_decoder, response_limits, event_tx).await?;
            Ok(None)
        }
        Err(err) => Ok(Some(format!("read failed: {err}"))),
    }
}

/// Flush the parse buffer by decoding complete RTSP responses and interleaved frames.
///
/// If the buffer starts with `$` it is parsed as an interleaved frame; otherwise it is treated
/// as one or more RTSP text messages. The loop drains as many complete messages as possible.
///
/// 通过解析完整的 RTSP 响应与交错帧来刷新解析缓冲区。
///
/// 若缓冲区以 `$` 开头，按交错帧解析；否则按一个或多个 RTSP 文本消息处理。循环尽可能
/// 排空所有完整消息。
async fn flush_parse_buffer(
    parse_buf: &mut BytesMut,
    response_decoder: &mut RtspResponseDecoder,
    response_limits: &RtspMessageLimits,
    event_tx: &mpsc::Sender<RtspClientEvent>,
) -> Result<(), String> {
    loop {
        if parse_buf.is_empty() {
            return Ok(());
        }
        if parse_buf[0] == b'$' {
            let Some(frame_header) = parse_interleaved_frame(parse_buf.as_ref()) else {
                return Ok(());
            };
            if parse_buf.len() < frame_header.total_len {
                return Ok(());
            }
            let frame = parse_buf.split_to(frame_header.total_len).freeze();
            let payload = frame.slice(4..);
            event_tx
                .send(RtspClientEvent::InterleavedFrame {
                    channel: frame_header.channel,
                    payload,
                })
                .await
                .map_err(|_| "event channel closed".to_string())?;
            continue;
        }

        let Some(message_len) = peek_rtsp_message_len(parse_buf.as_ref(), response_limits)? else {
            return Ok(());
        };
        let non_interleaved = parse_buf.split_to(message_len);
        response_decoder
            .feed(non_interleaved.as_ref())
            .map_err(|err| format!("response decode feed failed: {err}"))?;
        while let Some(response) = response_decoder
            .decode()
            .map_err(|err| format!("response decode failed: {err}"))?
        {
            event_tx
                .send(RtspClientEvent::Response { response })
                .await
                .map_err(|_| "event channel closed".to_string())?;
        }
    }
}

/// Check whether appending `incoming_len` bytes would exceed the configured buffer limit.
///
/// 检查追加 `incoming_len` 字节是否会超出配置的缓冲区限制。
fn validate_parse_buffer_growth(
    parse_buf: &BytesMut,
    incoming_len: usize,
    response_limits: &RtspMessageLimits,
) -> Result<(), String> {
    let total_len = parse_buf
        .len()
        .checked_add(incoming_len)
        .ok_or_else(|| "response buffer size limit exceeded: length overflow".to_string())?;
    if total_len > response_limits.max_buffer_size {
        return Err(format!(
            "response buffer size limit exceeded: max={}, actual={total_len}",
            response_limits.max_buffer_size
        ));
    }
    Ok(())
}

/// Peek at the buffer and return the total byte length of the next RTSP message.
///
/// Uses `\r\n\r\n` to find the header boundary, then parses `Content-Length` to compute the
/// full message length. Returns `None` while the header is incomplete.
///
/// 查看缓冲区并返回下一条 RTSP 消息的总字节长度。
///
/// 使用 `\r\n\r\n` 定位头边界，然后解析 `Content-Length` 计算完整消息长度。当头不完整时
/// 返回 `None`。
fn peek_rtsp_message_len(
    input: &[u8],
    response_limits: &RtspMessageLimits,
) -> Result<Option<usize>, String> {
    let Some(header_end) = find_rtsp_header_end(input) else {
        return Ok(None);
    };
    let header = &input[..header_end];
    let content_length = parse_content_length(header)?;
    if content_length > response_limits.max_body_size {
        return Err(format!(
            "response body size limit exceeded: max={}, actual={content_length}",
            response_limits.max_body_size
        ));
    }
    let total_len = header_end
        .checked_add(4)
        .and_then(|header_len| header_len.checked_add(content_length))
        .ok_or_else(|| "response length overflow".to_string())?;
    if total_len > response_limits.max_buffer_size {
        return Err(format!(
            "response buffer size limit exceeded: max={}, actual={total_len}",
            response_limits.max_buffer_size
        ));
    }
    if input.len() < total_len {
        return Ok(None);
    }
    Ok(Some(total_len))
}

/// Locate the end of the RTSP header block (`\r\n\r\n`).
///
/// 定位 RTSP 头块结束位置（`\r\n\r\n`）。
fn find_rtsp_header_end(input: &[u8]) -> Option<usize> {
    if input.len() < 4 {
        return None;
    }
    input.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the `Content-Length` header from the response header bytes.
///
/// Returns `0` if the header is absent, which is correct for RTSP responses without a body.
///
/// 从响应头字节中解析 `Content-Length`。
///
/// 若头不存在则返回 `0`，对应无体的 RTSP 响应。
fn parse_content_length(header: &[u8]) -> Result<usize, String> {
    let header_text =
        std::str::from_utf8(header).map_err(|_| "response header is not utf8".to_string())?;
    for line in header_text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            return value
                .parse::<usize>()
                .map_err(|_| "invalid content-length".to_string());
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_rtsp_core::encode_rtsp_response;
    use tokio::sync::mpsc;

    #[tokio::test(flavor = "current_thread")]
    async fn flush_parse_buffer_keeps_dollar_inside_rtsp_body() {
        let response_bytes = encode_rtsp_response(&cheetah_rtsp_core::RtspResponseMessage {
            version: "RTSP/1.0".to_string(),
            status_code: 200,
            reason_phrase: "OK".to_string(),
            headers: vec![cheetah_rtsp_core::RtspHeader {
                name: "CSeq".to_string(),
                value: "88".to_string(),
            }],
            body: Bytes::from_static(b"v=0\r\ns=$demo\r\n"),
        })
        .expect("encode response");

        let mut parse_buf = BytesMut::from(response_bytes.as_ref());
        let response_limits = RtspMessageLimits::default();
        let mut response_decoder = RtspResponseDecoder::with_limits(response_limits.clone());
        let (event_tx, mut event_rx) = mpsc::channel(4);

        flush_parse_buffer(
            &mut parse_buf,
            &mut response_decoder,
            &response_limits,
            &event_tx,
        )
        .await
        .expect("flush parse buffer");

        let event = event_rx.recv().await.expect("response event");
        match event {
            RtspClientEvent::Response { response } => {
                assert_eq!(response.status_code, 200);
                assert_eq!(response.header_value("CSeq"), Some("88"));
                assert_eq!(response.body.as_ref(), b"v=0\r\ns=$demo\r\n");
            }
            other => panic!("expected response event, got {other:?}"),
        }
        assert!(
            parse_buf.is_empty(),
            "parse buffer should be fully consumed"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn flush_parse_buffer_rejects_oversized_response_body_before_buffering_body() {
        let mut parse_buf = BytesMut::from(
            "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 524289\r\n\r\n".as_bytes(),
        );
        let response_limits = RtspMessageLimits::default();
        let mut response_decoder = RtspResponseDecoder::with_limits(response_limits.clone());
        let (event_tx, _event_rx) = mpsc::channel(4);

        let err = flush_parse_buffer(
            &mut parse_buf,
            &mut response_decoder,
            &response_limits,
            &event_tx,
        )
        .await
        .expect_err("oversized body should be rejected from headers");

        assert!(err.contains("body size limit"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_tcp_read_rejects_unfinished_header_beyond_buffer_limit() {
        let mut parse_buf = BytesMut::from(vec![b'A'; 1024 * 1024].as_slice());
        let response_limits = RtspMessageLimits::default();
        let mut response_decoder = RtspResponseDecoder::with_limits(response_limits.clone());
        let (event_tx, _event_rx) = mpsc::channel(4);
        let read_buf = [b'B'; 1];

        let err = handle_tcp_read(
            Ok(read_buf.len()),
            &mut parse_buf,
            &mut response_decoder,
            &response_limits,
            &event_tx,
            &read_buf,
        )
        .await
        .expect_err("unfinished header should be bounded by max buffer size");

        assert!(err.contains("buffer size limit"));
    }
}
