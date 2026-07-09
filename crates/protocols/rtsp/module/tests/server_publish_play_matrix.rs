use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

mod common;
use common::*;

#[derive(Debug, Clone, Copy)]
struct MulticastTransportView {
    destination: Ipv4Addr,
    rtp_port: u16,
    rtcp_port: u16,
    ttl: u8,
}

fn parse_transport_multicast(transport: &str) -> Option<MulticastTransportView> {
    let mut destination = None;
    let mut ports = None;
    let mut ttl = None;
    for part in transport.split(';').map(str::trim) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.eq_ignore_ascii_case("destination") {
            destination = value.parse::<Ipv4Addr>().ok();
        } else if name.eq_ignore_ascii_case("port") {
            let (rtp, rtcp) = value.split_once('-')?;
            ports = Some((rtp.parse::<u16>().ok()?, rtcp.parse::<u16>().ok()?));
        } else if name.eq_ignore_ascii_case("ttl") {
            ttl = value.parse::<u8>().ok();
        }
    }
    let (rtp_port, rtcp_port) = ports?;
    Some(MulticastTransportView {
        destination: destination?,
        rtp_port,
        rtcp_port,
        ttl: ttl?,
    })
}

async fn read_http_response_headers(stream: &mut TcpStream, stage: &str) -> String {
    let mut buf = Vec::<u8>::new();
    loop {
        let mut one = [0u8; 1];
        match timeout(Duration::from_secs(1), stream.read_exact(&mut one)).await {
            Ok(Ok(_)) => {
                buf.push(one[0]);
                if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
                    break;
                }
            }
            Ok(Err(err)) => panic!("read http header failed at {stage}: {err}"),
            Err(_) => panic!("read http header timeout at {stage}"),
        }
    }
    String::from_utf8(buf).expect("http header utf8")
}

async fn open_http_tunnel_pair(
    listen: std::net::SocketAddr,
    path: &str,
    cookie: &str,
) -> (TcpStream, TcpStream) {
    let get_req = format!(
        "GET {path} HTTP/1.0\r\nx-sessioncookie: {cookie}\r\nAccept: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n\r\n"
    );
    let post_req = format!(
        "POST {path} HTTP/1.0\r\nx-sessioncookie: {cookie}\r\nContent-Type: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n\r\n"
    );

    let mut get = connect_with_retry(listen).await;
    get.write_all(get_req.as_bytes())
        .await
        .expect("write get req");
    let get_headers = read_http_response_headers(&mut get, "MATRIX-HTTP-GET").await;
    assert!(
        get_headers.starts_with("HTTP/1.0 200"),
        "unexpected GET response: {get_headers:?}"
    );

    let mut post = connect_with_retry(listen).await;
    post.write_all(post_req.as_bytes())
        .await
        .expect("write post req");
    let post_headers = read_http_response_headers(&mut post, "MATRIX-HTTP-POST").await;
    assert!(
        post_headers.starts_with("HTTP/1.0 200"),
        "unexpected POST response: {post_headers:?}"
    );

    (get, post)
}

async fn tunnel_send_rtsp_request(post_stream: &mut TcpStream, request: &str) {
    let encoded = STANDARD.encode(request.as_bytes());
    post_stream
        .write_all(encoded.as_bytes())
        .await
        .expect("write tunnel rtsp request");
}

async fn read_rtp_interleaved_packet(
    stream: &mut TcpStream,
    expected_channel: u8,
    stage: &str,
) -> RtpPacket {
    for attempt in 0..8u32 {
        let (channel, payload) =
            read_interleaved_frame(stream, &format!("{stage}-{attempt}")).await;
        if channel != expected_channel {
            continue;
        }
        if let Some(packet) = RtpPacket::parse(&payload) {
            return packet;
        }
    }
    panic!("did not receive RTP interleaved packet on channel {expected_channel} at {stage}");
}

#[tokio::test(flavor = "current_thread")]
async fn server_play_matrix_udp_tcp_http_multicast() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    multicast:\n      enabled: true\n      group_start: 239.9.0.1\n      group_end: 239.9.0.8\n      port_start: 63200\n      port_end: 63231\n      ttl: 10\n      idle_release_ms: 200\n"
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

    let uri = format!("rtsp://{listen}/live/play-matrix");

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
    let announce_resp = read_response(&mut publisher, "MATRIX-PUBLISH-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let publisher_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtp");
    let publisher_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind publisher rtcp");
    let publisher_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        publisher_rtp
            .local_addr()
            .expect("publisher rtp addr")
            .port(),
        publisher_rtcp
            .local_addr()
            .expect("publisher rtcp addr")
            .port()
    );
    let setup_publish = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", &publisher_setup_transport)],
        &[],
    );
    write_request(&mut publisher, &setup_publish).await;
    let setup_publish_resp = read_response(&mut publisher, "MATRIX-PUBLISH-SETUP").await;
    assert_eq!(setup_publish_resp.status_code, 200);
    let setup_publish_transport = setup_publish_resp
        .header("Transport")
        .expect("publisher transport");
    let (publisher_server_rtp_port, _publisher_server_rtcp_port) =
        parse_transport_server_ports(setup_publish_transport)
            .expect("parse publisher server ports");

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "MATRIX-PUBLISH-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let mut udp_player = connect_with_retry(listen).await;
    let udp_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut udp_player, &udp_describe).await;
    let udp_describe_resp = read_response(&mut udp_player, "MATRIX-UDP-DESCRIBE").await;
    assert_eq!(udp_describe_resp.status_code, 200);
    let udp_session = udp_describe_resp
        .header("Session")
        .expect("udp session")
        .to_string();

    let udp_client_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind udp client rtp");
    let udp_client_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind udp client rtcp");
    let udp_setup_transport = format!(
        "RTP/AVP;unicast;client_port={}-{}",
        udp_client_rtp.local_addr().expect("udp rtp addr").port(),
        udp_client_rtcp.local_addr().expect("udp rtcp addr").port()
    );
    let udp_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&udp_session),
        &[("Transport", &udp_setup_transport)],
        &[],
    );
    write_request(&mut udp_player, &udp_setup).await;
    let udp_setup_resp = read_response(&mut udp_player, "MATRIX-UDP-SETUP").await;
    assert_eq!(udp_setup_resp.status_code, 200);
    let udp_transport = udp_setup_resp.header("Transport").expect("udp transport");
    let (udp_server_rtp_port, _udp_server_rtcp_port) =
        parse_transport_server_ports(udp_transport).expect("parse udp server ports");

    let udp_play = build_request("PLAY", &uri, 3, Some(&udp_session), &[], &[]);
    write_request(&mut udp_player, &udp_play).await;
    let udp_play_resp = read_response(&mut udp_player, "MATRIX-UDP-PLAY").await;
    assert_eq!(udp_play_resp.status_code, 200);

    let mut tcp_player = connect_with_retry(listen).await;
    let tcp_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut tcp_player, &tcp_describe).await;
    let tcp_describe_resp = read_response(&mut tcp_player, "MATRIX-TCP-DESCRIBE").await;
    assert_eq!(tcp_describe_resp.status_code, 200);
    let tcp_session = tcp_describe_resp
        .header("Session")
        .expect("tcp session")
        .to_string();

    let tcp_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&tcp_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut tcp_player, &tcp_setup).await;
    let tcp_setup_resp = read_response(&mut tcp_player, "MATRIX-TCP-SETUP").await;
    assert_eq!(tcp_setup_resp.status_code, 200);

    let tcp_play = build_request("PLAY", &uri, 3, Some(&tcp_session), &[], &[]);
    write_request(&mut tcp_player, &tcp_play).await;
    let tcp_play_resp = read_response(&mut tcp_player, "MATRIX-TCP-PLAY").await;
    assert_eq!(tcp_play_resp.status_code, 200);

    let path = "/live/play-matrix";
    let (mut http_get, mut http_post) = open_http_tunnel_pair(listen, path, "matrix-cookie").await;
    let http_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    tunnel_send_rtsp_request(&mut http_post, &http_describe).await;
    let http_describe_resp = read_response(&mut http_get, "MATRIX-HTTP-DESCRIBE").await;
    assert_eq!(http_describe_resp.status_code, 200);
    let http_session = http_describe_resp
        .header("Session")
        .expect("http session")
        .to_string();

    let http_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&http_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    tunnel_send_rtsp_request(&mut http_post, &http_setup).await;
    let http_setup_resp = read_response(&mut http_get, "MATRIX-HTTP-SETUP").await;
    assert_eq!(http_setup_resp.status_code, 200);

    let http_play = build_request("PLAY", &uri, 3, Some(&http_session), &[], &[]);
    tunnel_send_rtsp_request(&mut http_post, &http_play).await;
    let http_play_resp = read_response(&mut http_get, "MATRIX-HTTP-PLAY").await;
    assert_eq!(http_play_resp.status_code, 200);

    let mut multicast_player = connect_with_retry(listen).await;
    let multicast_describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    write_request(&mut multicast_player, &multicast_describe).await;
    let multicast_describe_resp =
        read_response(&mut multicast_player, "MATRIX-MCAST-DESCRIBE").await;
    assert_eq!(multicast_describe_resp.status_code, 200);
    let multicast_session = multicast_describe_resp
        .header("Session")
        .expect("multicast session")
        .to_string();

    let multicast_setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&multicast_session),
        &[("Transport", "RTP/AVP;multicast;port=5000-5001")],
        &[],
    );
    write_request(&mut multicast_player, &multicast_setup).await;
    let multicast_setup_resp = read_response(&mut multicast_player, "MATRIX-MCAST-SETUP").await;
    assert_eq!(multicast_setup_resp.status_code, 200);
    let multicast_transport = multicast_setup_resp
        .header("Transport")
        .expect("multicast transport");
    let parsed_multicast =
        parse_transport_multicast(multicast_transport).expect("parse multicast transport");
    assert!(parsed_multicast.destination.is_multicast());
    assert_eq!(
        parsed_multicast.rtcp_port,
        parsed_multicast.rtp_port.saturating_add(1)
    );
    assert_eq!(parsed_multicast.ttl, 10);

    let multicast_play = build_request("PLAY", &uri, 3, Some(&multicast_session), &[], &[]);
    write_request(&mut multicast_player, &multicast_play).await;
    let multicast_play_resp = read_response(&mut multicast_player, "MATRIX-MCAST-PLAY").await;
    assert_eq!(multicast_play_resp.status_code, 200);

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        10_001,
        540_000,
        0x0102_0304,
    )
    .await;
    let mut udp_buf = [0u8; 2048];
    let (udp_n, udp_from) = timeout(
        Duration::from_secs(1),
        udp_client_rtp.recv_from(&mut udp_buf),
    )
    .await
    .expect("udp play timeout")
    .expect("udp play recv");
    assert_eq!(udp_from.port(), udp_server_rtp_port);
    let udp_packet = RtpPacket::parse(&udp_buf[..udp_n]).expect("parse udp play rtp");
    assert_eq!(udp_packet.header.payload_type, 96);

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        10_002,
        543_600,
        0x0102_0304,
    )
    .await;
    let tcp_packet = read_rtp_interleaved_packet(&mut tcp_player, 2, "MATRIX-TCP-RTP").await;
    assert_eq!(tcp_packet.header.payload_type, 96);

    send_publish_udp_rtp_frame(
        &publisher_rtp,
        publisher_server_rtp_port,
        10_003,
        547_200,
        0x0102_0304,
    )
    .await;
    let http_packet = read_rtp_interleaved_packet(&mut http_get, 2, "MATRIX-HTTP-RTP").await;
    assert_eq!(http_packet.header.payload_type, 96);

    engine.stop().await;
}
