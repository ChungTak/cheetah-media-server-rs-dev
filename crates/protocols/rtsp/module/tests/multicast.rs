use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::net::TcpStream;
use tokio::time::sleep;

mod common;
use common::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MulticastTransportView {
    destination: Ipv4Addr,
    rtp_port: u16,
    rtcp_port: u16,
    ttl: u8,
}

fn parse_transport_multicast(transport: &str) -> Option<MulticastTransportView> {
    let mut destination = None;
    let mut port_pair = None;
    let mut ttl = None;

    for part in transport.split(';').map(str::trim) {
        if let Some((name, value)) = part.split_once('=') {
            if name.eq_ignore_ascii_case("destination") {
                destination = value.parse::<Ipv4Addr>().ok();
            } else if name.eq_ignore_ascii_case("port") {
                let (rtp_raw, rtcp_raw) = value.split_once('-')?;
                let rtp_port = rtp_raw.parse::<u16>().ok()?;
                let rtcp_port = rtcp_raw.parse::<u16>().ok()?;
                port_pair = Some((rtp_port, rtcp_port));
            } else if name.eq_ignore_ascii_case("ttl") {
                ttl = value.parse::<u8>().ok();
            }
        }
    }

    let (rtp_port, rtcp_port) = port_pair?;
    Some(MulticastTransportView {
        destination: destination?,
        rtp_port,
        rtcp_port,
        ttl: ttl?,
    })
}

async fn start_engine_with_rtsp_config(config_yaml: &str) -> Engine {
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

async fn setup_publish_session(
    listen: std::net::SocketAddr,
    stream_name: &str,
) -> (TcpStream, String, String) {
    let uri = format!("rtsp://{listen}/live/{stream_name}");
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
    let announce_resp = read_response(&mut publisher, "MULTICAST-PUBLISH-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let setup_resp = read_response(&mut publisher, "MULTICAST-PUBLISH-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "MULTICAST-PUBLISH-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    (publisher, session, uri)
}

async fn describe_and_setup_multicast(
    stream: &mut TcpStream,
    uri: &str,
    cseq_base: u32,
) -> (String, common::RtspResponse) {
    let describe = build_request("DESCRIBE", uri, cseq_base, None, &[], &[]);
    write_request(stream, &describe).await;
    let describe_resp = read_response(stream, "MULTICAST-PLAYER-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        cseq_base.saturating_add(1),
        Some(&session),
        &[("Transport", "RTP/AVP;multicast;port=5000-5001")],
        &[],
    );
    write_request(stream, &setup).await;
    let setup_resp = read_response(stream, "MULTICAST-PLAYER-SETUP").await;
    (session, setup_resp)
}

#[tokio::test(flavor = "current_thread")]
async fn multicast_setup_returns_transport_with_destination_port_ttl_and_ssrc() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    multicast:\n      enabled: true\n      group_start: 239.1.0.1\n      group_end: 239.1.0.8\n      port_start: 63000\n      port_end: 63031\n      ttl: 12\n      idle_release_ms: 200\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (_publisher, _publisher_session, uri) = setup_publish_session(listen, "mcast-setup").await;

    let mut player = connect_with_retry(listen).await;
    let (_player_session, setup_resp) = describe_and_setup_multicast(&mut player, &uri, 1).await;
    assert_eq!(setup_resp.status_code, 200);

    let transport = setup_resp.header("Transport").expect("setup transport");
    assert!(transport.contains("RTP/AVP;multicast"));
    let parsed = parse_transport_multicast(transport).expect("parse multicast transport");
    assert!(parsed.destination.is_multicast());
    assert!(parsed.destination.octets()[0] == 239);
    assert_eq!(parsed.ttl, 12);
    assert_eq!(parsed.rtp_port % 2, 0);
    assert_eq!(parsed.rtcp_port, parsed.rtp_port.saturating_add(1));
    assert!(parse_transport_ssrc(transport).is_some());

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn multicast_setup_reuses_sender_for_same_stream_track() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    multicast:\n      enabled: true\n      group_start: 239.2.0.1\n      group_end: 239.2.0.16\n      port_start: 63000\n      port_end: 63031\n      ttl: 16\n      idle_release_ms: 500\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (_publisher, _publisher_session, uri) = setup_publish_session(listen, "mcast-reuse").await;

    let mut player1 = connect_with_retry(listen).await;
    let (_session1, setup1_resp) = describe_and_setup_multicast(&mut player1, &uri, 1).await;
    assert_eq!(setup1_resp.status_code, 200);
    let transport1 = parse_transport_multicast(
        setup1_resp
            .header("Transport")
            .expect("player1 setup transport"),
    )
    .expect("parse player1 transport");

    let mut player2 = connect_with_retry(listen).await;
    let (_session2, setup2_resp) = describe_and_setup_multicast(&mut player2, &uri, 11).await;
    assert_eq!(setup2_resp.status_code, 200);
    let transport2 = parse_transport_multicast(
        setup2_resp
            .header("Transport")
            .expect("player2 setup transport"),
    )
    .expect("parse player2 transport");

    assert_eq!(transport1, transport2);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn multicast_sender_released_after_idle_and_reallocated() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let idle_release_ms = 60u64;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    multicast:\n      enabled: true\n      group_start: 239.3.0.1\n      group_end: 239.3.0.4\n      port_start: 63000\n      port_end: 63015\n      ttl: 10\n      idle_release_ms: {idle_release_ms}\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (_publisher, _publisher_session, uri) =
        setup_publish_session(listen, "mcast-release").await;

    let mut player1 = connect_with_retry(listen).await;
    let (session1, setup1_resp) = describe_and_setup_multicast(&mut player1, &uri, 1).await;
    assert_eq!(setup1_resp.status_code, 200);
    let transport1 = parse_transport_multicast(
        setup1_resp
            .header("Transport")
            .expect("player1 setup transport"),
    )
    .expect("parse player1 transport");

    let teardown1 = build_request("TEARDOWN", &uri, 3, Some(&session1), &[], &[]);
    write_request(&mut player1, &teardown1).await;
    let teardown1_resp = read_response(&mut player1, "MULTICAST-PLAYER1-TEARDOWN").await;
    assert_eq!(teardown1_resp.status_code, 200);

    sleep(Duration::from_millis(idle_release_ms.saturating_add(80))).await;

    let mut player2 = connect_with_retry(listen).await;
    let (_session2, setup2_resp) = describe_and_setup_multicast(&mut player2, &uri, 21).await;
    assert_eq!(setup2_resp.status_code, 200);
    let transport2 = parse_transport_multicast(
        setup2_resp
            .header("Transport")
            .expect("player2 setup transport"),
    )
    .expect("parse player2 transport");

    assert_ne!(transport1, transport2);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn multicast_subscription_released_when_track_is_resetup_as_tcp() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let idle_release_ms = 60u64;
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    multicast:\n      enabled: true\n      group_start: 239.4.0.1\n      group_end: 239.4.0.1\n      port_start: 63000\n      port_end: 63001\n      ttl: 10\n      idle_release_ms: {idle_release_ms}\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (_publisher1, _publisher1_session, uri1) =
        setup_publish_session(listen, "mcast-resetup-source").await;

    let mut player = connect_with_retry(listen).await;
    let (player_session, setup1_resp) = describe_and_setup_multicast(&mut player, &uri1, 1).await;
    assert_eq!(setup1_resp.status_code, 200);

    let resetup_tcp = build_request(
        "SETUP",
        &format!("{uri1}/trackID=0"),
        3,
        Some(&player_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut player, &resetup_tcp).await;
    let resetup_resp = read_response(&mut player, "MULTICAST-PLAYER-RESETUP-TCP").await;
    assert_eq!(resetup_resp.status_code, 200);

    sleep(Duration::from_millis(idle_release_ms.saturating_add(80))).await;

    let (_publisher2, _publisher2_session, uri2) =
        setup_publish_session(listen, "mcast-resetup-next").await;
    let mut next_player = connect_with_retry(listen).await;
    let (_next_session, next_setup_resp) =
        describe_and_setup_multicast(&mut next_player, &uri2, 11).await;
    assert_eq!(next_setup_resp.status_code, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn multicast_setup_returns_461_when_disabled() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (_publisher, _publisher_session, uri) =
        setup_publish_session(listen, "mcast-disabled").await;

    let mut player = connect_with_retry(listen).await;
    let (_session, setup_resp) = describe_and_setup_multicast(&mut player, &uri, 1).await;
    assert_eq!(setup_resp.status_code, 461);

    engine.stop().await;
}
