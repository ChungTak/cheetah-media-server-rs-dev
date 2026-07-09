use std::sync::Arc;

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;

mod common;
use common::*;

fn parse_transport_interleaved_channels(transport: &str) -> Option<(u8, u8)> {
    for part in transport.split(';') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("interleaved") {
            continue;
        }
        let (rtp, rtcp) = value.trim().split_once('-')?;
        let rtp = rtp.parse::<u8>().ok()?;
        let rtcp = rtcp.parse::<u8>().ok()?;
        return Some((rtp, rtcp));
    }
    None
}

async fn start_engine_with_rtsp_config(config_yaml: &str) -> cheetah_engine::Engine {
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(config_yaml)
        .expect("load rtsp module config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");
    engine
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_setup_without_interleaved_auto_assigns_channels_in_setup_order() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let uri = format!("rtsp://{listen}/live/tcp-auto-interleaved");

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
    let announce_resp = read_response(&mut publisher, "PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup_pub_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast")],
        &[],
    );
    write_request(&mut publisher, &setup_pub_video).await;
    let setup_pub_video_resp = read_response(&mut publisher, "PUBLISHER-SETUP-VIDEO").await;
    assert_eq!(setup_pub_video_resp.status_code, 200);
    let setup_pub_video_transport = setup_pub_video_resp
        .header("Transport")
        .expect("publisher video transport");
    assert_eq!(
        parse_transport_interleaved_channels(setup_pub_video_transport),
        Some((0, 1))
    );

    let setup_pub_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast")],
        &[],
    );
    write_request(&mut publisher, &setup_pub_audio).await;
    let setup_pub_audio_resp = read_response(&mut publisher, "PUBLISHER-SETUP-AUDIO").await;
    assert_eq!(setup_pub_audio_resp.status_code, 200);
    let setup_pub_audio_transport = setup_pub_audio_resp
        .header("Transport")
        .expect("publisher audio transport");
    assert_eq!(
        parse_transport_interleaved_channels(setup_pub_audio_transport),
        Some((2, 3))
    );

    let record = build_request("RECORD", &uri, 4, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let setup_play_video = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast")],
        &[],
    );
    write_request(&mut player, &setup_play_video).await;
    let setup_play_video_resp = read_response(&mut player, "PLAYER-SETUP-VIDEO").await;
    assert_eq!(setup_play_video_resp.status_code, 200);
    let setup_play_video_transport = setup_play_video_resp
        .header("Transport")
        .expect("player video transport");
    assert_eq!(
        parse_transport_interleaved_channels(setup_play_video_transport),
        Some((0, 1))
    );

    let setup_play_audio = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast")],
        &[],
    );
    write_request(&mut player, &setup_play_audio).await;
    let setup_play_audio_resp = read_response(&mut player, "PLAYER-SETUP-AUDIO").await;
    assert_eq!(setup_play_audio_resp.status_code, 200);
    let setup_play_audio_transport = setup_play_audio_resp
        .header("Transport")
        .expect("player audio transport");
    assert_eq!(
        parse_transport_interleaved_channels(setup_play_audio_transport),
        Some((2, 3))
    );

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_setup_rejects_conflicting_interleaved_channels() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let uri = format!("rtsp://{listen}/live/tcp-channel-conflict");

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
    let announce_resp = read_response(&mut publisher, "PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup_ok = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup_ok).await;
    let setup_ok_resp = read_response(&mut publisher, "PUBLISHER-SETUP-OK").await;
    assert_eq!(setup_ok_resp.status_code, 200);

    let setup_conflict = build_request(
        "SETUP",
        &format!("{uri}/trackID=1"),
        3,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=1-2")],
        &[],
    );
    write_request(&mut publisher, &setup_conflict).await;
    let setup_conflict_resp = read_response(&mut publisher, "PUBLISHER-SETUP-CONFLICT").await;
    assert_eq!(setup_conflict_resp.status_code, 461);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_play_teardown_emits_rtcp_bye_on_rtcp_interleaved_channel() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let uri = format!("rtsp://{listen}/live/tcp-bye-channel");

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
    let announce_resp = read_response(&mut publisher, "PUBLISHER-ANNOUNCE").await;
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
    let setup_publish_resp = read_response(&mut publisher, "PUBLISHER-SETUP").await;
    assert_eq!(setup_publish_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let mut player = connect_with_retry(listen).await;
    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let setup_play = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut player, &setup_play).await;
    let setup_play_resp = read_response(&mut player, "PLAYER-SETUP").await;
    assert_eq!(setup_play_resp.status_code, 200);

    let play = build_request("PLAY", &uri, 3, Some(&player_session), &[], &[]);
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "PLAYER-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let publish_packet = build_publish_h264_rtp(7000, 450_000, 0x1234_5678);
    send_interleaved_frame(&mut publisher, 0, &publish_packet).await;
    let (rtp_channel, rtp_payload) = read_interleaved_frame(&mut player, "PLAYER-RTP").await;
    assert_eq!(rtp_channel, 2);
    let rtp = RtpPacket::parse(&rtp_payload).expect("parse forwarded rtp");
    assert_eq!(rtp.header.payload_type, 96);

    let teardown = build_request("TEARDOWN", &uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &teardown).await;
    let teardown_resp = read_response(&mut player, "PLAYER-TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);

    let (bye_channel, bye_payload) = read_interleaved_frame(&mut player, "PLAYER-BYE").await;
    assert_eq!(bye_channel, 3);
    assert!(
        bye_payload.len() >= 8,
        "bye payload too short: {}",
        bye_payload.len()
    );
    assert_eq!(bye_payload[1], 203, "expected RTCP BYE packet type");

    engine.stop().await;
}
