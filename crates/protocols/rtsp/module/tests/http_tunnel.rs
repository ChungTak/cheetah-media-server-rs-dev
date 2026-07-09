use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

mod common;
use common::*;

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
    post_first: bool,
) -> (TcpStream, TcpStream) {
    let get_req = format!(
        "GET {path} HTTP/1.0\r\nx-sessioncookie: {cookie}\r\nAccept: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n\r\n"
    );
    let post_req = format!(
        "POST {path} HTTP/1.0\r\nx-sessioncookie: {cookie}\r\nContent-Type: application/x-rtsp-tunnelled\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n\r\n"
    );

    if post_first {
        let mut post = connect_with_retry(listen).await;
        post.write_all(post_req.as_bytes())
            .await
            .expect("write post req");
        let post_headers = read_http_response_headers(&mut post, "HTTP-POST-RESP").await;
        assert!(
            post_headers.starts_with("HTTP/1.0 200"),
            "unexpected post response: {post_headers:?}"
        );

        let mut get = connect_with_retry(listen).await;
        get.write_all(get_req.as_bytes())
            .await
            .expect("write get req");
        let get_headers = read_http_response_headers(&mut get, "HTTP-GET-RESP").await;
        assert!(
            get_headers.starts_with("HTTP/1.0 200"),
            "unexpected get response: {get_headers:?}"
        );
        return (get, post);
    }

    let mut get = connect_with_retry(listen).await;
    get.write_all(get_req.as_bytes())
        .await
        .expect("write get req");
    let get_headers = read_http_response_headers(&mut get, "HTTP-GET-RESP").await;
    assert!(
        get_headers.starts_with("HTTP/1.0 200"),
        "unexpected get response: {get_headers:?}"
    );

    let mut post = connect_with_retry(listen).await;
    post.write_all(post_req.as_bytes())
        .await
        .expect("write post req");
    let post_headers = read_http_response_headers(&mut post, "HTTP-POST-RESP").await;
    assert!(
        post_headers.starts_with("HTTP/1.0 200"),
        "unexpected post response: {post_headers:?}"
    );
    (get, post)
}

async fn tunnel_send_bytes(post_stream: &mut TcpStream, payload: &[u8]) {
    let encoded = STANDARD.encode(payload);
    post_stream
        .write_all(encoded.as_bytes())
        .await
        .expect("write tunnel payload");
}

async fn tunnel_send_rtsp_request(post_stream: &mut TcpStream, request: &str) {
    tunnel_send_bytes(post_stream, request.as_bytes()).await;
}

async fn tunnel_send_interleaved_frame(post_stream: &mut TcpStream, channel: u8, payload: &[u8]) {
    let len = payload.len().min(u16::MAX as usize);
    let mut frame = Vec::with_capacity(4 + len);
    frame.push(b'$');
    frame.push(channel);
    frame.extend_from_slice(&(len as u16).to_be_bytes());
    frame.extend_from_slice(&payload[..len]);
    tunnel_send_bytes(post_stream, &frame).await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_tunnel_describe_setup_play_receives_interleaved_rtp() {
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

    let uri = format!("rtsp://{listen}/live/http-tunnel-play");

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

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let setup_resp = read_response(&mut publisher, "PUBLISHER-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let path = "/live/http-tunnel-play";
    let (mut tunnel_get, mut tunnel_post) =
        open_http_tunnel_pair(listen, path, "cookie-http-play", false).await;

    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    tunnel_send_rtsp_request(&mut tunnel_post, &describe).await;
    let describe_resp = read_response(&mut tunnel_get, "TUNNEL-DESCRIBE").await;
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
    tunnel_send_rtsp_request(&mut tunnel_post, &setup_play).await;
    let setup_play_resp = read_response(&mut tunnel_get, "TUNNEL-SETUP").await;
    assert_eq!(setup_play_resp.status_code, 200);

    let play = build_request("PLAY", &uri, 3, Some(&player_session), &[], &[]);
    tunnel_send_rtsp_request(&mut tunnel_post, &play).await;
    let play_resp = read_response(&mut tunnel_get, "TUNNEL-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let publish_packet = build_publish_h264_rtp(9000, 540_000, 0x0102_0304);
    send_interleaved_frame(&mut publisher, 0, &publish_packet).await;
    let (channel, payload) = read_interleaved_frame(&mut tunnel_get, "TUNNEL-RTP").await;
    assert_eq!(channel, 2);
    let packet = RtpPacket::parse(&payload).expect("parse forwarded rtp");
    assert_eq!(packet.header.payload_type, 96);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_tunnel_play_uses_tcp_interleaved_mtu_not_udp_rtp_mtu() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n    rtp_mtu: 400\n");
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

    let uri = format!("rtsp://{listen}/live/http-tunnel-mtu");

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
    let announce_resp = read_response(&mut publisher, "MTU-PUBLISHER-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    write_request(&mut publisher, &setup).await;
    let setup_resp = read_response(&mut publisher, "MTU-PUBLISHER-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    write_request(&mut publisher, &record).await;
    let record_resp = read_response(&mut publisher, "MTU-PUBLISHER-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    let path = "/live/http-tunnel-mtu";
    let (mut tunnel_get, mut tunnel_post) =
        open_http_tunnel_pair(listen, path, "cookie-http-mtu", false).await;

    let describe = build_request("DESCRIBE", &uri, 1, None, &[], &[]);
    tunnel_send_rtsp_request(&mut tunnel_post, &describe).await;
    let describe_resp = read_response(&mut tunnel_get, "MTU-TUNNEL-DESCRIBE").await;
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
    tunnel_send_rtsp_request(&mut tunnel_post, &setup_play).await;
    let setup_play_resp = read_response(&mut tunnel_get, "MTU-TUNNEL-SETUP").await;
    assert_eq!(setup_play_resp.status_code, 200);

    let play = build_request("PLAY", &uri, 3, Some(&player_session), &[], &[]);
    tunnel_send_rtsp_request(&mut tunnel_post, &play).await;
    let play_resp = read_response(&mut tunnel_get, "MTU-TUNNEL-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let mut large_h264_nalu = vec![0x65u8];
    large_h264_nalu.extend(std::iter::repeat_n(0x88u8, 1500));
    let publish_packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 10_001,
            timestamp: 540_000,
            ssrc: 0x0102_0304,
            marker: true,
        },
        payload: Bytes::from(large_h264_nalu.clone()),
    };
    send_interleaved_frame(&mut publisher, 0, &publish_packet.encode()).await;

    let (channel, payload) = read_interleaved_frame(&mut tunnel_get, "MTU-TUNNEL-RTP").await;
    assert_eq!(channel, 2);
    let packet = RtpPacket::parse(&payload).expect("parse forwarded rtp");
    assert!(
        packet.header.marker,
        "TCP/HTTP tunnel PLAY should keep large H264 NAL in one interleaved RTP packet"
    );
    let nal_type = packet.payload.first().map(|b| b & 0x1f).unwrap_or_default();
    assert_ne!(
        nal_type, 28,
        "TCP/HTTP tunnel PLAY should not fragment large H264 into FU-A when constrained by udp rtp_mtu"
    );
    assert!(
        packet.payload.len() > 1000,
        "TCP/HTTP tunnel PLAY should emit payload size beyond configured udp rtp_mtu envelope"
    );

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_tunnel_announce_record_publishes_media_to_rtsp_player() {
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

    let uri = format!("rtsp://{listen}/live/http-tunnel-publish");
    let path = "/live/http-tunnel-publish";
    let (mut tunnel_get, mut tunnel_post) =
        open_http_tunnel_pair(listen, path, "cookie-http-publish", true).await;

    let announce = build_request(
        "ANNOUNCE",
        &uri,
        1,
        None,
        &[("Content-Type", "application/sdp")],
        ANNOUNCE_SDP.as_bytes(),
    );
    tunnel_send_rtsp_request(&mut tunnel_post, &announce).await;
    let announce_resp = read_response(&mut tunnel_get, "TUNNEL-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publisher_session = announce_resp
        .header("Session")
        .expect("publisher session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{uri}/trackID=0"),
        2,
        Some(&publisher_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        &[],
    );
    tunnel_send_rtsp_request(&mut tunnel_post, &setup).await;
    let setup_resp = read_response(&mut tunnel_get, "TUNNEL-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record = build_request("RECORD", &uri, 3, Some(&publisher_session), &[], &[]);
    tunnel_send_rtsp_request(&mut tunnel_post, &record).await;
    let record_resp = read_response(&mut tunnel_get, "TUNNEL-RECORD").await;
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

    let publish_packet = build_publish_h264_rtp(9100, 550_000, 0x1122_3344);
    tunnel_send_interleaved_frame(&mut tunnel_post, 0, &publish_packet).await;
    let (channel, payload) = read_interleaved_frame(&mut player, "PLAYER-RTP").await;
    assert_eq!(channel, 2);
    let packet = RtpPacket::parse(&payload).expect("parse forwarded rtp");
    assert_eq!(packet.header.payload_type, 96);

    engine.stop().await;
}
