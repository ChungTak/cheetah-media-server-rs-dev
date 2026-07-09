use std::net::UdpSocket as StdUdpSocket;
use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;

mod common;
use common::*;

fn find_free_even_odd_udp_pair() -> (u16, u16) {
    for _ in 0..256 {
        let probe = StdUdpSocket::bind("0.0.0.0:0").expect("bind udp probe socket");
        let probe_port = probe.local_addr().expect("probe local addr").port();
        drop(probe);
        let even = if probe_port.is_multiple_of(2) {
            probe_port
        } else {
            probe_port.saturating_add(1)
        };
        if even == 0 || even == u16::MAX {
            continue;
        }
        let Ok(guard_rtp) = StdUdpSocket::bind(("0.0.0.0", even)) else {
            continue;
        };
        let Ok(guard_rtcp) = StdUdpSocket::bind(("0.0.0.0", even.saturating_add(1))) else {
            drop(guard_rtp);
            continue;
        };
        drop(guard_rtp);
        drop(guard_rtcp);
        return (even, even.saturating_add(1));
    }
    panic!("failed to find free even/odd udp port pair");
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

async fn announce_publish_session(
    listen: std::net::SocketAddr,
    stream_name: &str,
) -> (tokio::net::TcpStream, String, String) {
    let uri = format!("rtsp://{listen}/live/{stream_name}");
    let mut stream = connect_with_retry(listen).await;
    let announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut stream, &announce).await;
    let announce_resp = read_response(&mut stream, "ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session = announce_resp
        .header("Session")
        .expect("publish session")
        .to_string();
    (stream, session, uri)
}

#[tokio::test(flavor = "current_thread")]
async fn udp_setup_rejects_third_party_destination_by_default() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n");
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (mut stream, session, uri) = announce_publish_session(listen, "udp-dst-reject").await;

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[(
            "Transport",
            "RTP/AVP/UDP;unicast;destination=127.0.0.2;client_port=5000-5001",
        )],
        &[],
    );
    write_request(&mut stream, &setup).await;
    let setup_resp = read_response(&mut stream, "SETUP-destination").await;
    assert_eq!(setup_resp.status_code, 461);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_setup_returns_461_when_server_port_pool_exhausted() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let (pool_start, pool_end) = find_free_even_odd_udp_pair();
    let hold_rtp = StdUdpSocket::bind(("0.0.0.0", pool_start)).expect("occupy rtp pool port");
    let hold_rtcp = StdUdpSocket::bind(("0.0.0.0", pool_end)).expect("occupy rtcp pool port");

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    udp:\n      server_port_pool_start: {pool_start}\n      server_port_pool_end: {pool_end}\n      bind_pair_attempts: 8\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;
    let (mut stream, session, uri) = announce_publish_session(listen, "udp-pool-exhausted").await;

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session),
        &[("Transport", "RTP/AVP/UDP;unicast;client_port=5000-5001")],
        &[],
    );
    write_request(&mut stream, &setup).await;
    let setup_resp = read_response(&mut stream, "SETUP-pool-exhausted").await;
    assert_eq!(setup_resp.status_code, 461);

    drop(hold_rtp);
    drop(hold_rtcp);
    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_teardown_releases_port_pair_back_to_pool() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let (pool_start, pool_end) = find_free_even_odd_udp_pair();
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    udp:\n      server_port_pool_start: {pool_start}\n      server_port_pool_end: {pool_end}\n      bind_pair_attempts: 8\n"
    );
    let engine = start_engine_with_rtsp_config(&config_yaml).await;

    let (mut publisher1, session1, uri1) = announce_publish_session(listen, "udp-teardown-1").await;
    let setup1 = build_request(
        "SETUP",
        &format!("{uri1}/trackID=0"),
        2,
        Some(&session1),
        &[("Transport", "RTP/AVP/UDP;unicast;client_port=5000-5001")],
        &[],
    );
    write_request(&mut publisher1, &setup1).await;
    let setup1_resp = read_response(&mut publisher1, "SETUP-1").await;
    assert_eq!(setup1_resp.status_code, 200);
    let (server_rtp1, server_rtcp1) = parse_transport_server_ports(
        setup1_resp
            .header("Transport")
            .expect("setup-1 transport header"),
    )
    .expect("parse setup-1 server ports");
    assert_eq!((server_rtp1, server_rtcp1), (pool_start, pool_end));

    let teardown1 = build_request("TEARDOWN", &uri1, 3, Some(&session1), &[], &[]);
    write_request(&mut publisher1, &teardown1).await;
    let teardown1_resp = read_response(&mut publisher1, "TEARDOWN-1").await;
    assert_eq!(teardown1_resp.status_code, 200);

    let (mut publisher2, session2, uri2) = announce_publish_session(listen, "udp-teardown-2").await;
    let setup2 = build_request(
        "SETUP",
        &format!("{uri2}/trackID=0"),
        2,
        Some(&session2),
        &[("Transport", "RTP/AVP/UDP;unicast;client_port=5002-5003")],
        &[],
    );
    write_request(&mut publisher2, &setup2).await;
    let setup2_resp = read_response(&mut publisher2, "SETUP-2").await;
    assert_eq!(setup2_resp.status_code, 200);
    let (server_rtp2, server_rtcp2) = parse_transport_server_ports(
        setup2_resp
            .header("Transport")
            .expect("setup-2 transport header"),
    )
    .expect("parse setup-2 server ports");
    assert_eq!((server_rtp2, server_rtcp2), (pool_start, pool_end));

    engine.stop().await;
}
