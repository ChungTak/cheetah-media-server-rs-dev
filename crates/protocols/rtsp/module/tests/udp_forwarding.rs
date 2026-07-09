// 来源：原 tests/keepalive.rs，按场景拆分。

use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

mod common;
use common::*;

#[tokio::test(flavor = "current_thread")]
async fn udp_publish_pause_record_controls_player_forwarding() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 100u32;
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

    let uri = format!("rtsp://{listen}/live/udp-publish-pause-forward");

    let mut publisher = connect_with_retry(listen).await;
    let publisher_announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &publisher_announce).await;
    let publisher_announce_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();
    assert!(publisher_session.ends_with(";timeout=100"));

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let publisher_rtp_port = publisher_rtp
        .local_addr()
        .expect("publisher rtp addr")
        .port();
    let publisher_rtcp_port = publisher_rtcp
        .local_addr()
        .expect("publisher rtcp addr")
        .port();
    let publisher_setup_transport =
        format!("RTP/AVP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port}");
    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", &publisher_setup_transport)],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "UDP-PF-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let client_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind client rtp");
    let client_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind client rtcp");
    let client_rtp_port = client_rtp.local_addr().expect("client rtp addr").port();
    let client_rtcp_port = client_rtcp.local_addr().expect("client rtcp addr").port();
    let player_setup_transport =
        format!("RTP/AVP;unicast;client_port={client_rtp_port}-{client_rtcp_port}");
    let player_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", &player_setup_transport)],
        &[],
    );
    write_request(&mut player, &player_setup).await;
    let player_setup_resp = read_response(&mut player, "UDP-PF-PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    let player_setup_transport_resp = player_setup_resp
        .header("Transport")
        .expect("player setup transport");
    let (player_server_rtp_port, player_server_rtcp_port) =
        parse_transport_server_ports(player_setup_transport_resp)
            .expect("parse player server ports");

    let player_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &player_play).await;
    let player_play_resp = read_response(&mut player, "UDP-PF-PLAYER-PLAY").await;
    assert_eq!(player_play_resp.status_code, 200);

    let before_pause = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtp,
        player_server_rtp_port,
    )
    .await;
    assert_eq!(before_pause.header.payload_type, 96);

    let publisher_pause = build_request("PAUSE", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_pause).await;
    let publisher_pause_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-PAUSE").await;
    assert_eq!(publisher_pause_resp.status_code, 200);

    drain_udp_socket(&client_rtp).await;
    drain_udp_socket(&client_rtcp).await;

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        8000,
        990_000,
        0x6677_8899,
    )
    .await;
    assert_no_udp_packet_for_duration(
        &client_rtp,
        Duration::from_millis(250),
        "publish paused should block player RTP forwarding",
    )
    .await;
    assert_no_udp_packet_for_duration(
        &client_rtcp,
        Duration::from_millis(250),
        "publish paused should block player RTCP emission",
    )
    .await;

    let publisher_record2 = build_request("RECORD", &uri, 5, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record2).await;
    let publisher_record2_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-RECORD-2").await;
    assert_eq!(publisher_record2_resp.status_code, 200);

    let after_resume = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtp,
        player_server_rtp_port,
    )
    .await;
    assert_eq!(after_resume.header.payload_type, 96);

    let player_teardown = build_request("TEARDOWN", &uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "UDP-PF-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);
    let (_bye_from, bye_payload) =
        wait_for_rtcp_packet_type(&client_rtcp, player_server_rtcp_port, 203).await;
    assert!(bye_payload.len() >= 8);
    assert_eq!(bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 6, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "UDP-PF-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_teardown_one_player_keeps_other_player_forwarding() {
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

    let uri = format!("rtsp://{listen}/live/udp-two-players-isolation");

    let mut publisher = connect_with_retry(listen).await;
    let publisher_announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &publisher_announce).await;
    let publisher_announce_resp = read_response(&mut publisher, "ISOL-PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let publisher_rtp_port = publisher_rtp
        .local_addr()
        .expect("publisher rtp addr")
        .port();
    let publisher_rtcp_port = publisher_rtcp
        .local_addr()
        .expect("publisher rtcp addr")
        .port();
    let publisher_setup_transport =
        format!("RTP/AVP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port}");
    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", &publisher_setup_transport)],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "ISOL-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "ISOL-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player1 = connect_with_retry(listen).await;
    let player1_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player1, &player1_describe).await;
    let player1_describe_resp = read_response(&mut player1, "ISOL-PLAYER1-DESCRIBE").await;
    assert_eq!(player1_describe_resp.status_code, 200);
    let player1_session = player1_describe_resp
        .header("Session")
        .expect("player1 session")
        .to_string();
    let player1_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtp");
    let player1_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtcp");
    let player1_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player1_rtp.local_addr().expect("player1 rtp addr").port(),
        player1_rtcp.local_addr().expect("player1 rtcp addr").port()
    );
    let player1_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player1_session),
        &[("Transport", &player1_setup_transport)],
        &[],
    );
    write_request(&mut player1, &player1_setup).await;
    let player1_setup_resp = read_response(&mut player1, "ISOL-PLAYER1-SETUP").await;
    assert_eq!(player1_setup_resp.status_code, 200);
    let player1_setup_transport_resp = player1_setup_resp
        .header("Transport")
        .expect("player1 setup transport");
    let (player1_server_rtp_port, player1_server_rtcp_port) =
        parse_transport_server_ports(player1_setup_transport_resp)
            .expect("parse player1 server ports");
    let player1_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player1_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player1, &player1_play).await;
    let player1_play_resp = read_response(&mut player1, "ISOL-PLAYER1-PLAY").await;
    assert_eq!(player1_play_resp.status_code, 200);

    let mut player2 = connect_with_retry(listen).await;
    let player2_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player2, &player2_describe).await;
    let player2_describe_resp = read_response(&mut player2, "ISOL-PLAYER2-DESCRIBE").await;
    assert_eq!(player2_describe_resp.status_code, 200);
    let player2_session = player2_describe_resp
        .header("Session")
        .expect("player2 session")
        .to_string();
    let player2_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtp");
    let player2_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtcp");
    let player2_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player2_rtp.local_addr().expect("player2 rtp addr").port(),
        player2_rtcp.local_addr().expect("player2 rtcp addr").port()
    );
    let player2_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player2_session),
        &[("Transport", &player2_setup_transport)],
        &[],
    );
    write_request(&mut player2, &player2_setup).await;
    let player2_setup_resp = read_response(&mut player2, "ISOL-PLAYER2-SETUP").await;
    assert_eq!(player2_setup_resp.status_code, 200);
    let player2_setup_transport_resp = player2_setup_resp
        .header("Transport")
        .expect("player2 setup transport");
    let (player2_server_rtp_port, player2_server_rtcp_port) =
        parse_transport_server_ports(player2_setup_transport_resp)
            .expect("parse player2 server ports");
    let player2_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player2_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player2, &player2_play).await;
    let player2_play_resp = read_response(&mut player2, "ISOL-PLAYER2-PLAY").await;
    assert_eq!(player2_play_resp.status_code, 200);

    let player1_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player1_rtp,
        player1_server_rtp_port,
    )
    .await;
    assert_eq!(player1_first.header.payload_type, 96);
    let player2_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player2_rtp,
        player2_server_rtp_port,
    )
    .await;
    assert_eq!(player2_first.header.payload_type, 96);

    let player1_teardown = build_request("TEARDOWN", &uri, 4, Some(&player1_session), &[], &[]);
    write_request(&mut player1, &player1_teardown).await;
    let player1_teardown_resp = read_response(&mut player1, "ISOL-PLAYER1-TEARDOWN").await;
    assert_eq!(player1_teardown_resp.status_code, 200);
    let (_player1_bye_from, player1_bye_payload) =
        wait_for_rtcp_packet_type(&player1_rtcp, player1_server_rtcp_port, 203).await;
    assert!(player1_bye_payload.len() >= 8);
    assert_eq!(player1_bye_payload[1], 203);

    drain_udp_socket(&player1_rtp).await;
    drain_udp_socket(&player1_rtcp).await;
    drain_udp_socket(&player2_rtp).await;
    drain_udp_socket(&player2_rtcp).await;

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        9100,
        1_080_000,
        0x7788_99aa,
    )
    .await;
    assert_no_udp_packet_for_duration(
        &player1_rtp,
        Duration::from_millis(250),
        "teardown player1 should stop player1 rtp forwarding",
    )
    .await;
    assert_no_udp_packet_for_duration(
        &player1_rtcp,
        Duration::from_millis(250),
        "teardown player1 should stop player1 rtcp emission",
    )
    .await;

    let player2_after_player1_teardown = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player2_rtp,
        player2_server_rtp_port,
    )
    .await;
    assert_eq!(player2_after_player1_teardown.header.payload_type, 96);

    let player2_keepalive =
        build_request("GET_PARAMETER", &uri, 4, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_keepalive).await;
    let player2_keepalive_resp = read_response(&mut player2, "ISOL-PLAYER2-GET_PARAMETER").await;
    assert_eq!(player2_keepalive_resp.status_code, 200);
    assert_eq!(
        player2_keepalive_resp.header("Session"),
        Some(player2_session.as_str())
    );

    let player2_teardown = build_request("TEARDOWN", &uri, 5, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_teardown).await;
    let player2_teardown_resp = read_response(&mut player2, "ISOL-PLAYER2-TEARDOWN").await;
    assert_eq!(player2_teardown_resp.status_code, 200);
    let (_player2_bye_from, player2_bye_payload) =
        wait_for_rtcp_packet_type(&player2_rtcp, player2_server_rtcp_port, 203).await;
    assert!(player2_bye_payload.len() >= 8);
    assert_eq!(player2_bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "ISOL-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_pause_one_player_keeps_other_player_forwarding() {
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

    let uri = format!("rtsp://{listen}/live/udp-two-players-pause-isolation");

    let mut publisher = connect_with_retry(listen).await;
    let publisher_announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &publisher_announce).await;
    let publisher_announce_resp =
        read_response(&mut publisher, "PAUSE-ISO-PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let publisher_rtp_port = publisher_rtp
        .local_addr()
        .expect("publisher rtp addr")
        .port();
    let publisher_rtcp_port = publisher_rtcp
        .local_addr()
        .expect("publisher rtcp addr")
        .port();
    let publisher_setup_transport =
        format!("RTP/AVP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port}");
    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", &publisher_setup_transport)],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "PAUSE-ISO-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "PAUSE-ISO-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player1 = connect_with_retry(listen).await;
    let player1_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player1, &player1_describe).await;
    let player1_describe_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-DESCRIBE").await;
    assert_eq!(player1_describe_resp.status_code, 200);
    let player1_session = player1_describe_resp
        .header("Session")
        .expect("player1 session")
        .to_string();
    let player1_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtp");
    let player1_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtcp");
    let player1_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player1_rtp.local_addr().expect("player1 rtp addr").port(),
        player1_rtcp.local_addr().expect("player1 rtcp addr").port()
    );
    let player1_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player1_session),
        &[("Transport", &player1_setup_transport)],
        &[],
    );
    write_request(&mut player1, &player1_setup).await;
    let player1_setup_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-SETUP").await;
    assert_eq!(player1_setup_resp.status_code, 200);
    let player1_setup_transport_resp = player1_setup_resp
        .header("Transport")
        .expect("player1 setup transport");
    let (player1_server_rtp_port, player1_server_rtcp_port) =
        parse_transport_server_ports(player1_setup_transport_resp)
            .expect("parse player1 server ports");
    let player1_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player1_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player1, &player1_play).await;
    let player1_play_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-PLAY").await;
    assert_eq!(player1_play_resp.status_code, 200);

    let mut player2 = connect_with_retry(listen).await;
    let player2_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player2, &player2_describe).await;
    let player2_describe_resp = read_response(&mut player2, "PAUSE-ISO-PLAYER2-DESCRIBE").await;
    assert_eq!(player2_describe_resp.status_code, 200);
    let player2_session = player2_describe_resp
        .header("Session")
        .expect("player2 session")
        .to_string();
    let player2_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtp");
    let player2_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtcp");
    let player2_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player2_rtp.local_addr().expect("player2 rtp addr").port(),
        player2_rtcp.local_addr().expect("player2 rtcp addr").port()
    );
    let player2_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player2_session),
        &[("Transport", &player2_setup_transport)],
        &[],
    );
    write_request(&mut player2, &player2_setup).await;
    let player2_setup_resp = read_response(&mut player2, "PAUSE-ISO-PLAYER2-SETUP").await;
    assert_eq!(player2_setup_resp.status_code, 200);
    let player2_setup_transport_resp = player2_setup_resp
        .header("Transport")
        .expect("player2 setup transport");
    let (player2_server_rtp_port, player2_server_rtcp_port) =
        parse_transport_server_ports(player2_setup_transport_resp)
            .expect("parse player2 server ports");
    let player2_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player2_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player2, &player2_play).await;
    let player2_play_resp = read_response(&mut player2, "PAUSE-ISO-PLAYER2-PLAY").await;
    assert_eq!(player2_play_resp.status_code, 200);

    let player1_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player1_rtp,
        player1_server_rtp_port,
    )
    .await;
    assert_eq!(player1_first.header.payload_type, 96);
    let player2_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player2_rtp,
        player2_server_rtp_port,
    )
    .await;
    assert_eq!(player2_first.header.payload_type, 96);

    let player1_pause = build_request("PAUSE", &uri, 4, Some(&player1_session), &[], &[]);
    write_request(&mut player1, &player1_pause).await;
    let player1_pause_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-PAUSE").await;
    assert_eq!(player1_pause_resp.status_code, 200);
    assert_eq!(player1_pause_resp.header("Range"), Some("npt=0.000-"));
    drain_udp_socket(&player1_rtp).await;
    drain_udp_socket(&player1_rtcp).await;
    drain_udp_socket(&player2_rtp).await;
    drain_udp_socket(&player2_rtcp).await;

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        9200,
        1_120_000,
        0x8899_aabb,
    )
    .await;
    assert_no_udp_packet_for_duration(
        &player1_rtp,
        Duration::from_millis(250),
        "paused player1 should stop player1 rtp forwarding",
    )
    .await;
    assert_no_udp_packet_for_duration(
        &player1_rtcp,
        Duration::from_millis(250),
        "paused player1 should stop player1 rtcp emission",
    )
    .await;
    let player2_after_player1_pause = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player2_rtp,
        player2_server_rtp_port,
    )
    .await;
    assert_eq!(player2_after_player1_pause.header.payload_type, 96);

    let player1_play2 = build_request(
        "PLAY",
        &uri,
        5,
        Some(&player1_session),
        &[("Range", "npt=5.000-")],
        &[],
    );
    write_request(&mut player1, &player1_play2).await;
    let player1_play2_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-PLAY-2").await;
    assert_eq!(player1_play2_resp.status_code, 200);
    assert_eq!(player1_play2_resp.header("Range"), Some("npt=5.000-"));
    let player1_after_resume = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player1_rtp,
        player1_server_rtp_port,
    )
    .await;
    assert_eq!(player1_after_resume.header.payload_type, 96);

    let player2_keepalive =
        build_request("GET_PARAMETER", &uri, 4, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_keepalive).await;
    let player2_keepalive_resp =
        read_response(&mut player2, "PAUSE-ISO-PLAYER2-GET_PARAMETER").await;
    assert_eq!(player2_keepalive_resp.status_code, 200);
    assert_eq!(
        player2_keepalive_resp.header("Session"),
        Some(player2_session.as_str())
    );

    let player1_teardown = build_request("TEARDOWN", &uri, 6, Some(&player1_session), &[], &[]);
    write_request(&mut player1, &player1_teardown).await;
    let player1_teardown_resp = read_response(&mut player1, "PAUSE-ISO-PLAYER1-TEARDOWN").await;
    assert_eq!(player1_teardown_resp.status_code, 200);
    let (_player1_bye_from, player1_bye_payload) =
        wait_for_rtcp_packet_type(&player1_rtcp, player1_server_rtcp_port, 203).await;
    assert!(player1_bye_payload.len() >= 8);
    assert_eq!(player1_bye_payload[1], 203);

    let player2_teardown = build_request("TEARDOWN", &uri, 5, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_teardown).await;
    let player2_teardown_resp = read_response(&mut player2, "PAUSE-ISO-PLAYER2-TEARDOWN").await;
    assert_eq!(player2_teardown_resp.status_code, 200);
    let (_player2_bye_from, player2_bye_payload) =
        wait_for_rtcp_packet_type(&player2_rtcp, player2_server_rtcp_port, 203).await;
    assert!(player2_bye_payload.len() >= 8);
    assert_eq!(player2_bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp =
        read_response(&mut publisher, "PAUSE-ISO-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_disconnect_one_player_keeps_other_player_forwarding() {
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

    let uri = format!("rtsp://{listen}/live/udp-two-players-disconnect-isolation");

    let mut publisher = connect_with_retry(listen).await;
    let publisher_announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut publisher, &publisher_announce).await;
    let publisher_announce_resp =
        read_response(&mut publisher, "DISC-ISO-PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let publisher_rtp_port = publisher_rtp
        .local_addr()
        .expect("publisher rtp addr")
        .port();
    let publisher_rtcp_port = publisher_rtcp
        .local_addr()
        .expect("publisher rtcp addr")
        .port();
    let publisher_setup_transport =
        format!("RTP/AVP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port}");
    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", &publisher_setup_transport)],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "DISC-ISO-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "DISC-ISO-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player1 = connect_with_retry(listen).await;
    let player1_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player1, &player1_describe).await;
    let player1_describe_resp = read_response(&mut player1, "DISC-ISO-PLAYER1-DESCRIBE").await;
    assert_eq!(player1_describe_resp.status_code, 200);
    let player1_session = player1_describe_resp
        .header("Session")
        .expect("player1 session")
        .to_string();
    let player1_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtp");
    let player1_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player1 rtcp");
    let player1_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player1_rtp.local_addr().expect("player1 rtp addr").port(),
        player1_rtcp.local_addr().expect("player1 rtcp addr").port()
    );
    let player1_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player1_session),
        &[("Transport", &player1_setup_transport)],
        &[],
    );
    write_request(&mut player1, &player1_setup).await;
    let player1_setup_resp = read_response(&mut player1, "DISC-ISO-PLAYER1-SETUP").await;
    assert_eq!(player1_setup_resp.status_code, 200);
    let player1_setup_transport_resp = player1_setup_resp
        .header("Transport")
        .expect("player1 setup transport");
    let (player1_server_rtp_port, _player1_server_rtcp_port) =
        parse_transport_server_ports(player1_setup_transport_resp)
            .expect("parse player1 server ports");
    let player1_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player1_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player1, &player1_play).await;
    let player1_play_resp = read_response(&mut player1, "DISC-ISO-PLAYER1-PLAY").await;
    assert_eq!(player1_play_resp.status_code, 200);

    let mut player2 = connect_with_retry(listen).await;
    let player2_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player2, &player2_describe).await;
    let player2_describe_resp = read_response(&mut player2, "DISC-ISO-PLAYER2-DESCRIBE").await;
    assert_eq!(player2_describe_resp.status_code, 200);
    let player2_session = player2_describe_resp
        .header("Session")
        .expect("player2 session")
        .to_string();
    let player2_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtp");
    let player2_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player2 rtcp");
    let player2_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        player2_rtp.local_addr().expect("player2 rtp addr").port(),
        player2_rtcp.local_addr().expect("player2 rtcp addr").port()
    );
    let player2_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player2_session),
        &[("Transport", &player2_setup_transport)],
        &[],
    );
    write_request(&mut player2, &player2_setup).await;
    let player2_setup_resp = read_response(&mut player2, "DISC-ISO-PLAYER2-SETUP").await;
    assert_eq!(player2_setup_resp.status_code, 200);
    let player2_setup_transport_resp = player2_setup_resp
        .header("Transport")
        .expect("player2 setup transport");
    let (player2_server_rtp_port, player2_server_rtcp_port) =
        parse_transport_server_ports(player2_setup_transport_resp)
            .expect("parse player2 server ports");
    let player2_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player2_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player2, &player2_play).await;
    let player2_play_resp = read_response(&mut player2, "DISC-ISO-PLAYER2-PLAY").await;
    assert_eq!(player2_play_resp.status_code, 200);

    let player1_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player1_rtp,
        player1_server_rtp_port,
    )
    .await;
    assert_eq!(player1_first.header.payload_type, 96);
    let player2_first = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &player2_rtp,
        player2_server_rtp_port,
    )
    .await;
    assert_eq!(player2_first.header.payload_type, 96);

    drop(player1);
    sleep(Duration::from_millis(120)).await;
    drain_udp_socket(&player1_rtp).await;
    drain_udp_socket(&player1_rtcp).await;
    drain_udp_socket(&player2_rtp).await;
    drain_udp_socket(&player2_rtcp).await;

    let mut player1_isolated = false;
    let mut player2_still_forwarding = false;
    let mut player2_recv_buf = [0u8; 2048];
    let mut player1_recv_buf = [0u8; 2048];
    for attempt in 0..6u16 {
        let seq = 9300u16.wrapping_add(attempt);
        let ts = 1_160_000u32.wrapping_add(u32::from(attempt) * 3600u32);
        send_publish_udp_rtp_frame(
            &publisher_rtp,
            publisher_server_rtp_port,
            seq,
            ts,
            0x99aa_bbcc,
        )
        .await;

        let player2_ok = match timeout(
            Duration::from_millis(250),
            player2_rtp.recv_from(&mut player2_recv_buf),
        )
        .await
        {
            Ok(Ok((n, from))) => {
                from.port() == player2_server_rtp_port
                    && RtpPacket::parse(&player2_recv_buf[..n]).is_some()
            }
            Ok(Err(err)) => panic!("recv player2 rtp failed: {err}"),
            Err(_) => false,
        };
        if !player2_ok {
            continue;
        }
        player2_still_forwarding = true;
        let player1_silent = timeout(
            Duration::from_millis(120),
            player1_rtp.recv_from(&mut player1_recv_buf),
        )
        .await
        .is_err();
        if player1_silent {
            player1_isolated = true;
            break;
        }
        drain_udp_socket(&player1_rtp).await;
    }
    assert!(
        player2_still_forwarding,
        "player2 should still receive rtp after player1 disconnect"
    );
    assert!(
        player1_isolated,
        "player1 still received rtp after TCP disconnect and cleanup window"
    );

    let player2_keepalive =
        build_request("GET_PARAMETER", &uri, 4, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_keepalive).await;
    let player2_keepalive_resp =
        read_response(&mut player2, "DISC-ISO-PLAYER2-GET_PARAMETER").await;
    assert_eq!(player2_keepalive_resp.status_code, 200);
    assert_eq!(
        player2_keepalive_resp.header("Session"),
        Some(player2_session.as_str())
    );

    let player2_teardown = build_request("TEARDOWN", &uri, 5, Some(&player2_session), &[], &[]);
    write_request(&mut player2, &player2_teardown).await;
    let player2_teardown_resp = read_response(&mut player2, "DISC-ISO-PLAYER2-TEARDOWN").await;
    assert_eq!(player2_teardown_resp.status_code, 200);
    let (_player2_bye_from, player2_bye_payload) =
        wait_for_rtcp_packet_type(&player2_rtcp, player2_server_rtcp_port, 203).await;
    assert!(player2_bye_payload.len() >= 8);
    assert_eq!(player2_bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp =
        read_response(&mut publisher, "DISC-ISO-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}
