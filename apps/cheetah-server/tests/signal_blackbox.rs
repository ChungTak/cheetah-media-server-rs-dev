//! Native HTTP black-box tests for the full `cheetah-server` binary.
//!
//! These tests spawn the real server process, configure the native HTTP adapter
//! to allow anonymous requests, and exercise the control/media endpoints over
//! TCP. They are intentionally independent of any in-process engine handle.
//!
//! `cheetah-server` 的 native HTTP 黑盒测试。本测试启动真实的服务器进程，
//! 通过 TCP 访问控制/媒体端点，不依赖进程内引擎句柄。

mod fixtures;

#[cfg(feature = "rtp")]
use std::net::SocketAddr;
#[cfg(feature = "rtp")]
use std::time::Duration;

use fixtures::*;
#[cfg(feature = "rtp")]
use tokio::net::UdpSocket;
#[cfg(feature = "rtp")]
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

    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();
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
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();
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
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();
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
    let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();

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
