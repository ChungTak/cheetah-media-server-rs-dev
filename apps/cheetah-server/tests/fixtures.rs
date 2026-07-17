//! Shared helpers for `cheetah-server` native HTTP black-box tests.
//!
//! Provides a raw HTTP/1.1 client, process spawning, PS/RTP fixture generation,
//! and small wait utilities used by the per-signal B-layer contracts.
//!
//! 本模块为 `cheetah-server` native HTTP 黑盒测试提供共享辅助：原始 HTTP/1.1 客户端、
//! 进程启动、PS/RTP 真实素材生成以及常用等待工具。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, PsDemuxEvent, PsDemuxer,
    PsDemuxerConfig, RtpHeader, RtpPacket, Timebase, TrackId, TrackInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

pub const SSRC: u32 = 0x12345678;
pub const INGEST_PT: u8 = 100;

pub fn server_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cheetah-server"))
}

pub async fn free_local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

pub fn write_config(control_port: u16, temp_dir: &Path, module_overrides: &str) -> PathBuf {
    let config_path = temp_dir.join("cheetah.yaml");
    let yaml = format!(
        r#"global:
  control:
    listen: "127.0.0.1:{control_port}"
  media:
    native:
      auth:
        mode: "none"
modules:
  rtmp:
    enabled: false
{module_overrides}
"#,
    );
    std::fs::write(&config_path, yaml).unwrap();
    config_path
}

pub fn gb28181_config(temp_dir: &Path) -> String {
    format!(
        r#"  rtp:
    enabled: true
    listen_udp: "0.0.0.0:0"
    listen_tcp: "0.0.0.0:0"
    rtcp_listen_udp: "0.0.0.0:0"
  record:
    enabled: true
    root_path: "{}"
  webhook-dispatcher:
    profiles: []
"#,
        temp_dir.join("record").display()
    )
}

pub fn server_stderr_path(temp_dir: &Path) -> PathBuf {
    temp_dir.join("server.stderr")
}

pub async fn spawn_server(config_path: &Path, temp_dir: &Path) -> Child {
    let stderr_path = server_stderr_path(temp_dir);
    let stderr_file = std::fs::File::create(&stderr_path)
        .unwrap_or_else(|e| panic!("create stderr log {}: {e}", stderr_path.display()));

    let mut child = Command::new(server_bin())
        .env("CHEETAH_CONFIG", config_path)
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .expect("spawn cheetah-server");

    sleep(Duration::from_millis(200)).await;
    if let Ok(Some(status)) = child.try_wait() {
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        panic!("cheetah-server exited early: {status}\n{stderr}");
    }
    child
}

pub async fn wait_for_server(port: u16) {
    let deadline = Duration::from_secs(15);
    timeout(deadline, async {
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                sleep(Duration::from_millis(200)).await;
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("server did not start in time");
}

pub async fn stop_server(mut child: Child) {
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}

async fn send_request(stream: &mut TcpStream, request: &str) {
    stream.write_all(request.as_bytes()).await.unwrap();
}

async fn read_http_response(stream: &mut TcpStream) -> (u16, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut content_length: Option<usize> = None;
    let mut header_end: Option<usize> = None;
    let mut status: Option<u16> = None;

    loop {
        let n = stream.read(&mut tmp).await.expect("read response");
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&tmp[..n]);

        if header_end.is_none() {
            if let Some(idx) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(idx + 4);
                let headers = std::str::from_utf8(&buffer[..idx]).unwrap();
                for line in headers.lines() {
                    let lower = line.to_ascii_lowercase();
                    if lower.starts_with("content-length:") {
                        content_length =
                            lower.split(':').nth(1).and_then(|v| v.trim().parse().ok());
                    }
                }
                if let Some(first) = headers.lines().next() {
                    status = first.split(' ').nth(1).and_then(|s| s.parse().ok());
                }
            }
        }

        if let Some(header_end) = header_end {
            let body_len = content_length.unwrap_or(0);
            if buffer.len() >= header_end + body_len {
                break;
            }
        }
    }

    let header_end = header_end.unwrap_or(buffer.len());
    let body = buffer[header_end..].to_vec();
    (status.unwrap_or(0), body)
}

pub async fn http_get(host: &str, port: u16, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect((host, port))
        .await
        .unwrap_or_else(|e| panic!("connect to {host}:{port}: {e}"));
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    send_request(&mut stream, &request).await;
    read_http_response(&mut stream).await
}

pub async fn http_post(
    host: &str,
    port: u16,
    path: &str,
    body: serde_json::Value,
) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect((host, port))
        .await
        .unwrap_or_else(|e| panic!("connect to {host}:{port}: {e}"));
    let payload = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    send_request(&mut stream, &request).await;
    read_http_response(&mut stream).await
}

pub async fn http_delete(host: &str, port: u16, path: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect((host, port))
        .await
        .unwrap_or_else(|e| panic!("connect to {host}:{port}: {e}"));
    let request =
        format!("DELETE {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    send_request(&mut stream, &request).await;
    read_http_response(&mut stream).await
}

pub fn parse_json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("invalid json: {e}; body: {}", String::from_utf8_lossy(body)))
}

pub fn media_key(vhost: &str, app: &str, stream: &str) -> serde_json::Value {
    serde_json::json!({
        "vhost": vhost,
        "app": app,
        "stream": stream,
        "schema": serde_json::Value::Null,
    })
}

pub fn rtp_receiver_request(
    media_key: serde_json::Value,
    port: Option<u16>,
    ssrc: u32,
    pt: u8,
    codec_hint: &str,
) -> serde_json::Value {
    serde_json::json!({
        "media_key": media_key,
        "port": port,
        "ip": null,
        "ssrc": ssrc,
        "enable_rtcp": false,
        "tcp_mode": serde_json::Value::Null,
        "payload_type": pt,
        "codec_hint": codec_hint,
        "reuse_port": false,
        "timeout_ms": 0,
    })
}

pub fn start_record_request(media_key: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "media_key": media_key,
        "format": "mp4",
        "template": "continuous",
        "segment_duration_ms": serde_json::Value::Null,
        "max_segments": serde_json::Value::Null,
        "storage_policy": serde_json::Value::Object(serde_json::Map::new()),
    })
}

pub async fn wait_for_online(
    host: &str,
    control_port: u16,
    vhost: &str,
    app: &str,
    stream: &str,
    max_wait: Duration,
) {
    let deadline = tokio::time::Instant::now() + max_wait;
    let path = format!("/api/v1/media/{vhost}/{app}/{stream}/online");
    while tokio::time::Instant::now() < deadline {
        let (status, body) = http_get(host, control_port, &path).await;
        if status == 200 {
            let value = parse_json(&body);
            if value["online"].as_str() == Some("online") {
                return;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("stream {vhost}/{app}/{stream} did not come online");
}

pub fn make_video_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x0A])],
        pps: vec![Bytes::from_static(&[0x68, 0xCE, 0x38, 0x80])],
        avcc: None,
    };
    track.refresh_readiness();
    track
}

pub fn make_audio_track() -> TrackInfo {
    TrackInfo::new(TrackId(0xC0), MediaKind::Audio, CodecId::G711A, 8_000)
}

pub fn make_video_frame(pts_us: i64) -> AVFrame {
    let mut payload = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A];
    payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65]);
    payload.extend_from_slice(b"video frame data");
    let mut frame = AVFrame::new(
        TrackId(0xE0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        90_000,
        90_000,
        Timebase::new(1, 90_000),
        Bytes::from(payload),
    );
    frame.pts_us = pts_us;
    frame.dts_us = pts_us;
    frame.flags.insert(FrameFlags::KEY);
    frame
}

pub fn make_audio_frame(pts_us: i64) -> AVFrame {
    let mut frame = AVFrame::new(
        TrackId(0xC0),
        MediaKind::Audio,
        CodecId::G711A,
        FrameFormat::G711Packet,
        90_080,
        90_080,
        Timebase::new(1, 8_000),
        Bytes::from_static(b"audio frame data"),
    );
    frame.pts_us = pts_us;
    frame.dts_us = pts_us;
    frame
}

pub fn mux_ps_frame(frame: &AVFrame) -> Bytes {
    let mut muxer = cheetah_codec::PsMuxer::new();
    muxer.add_track(make_video_track());
    muxer.add_track(make_audio_track());
    muxer.mux(frame).expect("mux frame")
}

pub fn encode_rtp(
    payload: Bytes,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    payload_type: u8,
) -> Bytes {
    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type,
            sequence_number: sequence,
            timestamp,
            ssrc,
            marker: false,
        },
        payload,
    };
    packet.encode()
}

pub async fn bind_udp_socket() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.expect("bind udp")
}

pub async fn send_rtp(
    socket: &UdpSocket,
    dest: std::net::SocketAddr,
    payload: Bytes,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    payload_type: u8,
) {
    let packet = encode_rtp(payload, ssrc, sequence, timestamp, payload_type);
    socket.send_to(&packet, dest).await.expect("send rtp");
}

pub async fn recv_rtp(
    socket: &UdpSocket,
    timeout_after: Duration,
) -> Option<(RtpHeader, Bytes, std::net::SocketAddr)> {
    let mut buf = vec![0u8; 2048];
    match timeout(timeout_after, socket.recv_from(&mut buf)).await {
        Ok(Ok((len, addr))) => {
            let bytes = Bytes::copy_from_slice(&buf[..len]);
            RtpPacket::parse(&bytes).map(|p| (p.header, p.payload, addr))
        }
        _ => None,
    }
}

#[test]
fn fixtures_are_real_and_self_consistent() {
    // The generated PS video packet must demux and expose a video track.
    let video_ps = mux_ps_frame(&make_video_frame(0));
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let events = demuxer.push(&video_ps);
    let tracks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            PsDemuxEvent::TrackInfo(t) => Some(t.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        tracks.iter().any(|t| t.media_kind == MediaKind::Video),
        "video PS should expose a video track"
    );

    // The generated PS audio packet must demux and expose an audio track.
    let audio_ps = mux_ps_frame(&make_audio_frame(80));
    let mut demuxer = PsDemuxer::new(PsDemuxerConfig::default());
    let events = demuxer.push(&audio_ps);
    let tracks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            PsDemuxEvent::TrackInfo(t) => Some(t.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        tracks.iter().any(|t| t.media_kind == MediaKind::Audio),
        "audio PS should expose an audio track"
    );

    // The same payloads wrapped in RTP must round-trip through the codec parser.
    let rtp = encode_rtp(video_ps, SSRC, 1, 0, INGEST_PT);
    let packet = RtpPacket::parse(&rtp).expect("RTP packet should parse");
    assert_eq!(packet.header.version, 2);
    assert_eq!(packet.header.ssrc, SSRC);
    assert_eq!(packet.header.payload_type, INGEST_PT);
}
