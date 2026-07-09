use std::sync::Arc;

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;

mod common;
use common::*;

#[tokio::test(flavor = "current_thread")]
async fn failed_transition_and_session_mismatch_do_not_corrupt_play_session() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 140u32;
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

    let uri = format!("rtsp://{listen}/live/state-mapping-play");

    let mut publisher = connect_with_retry(listen).await;
    let announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &announce).await;
    let announce_resp = read_response(&mut publisher, "MAP-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "MAP-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "MAP-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "MAP-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    assert!(player_session.ends_with(";timeout=140"));

    let player_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut player, &player_setup).await;
    let player_setup_resp = read_response(&mut player, "MAP-PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);

    let play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=7.000-")],
        &[],
    );
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "MAP-PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);
    assert_eq!(play_resp.header("Session"), Some(player_session.as_str()));
    assert_eq!(play_resp.header("Range"), Some("npt=7.000-"));

    let publish_packet_1 = build_publish_h264_rtp(6000, 450_000, 0x1234_5678);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_1).await;

    let first_payload = loop {
        let (channel, payload) = read_interleaved_frame(&mut player, "MAP-PLAYER-RTP-1").await;
        match channel {
            2 => break payload,
            3 => continue,
            _ => panic!("unexpected interleaved channel {channel}"),
        }
    };
    let first_forwarded = RtpPacket::parse(&first_payload).expect("parse first forwarded rtp");
    assert_eq!(first_forwarded.header.payload_type, 96);
    assert!(!first_forwarded.payload.is_empty());

    let invalid_record = build_request("RECORD", &uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &invalid_record).await;
    let invalid_record_resp = read_response(&mut player, "MAP-PLAYER-INVALID-RECORD").await;
    assert_eq!(invalid_record_resp.status_code, 455);
    assert_eq!(
        invalid_record_resp.body, b"RECORD requires ANNOUNCE/SETUP",
        "invalid RECORD in play mode must return 455 and keep existing state"
    );

    let player_session_token = player_session
        .split(';')
        .next()
        .expect("player session token");
    let wrong_session = format!("{player_session_token}-mismatch;timeout=140");
    let mismatched_play = build_request("PLAY", &uri, 5, Some(&wrong_session), &[], &[]);
    write_request(&mut player, &mismatched_play).await;
    let mismatched_play_resp = read_response(&mut player, "MAP-PLAYER-MISMATCH-PLAY").await;
    assert_eq!(mismatched_play_resp.status_code, 454);
    assert_eq!(
        mismatched_play_resp.body, b"session id mismatch",
        "session mismatch must return explicit 454"
    );

    let publish_packet_2 = build_publish_h264_rtp(6001, 453_600, 0x1234_5678);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_2).await;

    let second_payload = loop {
        let (channel, payload) = read_interleaved_frame(&mut player, "MAP-PLAYER-RTP-2").await;
        match channel {
            2 => break payload,
            3 => continue,
            _ => panic!("unexpected interleaved channel {channel}"),
        }
    };
    let second_forwarded = RtpPacket::parse(&second_payload).expect("parse second forwarded rtp");
    assert_eq!(second_forwarded.header.payload_type, 96);
    assert_ne!(
        second_forwarded.header.sequence_number, first_forwarded.header.sequence_number,
        "invalid request must not terminate existing play task"
    );

    let pause = build_request("PAUSE", &uri, 6, Some(&player_session), &[], &[]);
    write_request(&mut player, &pause).await;
    let pause_resp = read_response(&mut player, "MAP-PLAYER-PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Session"), Some(player_session.as_str()));
    assert_eq!(
        pause_resp.header("Range"),
        Some("npt=7.000-"),
        "pause range must keep last play result after failed requests"
    );

    let keepalive = build_request("GET_PARAMETER", &uri, 7, Some(&player_session), &[], &[]);
    write_request(&mut player, &keepalive).await;
    let keepalive_resp = read_response(&mut player, "MAP-PLAYER-GET_PARAMETER").await;
    assert_eq!(keepalive_resp.status_code, 200);
    assert_eq!(
        keepalive_resp.header("Session"),
        Some(player_session.as_str())
    );

    let player_teardown = build_request("TEARDOWN", &uri, 8, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "MAP-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "MAP-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}
