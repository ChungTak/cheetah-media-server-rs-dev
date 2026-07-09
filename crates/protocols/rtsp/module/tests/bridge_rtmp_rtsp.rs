use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtmp_core::{RtmpClientState, RtmpMessageStreamId, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
    RtmpCoreCommand,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{StreamKey, StreamManagerApi};
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

mod common;
use common::*;

const RTMP_MEDIA_STREAM_ID: u32 = RtmpMessageStreamId::MEDIA.get();

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    listen
}

async fn wait_for_publish_ready(client: &mut RtmpClientHandle, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting for rtmp publish ready at {stage}"
        );
        let remaining = deadline.saturating_duration_since(now);
        let event = timeout(remaining, client.recv_event())
            .await
            .expect("timeout waiting rtmp event")
            .expect("rtmp client event stream closed unexpectedly");
        if let ClientDriverEvent::Core {
            event: cheetah_rtmp_core::RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == RtmpClientState::Publishing {
                return;
            }
        }
    }
}

async fn wait_for_stream_tracks(
    engine: &Engine,
    stream_key: &StreamKey,
    expected: usize,
    stage: &str,
) {
    let api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(Some(snapshot)) = api.get_stream(stream_key).await {
            if snapshot.tracks.len() >= expected {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting stream tracks at {stage}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

fn h264_keyframe_payload_with_cts(cts_ms: i32) -> Bytes {
    assert!((0..=0x7f_ffff).contains(&cts_ms));
    let cts = cts_ms as u32;
    Bytes::from(vec![
        0x17,
        0x01,
        ((cts >> 16) & 0xff) as u8,
        ((cts >> 8) & 0xff) as u8,
        (cts & 0xff) as u8,
        0x00,
        0x00,
        0x00,
        0x01,
        0x65,
    ])
}

fn aac_sequence_header_48k_stereo() -> Bytes {
    Bytes::from_static(&[0xaf, 0x00, 0x11, 0x90])
}

fn aac_raw_payload() -> Bytes {
    Bytes::from_static(&[0xaf, 0x01, 0x12, 0x10])
}

async fn send_rtmp_publish_priming_media(client: &RtmpClientHandle) {
    let tx = client.core_command_sender();
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_sequence_header_48k_stereo(),
    })
    .await
    .expect("send rtmp aac sequence header");
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: h264_keyframe_payload_with_cts(0),
    })
    .await
    .expect("send rtmp h264 keyframe");
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 0,
        payload: aac_raw_payload(),
    })
    .await
    .expect("send rtmp aac raw");
}

async fn send_rtmp_media_for_timestamp_assertions(client: &RtmpClientHandle) {
    let tx = client.core_command_sender();
    tx.send_core(RtmpCoreCommand::SendVideo {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 100,
        payload: h264_keyframe_payload_with_cts(20),
    })
    .await
    .expect("send rtmp h264 timestamp sample");
    sleep(Duration::from_millis(20)).await;
    tx.send_core(RtmpCoreCommand::SendAudio {
        stream_id: RTMP_MEDIA_STREAM_ID,
        timestamp_ms: 100,
        payload: aac_raw_payload(),
    })
    .await
    .expect("send rtmp aac timestamp sample");
}

async fn recv_udp_rtp_until_timestamp(
    socket: &UdpSocket,
    expected_timestamp: u32,
    stage: &str,
) -> (RtpPacket, SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 2048];
    loop {
        assert!(
            Instant::now() < deadline,
            "timeout waiting udp rtp at {stage}"
        );
        let (n, from) = timeout(Duration::from_millis(350), socket.recv_from(&mut buf))
            .await
            .expect("timeout receiving udp rtp")
            .expect("receive udp rtp");
        let packet = RtpPacket::parse(&buf[..n]).expect("parse udp rtp packet");
        if packet.header.timestamp == expected_timestamp {
            return (packet, from);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rtmp_to_rtsp_tcp_interleaved_maps_video_pts_and_audio_dts() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtmp-rtsp-tcp";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n"
    );
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mut rtmp_publisher = start_client(
        runtime,
        rtmp_url,
        RtmpClientMode::Publish,
        RtmpClientDriverConfig::default(),
        cheetah_sdk::CancellationToken::new(),
    )
    .expect("start rtmp publish client");
    wait_for_publish_ready(&mut rtmp_publisher, "tcp").await;

    send_rtmp_publish_priming_media(&rtmp_publisher).await;
    wait_for_stream_tracks(&engine, &StreamKey::new("live", stream_name), 2, "tcp").await;

    let mut player = connect_with_retry(rtsp_listen).await;
    let describe = build_request("DESCRIBE", &rtsp_uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "BRIDGE-TCP-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let setup_video = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut player, &setup_video).await;
    let setup_video_resp = read_response(&mut player, "BRIDGE-TCP-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);

    let setup_audio = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=1"),
        3,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=4-5")],
        &[],
    );
    write_request(&mut player, &setup_audio).await;
    let setup_audio_resp = read_response(&mut player, "BRIDGE-TCP-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);

    let play = build_request("PLAY", &rtsp_uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "BRIDGE-TCP-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    send_rtmp_media_for_timestamp_assertions(&rtmp_publisher).await;

    let mut tcp_video = None;
    let mut tcp_audio = None;
    for attempt in 0..32 {
        let (channel, payload) =
            read_interleaved_frame(&mut player, &format!("BRIDGE-TCP-RTP-{attempt}")).await;
        match channel {
            2 => {
                if let Some(pkt) = RtpPacket::parse(&payload) {
                    if pkt.header.timestamp == 9_000 {
                        tcp_video = Some(pkt);
                    }
                }
            }
            4 => {
                if let Some(pkt) = RtpPacket::parse(&payload) {
                    if pkt.header.timestamp == 4_800 {
                        tcp_audio = Some(pkt);
                    }
                }
            }
            _ => {}
        }
        if tcp_video.is_some() && tcp_audio.is_some() {
            break;
        }
    }

    let tcp_video = tcp_video.expect("receive tcp video rtp");
    let tcp_audio = tcp_audio.expect("receive tcp audio rtp");

    assert_eq!(
        tcp_video.header.timestamp, 9_000,
        "video RTP timestamp must use RTMP->AVFrame DTS (100ms at 90kHz)"
    );
    assert_eq!(
        tcp_audio.header.timestamp, 4_800,
        "audio RTP timestamp must use RTMP->AVFrame DTS at 48kHz"
    );

    let teardown = build_request("TEARDOWN", &rtsp_uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &teardown).await;
    let teardown_resp = read_response(&mut player, "BRIDGE-TCP-TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);

    rtmp_publisher.shutdown();
    let _ = rtmp_publisher.wait().await;
    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn rtmp_to_rtsp_udp_unicast_maps_video_pts_and_audio_dts() {
    let rtsp_listen = reserve_listen_addr();
    let rtmp_listen = reserve_listen_addr();
    let stream_name = "bridge-rtmp-rtsp-udp";
    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/{stream_name}");
    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/{stream_name}"))
        .expect("parse rtmp url");

    let config = Arc::new(ConfigStore::new());
    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n"
    );
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mut rtmp_publisher = start_client(
        runtime,
        rtmp_url,
        RtmpClientMode::Publish,
        RtmpClientDriverConfig::default(),
        cheetah_sdk::CancellationToken::new(),
    )
    .expect("start rtmp publish client");
    wait_for_publish_ready(&mut rtmp_publisher, "udp").await;

    send_rtmp_publish_priming_media(&rtmp_publisher).await;
    wait_for_stream_tracks(&engine, &StreamKey::new("live", stream_name), 2, "udp").await;

    let mut player = connect_with_retry(rtsp_listen).await;
    let describe = build_request("DESCRIBE", &rtsp_uri, 1, None, &[], &[]);
    write_request(&mut player, &describe).await;
    let describe_resp = read_response(&mut player, "BRIDGE-UDP-DESCRIBE").await;
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
    let audio_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtp");
    let audio_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind audio rtcp");

    let setup_video_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        video_rtp.local_addr().expect("video rtp addr").port(),
        video_rtcp.local_addr().expect("video rtcp addr").port()
    );
    let setup_video = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&player_session),
        &[("Transport", &setup_video_transport)],
        &[],
    );
    write_request(&mut player, &setup_video).await;
    let setup_video_resp = read_response(&mut player, "BRIDGE-UDP-SETUP-VIDEO").await;
    assert_eq!(setup_video_resp.status_code, 200);
    let setup_video_transport_resp = setup_video_resp
        .header("Transport")
        .expect("video setup transport");
    let (video_server_rtp_port, _video_server_rtcp_port) =
        parse_transport_server_ports(setup_video_transport_resp).expect("parse video server ports");

    let setup_audio_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        audio_rtp.local_addr().expect("audio rtp addr").port(),
        audio_rtcp.local_addr().expect("audio rtcp addr").port()
    );
    let setup_audio = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=1"),
        3,
        Some(&player_session),
        &[("Transport", &setup_audio_transport)],
        &[],
    );
    write_request(&mut player, &setup_audio).await;
    let setup_audio_resp = read_response(&mut player, "BRIDGE-UDP-SETUP-AUDIO").await;
    assert_eq!(setup_audio_resp.status_code, 200);
    let setup_audio_transport_resp = setup_audio_resp
        .header("Transport")
        .expect("audio setup transport");
    let (audio_server_rtp_port, _audio_server_rtcp_port) =
        parse_transport_server_ports(setup_audio_transport_resp).expect("parse audio server ports");

    let play = build_request("PLAY", &rtsp_uri, 4, Some(&player_session), &[], &[]);
    write_request(&mut player, &play).await;
    let play_resp = read_response(&mut player, "BRIDGE-UDP-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    send_rtmp_media_for_timestamp_assertions(&rtmp_publisher).await;

    let (udp_video, video_from) = recv_udp_rtp_until_timestamp(&video_rtp, 9_000, "video").await;
    assert_eq!(video_from.port(), video_server_rtp_port);

    let (udp_audio, audio_from) = recv_udp_rtp_until_timestamp(&audio_rtp, 4_800, "audio").await;
    assert_eq!(audio_from.port(), audio_server_rtp_port);

    assert_eq!(
        udp_video.header.timestamp, 9_000,
        "video RTP timestamp must use RTMP->AVFrame DTS (100ms at 90kHz)"
    );
    assert_eq!(
        udp_audio.header.timestamp, 4_800,
        "audio RTP timestamp must use RTMP->AVFrame DTS at 48kHz"
    );

    let teardown = build_request("TEARDOWN", &rtsp_uri, 5, Some(&player_session), &[], &[]);
    write_request(&mut player, &teardown).await;
    let teardown_resp = read_response(&mut player, "BRIDGE-UDP-TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);

    rtmp_publisher.shutdown();
    let _ = rtmp_publisher.wait().await;
    engine.stop().await;
}
