//! Native HTTP black-box tests for the full `cheetah-server` binary.
//!
//! These tests spawn the real server process, configure the native HTTP adapter
//! to allow anonymous requests, and exercise the control/media endpoints over
//! TCP. They are intentionally independent of any in-process engine handle.
//!
//! `cheetah-server` 的 native HTTP 黑盒测试。本测试启动真实的服务器进程，
//! 通过 TCP 访问控制/媒体端点，不依赖进程内引擎句柄。

mod fixtures;

use std::time::Duration;

use fixtures::*;
#[cfg(feature = "proxy-rtsp")]
use tokio::net::TcpListener;
#[cfg(feature = "rtp")]
use tokio::net::UdpSocket;
use tokio::time::sleep;

#[tokio::test(flavor = "current_thread")]
async fn server_exposes_media_capabilities_over_http() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_blackbox_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let (status, body) = http_get("127.0.0.1", control_port, "/api/v1/media/capabilities").await;
    assert_eq!(
        status,
        200,
        "capabilities should return 200: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(&body);
    assert!(
        json.get("capabilities").is_some(),
        "capabilities field missing: {json}"
    );
    assert!(
        json.get("version").is_some(),
        "version field missing: {json}"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "rtp")]
async fn gb28181_can_ingest_ps_over_udp_and_query_online() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_gb28181_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "gb28181";
    let stream = "cam_001";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), SSRC, INGEST_PT, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open receiver failed: {}",
        String::from_utf8_lossy(&body)
    );

    let session = parse_json(&body);
    let session_id = session["session_id"].as_str().unwrap();
    let recv_port = session["local_port"].as_u64().unwrap() as u16;
    assert_ne!(recv_port, 0);

    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let socket = bind_udp_socket().await;

    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_video_frame(0)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        2,
        80,
        INGEST_PT,
    )
    .await;

    let mut seq: u16 = 3;
    let mut pts: i64 = 100_000;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            SSRC,
            seq,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts + 80)),
            SSRC,
            seq,
            ((pts + 80) / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;

        let (status, body) = http_get(
            "127.0.0.1",
            control_port,
            &format!("/api/v1/media/{vhost}/{app}/{stream}/online"),
        )
        .await;
        if status == 200 {
            let value = parse_json(&body);
            if value["online"].as_str() == Some("online") {
                break;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }

    wait_for_online(
        "127.0.0.1",
        control_port,
        vhost,
        app,
        stream,
        Duration::from_secs(5),
    )
    .await;

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media failed: {}",
        String::from_utf8_lossy(&body)
    );
    let info = parse_json(&body);
    assert!(
        info["tracks"].as_array().unwrap().len() >= 2,
        "expected video and audio tracks: {info}"
    );

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}/keyframe"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "keyframe request failed: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete rtp session failed: {} {}",
        status,
        String::from_utf8_lossy(&body)
    );

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{session_id}"),
    )
    .await;
    assert!(
        status == 404 || status == 410 || parse_json(&body)["state"].as_str() == Some("Closed"),
        "session should be gone or closed: {status} {}",
        String::from_utf8_lossy(&body)
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(all(feature = "rtp", feature = "record"))]
async fn gb28181_can_record_mp4_and_download_file() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_gb28181_rec_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "gb28181";
    let stream = "cam_rec_001";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), SSRC, INGEST_PT, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open receiver failed: {}",
        String::from_utf8_lossy(&body)
    );

    let session = parse_json(&body);
    let session_id = session["session_id"].as_str().unwrap();
    let recv_port = session["local_port"].as_u64().unwrap() as u16;
    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let socket = bind_udp_socket().await;

    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_video_frame(0)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    send_rtp(
        &socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        2,
        80,
        INGEST_PT,
    )
    .await;

    wait_for_online(
        "127.0.0.1",
        control_port,
        vhost,
        app,
        stream,
        Duration::from_secs(5),
    )
    .await;

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/record/tasks",
        start_record_request(key.clone()),
    )
    .await;
    assert_eq!(
        status,
        200,
        "start record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let record_task = parse_json(&body);
    let task_id = record_task["task_id"].as_str().unwrap();
    assert_eq!(record_task["state"].as_str(), Some("running"));

    let mut seq: u16 = 3;
    let mut pts: i64 = 100_000;
    for _ in 0..10 {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            SSRC,
            seq,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts + 80)),
            SSRC,
            seq,
            ((pts + 80) / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;
        sleep(Duration::from_millis(50)).await;
    }

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/record/tasks/{task_id}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "stop record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let stopped = parse_json(&body);
    assert!(
        stopped["state"].as_str() == Some("completed")
            || stopped["state"].as_str() == Some("stopping"),
        "unexpected record state: {stopped}"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut file_handle = None;
    while tokio::time::Instant::now() < deadline && file_handle.is_none() {
        let (status, body) = http_get(
            "127.0.0.1",
            control_port,
            &format!("/api/v1/record/files?app={app}&stream={stream}"),
        )
        .await;
        assert_eq!(
            status,
            200,
            "list record files failed: {}",
            String::from_utf8_lossy(&body)
        );
        let page = parse_json(&body);
        if let Some(items) = page["items"].as_array() {
            if let Some(first) = items.first() {
                file_handle = first["path_handle"].as_str().map(|s| s.to_string());
            }
        }
        if file_handle.is_none() {
            sleep(Duration::from_millis(100)).await;
        }
    }
    let file_handle = file_handle.expect("record file should be produced");

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/files/{file_handle}/download"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "download record file failed: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        body.starts_with(b"ftyp") || body.windows(4).any(|w| w == b"ftyp"),
        "downloaded file does not look like MP4: {} bytes",
        body.len()
    );

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete rtp session returned {status}"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "rtp")]
async fn gb28181_can_egress_stream_over_rtp_sender() {
    let control_port = free_local_port().await;
    let temp_dir =
        std::env::temp_dir().join(format!("cheetah_gb28181_send_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "gb28181";
    let stream = "cam_send_001";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), SSRC, INGEST_PT, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open receiver failed: {}",
        String::from_utf8_lossy(&body)
    );
    let recv_session = parse_json(&body);
    let recv_session_id = recv_session["session_id"].as_str().unwrap();
    let recv_port = recv_session["local_port"].as_u64().unwrap() as u16;
    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let send_socket = bind_udp_socket().await;

    send_rtp(
        &send_socket,
        recv_addr,
        mux_ps_frame(&make_video_frame(0)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    send_rtp(
        &send_socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        2,
        80,
        INGEST_PT,
    )
    .await;

    wait_for_online(
        "127.0.0.1",
        control_port,
        vhost,
        app,
        stream,
        Duration::from_secs(5),
    )
    .await;

    let egress_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink_addr = egress_socket.local_addr().unwrap();
    let sender_body = serde_json::json!({
        "media_key": key,
        "destination_endpoint": sink_addr.to_string(),
        "ssrc": 0xDEADBEEFu32,
        "payload_type": 96,
        "codec_hint": "ps",
        "mode": "active",
    });

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/senders",
        sender_body,
    )
    .await;
    assert_eq!(
        status,
        200,
        "open sender failed: {}",
        String::from_utf8_lossy(&body)
    );
    let sender = parse_json(&body);
    let sender_session_id = sender["session_id"].as_str().unwrap();
    assert!(sender_session_id.starts_with("send:"));

    let mut seq: u16 = 3;
    let mut pts: i64 = 100_000;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_rtp = false;
    while tokio::time::Instant::now() < deadline && !saw_rtp {
        send_rtp(
            &send_socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            SSRC,
            seq,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;

        if let Some((header, _payload, _addr)) =
            recv_rtp(&egress_socket, Duration::from_millis(200)).await
        {
            assert_eq!(header.version, 2);
            assert_eq!(header.payload_type, 96);
            saw_rtp = true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_rtp, "expected RTP packets from the sender");

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{sender_session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete sender session returned {status}"
    );

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{recv_session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete receiver session returned {status}"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "rtp")]
async fn gb28181_can_do_talkback_audio_round_trip() {
    let control_port = free_local_port().await;
    let temp_dir =
        std::env::temp_dir().join(format!("cheetah_gb28181_talk_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "gb28181";
    let stream = "cam_talk_001";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), SSRC, INGEST_PT, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open receiver failed: {}",
        String::from_utf8_lossy(&body)
    );
    let recv_session = parse_json(&body);
    let recv_session_id = recv_session["session_id"].as_str().unwrap();
    let recv_port = recv_session["local_port"].as_u64().unwrap() as u16;
    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();

    let talk_socket = bind_udp_socket().await;
    let src_addr = talk_socket.local_addr().unwrap();

    send_rtp(
        &talk_socket,
        recv_addr,
        mux_ps_frame(&make_audio_frame(80)),
        SSRC,
        1,
        0,
        INGEST_PT,
    )
    .await;
    sleep(Duration::from_millis(300)).await;

    let talk_body = serde_json::json!({
        "media_key": key,
        "destination_endpoint": src_addr.to_string(),
        "ssrc": SSRC,
        "payload_type": 8,
        "codec_hint": "raw_audio",
        "mode": "talk",
    });
    let (status, body) =
        http_post("127.0.0.1", control_port, "/api/v1/rtp/senders", talk_body).await;
    assert_eq!(
        status,
        200,
        "open talkback failed: {}",
        String::from_utf8_lossy(&body)
    );

    let mut saw_talkback = false;
    let mut seq: u16 = 2;
    let mut pts: i64 = 160;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !saw_talkback {
        send_rtp(
            &talk_socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts)),
            SSRC,
            seq,
            (pts / 100 * 9) as u32,
            INGEST_PT,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 80;

        if let Some((header, _payload, addr)) =
            recv_rtp(&talk_socket, Duration::from_millis(100)).await
        {
            if addr == recv_addr && header.payload_type == 8 {
                saw_talkback = true;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_talkback, "expected talkback RTP from the receiver");

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{recv_session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete talk session returned {status}"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "proxy-rtsp")]
async fn onvif_rtsp_proxy_default_ssrf_is_rejected() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_onvif_ssrf_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(
        control_port,
        &temp_dir,
        &onvif_config(&temp_dir, false, &[]),
    );
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let key = media_key("__defaultVhost__", "onvif", "cam_ssrf");
    let (status, _body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/proxies/pull",
        pull_proxy_request("rtsp://127.0.0.1:554/live/reject", key),
    )
    .await;

    assert_ne!(
        status, 200,
        "SSRF should reject loopback RTSP source without allowlist"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(all(feature = "proxy-rtsp", feature = "snapshot", feature = "record"))]
async fn onvif_can_pull_rtsp_proxy_and_use_media_operations() {
    if !ffmpeg_available() {
        return;
    }
    let control_port = free_local_port().await;
    let rtsp_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_onvif_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let (sps, pps, idr) = generate_h264_keyframe();
    let sdp = h264_sdp(&sps, &pps);
    let mut frames = Vec::new();
    for i in 0..40u32 {
        let ts = 90_000u32.wrapping_add(i * 4_500);
        let seq = ((i + 1) % (u16::MAX as u32 + 1)) as u16;
        frames.push(h264_rtp_packet(&idr, seq, ts, 0x11223344, 96));
    }

    let source_uri = format!("rtsp://127.0.0.1:{rtsp_port}/live/onvif-source");
    let rtsp_listener = TcpListener::bind(format!("127.0.0.1:{rtsp_port}"))
        .await
        .unwrap();
    let rtsp_handle = tokio::spawn(async move {
        let (socket, _) = rtsp_listener.accept().await.unwrap();
        run_interleaved_rtsp_source(
            socket,
            format!("rtsp://127.0.0.1:{rtsp_port}/live/onvif-source"),
            sdp,
            frames,
            Duration::from_millis(50),
        )
        .await;
    });

    let config_path = write_config(
        control_port,
        &temp_dir,
        &onvif_config(&temp_dir, true, &["127.0.0.0/8"]),
    );
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "onvif";
    let stream = "cam_001";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/proxies/pull",
        pull_proxy_request(&source_uri, key.clone()),
    )
    .await;
    assert_eq!(
        status,
        200,
        "create RTSP pull proxy failed: {}",
        String::from_utf8_lossy(&body)
    );
    let proxy = parse_json(&body);
    let proxy_id = proxy["proxy_id"].as_str().unwrap();

    wait_for_proxy_connected("127.0.0.1", control_port, proxy_id, Duration::from_secs(10)).await;

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/proxies/pull/{proxy_id}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get proxy failed: {}",
        String::from_utf8_lossy(&body)
    );
    let proxy = parse_json(&body);
    assert_eq!(proxy["state"].as_str(), Some("connected"));

    wait_for_online(
        "127.0.0.1",
        control_port,
        vhost,
        app,
        stream,
        Duration::from_secs(10),
    )
    .await;

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media failed: {}",
        String::from_utf8_lossy(&body)
    );
    let info = parse_json(&body);
    let tracks = info["tracks"].as_array().unwrap();
    assert!(
        tracks.iter().any(|t| t["media_type"] == "video"),
        "expected a video track: {info}"
    );

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}/urls"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media urls failed: {}",
        String::from_utf8_lossy(&body)
    );
    let urls = parse_json(&body)["urls"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!urls.is_empty(), "expected at least one playback URL");
    assert!(
        urls.iter()
            .all(|u| u["available"].as_bool().unwrap_or(false)),
        "all playback URLs should be available: {urls:?}"
    );

    let (status, _body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}/keyframe"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 200, "keyframe request failed");

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/snapshots",
        snapshot_request(key.clone()),
    )
    .await;
    assert_eq!(
        status,
        200,
        "take snapshot failed: {}",
        String::from_utf8_lossy(&body)
    );
    let snap = parse_json(&body);
    let snapshot_id = snap["snapshot_id"].as_str().unwrap();
    assert_eq!(snap["state"].as_str(), Some("completed"));

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/snapshots/{snapshot_id}/download"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "download snapshot failed: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        body.starts_with(&[0xff, 0xd8]),
        "downloaded snapshot is not a JPEG: {} bytes",
        body.len()
    );

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/record/tasks",
        start_record_request(key.clone()),
    )
    .await;
    assert_eq!(
        status,
        200,
        "start record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let record_task = parse_json(&body);
    let task_id = record_task["task_id"].as_str().unwrap();
    assert_eq!(record_task["state"].as_str(), Some("running"));

    sleep(Duration::from_secs(2)).await;

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/record/tasks/{task_id}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "stop record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let stopped = parse_json(&body);
    assert!(
        ["completed", "stopping"].contains(&stopped["state"].as_str().unwrap_or("")),
        "unexpected record state: {stopped}"
    );

    let file_handle = wait_for_record_file(
        "127.0.0.1",
        control_port,
        app,
        stream,
        Duration::from_secs(10),
    )
    .await;

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/files/{file_handle}/download"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "download record file failed: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        body.windows(4).any(|w| w == b"ftyp"),
        "downloaded file does not look like MP4: {} bytes",
        body.len()
    );

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/proxies/pull/{proxy_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete proxy returned {status}"
    );

    wait_for_stream_offline(
        "127.0.0.1",
        control_port,
        vhost,
        app,
        stream,
        Duration::from_secs(10),
    )
    .await;

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/record/files/{file_handle}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete record file returned {status}"
    );

    let (status, _body) = http_delete_with_body(
        "127.0.0.1",
        control_port,
        "/api/v1/snapshots/directories",
        serde_json::json!({
            "media_key": key,
        }),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete snapshots returned {status}"
    );

    rtsp_handle.abort();
    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(all(feature = "rtp", feature = "record"))]
async fn homekit_can_ingest_and_egress_over_rtp() {
    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_homekit_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(control_port, &temp_dir, &gb28181_config(&temp_dir));
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "homekit";
    let stream = "cam_001";
    let key = media_key(vhost, app, stream);

    let recv_ssrc = 0xAABBCCDDu32;
    let recv_pt = 100u8;

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), recv_ssrc, recv_pt, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open RTP receiver failed: {}",
        String::from_utf8_lossy(&body)
    );
    let session = parse_json(&body);
    let recv_session_id = session["session_id"].as_str().unwrap();
    let recv_port = session["local_port"].as_u64().unwrap() as u16;
    assert_ne!(recv_port, 0);

    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let socket = bind_udp_socket().await;

    let mut seq: u16 = 1;
    let mut pts: i64 = 100_000;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            recv_ssrc,
            seq,
            (pts / 100 * 9) as u32,
            recv_pt,
        )
        .await;
        seq = seq.wrapping_add(1);
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts + 80)),
            recv_ssrc,
            seq,
            ((pts + 80) / 100 * 9) as u32,
            recv_pt,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;

        let (status, body) = http_get(
            "127.0.0.1",
            control_port,
            &format!("/api/v1/media/{vhost}/{app}/{stream}/online"),
        )
        .await;
        if status == 200 && parse_json(&body)["online"].as_str() == Some("online") {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media failed: {}",
        String::from_utf8_lossy(&body)
    );
    let info = parse_json(&body);
    let tracks = info["tracks"].as_array().unwrap();
    assert!(
        tracks.iter().any(|t| t["media_type"] == "video"),
        "expected video track: {info}"
    );
    assert!(
        tracks.iter().any(|t| t["media_type"] == "audio"),
        "expected audio track: {info}"
    );

    let (status, _body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}/keyframe"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 200, "keyframe request failed");

    let sink = bind_udp_socket().await;
    let sink_addr = sink.local_addr().unwrap();
    let sink_port = sink_addr.port();

    let send_ssrc = 0x11223344u32;
    let send_pt = 100u8;
    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/senders",
        rtp_sender_request(
            key.clone(),
            &format!("127.0.0.1:{sink_port}"),
            send_ssrc,
            send_pt,
            "ps",
        ),
    )
    .await;
    assert_eq!(
        status,
        200,
        "create RTP sender failed: {}",
        String::from_utf8_lossy(&body)
    );
    let sender = parse_json(&body);
    let sender_id = sender["session_id"].as_str().unwrap();

    for _ in 0..20 {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            recv_ssrc,
            seq,
            (pts / 100 * 9) as u32,
            recv_pt,
        )
        .await;
        seq = seq.wrapping_add(1);
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_audio_frame(pts + 80)),
            recv_ssrc,
            seq,
            ((pts + 80) / 100 * 9) as u32,
            recv_pt,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;
        sleep(Duration::from_millis(20)).await;
    }

    let mut received = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Some((header, payload, _src)) = recv_rtp(&sink, Duration::from_millis(200)).await {
            assert_eq!(header.ssrc, send_ssrc, "sender used unexpected SSRC");
            assert!(
                header.payload_type > 0,
                "RTP sender should set a valid payload type"
            );
            assert!(!payload.is_empty(), "RTP payload should not be empty");
            received += 1;
            if received >= 5 {
                break;
            }
        }
    }
    assert!(
        received >= 5,
        "expected RTP sender to egress packets, got {received}"
    );

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{sender_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete RTP sender returned {status}"
    );

    sleep(Duration::from_millis(300)).await;

    for _ in 0..10 {
        send_rtp(
            &socket,
            recv_addr,
            mux_ps_frame(&make_video_frame(pts)),
            recv_ssrc,
            seq,
            (pts / 100 * 9) as u32,
            recv_pt,
        )
        .await;
        seq = seq.wrapping_add(1);
        pts += 100_000;
    }

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{sender_id}"),
    )
    .await;
    assert_eq!(
        status,
        404,
        "RTP sender session should be deleted: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{recv_session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete RTP receiver returned {status}"
    );

    stop_server(child).await;
}

#[tokio::test(flavor = "current_thread")]
#[cfg(all(feature = "rtp", feature = "record"))]
async fn matter_can_subscribe_to_webhook_events_and_cancel() {
    let (webhook_addr, mut events) = start_webhook_receiver().await;
    let webhook_url = format!("http://{webhook_addr}/");

    let control_port = free_local_port().await;
    let temp_dir = std::env::temp_dir().join(format!("cheetah_matter_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let config_path = write_config(
        control_port,
        &temp_dir,
        &matter_config(&temp_dir, &webhook_url),
    );
    let child = spawn_server(&config_path, &temp_dir).await;
    wait_for_server(control_port).await;

    let vhost = "__defaultVhost__";
    let app = "matter";
    let stream = "device_1";
    let key = media_key(vhost, app, stream);

    let (status, body) = http_get("127.0.0.1", control_port, "/api/v1/media/capabilities").await;
    assert_eq!(
        status,
        200,
        "capabilities query failed: {}",
        String::from_utf8_lossy(&body)
    );
    let caps = parse_json(&body);
    assert!(
        caps.get("capabilities").is_some() && caps.get("version").is_some(),
        "capabilities should include fields: {caps}"
    );

    let ssrc = 0xDEADBEEFu32;
    let pt = 100u8;
    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/rtp/receivers",
        rtp_receiver_request(key.clone(), Some(0), ssrc, pt, "ps"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "open RTP receiver failed: {}",
        String::from_utf8_lossy(&body)
    );
    let session = parse_json(&body);
    let recv_session_id = session["session_id"].as_str().unwrap();
    let recv_port = session["local_port"].as_u64().unwrap() as u16;

    let recv_addr = format!("127.0.0.1:{recv_port}")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let socket = bind_udp_socket().await;

    let sender_handle = tokio::spawn(async move {
        let mut seq: u16 = 1;
        let mut pts: i64 = 100_000;
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        loop {
            ticker.tick().await;
            send_rtp(
                &socket,
                recv_addr,
                mux_ps_frame(&make_video_frame(pts)),
                ssrc,
                seq,
                (pts / 100 * 9) as u32,
                pt,
            )
            .await;
            seq = seq.wrapping_add(1);
            send_rtp(
                &socket,
                recv_addr,
                mux_ps_frame(&make_audio_frame(pts + 80)),
                ssrc,
                seq,
                ((pts + 80) / 100 * 9) as u32,
                pt,
            )
            .await;
            seq = seq.wrapping_add(1);
            pts += 100_000;
        }
    });

    let online_event =
        recv_event_of_type(&mut events, "stream_online_changed", Duration::from_secs(5)).await;
    let payload = &online_event["payload"];
    assert_eq!(payload["header"]["media_key"]["stream"], "device_1");
    assert_eq!(payload["online"], "online");
    assert!(!payload["header"]["event_id"].as_str().unwrap().is_empty());
    assert!(payload["header"]["occurred_at"].as_i64().unwrap_or(0) > 0);

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media failed: {}",
        String::from_utf8_lossy(&body)
    );
    let info = parse_json(&body);
    let tracks = info["tracks"].as_array().unwrap();
    assert!(tracks.iter().any(|t| t["media_type"] == "video"));
    assert!(tracks.iter().any(|t| t["media_type"] == "audio"));

    let (status, body) = http_get(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/media/{vhost}/{app}/{stream}/urls"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "get media urls failed: {}",
        String::from_utf8_lossy(&body)
    );
    let urls = parse_json(&body);
    assert!(
        urls["urls"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "playback URLs should be present"
    );

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/record/tasks",
        start_record_request(key.clone()),
    )
    .await;
    assert_eq!(
        status,
        200,
        "start record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let record = parse_json(&body);
    let record_task_id = record["task_id"].as_str().unwrap();

    sleep(Duration::from_millis(500)).await;

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/record/tasks/{record_task_id}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "stop record failed: {}",
        String::from_utf8_lossy(&body)
    );
    let stopped = parse_json(&body);
    assert!(
        ["completed", "stopping"].contains(&stopped["state"].as_str().unwrap_or("")),
        "unexpected record state: {stopped}"
    );

    let record_event =
        recv_event_of_type(&mut events, "record_completed", Duration::from_secs(15)).await;
    let rpayload = &record_event["payload"];
    assert_eq!(rpayload["task_id"], record_task_id);
    assert!(!rpayload["file_path"].as_str().unwrap().is_empty());
    assert!(!rpayload["header"]["event_id"].as_str().unwrap().is_empty());
    assert!(rpayload["file_size"].as_u64().unwrap_or(0) > 0);

    if cfg!(feature = "snapshot") && ffmpeg_available() {
        let (status, body) = http_post(
            "127.0.0.1",
            control_port,
            "/api/v1/snapshots",
            serde_json::json!({
                "media_key": key,
                "format": "jpg",
                "timeout_ms": 15000,
            }),
        )
        .await;
        assert_eq!(
            status,
            200,
            "take snapshot failed: {}",
            String::from_utf8_lossy(&body)
        );
        let snap = parse_json(&body);
        let snapshot_id = snap["snapshot_id"].as_str().unwrap();

        let snapshot_event =
            recv_event_of_type(&mut events, "snapshot_completed", Duration::from_secs(10)).await;
        let spayload = &snapshot_event["payload"];
        assert_eq!(spayload["snapshot_id"], snapshot_id);
        assert!(!spayload["path_handle"].as_str().unwrap().is_empty());
        assert!(!spayload["header"]["event_id"].as_str().unwrap().is_empty());
    }

    while events.try_recv().is_ok() {}

    let (status, body) = http_patch(
        "127.0.0.1",
        control_port,
        "/api/v1/config/modules/webhook-dispatcher",
        serde_json::json!({"patch": {"profiles": []}}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "patch webhook-dispatcher config failed: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, body) = http_post(
        "127.0.0.1",
        control_port,
        "/api/v1/record/tasks",
        start_record_request(key.clone()),
    )
    .await;
    assert_eq!(status, 200, "start second record failed");
    let second_record_id = parse_json(&body)["task_id"].as_str().unwrap().to_string();

    sleep(Duration::from_millis(200)).await;

    let (status, _body) = http_post(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/record/tasks/{second_record_id}/stop"),
        serde_json::json!({}),
    )
    .await;
    assert!(status == 200, "stop second record returned {status}");

    if cfg!(feature = "snapshot") && ffmpeg_available() {
        let (status, _body) = http_post(
            "127.0.0.1",
            control_port,
            "/api/v1/snapshots",
            serde_json::json!({
                "media_key": key,
                "format": "jpg",
                "timeout_ms": 15000,
            }),
        )
        .await;
        assert_eq!(status, 200, "take second snapshot failed");
    }

    let no_event = tokio::time::timeout(Duration::from_secs(3), events.recv()).await;
    assert!(
        no_event.is_err(),
        "webhook events should stop after cancelling profile"
    );

    sender_handle.abort();

    let (status, _body) = http_delete(
        "127.0.0.1",
        control_port,
        &format!("/api/v1/rtp/sessions/{recv_session_id}"),
    )
    .await;
    assert!(
        status == 200 || status == 204,
        "delete RTP receiver returned {status}"
    );

    stop_server(child).await;
}
