//! Shared helpers for `cheetah-server` native HTTP black-box tests.
//!
//! Provides a raw HTTP/1.1 client, process spawning, PS/RTP fixture generation,
//! and small wait utilities used by the per-signal B-layer contracts.
//!
//! 本模块为 `cheetah-server` native HTTP 黑盒测试提供共享辅助：原始 HTTP/1.1 客户端、
//! 进程启动、PS/RTP 真实素材生成以及常用等待工具。

use std::path::{Path, PathBuf};
#[cfg(feature = "proxy-rtsp")]
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::time::Duration;

#[cfg(feature = "proxy-rtsp")]
use base64::Engine;
use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, PsDemuxEvent, PsDemuxer,
    PsDemuxerConfig, RtpHeader, RtpPacket, Timebase, TrackId, TrackInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "rtp")]
use tokio::net::UdpSocket;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command as TokioCommand};
#[cfg(feature = "proxy-rtsp")]
use tokio::time::interval;
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
{module_overrides}
"#,
    );
    std::fs::write(&config_path, yaml).unwrap();
    config_path
}

pub fn gb28181_config(temp_dir: &Path) -> String {
    format!(
        r#"  rtmp:
    enabled: false
  rtp:
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

    let mut child = TokioCommand::new(server_bin())
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

#[cfg(feature = "proxy-rtsp")]
pub async fn http_delete_with_body(
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
        "DELETE {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
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

#[cfg(feature = "rtp")]
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

#[cfg(feature = "rtp")]
pub async fn bind_udp_socket() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.expect("bind udp")
}

#[cfg(feature = "rtp")]
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

#[cfg(feature = "rtp")]
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

#[cfg(feature = "proxy-rtsp")]
pub fn onvif_config(temp_dir: &Path, rtmp_enabled: bool, ssrf_allowlist: &[&str]) -> String {
    let allowlist_yaml = if ssrf_allowlist.is_empty() {
        String::new()
    } else {
        ssrf_allowlist
            .iter()
            .map(|cidr| format!("      - \"{cidr}\""))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let snapshot_root = temp_dir.join("snapshot").display().to_string();
    let record_root = temp_dir.join("record").display().to_string();
    let rtmp_listen = if rtmp_enabled {
        "0.0.0.0:0"
    } else {
        "0.0.0.0:1935"
    };
    format!(
        r#"  rtmp:
    enabled: {rtmp_enabled}
    listen: "{rtmp_listen}"
  proxy:
    retry_max: 0
    connect_timeout_ms: 5000
{allowlist}
  snapshot:
    root_path: "{snapshot_root}"
  record:
    enabled: true
    root_path: "{record_root}"
  webhook-dispatcher:
    profiles: []
"#,
        allowlist = if allowlist_yaml.is_empty() {
            "    ssrf_allowlist_cidrs: []".to_string()
        } else {
            format!("    ssrf_allowlist_cidrs:\n{allowlist_yaml}")
        },
    )
}

#[cfg(feature = "proxy-rtsp")]
pub fn pull_proxy_request(source_url: &str, destination: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "source_url": source_url,
        "destination": destination,
        "timeout_ms": 10000,
    })
}

#[cfg(feature = "proxy-rtsp")]
pub fn snapshot_request(media_key: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "media_key": media_key,
        "format": "jpg",
        "timeout_ms": 15000,
    })
}

#[cfg(feature = "proxy-rtsp")]
pub async fn wait_for_proxy_connected(host: &str, port: u16, proxy_id: &str, max_wait: Duration) {
    let deadline = tokio::time::Instant::now() + max_wait;
    let path = format!("/api/v1/proxies/pull/{proxy_id}");
    while tokio::time::Instant::now() < deadline {
        let (status, body) = http_get(host, port, &path).await;
        if status == 200 {
            let value = parse_json(&body);
            if value["state"].as_str() == Some("connected") {
                return;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("proxy {proxy_id} did not become connected");
}

#[cfg(feature = "proxy-rtsp")]
pub async fn wait_for_record_file(
    host: &str,
    port: u16,
    app: &str,
    stream: &str,
    max_wait: Duration,
) -> String {
    let deadline = tokio::time::Instant::now() + max_wait;
    let path = format!("/api/v1/record/files?app={app}&stream={stream}");
    while tokio::time::Instant::now() < deadline {
        let (status, body) = http_get(host, port, &path).await;
        assert_eq!(status, 200, "list record files failed");
        let page = parse_json(&body);
        if let Some(items) = page["items"].as_array() {
            if let Some(first) = items.first() {
                if let Some(handle) = first["path_handle"].as_str() {
                    return handle.to_string();
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("record file not produced for {app}/{stream}");
}

#[cfg(feature = "proxy-rtsp")]
pub async fn wait_for_stream_offline(
    host: &str,
    port: u16,
    vhost: &str,
    app: &str,
    stream: &str,
    max_wait: Duration,
) {
    let deadline = tokio::time::Instant::now() + max_wait;
    let path = format!("/api/v1/media/{vhost}/{app}/{stream}/online");
    while tokio::time::Instant::now() < deadline {
        let (status, body) = http_get(host, port, &path).await;
        if status == 200 {
            let value = parse_json(&body);
            if value["online"].as_str() != Some("online") {
                return;
            }
        } else {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("stream {vhost}/{app}/{stream} did not go offline");
}

#[cfg(feature = "proxy-rtsp")]
pub fn generate_h264_keyframe() -> (Bytes, Bytes, Bytes) {
    let output = StdCommand::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=8x6:rate=1",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-frames:v",
            "1",
            "-f",
            "h264",
            "-",
        ])
        .output()
        .expect("spawn ffmpeg for H264 fixture");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let nals = split_annex_b(&output.stdout);
    let mut sps = None;
    let mut pps = None;
    let mut idr = None;
    for nal in nals {
        if nal.is_empty() {
            continue;
        }
        let nal_type = nal[0] & 0x1f;
        match nal_type {
            7 if sps.is_none() => sps = Some(Bytes::from(nal)),
            8 if pps.is_none() => pps = Some(Bytes::from(nal)),
            5 if idr.is_none() => idr = Some(Bytes::from(nal)),
            _ => {}
        }
    }
    (
        sps.expect("missing SPS in H264 fixture"),
        pps.expect("missing PPS in H264 fixture"),
        idr.expect("missing IDR in H264 fixture"),
    )
}

#[cfg(feature = "proxy-rtsp")]
fn split_annex_b(data: &[u8]) -> Vec<Vec<u8>> {
    let mut nals = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        let start_len = if data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && i + 3 < data.len()
            && data[i + 3] == 1
        {
            4
        } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            3
        } else {
            i += 1;
            continue;
        };

        let nal_start = i + start_len;
        let mut j = nal_start;
        while j + 2 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && data[j + 2] == 0
                && j + 3 < data.len()
                && data[j + 3] == 1
            {
                break;
            }
            if data[j] == 0 && data[j + 1] == 0 && data[j + 2] == 1 {
                break;
            }
            j += 1;
        }
        if j + 2 >= data.len() {
            j = data.len();
        }
        nals.push(data[nal_start..j].to_vec());
        i = nal_start;
    }
    nals
}

#[cfg(feature = "proxy-rtsp")]
pub fn h264_sdp(sps: &Bytes, pps: &Bytes) -> String {
    let sps_b64 = base64::engine::general_purpose::STANDARD.encode(sps);
    let pps_b64 = base64::engine::general_purpose::STANDARD.encode(pps);
    format!(
        "v=0\r\n\
         o=- 0 0 IN IP4 127.0.0.1\r\n\
         s=onvif-source\r\n\
         t=0 0\r\n\
         m=video 0 RTP/AVP 96\r\n\
         a=rtpmap:96 H264/90000\r\n\
         a=fmtp:96 packetization-mode=1; sprop-parameter-sets={sps_b64},{pps_b64}\r\n\
         a=control:trackID=0\r\n"
    )
}

#[cfg(feature = "proxy-rtsp")]
pub fn h264_rtp_packet(
    payload: &[u8],
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload_type: u8,
) -> Vec<u8> {
    let mut rtp = Vec::with_capacity(12 + payload.len());
    rtp.push(0x80);
    rtp.push(0x80 | (payload_type & 0x7f));
    rtp.extend_from_slice(&sequence.to_be_bytes());
    rtp.extend_from_slice(&timestamp.to_be_bytes());
    rtp.extend_from_slice(&ssrc.to_be_bytes());
    rtp.extend_from_slice(payload);
    rtp
}

#[cfg(feature = "proxy-rtsp")]
pub async fn run_interleaved_rtsp_source(
    mut socket: TcpStream,
    uri: String,
    sdp: String,
    frames: Vec<Vec<u8>>,
    frame_interval: Duration,
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
    write_rtsp_response(
        &mut socket,
        describe_cseq,
        &[("Content-Type", "application/sdp")],
        sdp.as_bytes(),
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

    let mut next = 0usize;
    let mut tick = interval(frame_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut drain = [0u8; 1];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if frames.is_empty() {
                    continue;
                }
                let payload = &frames[next % frames.len()];
                if send_interleaved_frame(&mut socket, 0, payload).await.is_err() {
                    break;
                }
                next += 1;
            }
            res = socket.read(&mut drain) => {
                if res.unwrap_or(0) == 0 {
                    break;
                }
            }
        }
    }
}

#[cfg(feature = "proxy-rtsp")]
async fn send_interleaved_frame(
    socket: &mut TcpStream,
    channel: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let len = payload.len().min(u16::MAX as usize);
    let mut packet = Vec::with_capacity(4 + len);
    packet.push(b'$');
    packet.push(channel);
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&payload[..len]);
    socket.write_all(&packet).await
}

#[cfg(feature = "proxy-rtsp")]
async fn read_rtsp_request(socket: &mut TcpStream) -> String {
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

#[cfg(feature = "proxy-rtsp")]
fn extract_cseq(request: &str) -> Option<u32> {
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

#[cfg(feature = "proxy-rtsp")]
async fn write_rtsp_response(
    socket: &mut TcpStream,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    write_rtsp_status_response(socket, 200, "OK", cseq, headers, body).await;
}

#[cfg(feature = "proxy-rtsp")]
async fn write_rtsp_status_response(
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

#[cfg(feature = "proxy-rtsp")]
fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(feature = "proxy-rtsp")]
fn parse_content_length(header_text: &str) -> usize {
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
