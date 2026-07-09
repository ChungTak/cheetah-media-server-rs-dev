use std::sync::Arc;
use std::time::{Duration, Instant};

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::time::sleep;

mod common;
use common::*;

async fn start_engine_with_rtsp_config(config_yaml: &str) -> cheetah_engine::Engine {
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");
    engine
}

async fn publish_stream_minimal(publisher: &mut tokio::net::TcpStream, uri: &str) {
    let announce = build_request(
        "ANNOUNCE",
        uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(publisher, &announce).await;
    let announce_resp = read_response(publisher, "WAIT-PUBLISH-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publish session")
        .to_string();

    let setup_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(publisher, &setup_video).await;
    let setup_video_resp = read_response(publisher, "WAIT-PUBLISH-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);

    let setup_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(publisher, &setup_audio).await;
    let setup_audio_resp = read_response(publisher, "WAIT-PUBLISH-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);

    let record = build_request("RECORD", uri, 4, Some(&session), &[], &[]);
    write_request(publisher, &record).await;
    let record_resp = read_response(publisher, "WAIT-PUBLISH-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    // Ensure stream snapshot is materialized for DESCRIBE by ingesting one RTP frame.
    let publish_rtp = build_publish_h264_rtp(3000, 90_000, 0x1122_3344);
    send_interleaved_frame(publisher, 0, &publish_rtp).await;
}

#[tokio::test(flavor = "current_thread")]
async fn describe_waits_for_stream_until_publisher_online() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let wait_timeout_ms = 800u64;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    play_wait_source_timeout_ms: {wait_timeout_ms}\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;

    let uri = format!("rtsp://{listen}/live/describe-wait-online");
    let mut player = connect_with_retry(listen).await;

    let describe_task = tokio::spawn(async move {
        let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
        let started = Instant::now();
        write_request(&mut player, &describe).await;
        let resp = read_response(&mut player, "WAIT-PLAYER-DESCRIBE").await;
        (resp, started.elapsed())
    });

    sleep(Duration::from_millis(120)).await;
    let publish_uri = format!("rtsp://{listen}/live/describe-wait-online");
    let mut publisher = connect_with_retry(listen).await;
    publish_stream_minimal(&mut publisher, &publish_uri).await;

    let (describe_resp, elapsed) = describe_task.await.expect("describe task join");
    assert_eq!(describe_resp.status_code, 200);
    assert!(
        elapsed >= Duration::from_millis(100),
        "describe should wait for late publisher, elapsed={elapsed:?}"
    );

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn pipelined_setup_waits_for_pending_describe_state() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let wait_timeout_ms = 800u64;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    play_wait_source_timeout_ms: {wait_timeout_ms}\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;

    let uri = format!("rtsp://{listen}/live/describe-pipeline");
    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        None,
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut player, &describe).await;
    write_request(&mut player, &setup).await;

    sleep(Duration::from_millis(120)).await;
    let publish_uri = format!("rtsp://{listen}/live/describe-pipeline");
    let mut publisher = connect_with_retry(listen).await;
    publish_stream_minimal(&mut publisher, &publish_uri).await;

    let describe_resp = read_response(&mut player, "PIPELINED-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let setup_resp = read_response(&mut player, "PIPELINED-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn describe_wait_timeout_returns_404_when_source_never_online() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let wait_timeout_ms = 220u64;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    play_wait_source_timeout_ms: {wait_timeout_ms}\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;

    let uri = format!("rtsp://{listen}/live/describe-timeout");
    let mut player = connect_with_retry(listen).await;

    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    let started = Instant::now();
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "WAIT-PLAYER-DESCRIBE-TIMEOUT").await;
    let elapsed = started.elapsed();

    assert_eq!(describe_resp.status_code, 404);
    assert_eq!(describe_resp.body, b"stream not found");
    assert!(
        elapsed >= Duration::from_millis(180),
        "describe should wait until timeout before 404, elapsed={elapsed:?}"
    );

    engine.stop().await;
}
