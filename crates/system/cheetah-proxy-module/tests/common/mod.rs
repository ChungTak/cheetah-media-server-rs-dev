//! Shared helpers for RTSP pull integration tests in the proxy module crate.
//!
//! 本 crate RTSP 拉流集成测试的共享 helper，从 `cheetah-connector` 测试复制并适配。

#![allow(dead_code)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

pub(crate) async fn read_rtsp_request(socket: &mut TcpStream) -> String {
    let mut buf = Vec::<u8>::new();
    loop {
        if let Some(header_end) = find_header_end(&buf) {
            let header = &buf[..header_end];
            let header_text = std::str::from_utf8(header).expect("rtsp request utf8");
            let content_length = parse_content_length(header_text);
            let total_len = header_end + 4 + content_length;
            while buf.len() < total_len {
                let mut chunk = [0u8; 1024];
                let n = timeout(Duration::from_secs(2), socket.read(&mut chunk))
                    .await
                    .expect("read request body timeout")
                    .expect("read request body");
                assert!(n > 0, "peer closed while reading request");
                buf.extend_from_slice(&chunk[..n]);
            }
            return String::from_utf8(buf[..total_len].to_vec()).expect("request utf8");
        }
        let mut chunk = [0u8; 1024];
        let n = timeout(Duration::from_secs(2), socket.read(&mut chunk))
            .await
            .expect("read request timeout")
            .expect("read request");
        assert!(n > 0, "peer closed while reading request");
        buf.extend_from_slice(&chunk[..n]);
    }
}

pub(crate) fn extract_cseq(request: &str) -> Option<u32> {
    for line in request.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("cseq") {
            return value.trim().parse::<u32>().ok();
        }
    }
    None
}

pub(crate) async fn write_rtsp_response(
    socket: &mut TcpStream,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    write_rtsp_status_response(socket, 200, "OK", cseq, headers, body).await;
}

pub(crate) async fn write_rtsp_status_response(
    socket: &mut TcpStream,
    status: u16,
    reason: &str,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    let mut response = format!("RTSP/1.0 {status} {reason}\r\nCSeq: {cseq}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    socket
        .write_all(response.as_bytes())
        .await
        .expect("write response header");
    if !body.is_empty() {
        socket.write_all(body).await.expect("write response body");
    }
}

pub(crate) fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

pub(crate) fn parse_content_length(header_text: &str) -> usize {
    for line in header_text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().unwrap_or(0);
        }
    }
    0
}

pub(crate) async fn send_interleaved_frame(socket: &mut TcpStream, channel: u8, payload: &[u8]) {
    let len = payload.len().min(u16::MAX as usize);
    let mut packet = Vec::with_capacity(4 + len);
    packet.push(b'$');
    packet.push(channel);
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&payload[..len]);
    socket
        .write_all(&packet)
        .await
        .expect("write interleaved frame");
}

pub(crate) fn h264_sdp() -> &'static str {
    "v=0\r\n\
     o=- 0 0 IN IP4 127.0.0.1\r\n\
     s=pull-source\r\n\
     t=0 0\r\n\
     m=video 0 RTP/AVP 96\r\n\
     a=rtpmap:96 H264/90000\r\n\
     a=control:trackID=0\r\n"
}

pub(crate) fn h264_rtp_packet() -> Vec<u8> {
    let mut rtp = Vec::new();
    // RTP header: V=2, P=0, X=0, CC=0, M=1, PT=96, seq=1, ts=90000, ssrc=0x11223344
    rtp.extend_from_slice(&[
        0x80, 0xE0, 0x00, 0x01, 0x00, 0x01, 0x5C, 0x00, 0x11, 0x22, 0x33, 0x44,
    ]);
    // H264 NAL unit (5 = IDR slice) + payload
    rtp.extend_from_slice(&[0x65, 0x88, 0x84, 0x21]);
    rtp
}

pub(crate) async fn run_interleaved_rtsp_source(
    mut socket: TcpStream,
    uri: String,
    send_after_play: Option<Vec<u8>>,
) {
    let options_req = read_rtsp_request(&mut socket).await;
    assert!(options_req.starts_with(&format!("OPTIONS {uri} RTSP/1.0")));
    let options_cseq = extract_cseq(&options_req).expect("options cseq");
    write_rtsp_response(
        &mut socket,
        options_cseq,
        &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
        &[],
    )
    .await;

    let describe_req = read_rtsp_request(&mut socket).await;
    assert!(describe_req.starts_with(&format!("DESCRIBE {uri} RTSP/1.0")));
    assert!(describe_req.contains("Accept: application/sdp"));
    let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
    let sdp = h264_sdp().as_bytes();
    write_rtsp_response(
        &mut socket,
        describe_cseq,
        &[("Content-Type", "application/sdp")],
        sdp,
    )
    .await;

    let setup_req = read_rtsp_request(&mut socket).await;
    assert!(setup_req.starts_with(&format!("SETUP {uri}/trackID=0 RTSP/1.0")));
    assert!(setup_req.contains("Transport: RTP/AVP/TCP;unicast;interleaved=0-1"));
    let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
    write_rtsp_response(
        &mut socket,
        setup_cseq,
        &[
            ("Session", "pull-session-1;timeout=60"),
            ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
        ],
        &[],
    )
    .await;

    let play_req = read_rtsp_request(&mut socket).await;
    assert!(play_req.starts_with(&format!("PLAY {uri} RTSP/1.0")));
    assert!(play_req.contains("Session: pull-session-1"));
    let play_cseq = extract_cseq(&play_req).expect("play cseq");
    write_rtsp_response(
        &mut socket,
        play_cseq,
        &[("Session", "pull-session-1")],
        &[],
    )
    .await;

    if let Some(payload) = send_after_play {
        send_interleaved_frame(&mut socket, 0, &payload).await;
    }

    let mut drain = [0u8; 1];
    let _ = timeout(Duration::from_secs(5), socket.read(&mut drain)).await;
}
