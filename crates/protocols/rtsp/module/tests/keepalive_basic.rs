// 来源：原 tests/keepalive.rs，按场景拆分。

use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;

mod common;
use common::*;

#[tokio::test(flavor = "current_thread")]
async fn get_and_set_parameter_keepalive_roundtrip() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 75u32;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    session_timeout_secs: {session_timeout_secs}\n"
    );

    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(&config_yaml)
        .expect("load rtsp module config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mut stream = connect_with_retry(listen).await;
    let uri = format!("rtsp://{listen}/live/keepalive");

    let options = build_request("OPTIONS", &uri, 1, None, &[], &[]);
    write_request(&mut stream, &options).await;
    let options_resp = read_response(&mut stream, "OPTIONS").await;
    assert_eq!(options_resp.status_code, 200);

    let announce = build_request(
        "ANNOUNCE",
        &uri,
        2,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut stream, &announce).await;
    let announce_resp = read_response(&mut stream, "ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session_header = announce_resp
        .header("Session")
        .expect("announce response should include session")
        .to_string();
    assert!(session_header.ends_with(";timeout=75"));

    let get_parameter_body = "ping\r\nposition\r\n";
    let get_parameter = build_request(
        "GET_PARAMETER",
        &uri,
        3,
        Some(&session_header),
        &[("Content-Type", "text/parameters")],
        get_parameter_body.as_bytes(),
    );
    write_request(&mut stream, &get_parameter).await;
    let get_parameter_resp = read_response(&mut stream, "GET_PARAMETER").await;
    assert_eq!(get_parameter_resp.status_code, 200);
    assert_eq!(
        get_parameter_resp.header("Session"),
        Some(session_header.as_str())
    );
    assert_eq!(
        get_parameter_resp.header("Content-Type"),
        Some("text/parameters")
    );
    assert_eq!(get_parameter_resp.body, get_parameter_body.as_bytes());

    let set_parameter_body = "volume: 1\r\n";
    let set_parameter = build_request(
        "SET_PARAMETER",
        &uri,
        4,
        Some(&session_header),
        &[("Content-Type", "text/parameters")],
        set_parameter_body.as_bytes(),
    );
    write_request(&mut stream, &set_parameter).await;
    let set_parameter_resp = read_response(&mut stream, "SET_PARAMETER").await;
    assert_eq!(set_parameter_resp.status_code, 200);
    assert_eq!(
        set_parameter_resp.header("Session"),
        Some(session_header.as_str())
    );
    assert!(set_parameter_resp.body.is_empty());

    let teardown = build_request("TEARDOWN", &uri, 5, Some(&session_header), &[], &[]);
    write_request(&mut stream, &teardown).await;
    let teardown_resp = read_response(&mut stream, "TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);
    assert_eq!(
        teardown_resp.header("Session"),
        Some(session_header.as_str())
    );

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn publish_pause_record_keepalive_roundtrip() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 90u32;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    session_timeout_secs: {session_timeout_secs}\n"
    );

    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(&config_yaml)
        .expect("load rtsp module config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mut stream = connect_with_retry(listen).await;
    let uri = format!("rtsp://{listen}/live/publish-keepalive");

    let options = build_request("OPTIONS", &uri, 1, None, &[], &[]);
    write_request(&mut stream, &options).await;
    let options_resp = read_response(&mut stream, "OPTIONS").await;
    assert_eq!(options_resp.status_code, 200);

    let announce = build_request(
        "ANNOUNCE",
        &uri,
        2,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut stream, &announce).await;
    let announce_resp = read_response(&mut stream, "ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session_header = announce_resp
        .header("Session")
        .expect("announce response should include session")
        .to_string();
    assert!(session_header.ends_with(";timeout=90"));

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        3,
        Some(&session_header),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut stream, &setup).await;
    let setup_resp = read_response(&mut stream, "SETUP").await;
    assert_eq!(setup_resp.status_code, 200);
    assert_eq!(setup_resp.header("Session"), Some(session_header.as_str()));
    assert!(setup_resp
        .header("Transport")
        .is_some_and(|v| v.contains("interleaved=0-1")));

    let record = build_request("RECORD", &uri, 4, Some(&session_header), &[], &[]);
    write_request(&mut stream, &record).await;
    let record_resp = read_response(&mut stream, "RECORD").await;
    assert_eq!(record_resp.status_code, 200);
    assert_eq!(record_resp.header("Session"), Some(session_header.as_str()));

    let sr_sender_ssrc = 0x1122_3344u32;
    let sr_payload = build_rtcp_sender_report_packet(sr_sender_ssrc, 90_000, 1, 1200);
    send_interleaved_frame(&mut stream, 1, &sr_payload).await;
    let (rr_channel, rr_payload) = read_interleaved_frame(&mut stream, "PUBLISHER-RR").await;
    assert_eq!(rr_channel, 1);
    assert!(
        rr_payload.len() >= 12,
        "rtcp rr payload too short: {}",
        rr_payload.len()
    );
    assert_eq!(rr_payload[1], 201, "expected RTCP RR packet type");
    assert_eq!(
        read_u32_be(&rr_payload[8..12]).expect("rr sender ssrc"),
        sr_sender_ssrc,
        "rr report block should target sr sender ssrc"
    );

    let pause = build_request("PAUSE", &uri, 5, Some(&session_header), &[], &[]);
    write_request(&mut stream, &pause).await;
    let pause_resp = read_response(&mut stream, "PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Session"), Some(session_header.as_str()));
    assert!(pause_resp.header("Range").is_none());

    let get_parameter = build_request("GET_PARAMETER", &uri, 6, Some(&session_header), &[], &[]);
    write_request(&mut stream, &get_parameter).await;
    let get_parameter_resp = read_response(&mut stream, "GET_PARAMETER").await;
    assert_eq!(get_parameter_resp.status_code, 200);
    assert_eq!(
        get_parameter_resp.header("Session"),
        Some(session_header.as_str())
    );
    assert!(get_parameter_resp.header("Content-Type").is_none());
    assert!(get_parameter_resp.body.is_empty());

    let record2 = build_request("RECORD", &uri, 7, Some(&session_header), &[], &[]);
    write_request(&mut stream, &record2).await;
    let record2_resp = read_response(&mut stream, "RECORD-2").await;
    assert_eq!(record2_resp.status_code, 200);
    assert_eq!(
        record2_resp.header("Session"),
        Some(session_header.as_str())
    );

    let teardown = build_request("TEARDOWN", &uri, 8, Some(&session_header), &[], &[]);
    write_request(&mut stream, &teardown).await;
    let teardown_resp = read_response(&mut stream, "TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);
    assert_eq!(
        teardown_resp.header("Session"),
        Some(session_header.as_str())
    );

    engine.stop().await;
}
