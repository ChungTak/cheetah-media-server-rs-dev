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

pub(super) struct HttpTunnelClientContext {
    pub(super) peer: SocketAddr,
    pub(super) path: String,
    pub(super) session_cookie: String,
    pub(super) event_tx: mpsc::Sender<RtspClientEvent>,
    pub(super) cancel: CancellationToken,
    pub(super) config: RtspClientConfig,
}

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

struct HttpResponseHeader {
    status_code: u16,
    remaining: Bytes,
}

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

fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

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

fn find_rtsp_header_end(input: &[u8]) -> Option<usize> {
    if input.len() < 4 {
        return None;
    }
    input.windows(4).position(|w| w == b"\r\n\r\n")
}

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
