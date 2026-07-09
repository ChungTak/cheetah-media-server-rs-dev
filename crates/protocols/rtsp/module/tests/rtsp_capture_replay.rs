use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::{Engine, EngineBuilder};
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleState, StreamKey, StreamManagerApi};
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::time::{sleep, timeout};

mod common;
use common::{
    connect_with_retry, parse_transport_server_ports, read_interleaved_frame, read_response,
    write_request, RtspResponse,
};

const H264_TCP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_tcp_publish_play.rtspcap"
);
const H264_UDP_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/standard/h264_udp_publish_play.rtspcap"
);
const AV1_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/av1_probe.rtspcap"
);
const VP8_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp8_probe.rtspcap"
);
const VP9_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/vp9_probe.rtspcap"
);
const H266_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/h266_probe.rtspcap"
);
const HIGH_BITRATE_PROBE_CAPTURE: &[u8] = include_bytes!(
    "../../testing/property-tests/tests/testdata/rtsp-capture/probes/high_bitrate_probe.rtspcap"
);

const KIND_RTSP_TCP_C2S: u8 = 1;
const KIND_UDP_PUBLISH_RTP: u8 = 3;
const KIND_UDP_PUBLISH_RTCP: u8 = 4;
#[derive(Debug, Clone)]
struct RawRecord {
    kind: u8,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ParsedRequest {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_interleaved_capture_publish_and_play_replay() {
    let listen = reserve_listen_addr();
    let engine = start_engine(listen).await;

    let records = decode_rtspcap(H264_TCP_CAPTURE).expect("decode h264 tcp fixture");
    let publish_options =
        find_request(&records, |req| req.method == "OPTIONS").expect("fixture OPTIONS request");
    let publish_announce =
        find_request(&records, |req| req.method == "ANNOUNCE").expect("fixture ANNOUNCE request");
    let publish_setup = find_request(&records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport").is_some_and(|v| v.contains("mode=RECORD"))
    })
    .expect("fixture publish SETUP request");
    let publish_record =
        find_request(&records, |req| req.method == "RECORD").expect("fixture RECORD request");
    let player_describe =
        find_request(&records, |req| req.method == "DESCRIBE").expect("fixture DESCRIBE request");
    let player_setup = find_request(&records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport").is_some_and(|v| !v.contains("mode=RECORD"))
    })
    .expect("fixture play SETUP request");
    let player_play =
        find_request(&records, |req| req.method == "PLAY").expect("fixture PLAY request");

    let announce_uri = rewrite_uri_authority(&publish_announce.uri, listen);
    let stream_name = stream_name_from_uri(&announce_uri);

    let mut publisher = connect_with_retry(listen).await;

    let options_req = render_request(&publish_options, listen, None, None, None, None);
    write_request(&mut publisher, &options_req).await;
    let options_resp = read_response(&mut publisher, "CAPTURE-PUBLISH-OPTIONS").await;
    assert_eq!(options_resp.status_code, 200);

    let announce_req = render_request(&publish_announce, listen, None, None, None, None);
    write_request(&mut publisher, &announce_req).await;
    let announce_resp = read_response(&mut publisher, "CAPTURE-PUBLISH-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publish_session = announce_resp
        .header("Session")
        .expect("publish session")
        .to_string();

    let setup_req = render_request(
        &publish_setup,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher, &setup_req).await;
    let setup_resp = read_response(&mut publisher, "CAPTURE-PUBLISH-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let record_req = render_request(
        &publish_record,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher, &record_req).await;
    let record_resp = read_response(&mut publisher, "CAPTURE-PUBLISH-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    wait_for_stream_tracks(
        &engine,
        &StreamKey::new("live", &stream_name),
        1,
        "capture-publish",
    )
    .await;

    let mut player = connect_with_retry(listen).await;
    let describe_req = render_request(&player_describe, listen, None, None, None, None);
    write_request(&mut player, &describe_req).await;
    let describe_resp = read_response(&mut player, "CAPTURE-PLAY-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    let expected_pt = parse_sdp_video_payload_type(&describe_resp)
        .expect("describe sdp should expose video payload type");

    let describe_base_uri = describe_resp
        .header("Content-Base")
        .map(|value| rewrite_uri_authority(value.trim_end_matches('/'), listen))
        .unwrap_or_else(|| announce_uri.clone());
    let player_setup_uri =
        parse_setup_uri_from_describe(&describe_resp, &describe_base_uri, listen)
            .expect("describe sdp should include usable control uri");
    let setup_req = render_request(
        &player_setup,
        listen,
        Some(&player_session),
        Some("RTP/AVP/TCP;unicast;interleaved=2-3"),
        None,
        Some(&player_setup_uri),
    );
    write_request(&mut player, &setup_req).await;
    let setup_resp = read_response(&mut player, "CAPTURE-PLAY-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let play_req = render_request(
        &player_play,
        listen,
        Some(&player_session),
        None,
        None,
        Some(&announce_uri),
    );
    write_request(&mut player, &play_req).await;
    let play_resp = read_response(&mut player, "CAPTURE-PLAY-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let publish_interleaved_records = records
        .iter()
        .filter(|record| record.kind == KIND_RTSP_TCP_C2S)
        .filter(|record| record.payload.first().copied() == Some(b'$'))
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    assert!(
        !publish_interleaved_records.is_empty(),
        "fixture should include C2S interleaved publish payloads"
    );
    for payload in &publish_interleaved_records {
        publisher
            .write_all(payload)
            .await
            .expect("write raw interleaved publish payload");
    }

    let mut got_rtp = None;
    for attempt in 0..24 {
        let (channel, payload) =
            read_interleaved_frame(&mut player, &format!("CAPTURE-PLAY-RTP-{attempt}")).await;
        if channel != 2 {
            continue;
        }
        if let Some(packet) = RtpPacket::parse(&payload) {
            got_rtp = Some(packet);
            break;
        }
    }
    let packet = got_rtp.expect("player should receive forwarded interleaved RTP");
    assert_eq!(packet.header.payload_type, expected_pt);
    assert!(!packet.payload.is_empty());

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn udp_capture_publish_and_play_replay() {
    let listen = reserve_listen_addr();
    let engine = start_engine(listen).await;

    let records = decode_rtspcap(H264_UDP_CAPTURE).expect("decode h264 udp fixture");
    let publish_options =
        find_request(&records, |req| req.method == "OPTIONS").expect("fixture OPTIONS request");
    let publish_announce =
        find_request(&records, |req| req.method == "ANNOUNCE").expect("fixture ANNOUNCE request");
    let publish_setup = find_request(&records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport").is_some_and(|v| v.contains("mode=RECORD"))
    })
    .expect("fixture publish SETUP request");
    let publish_record =
        find_request(&records, |req| req.method == "RECORD").expect("fixture RECORD request");
    let player_describe =
        find_request(&records, |req| req.method == "DESCRIBE").expect("fixture DESCRIBE request");
    let player_setup = find_request(&records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport")
                .is_some_and(|v| !v.contains("mode=RECORD") && v.contains("client_port"))
    })
    .expect("fixture play SETUP request");
    let player_play =
        find_request(&records, |req| req.method == "PLAY").expect("fixture PLAY request");

    let publish_rtp_datagrams = records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTP)
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    let publish_rtcp_datagrams = records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTCP)
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    assert!(
        !publish_rtp_datagrams.is_empty(),
        "fixture should include udp publish rtp datagrams"
    );

    let announce_uri = rewrite_uri_authority(&publish_announce.uri, listen);
    let stream_name = stream_name_from_uri(&announce_uri);

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
    let publish_transport = format!(
        "RTP/AVP/UDP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port};mode=RECORD"
    );

    let mut publisher = connect_with_retry(listen).await;

    let options_req = render_request(&publish_options, listen, None, None, None, None);
    write_request(&mut publisher, &options_req).await;
    let options_resp = read_response(&mut publisher, "UDP-CAPTURE-PUBLISH-OPTIONS").await;
    assert_eq!(options_resp.status_code, 200);

    let announce_req = render_request(&publish_announce, listen, None, None, None, None);
    write_request(&mut publisher, &announce_req).await;
    let announce_resp = read_response(&mut publisher, "UDP-CAPTURE-PUBLISH-ANNOUNCE").await;
    assert_eq!(announce_resp.status_code, 200);
    let publish_session = announce_resp
        .header("Session")
        .expect("publish session")
        .to_string();

    let setup_req = render_request(
        &publish_setup,
        listen,
        Some(&publish_session),
        Some(&publish_transport),
        None,
        None,
    );
    write_request(&mut publisher, &setup_req).await;
    let setup_resp = read_response(&mut publisher, "UDP-CAPTURE-PUBLISH-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);
    let publish_transport_resp = setup_resp
        .header("Transport")
        .expect("publish setup transport");
    let (publish_server_rtp_port, publish_server_rtcp_port) =
        parse_transport_server_ports(publish_transport_resp)
            .expect("parse publish setup server ports");

    let record_req = render_request(
        &publish_record,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher, &record_req).await;
    let record_resp = read_response(&mut publisher, "UDP-CAPTURE-PUBLISH-RECORD").await;
    assert_eq!(record_resp.status_code, 200);

    wait_for_stream_tracks(
        &engine,
        &StreamKey::new("live", &stream_name),
        1,
        "udp-capture-publish",
    )
    .await;

    let player_rtp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player rtp");
    let player_rtcp = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind player rtcp");
    let player_rtp_port = player_rtp.local_addr().expect("player rtp addr").port();
    let player_rtcp_port = player_rtcp.local_addr().expect("player rtcp addr").port();
    let player_transport =
        format!("RTP/AVP;unicast;client_port={player_rtp_port}-{player_rtcp_port}");

    let mut player = connect_with_retry(listen).await;
    let describe_req = render_request(&player_describe, listen, None, None, None, None);
    write_request(&mut player, &describe_req).await;
    let describe_resp = read_response(&mut player, "UDP-CAPTURE-PLAY-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let player_session = describe_resp
        .header("Session")
        .expect("player session")
        .to_string();
    let expected_pt = parse_sdp_video_payload_type(&describe_resp)
        .expect("describe sdp should expose video payload type");
    let describe_base_uri = describe_resp
        .header("Content-Base")
        .map(|value| rewrite_uri_authority(value.trim_end_matches('/'), listen))
        .unwrap_or_else(|| announce_uri.clone());
    let player_setup_uri =
        parse_setup_uri_from_describe(&describe_resp, &describe_base_uri, listen)
            .expect("describe sdp should include usable control uri");

    let player_setup_req = render_request(
        &player_setup,
        listen,
        Some(&player_session),
        Some(&player_transport),
        None,
        Some(&player_setup_uri),
    );
    write_request(&mut player, &player_setup_req).await;
    let player_setup_resp = read_response(&mut player, "UDP-CAPTURE-PLAY-SETUP").await;
    assert_eq!(player_setup_resp.status_code, 200);
    let play_transport_resp = player_setup_resp
        .header("Transport")
        .expect("play setup transport");
    let (play_server_rtp_port, play_server_rtcp_port) =
        parse_transport_server_ports(play_transport_resp).expect("parse play setup server ports");

    let play_req = render_request(
        &player_play,
        listen,
        Some(&player_session),
        None,
        None,
        Some(&announce_uri),
    );
    write_request(&mut player, &play_req).await;
    let play_resp = read_response(&mut player, "UDP-CAPTURE-PLAY-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let publish_rtp_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtp_port));
    let publish_rtcp_target = SocketAddr::from(([127, 0, 0, 1], publish_server_rtcp_port));
    let mut got_rtp = None;
    let mut got_rtcp_type = None;
    let mut seq_samples = Vec::<u16>::new();
    let mut rtp_recv_buf = [0u8; 2048];
    let mut rtcp_recv_buf = [0u8; 2048];
    let send_rounds = publish_rtp_datagrams.len().min(256);

    for (round, rtp_datagram) in publish_rtp_datagrams.iter().enumerate().take(send_rounds) {
        publisher_rtp
            .send_to(rtp_datagram, publish_rtp_target)
            .await
            .expect("send fixture udp publish rtp");
        if !publish_rtcp_datagrams.is_empty() && round % 4 == 0 {
            let rtcp_idx = (round / 4) % publish_rtcp_datagrams.len();
            publisher_rtcp
                .send_to(&publish_rtcp_datagrams[rtcp_idx], publish_rtcp_target)
                .await
                .expect("send fixture udp publish rtcp");
        }

        if got_rtp.is_none() {
            if let Ok(Ok((n, from))) = timeout(
                Duration::from_millis(20),
                player_rtp.recv_from(&mut rtp_recv_buf),
            )
            .await
            {
                if from.port() == play_server_rtp_port {
                    if let Some(packet) = RtpPacket::parse(&rtp_recv_buf[..n]) {
                        seq_samples.push(packet.header.sequence_number);
                        got_rtp = Some(packet);
                    }
                }
            }
        } else if let Ok(Ok((n, from))) = timeout(
            Duration::from_millis(20),
            player_rtp.recv_from(&mut rtp_recv_buf),
        )
        .await
        {
            if from.port() == play_server_rtp_port {
                if let Some(packet) = RtpPacket::parse(&rtp_recv_buf[..n]) {
                    if let Some(first) = got_rtp.as_ref() {
                        if packet.header.ssrc == first.header.ssrc {
                            seq_samples.push(packet.header.sequence_number);
                        }
                    }
                }
            }
        }

        if got_rtcp_type.is_none() {
            if let Ok(Ok((n, from))) = timeout(
                Duration::from_millis(20),
                player_rtcp.recv_from(&mut rtcp_recv_buf),
            )
            .await
            {
                if from.port() == play_server_rtcp_port && n >= 2 {
                    let packet_type = rtcp_recv_buf[1];
                    if (200..=204).contains(&packet_type) {
                        got_rtcp_type = Some(packet_type);
                    }
                }
            }
        }

        if got_rtp.is_some() && got_rtcp_type.is_some() && seq_samples.len() >= 3 {
            break;
        }
    }

    let packet = got_rtp.expect("player should receive forwarded udp rtp");
    assert_eq!(packet.header.payload_type, expected_pt);
    assert!(!packet.payload.is_empty());
    assert!(
        got_rtcp_type.is_some(),
        "player should receive at least one valid RTCP packet type"
    );
    if seq_samples.len() >= 2 {
        let monotonic = seq_samples
            .windows(2)
            .all(|pair| pair[1].wrapping_sub(pair[0]) > 0);
        assert!(
            monotonic,
            "same-ssrc RTP sequence should be increasing, got {seq_samples:?}"
        );
    }

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn probe_capture_replay_keeps_rtsp_module_running() {
    let fixtures = [
        ("av1_probe", AV1_PROBE_CAPTURE),
        ("vp8_probe", VP8_PROBE_CAPTURE),
        ("vp9_probe", VP9_PROBE_CAPTURE),
        ("h266_probe", H266_PROBE_CAPTURE),
        ("high_bitrate_probe", HIGH_BITRATE_PROBE_CAPTURE),
    ];

    for (case, capture) in fixtures {
        let listen = reserve_listen_addr();
        let engine = start_engine(listen).await;
        let records = decode_rtspcap(capture).expect("decode probe fixture");
        replay_probe_publish_best_effort(&records, listen, case).await;
        assert_rtsp_module_state(&engine, ModuleState::Running, case);
        assert_engine_health_ok(&engine, case);
        engine.stop().await;
        assert_rtsp_module_state(&engine, ModuleState::Stopped, case);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn udp_fault_views_keep_engine_healthy_and_module_stoppable() {
    let records = decode_rtspcap(H264_UDP_CAPTURE).expect("decode h264 udp fixture");
    let base_rtp = records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTP)
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    let base_rtcp = records
        .iter()
        .filter(|record| record.kind == KIND_UDP_PUBLISH_RTCP)
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    assert!(!base_rtp.is_empty(), "udp fault seed should contain rtp");

    let views = build_udp_fault_views(&base_rtp, &base_rtcp);
    assert!(!views.is_empty(), "udp fault views should not be empty");

    for view in views {
        let listen = reserve_listen_addr();
        let engine = start_engine(listen).await;
        let publish = setup_udp_publish_session(&records, listen, view.name).await;
        for datagram in &view.rtp {
            if publish
                .publisher_rtp
                .send_to(
                    datagram,
                    SocketAddr::from(([127, 0, 0, 1], publish.server_rtp_port)),
                )
                .await
                .is_err()
            {
                break;
            }
        }
        for datagram in &view.rtcp {
            if publish
                .publisher_rtcp
                .send_to(
                    datagram,
                    SocketAddr::from(([127, 0, 0, 1], publish.server_rtcp_port)),
                )
                .await
                .is_err()
            {
                break;
            }
        }

        assert_rtsp_module_state(&engine, ModuleState::Running, view.name);
        assert_engine_health_ok(&engine, view.name);
        drop(publish.publisher_stream);
        engine.stop().await;
        assert_rtsp_module_state(&engine, ModuleState::Stopped, view.name);
    }
}

fn reserve_listen_addr() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let addr = probe.local_addr().expect("probe local addr");
    drop(probe);
    addr
}

async fn start_engine(listen: SocketAddr) -> Engine {
    let config = Arc::new(ConfigStore::new());
    let config_yaml =
        format!("modules:\n  rtsp:\n    listen: \"{listen}\"\n    start_from_keyframe: false\n");
    config
        .load_yaml_str(&config_yaml)
        .expect("load rtsp config yaml");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");
    engine
}

async fn wait_for_stream_tracks(
    engine: &Engine,
    stream_key: &StreamKey,
    expected_tracks: usize,
    stage: &str,
) {
    let api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Some(snapshot)) = api.get_stream(stream_key).await {
            if snapshot.tracks.len() >= expected_tracks {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting tracks at {stage}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

fn stream_name_from_uri(uri: &str) -> String {
    let path = uri
        .split_once("://")
        .and_then(|(_, right)| right.split_once('/').map(|(_, p)| p))
        .unwrap_or(uri);
    path.split('/')
        .rfind(|part| !part.is_empty())
        .unwrap_or("capture")
        .to_string()
}

fn rewrite_uri_authority(uri: &str, listen: SocketAddr) -> String {
    if let Some(rest) = uri.strip_prefix("rtsp://127.0.0.1:8554") {
        format!("rtsp://{listen}{rest}")
    } else {
        uri.to_string()
    }
}

fn render_request(
    req: &ParsedRequest,
    listen: SocketAddr,
    session_override: Option<&str>,
    transport_override: Option<&str>,
    cseq_override: Option<u32>,
    uri_override: Option<&str>,
) -> String {
    let uri = uri_override
        .map(str::to_string)
        .unwrap_or_else(|| rewrite_uri_authority(&req.uri, listen));
    let mut out = format!("{} {} RTSP/1.0\r\n", req.method, uri);

    let mut has_cseq = false;
    for (name, value) in &req.headers {
        if name.eq_ignore_ascii_case("CSeq") {
            has_cseq = true;
            let cseq = cseq_override
                .map(|v| v.to_string())
                .unwrap_or_else(|| value.clone());
            out.push_str(&format!("CSeq: {cseq}\r\n"));
            continue;
        }
        if name.eq_ignore_ascii_case("Session") {
            if let Some(session) = session_override {
                out.push_str(&format!("Session: {session}\r\n"));
            } else {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            continue;
        }
        if name.eq_ignore_ascii_case("Transport") {
            if let Some(transport) = transport_override {
                out.push_str(&format!("Transport: {transport}\r\n"));
            } else {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            continue;
        }
        if name.eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    if !has_cseq {
        let cseq = cseq_override.unwrap_or(1);
        out.push_str(&format!("CSeq: {cseq}\r\n"));
    }

    out.push_str(&format!("Content-Length: {}\r\n\r\n", req.body.len()));
    out.push_str(std::str::from_utf8(&req.body).expect("fixture request body utf-8"));
    out
}

fn header_value<'a>(req: &'a ParsedRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn parse_sdp_video_payload_type(response: &RtspResponse) -> Option<u8> {
    let sdp = std::str::from_utf8(&response.body).ok()?;
    for line in sdp.lines() {
        if let Some(rest) = line.strip_prefix("m=video ") {
            let mut parts = rest.split_whitespace();
            let _port = parts.next()?;
            let _proto = parts.next()?;
            let pt = parts.next()?;
            if let Ok(value) = pt.parse::<u8>() {
                return Some(value);
            }
        }
    }
    None
}

fn parse_setup_uri_from_describe(
    response: &RtspResponse,
    base_uri: &str,
    listen: SocketAddr,
) -> Option<String> {
    let sdp = std::str::from_utf8(&response.body).ok()?;
    let mut current_media = None::<&str>;
    let mut first_control = None::<&str>;
    let mut video_control = None::<&str>;

    for raw_line in sdp.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("m=") {
            current_media = rest.split_whitespace().next();
            continue;
        }
        if let Some(control) = line.strip_prefix("a=control:") {
            let control = control.trim();
            if control.is_empty() {
                continue;
            }
            if first_control.is_none() {
                first_control = Some(control);
            }
            if current_media.is_some_and(|media| media.eq_ignore_ascii_case("video")) {
                video_control = Some(control);
            }
        }
    }

    let selected_control = video_control.or(first_control)?;
    Some(resolve_control_uri(selected_control, base_uri, listen))
}

fn resolve_control_uri(control: &str, base_uri: &str, listen: SocketAddr) -> String {
    if control.starts_with("rtsp://") {
        return rewrite_uri_authority(control, listen);
    }
    if control.starts_with('/') {
        return format!("rtsp://{listen}{control}");
    }
    format!(
        "{}/{}",
        base_uri.trim_end_matches('/'),
        control.trim_start_matches('/')
    )
}

fn find_request<F>(records: &[RawRecord], predicate: F) -> Option<ParsedRequest>
where
    F: Fn(&ParsedRequest) -> bool,
{
    for record in records {
        if record.kind != KIND_RTSP_TCP_C2S {
            continue;
        }
        let Some(request) = parse_request(&record.payload) else {
            continue;
        };
        if predicate(&request) {
            return Some(request);
        }
    }
    None
}

fn parse_request(payload: &[u8]) -> Option<ParsedRequest> {
    let text = std::str::from_utf8(payload).ok()?;
    let header_end = text.find("\r\n\r\n")?;
    let header_text = &text[..header_end];
    let mut lines = header_text.split("\r\n");
    let start = lines.next()?;
    let mut start_parts = start.split_whitespace();
    let method = start_parts.next()?.to_string();
    if !method
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
    {
        return None;
    }
    let uri = start_parts.next()?.to_string();
    let _version = start_parts.next()?;

    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);

    let body_start = header_end + 4;
    if payload.len() < body_start + content_length {
        return None;
    }
    let body = payload[body_start..body_start + content_length].to_vec();

    Some(ParsedRequest {
        method,
        uri,
        headers,
        body,
    })
}

fn decode_rtspcap(bytes: &[u8]) -> Result<Vec<RawRecord>, String> {
    if bytes.len() < 8 {
        return Err("truncated rtspcap header".to_string());
    }
    if &bytes[..4] != b"RSF1" {
        return Err("bad rtspcap magic".to_string());
    }

    let mut cursor = 4usize;
    let count = read_u32(bytes, &mut cursor)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = read_u8(bytes, &mut cursor)?;
        let _flags = read_u8(bytes, &mut cursor)?;
        let _flow_id = read_u16(bytes, &mut cursor)?;
        let _delta_us = read_u32(bytes, &mut cursor)?;
        let payload_len = read_u32(bytes, &mut cursor)? as usize;
        if payload_len == 0 {
            return Err("zero length payload record".to_string());
        }
        let payload = read_bytes(bytes, &mut cursor, payload_len)?.to_vec();
        out.push(RawRecord { kind, payload });
    }
    if cursor != bytes.len() {
        return Err("rtspcap trailing bytes".to_string());
    }
    Ok(out)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, String> {
    if *cursor + 1 > bytes.len() {
        return Err("truncated u8".to_string());
    }
    let value = bytes[*cursor];
    *cursor += 1;
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, String> {
    let raw = read_bytes(bytes, cursor, 2)?;
    Ok(u16::from_be_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let raw = read_bytes(bytes, cursor, 4)?;
    Ok(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> Result<&'a [u8], String> {
    if *cursor + len > bytes.len() {
        return Err("truncated bytes".to_string());
    }
    let out = &bytes[*cursor..*cursor + len];
    *cursor += len;
    Ok(out)
}

struct UdpPublishSession {
    publisher_stream: tokio::net::TcpStream,
    publisher_rtp: UdpSocket,
    publisher_rtcp: UdpSocket,
    server_rtp_port: u16,
    server_rtcp_port: u16,
}

#[derive(Debug, Clone)]
struct UdpFaultView {
    name: &'static str,
    rtp: Vec<Vec<u8>>,
    rtcp: Vec<Vec<u8>>,
}

async fn setup_udp_publish_session(
    records: &[RawRecord],
    listen: SocketAddr,
    stage: &str,
) -> UdpPublishSession {
    let publish_options = find_request(records, |req| req.method == "OPTIONS");
    let publish_announce =
        find_request(records, |req| req.method == "ANNOUNCE").expect("fixture ANNOUNCE request");
    let publish_setup = find_request(records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport").is_some_and(|v| v.contains("mode=RECORD"))
    })
    .expect("fixture publish SETUP request");
    let publish_record =
        find_request(records, |req| req.method == "RECORD").expect("fixture RECORD request");

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
    let publish_transport = format!(
        "RTP/AVP/UDP;unicast;client_port={publisher_rtp_port}-{publisher_rtcp_port};mode=RECORD"
    );

    let mut publisher_stream = connect_with_retry(listen).await;

    if let Some(options) = publish_options.as_ref() {
        let options_req = render_request(options, listen, None, None, None, None);
        write_request(&mut publisher_stream, &options_req).await;
        let options_resp =
            read_response(&mut publisher_stream, &format!("{stage}-PUBLISH-OPTIONS")).await;
        assert_eq!(options_resp.status_code, 200);
    }

    let announce_req = render_request(&publish_announce, listen, None, None, None, None);
    write_request(&mut publisher_stream, &announce_req).await;
    let announce_resp =
        read_response(&mut publisher_stream, &format!("{stage}-PUBLISH-ANNOUNCE")).await;
    assert_eq!(announce_resp.status_code, 200);
    let publish_session = announce_resp
        .header("Session")
        .expect("publish session")
        .to_string();

    let setup_req = render_request(
        &publish_setup,
        listen,
        Some(&publish_session),
        Some(&publish_transport),
        None,
        None,
    );
    write_request(&mut publisher_stream, &setup_req).await;
    let setup_resp = read_response(&mut publisher_stream, &format!("{stage}-PUBLISH-SETUP")).await;
    assert_eq!(setup_resp.status_code, 200);
    let setup_transport = setup_resp
        .header("Transport")
        .expect("publish setup transport");
    let (server_rtp_port, server_rtcp_port) =
        parse_transport_server_ports(setup_transport).expect("parse publish setup server ports");

    let record_req = render_request(
        &publish_record,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher_stream, &record_req).await;
    let record_resp =
        read_response(&mut publisher_stream, &format!("{stage}-PUBLISH-RECORD")).await;
    assert_eq!(record_resp.status_code, 200);

    UdpPublishSession {
        publisher_stream,
        publisher_rtp,
        publisher_rtcp,
        server_rtp_port,
        server_rtcp_port,
    }
}

async fn replay_probe_publish_best_effort(records: &[RawRecord], listen: SocketAddr, stage: &str) {
    let Some(announce_req) = find_request(records, |req| req.method == "ANNOUNCE") else {
        return;
    };
    let mut publisher = connect_with_retry(listen).await;
    let announce_wire = render_request(&announce_req, listen, None, None, None, None);
    write_request(&mut publisher, &announce_wire).await;
    let announce_resp = read_response(&mut publisher, &format!("{stage}-PROBE-ANNOUNCE")).await;
    if announce_resp.status_code != 200 {
        return;
    }

    let Some(publish_setup) = find_request(records, |req| {
        req.method == "SETUP"
            && header_value(req, "Transport").is_some_and(|v| v.contains("mode=RECORD"))
    }) else {
        return;
    };
    let Some(publish_record) = find_request(records, |req| req.method == "RECORD") else {
        return;
    };
    let publish_session = announce_resp
        .header("Session")
        .expect("probe publish session")
        .to_string();
    let setup_req = render_request(
        &publish_setup,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher, &setup_req).await;
    let setup_resp = read_response(&mut publisher, &format!("{stage}-PROBE-SETUP")).await;
    if setup_resp.status_code != 200 {
        return;
    }
    let record_req = render_request(
        &publish_record,
        listen,
        Some(&publish_session),
        None,
        None,
        None,
    );
    write_request(&mut publisher, &record_req).await;
    let record_resp = read_response(&mut publisher, &format!("{stage}-PROBE-RECORD")).await;
    if record_resp.status_code != 200 {
        return;
    }

    let interleaved = records
        .iter()
        .filter(|record| record.kind == KIND_RTSP_TCP_C2S)
        .filter(|record| record.payload.first().copied() == Some(b'$'))
        .take(8)
        .map(|record| record.payload.clone())
        .collect::<Vec<_>>();
    for payload in &interleaved {
        let _ = publisher.write_all(payload).await;
    }
}

fn build_udp_fault_views(base_rtp: &[Vec<u8>], base_rtcp: &[Vec<u8>]) -> Vec<UdpFaultView> {
    vec![
        UdpFaultView {
            name: "udp_drop_every_5th",
            rtp: drop_every_nth(base_rtp, 5),
            rtcp: base_rtcp.to_vec(),
        },
        UdpFaultView {
            name: "udp_drop_first_fu_start",
            rtp: drop_first_fu_start(base_rtp),
            rtcp: base_rtcp.to_vec(),
        },
        UdpFaultView {
            name: "udp_drop_rtcp_sr",
            rtp: base_rtp.to_vec(),
            rtcp: drop_first_rtcp_sr(base_rtcp),
        },
        UdpFaultView {
            name: "udp_swap_adjacent",
            rtp: swap_adjacent(base_rtp),
            rtcp: base_rtcp.to_vec(),
        },
        UdpFaultView {
            name: "udp_reverse_window_4",
            rtp: reverse_small_window(base_rtp, 4),
            rtcp: base_rtcp.to_vec(),
        },
        UdpFaultView {
            name: "udp_duplicate_old_sequence",
            rtp: duplicate_old_sequence(base_rtp),
            rtcp: base_rtcp.to_vec(),
        },
        UdpFaultView {
            name: "udp_truncate_payload_half",
            rtp: truncate_payload_half(base_rtp),
            rtcp: base_rtcp.to_vec(),
        },
    ]
}

fn drop_every_nth(payloads: &[Vec<u8>], n: usize) -> Vec<Vec<u8>> {
    let out = payloads
        .iter()
        .enumerate()
        .filter(|(idx, _)| (idx + 1) % n != 0)
        .map(|(_, payload)| payload.clone())
        .collect::<Vec<_>>();
    if out.is_empty() {
        payloads.iter().take(1).cloned().collect()
    } else {
        out
    }
}

fn drop_first_fu_start(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut dropped = false;
    let mut out = Vec::new();
    for payload in payloads {
        if !dropped && is_h264_fu_start(payload) {
            dropped = true;
            continue;
        }
        out.push(payload.clone());
    }
    if out.is_empty() {
        payloads.iter().take(1).cloned().collect()
    } else {
        out
    }
}

fn is_h264_fu_start(packet: &[u8]) -> bool {
    if packet.len() < 14 {
        return false;
    }
    let csrc_count = (packet[0] & 0x0f) as usize;
    let header_len = 12 + csrc_count * 4;
    if packet.len() < header_len + 2 {
        return false;
    }
    let nalu_type = packet[header_len] & 0x1f;
    let fu_header = packet[header_len + 1];
    nalu_type == 28 && (fu_header & 0x80) != 0
}

fn drop_first_rtcp_sr(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut dropped = false;
    let mut out = Vec::new();
    for payload in payloads {
        if !dropped && payload.len() >= 2 && payload[1] == 200 {
            dropped = true;
            continue;
        }
        out.push(payload.clone());
    }
    out
}

fn swap_adjacent(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    let mut idx = 0usize;
    while idx + 1 < out.len() {
        out.swap(idx, idx + 1);
        idx += 2;
    }
    out
}

fn reverse_small_window(payloads: &[Vec<u8>], window: usize) -> Vec<Vec<u8>> {
    let mut out = payloads.to_vec();
    let mut start = 0usize;
    while start < out.len() {
        let end = (start + window).min(out.len());
        out[start..end].reverse();
        start += window;
    }
    out
}

fn duplicate_old_sequence(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if payloads.is_empty() {
        return Vec::new();
    }
    let mut out = payloads.to_vec();
    out.push(payloads[0].clone());
    out
}

fn truncate_payload_half(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .map(|payload| {
            let keep = (payload.len() / 2).max(1);
            payload[..keep].to_vec()
        })
        .collect()
}

fn assert_engine_health_ok(engine: &Engine, stage: &str) {
    let health = engine.health_api();
    assert!(health.is_live(), "engine must stay live at {stage}");
    assert!(health.is_ready(), "engine must stay ready at {stage}");
}

fn assert_rtsp_module_state(engine: &Engine, expected: ModuleState, stage: &str) {
    let states = engine.module_manager_api().modules();
    let actual = states
        .iter()
        .find(|(module_id, _)| module_id.0 == "rtsp")
        .map(|(_, state)| *state)
        .unwrap_or_else(|| panic!("missing rtsp module state at {stage}"));
    assert_eq!(
        actual, expected,
        "rtsp module state mismatch at {stage}, expected {expected:?}, got {actual:?}"
    );
}
