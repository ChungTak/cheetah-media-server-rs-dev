// 来源：原 tests/keepalive.rs，按场景拆分。

use std::sync::Arc;
use std::time::Duration;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::net::UdpSocket;

mod common;
use common::*;

#[tokio::test(flavor = "current_thread")]
async fn play_multitrack_setup_includes_both_tracks_in_rtp_info() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
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

    let uri = format!("rtsp://{listen}/live/play-multitrack-rtp-info");

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
    let announce_resp = read_response(&mut publisher, "MULTI-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup_publish = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup_publish).await;
    let setup_publish_resp = read_response(&mut publisher, "MULTI-PUBLISHER-SETUP").await;
    assert_eq!(setup_publish_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "MULTI-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "MULTI-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let video_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind video rtp");
    let video_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind video rtcp");
    let video_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        video_rtp.local_addr().expect("video rtp addr").port(),
        video_rtcp.local_addr().expect("video rtcp addr").port()
    );
    let setup_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", &video_transport)],
        &[],
    );
    write_request(&mut player, &setup_video).await;
    let setup_video_resp = read_response(&mut player, "MULTI-PLAYER-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);
    let setup_video_transport_resp = setup_video_resp
        .header("Transport")
        .expect("video setup transport");
    let (_video_server_rtp_port, video_server_rtcp_port) =
        parse_transport_server_ports(setup_video_transport_resp).expect("parse video server ports");

    let audio_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtp");
    let audio_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtcp");
    let audio_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        audio_rtp.local_addr().expect("audio rtp addr").port(),
        audio_rtcp.local_addr().expect("audio rtcp addr").port()
    );
    let setup_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&player_session),
        &[("Transport", &audio_transport)],
        &[],
    );
    write_request(&mut player, &setup_audio).await;
    let setup_audio_resp = read_response(&mut player, "MULTI-PLAYER-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);
    let setup_audio_transport_resp = setup_audio_resp
        .header("Transport")
        .expect("audio setup transport");
    let (_audio_server_rtp_port, audio_server_rtcp_port) =
        parse_transport_server_ports(setup_audio_transport_resp).expect("parse audio server ports");

    let play = build_request(
        "PLAY",
        &uri,
        4,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "MULTI-PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);
    let rtp_info = play_resp.header("RTP-Info").expect("RTP-Info header");
    let rtp_info_entries: Vec<&str> = rtp_info.split(',').collect();
    assert_eq!(
        rtp_info_entries.len(),
        2,
        "expected 2 RTP-Info entries, got: {rtp_info}"
    );
    assert!(
        rtp_info.contains("trackID=0"),
        "missing trackID=0 in {rtp_info}"
    );
    assert!(
        rtp_info.contains("trackID=1"),
        "missing trackID=1 in {rtp_info}"
    );

    let player_teardown = build_request("TEARDOWN", &uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "MULTI-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);
    let (video_bye_from, video_bye_payload) =
        wait_for_rtcp_packet_type(&video_rtcp, video_server_rtcp_port, 203).await;
    assert_eq!(video_bye_from.port(), video_server_rtcp_port);
    assert!(video_bye_payload.len() >= 8);
    assert_eq!(video_bye_payload[1], 203);
    let (audio_bye_from, audio_bye_payload) =
        wait_for_rtcp_packet_type(&audio_rtcp, audio_server_rtcp_port, 203).await;
    assert_eq!(audio_bye_from.port(), audio_server_rtcp_port);
    assert!(audio_bye_payload.len() >= 8);
    assert_eq!(audio_bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "MULTI-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn play_multitrack_pause_play_teardown_emits_single_bye_per_track() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
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

    let uri = format!("rtsp://{listen}/live/play-multitrack-pause-play-bye");

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
    let announce_resp = read_response(&mut publisher, "M2-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup_publish = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup_publish).await;
    let setup_publish_resp = read_response(&mut publisher, "M2-PUBLISHER-SETUP").await;
    assert_eq!(setup_publish_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "M2-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "M2-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let video_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind video rtp");
    let video_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind video rtcp");
    let video_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        video_rtp.local_addr().expect("video rtp addr").port(),
        video_rtcp.local_addr().expect("video rtcp addr").port()
    );
    let setup_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", &video_transport)],
        &[],
    );
    write_request(&mut player, &setup_video).await;
    let setup_video_resp = read_response(&mut player, "M2-PLAYER-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);
    let setup_video_transport_resp = setup_video_resp
        .header("Transport")
        .expect("video setup transport");
    let (_video_server_rtp_port, video_server_rtcp_port) =
        parse_transport_server_ports(setup_video_transport_resp).expect("parse video server ports");

    let audio_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtp");
    let audio_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtcp");
    let audio_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        audio_rtp.local_addr().expect("audio rtp addr").port(),
        audio_rtcp.local_addr().expect("audio rtcp addr").port()
    );
    let setup_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&player_session),
        &[("Transport", &audio_transport)],
        &[],
    );
    write_request(&mut player, &setup_audio).await;
    let setup_audio_resp = read_response(&mut player, "M2-PLAYER-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);
    let setup_audio_transport_resp = setup_audio_resp
        .header("Transport")
        .expect("audio setup transport");
    let (_audio_server_rtp_port, audio_server_rtcp_port) =
        parse_transport_server_ports(setup_audio_transport_resp).expect("parse audio server ports");

    let play1 = build_request(
        "PLAY",
        &uri,
        4,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &play1).await;
    let play1_resp = read_response(&mut player, "M2-PLAYER-PLAY-1").await;
    assert_eq!(play1_resp.status_code, 200);
    let play1_rtp_info = play1_resp.header("RTP-Info").expect("play1 RTP-Info");
    assert!(play1_rtp_info.contains("trackID=0"));
    assert!(play1_rtp_info.contains("trackID=1"));

    let pause = build_request("PAUSE", &uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &pause).await;
    let pause_resp = read_response(&mut player, "M2-PLAYER-PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Range"), Some("npt=0.000-"));
    drain_udp_socket(&video_rtcp).await;
    drain_udp_socket(&audio_rtcp).await;

    let play2 = build_request(
        "PLAY",
        &uri,
        6,
        Some(&player_session),
        &[("Range", "npt=10.000-")],
        &[],
    );
    write_request(&mut player, &play2).await;
    let play2_resp = read_response(&mut player, "M2-PLAYER-PLAY-2").await;
    assert_eq!(play2_resp.status_code, 200);
    assert_eq!(play2_resp.header("Range"), Some("npt=10.000-"));
    let play2_rtp_info = play2_resp.header("RTP-Info").expect("play2 RTP-Info");
    assert!(play2_rtp_info.contains("trackID=0"));
    assert!(play2_rtp_info.contains("trackID=1"));
    drain_udp_socket(&video_rtcp).await;
    drain_udp_socket(&audio_rtcp).await;

    let player_teardown = build_request("TEARDOWN", &uri, 7, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "M2-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);

    let (video_bye_from, video_bye_payload) =
        wait_for_rtcp_packet_type(&video_rtcp, video_server_rtcp_port, 203).await;
    assert_eq!(video_bye_from.port(), video_server_rtcp_port);
    assert!(video_bye_payload.len() >= 8);
    assert_eq!(video_bye_payload[1], 203);
    assert_no_udp_packet_for_duration(
        &video_rtcp,
        Duration::from_millis(150),
        "video rtcp should only receive one BYE after teardown",
    )
    .await;

    let (audio_bye_from, audio_bye_payload) =
        wait_for_rtcp_packet_type(&audio_rtcp, audio_server_rtcp_port, 203).await;
    assert_eq!(audio_bye_from.port(), audio_server_rtcp_port);
    assert!(audio_bye_payload.len() >= 8);
    assert_eq!(audio_bye_payload[1], 203);
    assert_no_udp_packet_for_duration(
        &audio_rtcp,
        Duration::from_millis(150),
        "audio rtcp should only receive one BYE after teardown",
    )
    .await;

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "M2-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}
