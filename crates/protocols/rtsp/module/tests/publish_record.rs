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
async fn udp_publish_pause_record_rtcp_rr_continuity() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let session_timeout_secs = 95u32;
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
    let uri = format!("rtsp://{listen}/live/udp-publish-pause-record");

    let announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    write_request(&mut stream, &announce).await;
    let announce_resp = read_response(&mut stream, "UDP-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let session_header = announce_resp
        .header("Session")
        .expect("announce session")
        .to_string();
    assert!(session_header.ends_with(";timeout=95"));

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
    let setup_transport =
        format!("RTP/AVP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port}");

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&session_header),
        &[("Transport", &setup_transport)],
        &[],
    );
    write_request(&mut stream, &setup).await;
    let setup_resp = read_response(&mut stream, "UDP-PUBLISHER-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);
    assert_eq!(setup_resp.header("Session"), Some(session_header.as_str()));
    let setup_transport_resp = setup_resp.header("Transport").expect("setup transport");
    let (_publisher_server_rtp_port, publisher_server_rtcp_port) =
        parse_transport_server_ports(setup_transport_resp).expect("parse publisher server ports");

    let record = build_request("RECORD", &uri, 3, Some(&session_header), &[], &[]);
    write_request(&mut stream, &record).await;
    let record_resp = read_response(&mut stream, "UDP-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let first_sr_ssrc = 0x1020_3040u32;
    let (first_rr_from, first_rr_payload) = send_publish_sr_and_receive_rr_with_retry(
        &publisher_rtcp,
        publisher_server_rtcp_port,
        first_sr_ssrc,
    )
    .await;
    assert_eq!(first_rr_from.port(), publisher_server_rtcp_port);
    assert!(first_rr_payload.len() >= 12);
    assert_eq!(first_rr_payload[1], 201);
    assert_eq!(
        read_u32_be(&first_rr_payload[8..12]).expect("first rr sender ssrc"),
        first_sr_ssrc
    );

    let pause = build_request("PAUSE", &uri, 4, Some(&session_header), &[], &[]);
    write_request(&mut stream, &pause).await;
    let pause_resp = read_response(&mut stream, "UDP-PUBLISHER-PAUSE").await;
    assert_eq!(pause_resp.status_code, 200);
    assert_eq!(pause_resp.header("Session"), Some(session_header.as_str()));

    drain_udp_socket(&publisher_rtcp).await;
    send_publish_udp_rtcp_sr(
        &publisher_rtcp,
        publisher_server_rtcp_port,
        0x2030_4050,
        270_000,
        3,
        3600,
    )
    .await;
    assert_no_udp_packet_for_duration(
        &publisher_rtcp,
        Duration::from_millis(250),
        "paused publish should not emit RR",
    )
    .await;

    let record2 = build_request("RECORD", &uri, 5, Some(&session_header), &[], &[]);
    write_request(&mut stream, &record2).await;
    let record2_resp = read_response(&mut stream, "UDP-PUBLISHER-RECORD-2").await;
    assert_eq!(record2_resp.status_code, 200);
    assert_eq!(
        record2_resp.header("Session"),
        Some(session_header.as_str())
    );

    let second_sr_ssrc = 0x5060_7080u32;
    let (second_rr_from, second_rr_payload) = send_publish_sr_and_receive_rr_with_retry(
        &publisher_rtcp,
        publisher_server_rtcp_port,
        second_sr_ssrc,
    )
    .await;
    assert_eq!(second_rr_from.port(), publisher_server_rtcp_port);
    assert!(second_rr_payload.len() >= 12);
    assert_eq!(second_rr_payload[1], 201);
    assert_eq!(
        read_u32_be(&second_rr_payload[8..12]).expect("second rr sender ssrc"),
        second_sr_ssrc
    );

    let teardown = build_request("TEARDOWN", &uri, 6, Some(&session_header), &[], &[]);
    write_request(&mut stream, &teardown).await;
    let teardown_resp = read_response(&mut stream, "UDP-PUBLISHER-TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);
    assert_eq!(
        teardown_resp.header("Session"),
        Some(session_header.as_str())
    );

    engine.stop().await;
}
