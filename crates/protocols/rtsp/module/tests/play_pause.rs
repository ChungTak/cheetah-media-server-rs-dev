// 来源：原 tests/keepalive.rs，按场景拆分。

use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tokio::time::timeout;

mod common;
use common::*;

fn parse_single_rtp_info_seq_rtptime(header: &str) -> Option<(u16, u32)> {
    let entry = header.split(',').next()?.trim();
    let mut seq = None;
    let mut rtptime = None;
    for part in entry.split(';') {
        let (name, value) = part.split_once('=')?;
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("seq") {
            seq = value.parse::<u16>().ok();
        } else if name.eq_ignore_ascii_case("rtptime") {
            rtptime = value.parse::<u32>().ok();
        }
    }
    Some((seq?, rtptime?))
}

#[tokio::test(flavor = "current_thread")]
async fn play_mode_keepalive_preserves_session_and_range() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 105u32;
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

    let uri = format!("rtsp://{listen}/live/play-keepalive");

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
    let publisher_announce_resp = read_response(&mut publisher, "PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();
    assert!(publisher_session.ends_with(";timeout=105"));

    let publisher_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &publisher_setup).await;
    let publisher_setup_resp = read_response(&mut publisher, "PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    assert_eq!(
        publisher_setup_resp.header("Session"),
        Some(publisher_session.as_str())
    );

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;

    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    assert!(player_session.ends_with(";timeout=105"));
    assert_eq!(
        describe_resp.header("Content-Type"),
        Some("application/sdp")
    );

    let player_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut player, &player_setup).await;
    let player_setup_resp = read_response(&mut player, "PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    assert_eq!(
        player_setup_resp.header("Session"),
        Some(player_session.as_str())
    );

    let player_play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=5-"), ("Scale", "1.0")],
        &[],
    );
    write_request(&mut player, &player_play).await;
    let player_play_resp = read_response(&mut player, "PLAYER-PLAY").await;
    assert_eq!(player_play_resp.status_code, 200);
    assert_eq!(
        player_play_resp.header("Session"),
        Some(player_session.as_str())
    );
    assert_eq!(player_play_resp.header("Range"), Some("npt=5-"));
    assert_eq!(player_play_resp.header("Scale"), Some("1.0"));

    let keepalive_body = "position\r\n";
    let keepalive = build_request(
        "GET_PARAMETER",
        &uri,
        4,
        Some(&player_session),
        &[("Content-Type", "text/parameters")],
        keepalive_body.as_bytes(),
    );
    write_request(&mut player, &keepalive).await;
    let keepalive_resp = read_response(&mut player, "PLAYER-GET_PARAMETER").await;
    assert_eq!(keepalive_resp.status_code, 200);
    assert_eq!(
        keepalive_resp.header("Session"),
        Some(player_session.as_str())
    );
    assert_eq!(
        keepalive_resp.header("Content-Type"),
        Some("text/parameters")
    );
    assert_eq!(keepalive_resp.body, keepalive_body.as_bytes());

    let player_pause = build_request("PAUSE", &uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_pause).await;
    let player_pause_resp = read_response(&mut player, "PLAYER-PAUSE").await;
    assert_eq!(player_pause_resp.status_code, 200);
    assert_eq!(
        player_pause_resp.header("Session"),
        Some(player_session.as_str())
    );
    assert_eq!(player_pause_resp.header("Range"), Some("npt=5-"));
    assert_eq!(player_pause_resp.header("Scale"), Some("1.0"));

    let player_teardown = build_request("TEARDOWN", &uri, 6, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_interleaved_play_pause_play_rtp_rtcp_continuity() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 115u32;
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

    let uri = format!("rtsp://{listen}/live/tcp-play-pause-play");

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
    let publisher_announce_resp = read_response(&mut publisher, "TCP-P2-PUBLISHER-ANNOUNCE").await;
    assert_eq!(publisher_announce_resp.status_code, 200);
    let publisher_session = publisher_announce_resp
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
    let publisher_setup_resp = read_response(&mut publisher, "TCP-P2-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "TCP-P2-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "TCP-P2-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    assert!(player_session.ends_with(";timeout=115"));

    let player_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut player, &player_setup).await;
    let player_setup_resp = read_response(&mut player, "TCP-P2-PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    assert_eq!(
        player_setup_resp.header("Session"),
        Some(player_session.as_str())
    );
    assert!(player_setup_resp
        .header("Transport")
        .is_some_and(|v| v.contains("interleaved=2-3")));

    let play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "TCP-P2-PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);
    assert_eq!(play_resp.header("Session"), Some(player_session.as_str()));
    assert_eq!(play_resp.header("Range"), Some("npt=0.000-"));

    let publish_packet_1 = build_publish_h264_rtp(5000, 360_000, 0x0102_0304);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_1).await;

    let (first_channel, first_payload) =
        read_interleaved_frame(&mut player, "TCP-P2-PLAYER-RTP-1").await;
    assert_eq!(first_channel, 2);
    let first_rtp = RtpPacket::parse(&first_payload).expect("parse first forwarded rtp");
    assert_eq!(first_rtp.header.payload_type, 96);
    assert!(first_rtp.header.marker);
    assert!(!first_rtp.payload.is_empty());

    let (rtcp1_channel, rtcp1_payload) =
        read_interleaved_frame(&mut player, "TCP-P2-PLAYER-RTCP-1").await;
    assert_eq!(rtcp1_channel, 3);
    assert!(
        rtcp1_payload.len() >= 8,
        "rtcp1 too short: {}",
        rtcp1_payload.len()
    );
    let mut rtcp_types = vec![rtcp1_payload[1]];

    let publish_packet_1b = build_publish_h264_rtp(5001, 363_600, 0x0102_0304);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_1b).await;

    let mut forwarded_rtp_count = 1usize;
    for attempt in 0..4usize {
        let (channel, payload) =
            read_interleaved_frame(&mut player, &format!("TCP-P2-PLAYER-POST-RTCP-{attempt}"))
                .await;
        match channel {
            2 => {
                let packet = RtpPacket::parse(&payload).expect("parse additional forwarded rtp");
                assert_eq!(packet.header.payload_type, 96);
                assert!(packet.header.marker);
                forwarded_rtp_count = forwarded_rtp_count.saturating_add(1);
            }
            3 => {
                assert!(
                    payload.len() >= 8,
                    "additional rtcp too short: {}",
                    payload.len()
                );
                rtcp_types.push(payload[1]);
            }
            _ => panic!("unexpected interleaved channel {channel}"),
        }
        if forwarded_rtp_count >= 2 && rtcp_types.contains(&202) && rtcp_types.contains(&200) {
            break;
        }
    }
    assert!(
        forwarded_rtp_count >= 2,
        "expected at least 2 forwarded RTP packets, got {forwarded_rtp_count}"
    );
    assert!(
        rtcp_types.contains(&202),
        "expected RTCP SDES(202), got: {rtcp_types:?}"
    );
    assert!(
        rtcp_types.contains(&200),
        "expected RTCP SR(200), got: {rtcp_types:?}"
    );

    let pause = build_request("PAUSE", &uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &pause).await;
    let pause_resp = read_response(&mut player, "TCP-P2-PLAYER-PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Range"), Some("npt=0.000-"));

    let publish_packet_2 = build_publish_h264_rtp(5002, 367_200, 0x0102_0304);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_2).await;
    let mut paused_buf = [0u8; 1];
    match timeout(Duration::from_millis(250), player.read(&mut paused_buf)).await {
        Ok(Ok(n)) => panic!("unexpected interleaved payload while paused: {n} bytes"),
        Ok(Err(err)) => panic!("read player interleaved while paused failed: {err}"),
        Err(_) => {}
    }

    let resume_play = build_request(
        "PLAY",
        &uri,
        5,
        Some(&player_session),
        &[("Range", "npt=9.000-")],
        &[],
    );
    write_request(&mut player, &resume_play).await;
    let resume_play_resp = read_response(&mut player, "TCP-P2-PLAYER-PLAY-2").await;
    assert_eq!(resume_play_resp.status_code, 200);
    assert_eq!(resume_play_resp.header("Range"), Some("npt=9.000-"));
    let play2_rtp_info = resume_play_resp.header("RTP-Info").expect("play2 RTP-Info");
    let (_play2_seq, play2_rtptime) =
        parse_single_rtp_info_seq_rtptime(play2_rtp_info).expect("parse play2 RTP-Info");
    assert!(
        play2_rtptime > 0,
        "PLAY-2 RTP-Info should use non-zero runtime rtp timestamp, got: {play2_rtp_info}"
    );

    let publish_packet_3 = build_publish_h264_rtp(5003, 370_800, 0x0102_0304);
    send_interleaved_frame(&mut publisher, 0, &publish_packet_3).await;
    let (resumed_rtp_channel, resumed_rtp_payload) =
        read_interleaved_frame(&mut player, "TCP-P2-PLAYER-RTP-2").await;
    assert_eq!(resumed_rtp_channel, 2);
    let resumed_rtp = RtpPacket::parse(&resumed_rtp_payload).expect("parse resumed forwarded rtp");
    assert_eq!(resumed_rtp.header.payload_type, 96);
    assert!(resumed_rtp.header.marker);
    assert!(!resumed_rtp.payload.is_empty());

    let player_teardown = build_request("TEARDOWN", &uri, 6, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "TCP-P2-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "TCP-P2-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_play_keepalive_roundtrip() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 120u32;
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

    let uri = format!("rtsp://{listen}/live/udp-play-keepalive");

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
    let publisher_announce_resp = read_response(&mut publisher, "UDP-PUBLISHER-ANNOUNCE").await;
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
    let publisher_setup_resp = read_response(&mut publisher, "UDP-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    assert_eq!(
        publisher_setup_resp.header("Session"),
        Some(publisher_session.as_str())
    );
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    assert!(publisher_setup_transport_resp.contains("RTP/AVP;unicast"));
    assert!(publisher_setup_transport_resp.contains("server_port="));
    let (publisher_server_rtp_port, publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");
    assert!(publisher_server_rtp_port > 0);
    assert!(publisher_server_rtcp_port > 0);
    assert_ne!(publisher_server_rtp_port, publisher_server_rtcp_port);

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "UDP-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "UDP-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    assert!(player_session.ends_with(";timeout=120"));

    let client_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind client rtp");
    let client_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind client rtcp");
    let client_rtp_port = client_rtp.local_addr().expect("rtp addr").port();
    let client_rtcp_port = client_rtcp.local_addr().expect("rtcp addr").port();

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
    let player_setup_resp = read_response(&mut player, "UDP-PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    assert_eq!(
        player_setup_resp.header("Session"),
        Some(player_session.as_str())
    );
    let setup_transport_resp = player_setup_resp
        .header("Transport")
        .expect("setup transport");
    assert!(setup_transport_resp.contains("RTP/AVP;unicast"));
    assert!(setup_transport_resp.contains("server_port="));
    assert!(setup_transport_resp.contains("ssrc="));
    let play_ssrc = parse_transport_ssrc(setup_transport_resp).expect("parse play ssrc");
    let (player_server_rtp_port, player_server_rtcp_port) =
        parse_transport_server_ports(setup_transport_resp).expect("parse server ports");
    assert!(player_server_rtp_port > 0);
    assert!(player_server_rtcp_port > 0);
    assert_ne!(player_server_rtp_port, player_server_rtcp_port);

    let play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "UDP-PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);
    assert_eq!(play_resp.header("Session"), Some(player_session.as_str()));
    assert_eq!(play_resp.header("Range"), Some("npt=0.000-"));
    assert_eq!(play_resp.header("Scale"), Some("1.0"));

    let forwarded_rtp = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtp,
        player_server_rtp_port,
    )
    .await;
    assert_eq!(forwarded_rtp.header.payload_type, 96);
    assert!(forwarded_rtp.header.marker);
    assert!(
        !forwarded_rtp.payload.is_empty(),
        "forwarded rtp payload should not be empty"
    );
    assert_eq!(forwarded_rtp.payload[0] & 0x1f, 24);
    let (play_rtcp_types, play_sr_sender_ssrcs) = receive_play_udp_rtcp_packets_with_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtcp,
        player_server_rtcp_port,
    )
    .await;
    assert!(
        play_rtcp_types.contains(&202),
        "expected RTCP SDES(202), got types: {play_rtcp_types:?}"
    );
    assert!(
        play_rtcp_types.contains(&200),
        "expected RTCP SR(200), got types: {play_rtcp_types:?}"
    );
    assert!(
        play_sr_sender_ssrcs.contains(&play_ssrc),
        "expected SR sender ssrc {play_ssrc:08X}, got {play_sr_sender_ssrcs:08X?}"
    );

    let publish_sr_ssrc = 0x5566_7788u32;
    let (publisher_rr_from, publisher_rr_payload) = send_publish_sr_and_receive_rr_with_retry(
        &publisher_rtcp,
        publisher_server_rtcp_port,
        publish_sr_ssrc,
    )
    .await;
    assert_eq!(publisher_rr_from.port(), publisher_server_rtcp_port);
    assert!(
        publisher_rr_payload.len() >= 12,
        "publisher rr payload too short: {}",
        publisher_rr_payload.len()
    );
    assert_eq!(
        publisher_rr_payload[1], 201,
        "expected publisher rtcp RR packet type"
    );
    assert_eq!(
        read_u32_be(&publisher_rr_payload[8..12]).expect("publisher rr sender ssrc"),
        publish_sr_ssrc,
        "publisher rr report block should target sr sender ssrc"
    );

    let keepalive_body = "ping\r\n";
    let keepalive = build_request(
        "GET_PARAMETER",
        &uri,
        4,
        Some(&player_session),
        &[("Content-Type", "text/parameters")],
        keepalive_body.as_bytes(),
    );
    write_request(&mut player, &keepalive).await;
    let keepalive_resp = read_response(&mut player, "UDP-PLAYER-GET_PARAMETER").await;
    assert_eq!(keepalive_resp.status_code, 200);
    assert_eq!(
        keepalive_resp.header("Session"),
        Some(player_session.as_str())
    );
    assert_eq!(
        keepalive_resp.header("Content-Type"),
        Some("text/parameters")
    );
    assert_eq!(keepalive_resp.body, keepalive_body.as_bytes());

    let player_teardown = build_request("TEARDOWN", &uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "UDP-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);
    let (bye_from, bye_payload) =
        wait_for_rtcp_packet_type(&client_rtcp, player_server_rtcp_port, 203).await;
    assert_eq!(bye_from.port(), player_server_rtcp_port);
    assert!(
        bye_payload.len() >= 8,
        "play BYE payload too short: {}",
        bye_payload.len()
    );
    assert_eq!(bye_payload[1], 203, "expected play RTCP BYE packet type");

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "UDP-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_play_pause_play_rtcp_continuity() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 130u32;
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

    let uri = format!("rtsp://{listen}/live/udp-play-pause-play");

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
    let publisher_announce_resp = read_response(&mut publisher, "UDP-P2-PUBLISHER-ANNOUNCE").await;
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
    let publisher_setup_resp = read_response(&mut publisher, "UDP-P2-PUBLISHER-SETUP").await;
    assert_eq!(publisher_setup_resp.status_code, 200);
    let publisher_setup_transport_resp = publisher_setup_resp
        .header("Transport")
        .expect("publisher setup transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(publisher_setup_transport_resp)
            .expect("parse publisher server ports");

    let publisher_record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_record).await;
    let publisher_record_resp = read_response(&mut publisher, "UDP-P2-PUBLISHER-RECORD").await;
    assert_eq!(publisher_record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "UDP-P2-PLAYER-DESCRIBE").await;
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
    let player_setup_resp = read_response(&mut player, "UDP-P2-PLAYER-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    let player_setup_transport_resp = player_setup_resp
        .header("Transport")
        .expect("player setup transport");
    let play_ssrc = parse_transport_ssrc(player_setup_transport_resp).expect("parse play ssrc");
    let (player_server_rtp_port, player_server_rtcp_port) =
        parse_transport_server_ports(player_setup_transport_resp)
            .expect("parse player server ports");

    let play = build_request(
        "PLAY",
        &uri,
        3,
        Some(&player_session),
        &[("Range", "npt=0.000-")],
        &[],
    );
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "UDP-P2-PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);
    assert_eq!(play_resp.header("Range"), Some("npt=0.000-"));

    let first_forwarded = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtp,
        player_server_rtp_port,
    )
    .await;
    assert_eq!(first_forwarded.header.payload_type, 96);
    let (first_rtcp_types, first_sr_sender_ssrcs) = receive_play_udp_rtcp_packets_with_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtcp,
        player_server_rtcp_port,
    )
    .await;
    assert!(first_rtcp_types.contains(&202));
    assert!(first_rtcp_types.contains(&200));
    assert!(first_sr_sender_ssrcs.contains(&play_ssrc));

    let pause = build_request("PAUSE", &uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &pause).await;
    let pause_resp = read_response(&mut player, "UDP-P2-PLAYER-PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Range"), Some("npt=0.000-"));

    drain_udp_socket(&client_rtp).await;
    drain_udp_socket(&client_rtcp).await;

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        7000,
        900_000,
        0x2233_4455,
    )
    .await;
    assert_no_udp_packet_for_duration(
        &client_rtp,
        Duration::from_millis(250),
        "paused play should not forward RTP",
    )
    .await;
    assert_no_udp_packet_for_duration(
        &client_rtcp,
        Duration::from_millis(250),
        "paused play should not emit RTCP",
    )
    .await;

    let resume_play = build_request(
        "PLAY",
        &uri,
        5,
        Some(&player_session),
        &[("Range", "npt=12.000-")],
        &[],
    );
    write_request(&mut player, &resume_play).await;
    let resume_play_resp = read_response(&mut player, "UDP-P2-PLAYER-PLAY-2").await;
    assert_eq!(resume_play_resp.status_code, 200);
    assert_eq!(resume_play_resp.header("Range"), Some("npt=12.000-"));
    assert_eq!(resume_play_resp.header("Scale"), Some("1.0"));

    let resumed_forwarded = receive_udp_play_rtp_with_publish_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtp,
        player_server_rtp_port,
    )
    .await;
    assert_eq!(resumed_forwarded.header.payload_type, 96);
    assert!(resumed_forwarded.header.marker);
    let (resumed_rtcp_types, resumed_sr_sender_ssrcs) = receive_play_udp_rtcp_packets_with_retry(
        &publisher_rtp,
        publisher_server_rtp_port,
        &client_rtcp,
        player_server_rtcp_port,
    )
    .await;
    assert!(resumed_rtcp_types.contains(&202));
    assert!(resumed_rtcp_types.contains(&200));
    assert!(resumed_sr_sender_ssrcs.contains(&play_ssrc));

    let player_teardown = build_request("TEARDOWN", &uri, 6, Some(&player_session), &[], &[]);
    write_request(&mut player, &player_teardown).await;
    let player_teardown_resp = read_response(&mut player, "UDP-P2-PLAYER-TEARDOWN").await;
    assert_eq!(player_teardown_resp.status_code, 200);
    let (bye_from, bye_payload) =
        wait_for_rtcp_packet_type(&client_rtcp, player_server_rtcp_port, 203).await;
    assert_eq!(bye_from.port(), player_server_rtcp_port);
    assert!(bye_payload.len() >= 8);
    assert_eq!(bye_payload[1], 203);

    let publisher_teardown = build_request("TEARDOWN", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &publisher_teardown).await;
    let publisher_teardown_resp = read_response(&mut publisher, "UDP-P2-PUBLISHER-TEARDOWN").await;
    assert_eq!(publisher_teardown_resp.status_code, 200);

    engine.stop().await;
}
