use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::RtpPacket;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{StreamKey, StreamManagerApi};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Notify};
use tokio::time::{sleep, timeout};

#[tokio::test(flavor = "current_thread")]
async fn relay_job_hidden_stream_is_observable_and_forwards_to_remote_target() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind relay source listener");
    let source_addr = source_listener.local_addr().expect("relay source addr");
    let source_url = format!("rtsp://{source_addr}/live/relay-source");
    let source_url_for_server = source_url.clone();

    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind relay target listener");
    let target_addr = target_listener.local_addr().expect("relay target addr");
    let target_url = format!("rtsp://{target_addr}/live/relay-target");
    let target_url_for_server = target_url.clone();

    let record_ready = Arc::new(Notify::new());
    let source_record_ready = record_ready.clone();
    let target_record_ready = record_ready.clone();
    let (forwarded_tx, forwarded_rx) = oneshot::channel::<()>();

    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener
            .accept()
            .await
            .expect("accept relay source client");

        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("relay source options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_url_for_server} RTSP/1.0")));
        let describe_cseq = extract_cseq(&describe_req).expect("relay source describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=relay-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {source_url_for_server}/trackID=0 RTSP/1.0")));
        let setup_cseq = extract_cseq(&setup_req).expect("relay source setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "relay-source-session;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.starts_with(&format!("PLAY {source_url_for_server} RTSP/1.0")));
        let play_cseq = extract_cseq(&play_req).expect("relay source play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "relay-source-session")],
            &[],
        )
        .await;

        timeout(Duration::from_secs(3), source_record_ready.notified())
            .await
            .expect("wait relay record ready timeout");

        let mut rtp = Vec::new();
        rtp.extend_from_slice(&[
            0x80, 0xE0, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x11, 0x22, 0x33, 0x44, 0x65, 0x88,
            0x99,
        ]);
        let mut interleaved = Vec::with_capacity(rtp.len() + 4);
        interleaved.push(b'$');
        interleaved.push(0);
        interleaved.extend_from_slice(&(rtp.len() as u16).to_be_bytes());
        interleaved.extend_from_slice(&rtp);
        socket
            .write_all(&interleaved)
            .await
            .expect("write relay source rtp");

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(2), socket.read(&mut drain)).await;
    });

    let target_server = tokio::spawn(async move {
        let (mut socket, _) = target_listener
            .accept()
            .await
            .expect("accept relay target client");

        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {target_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("relay target options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
            &[],
        )
        .await;

        let announce_req = read_rtsp_request(&mut socket).await;
        assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        assert!(announce_req.contains("m=video 0 RTP/AVP 96"));
        let announce_cseq = extract_cseq(&announce_req).expect("relay target announce cseq");
        write_rtsp_response(
            &mut socket,
            announce_cseq,
            &[("Session", "relay-target-session;timeout=60")],
            &[],
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {target_url_for_server}/trackID=0 RTSP/1.0")));
        let setup_cseq = extract_cseq(&setup_req).expect("relay target setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "relay-target-session;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let record_req = read_rtsp_request(&mut socket).await;
        assert!(record_req.starts_with(&format!("RECORD {target_url_for_server} RTSP/1.0")));
        let record_cseq = extract_cseq(&record_req).expect("relay target record cseq");
        write_rtsp_response(
            &mut socket,
            record_cseq,
            &[("Session", "relay-target-session")],
            &[],
        )
        .await;
        target_record_ready.notify_waiters();

        let (channel, payload) = read_interleaved_frame(&mut socket).await;
        assert_eq!(channel, 0, "relay RTP must be on channel 0");
        let packet = RtpPacket::parse(&payload).expect("parse relay pushed rtp");
        assert!(!packet.payload.is_empty());
        let _ = forwarded_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    relay_jobs:\n      - name: \"relay-main\"\n        enabled: true\n        source_url: \"{source_url}\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
    );
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(&config_yaml)
        .expect("load relay config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    let stream_api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let hidden_stream = StreamKey::new("__relay", "relay-main");
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(Some(snapshot)) = stream_api.get_stream(&hidden_stream).await {
                if !snapshot.tracks.is_empty() {
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("wait hidden relay stream timeout");

    timeout(Duration::from_secs(4), forwarded_rx)
        .await
        .expect("wait relay forwarded frame timeout")
        .expect("forwarded signal");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");

    let _ = source_server.await;
    let _ = target_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn relay_job_retries_after_remote_source_disconnect() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind relay source listener");
    let source_addr = source_listener.local_addr().expect("relay source addr");
    let source_url = format!("rtsp://{source_addr}/live/retry-source");
    let source_url_for_server = source_url.clone();

    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind relay target listener");
    let target_addr = target_listener.local_addr().expect("relay target addr");
    let target_url = format!("rtsp://{target_addr}/live/retry-target");
    let target_url_for_server = target_url.clone();

    let (second_source_session_tx, second_source_session_rx) = oneshot::channel::<()>();

    let source_server = tokio::spawn(async move {
        let (mut first_socket, _) = source_listener
            .accept()
            .await
            .expect("accept first relay source client");
        handle_pull_source_session(
            &mut first_socket,
            &source_url_for_server,
            "relay-retry-source-1",
        )
        .await;
        drop(first_socket);

        let (mut second_socket, _) = source_listener
            .accept()
            .await
            .expect("accept second relay source client");
        handle_pull_source_session(
            &mut second_socket,
            &source_url_for_server,
            "relay-retry-source-2",
        )
        .await;
        let _ = second_source_session_tx.send(());
        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(3), second_socket.read(&mut drain)).await;
    });

    let target_server = tokio::spawn(async move {
        let (mut first_socket, _) = target_listener
            .accept()
            .await
            .expect("accept first relay target client");
        handle_push_target_session(
            &mut first_socket,
            &target_url_for_server,
            "relay-retry-target-1",
        )
        .await;
        let mut first_drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(3), first_socket.read(&mut first_drain)).await;
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    relay_jobs:\n      - name: \"relay-retry\"\n        enabled: true\n        source_url: \"{source_url}\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
    );
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(&config_yaml)
        .expect("load relay config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    timeout(Duration::from_secs(5), second_source_session_rx)
        .await
        .expect("wait second source session timeout")
        .expect("second source session signal");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");

    let _ = source_server.await;
    let _ = target_server.await;
}

async fn read_rtsp_request(socket: &mut TcpStream) -> String {
    let mut buf = Vec::<u8>::new();
    loop {
        if let Some(header_end) = find_header_end(&buf) {
            let header = &buf[..header_end];
            let header_text = std::str::from_utf8(header).expect("rtsp request utf8");
            let content_length = parse_content_length(header_text);
            let total_len = header_end + 4 + content_length;
            while buf.len() < total_len {
                let mut chunk = [0u8; 1024];
                let n = timeout(Duration::from_secs(2), socket.read(&mut chunk))
                    .await
                    .expect("read request body timeout")
                    .expect("read request body");
                assert!(n > 0, "peer closed while reading request");
                buf.extend_from_slice(&chunk[..n]);
            }
            return String::from_utf8(buf[..total_len].to_vec()).expect("request utf8");
        }

        let mut chunk = [0u8; 1024];
        let n = timeout(Duration::from_secs(2), socket.read(&mut chunk))
            .await
            .expect("read request timeout")
            .expect("read request");
        assert!(n > 0, "peer closed while reading request");
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn extract_cseq(request: &str) -> Option<u32> {
    for line in request.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("cseq") {
            return value.trim().parse::<u32>().ok();
        }
    }
    None
}

async fn write_rtsp_response(
    socket: &mut TcpStream,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    let mut response = format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    socket
        .write_all(response.as_bytes())
        .await
        .expect("write response header");
    if !body.is_empty() {
        socket.write_all(body).await.expect("write response body");
    }
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(header_text: &str) -> usize {
    for line in header_text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().unwrap_or(0);
        }
    }
    0
}

async fn read_interleaved_frame(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 4];
    timeout(Duration::from_secs(2), socket.read_exact(&mut header))
        .await
        .expect("read interleaved header timeout")
        .expect("read interleaved header");
    assert_eq!(header[0], b'$', "expected interleaved frame");
    let channel = header[1];
    let payload_len = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        timeout(Duration::from_secs(2), socket.read_exact(&mut payload))
            .await
            .expect("read interleaved payload timeout")
            .expect("read interleaved payload");
    }
    (channel, payload)
}

async fn handle_pull_source_session(socket: &mut TcpStream, source_url: &str, session_id: &str) {
    let options_req = read_rtsp_request(socket).await;
    assert!(options_req.starts_with(&format!("OPTIONS {source_url} RTSP/1.0")));
    let options_cseq = extract_cseq(&options_req).expect("source options cseq");
    write_rtsp_response(
        socket,
        options_cseq,
        &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
        &[],
    )
    .await;

    let describe_req = read_rtsp_request(socket).await;
    assert!(describe_req.starts_with(&format!("DESCRIBE {source_url} RTSP/1.0")));
    let describe_cseq = extract_cseq(&describe_req).expect("source describe cseq");
    let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=relay-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
    write_rtsp_response(
        socket,
        describe_cseq,
        &[("Content-Type", "application/sdp")],
        sdp,
    )
    .await;

    let setup_req = read_rtsp_request(socket).await;
    assert!(setup_req.starts_with(&format!("SETUP {source_url}/trackID=0 RTSP/1.0")));
    let setup_cseq = extract_cseq(&setup_req).expect("source setup cseq");
    let setup_session = format!("{session_id};timeout=60");
    write_rtsp_response(
        socket,
        setup_cseq,
        &[
            ("Session", setup_session.as_str()),
            ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
        ],
        &[],
    )
    .await;

    let play_req = read_rtsp_request(socket).await;
    assert!(play_req.starts_with(&format!("PLAY {source_url} RTSP/1.0")));
    let play_cseq = extract_cseq(&play_req).expect("source play cseq");
    write_rtsp_response(socket, play_cseq, &[("Session", session_id)], &[]).await;
}

async fn handle_push_target_session(socket: &mut TcpStream, target_url: &str, session_id: &str) {
    let options_req = read_rtsp_request(socket).await;
    assert!(options_req.starts_with(&format!("OPTIONS {target_url} RTSP/1.0")));
    let options_cseq = extract_cseq(&options_req).expect("target options cseq");
    write_rtsp_response(
        socket,
        options_cseq,
        &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
        &[],
    )
    .await;

    let announce_req = read_rtsp_request(socket).await;
    assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url} RTSP/1.0")));
    assert!(announce_req.contains("m=video 0 RTP/AVP 96"));
    let announce_cseq = extract_cseq(&announce_req).expect("target announce cseq");
    let announce_session = format!("{session_id};timeout=60");
    write_rtsp_response(
        socket,
        announce_cseq,
        &[("Session", announce_session.as_str())],
        &[],
    )
    .await;

    let setup_req = read_rtsp_request(socket).await;
    assert!(setup_req.starts_with(&format!("SETUP {target_url}/trackID=0 RTSP/1.0")));
    let setup_cseq = extract_cseq(&setup_req).expect("target setup cseq");
    let setup_session = format!("{session_id};timeout=60");
    write_rtsp_response(
        socket,
        setup_cseq,
        &[
            ("Session", setup_session.as_str()),
            ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
        ],
        &[],
    )
    .await;

    let record_req = read_rtsp_request(socket).await;
    assert!(record_req.starts_with(&format!("RECORD {target_url} RTSP/1.0")));
    let record_cseq = extract_cseq(&record_req).expect("target record cseq");
    write_rtsp_response(socket, record_cseq, &[("Session", session_id)], &[]).await;
}
