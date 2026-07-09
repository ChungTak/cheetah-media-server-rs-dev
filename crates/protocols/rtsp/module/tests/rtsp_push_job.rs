use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId, TrackInfo,
};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{PublisherOptions, StreamKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};

#[tokio::test(flavor = "current_thread")]
async fn push_job_supervisor_starts_and_stops_with_module_lifecycle() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-demo\"\n        enabled: true\n        source_stream_key: \"live/push-source\"\n        target_url: \"rtsp://127.0.0.1:8554/live/push-target\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
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

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
}

#[tokio::test(flavor = "current_thread")]
async fn push_job_subscribes_source_and_sends_announce_sdp() {
    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target addr");
    let target_url = format!("rtsp://{target_addr}/live/push-target");
    let target_url_for_server = target_url.clone();
    let (announce_tx, announce_rx) = tokio::sync::oneshot::channel::<String>();
    let target_server = tokio::spawn(async move {
        let (mut socket, _) = target_listener.accept().await.expect("accept push target");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {target_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
            &[],
        )
        .await;

        let announce_req = read_rtsp_request(&mut socket).await;
        assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        assert!(announce_req.contains("Content-Type: application/sdp"));
        assert!(
            announce_req.contains("m=video 0 RTP/AVP 96"),
            "announce sdp should include video media section"
        );
        assert!(
            announce_req.contains("a=rtpmap:96 H264/90000"),
            "announce sdp should include h264 rtpmap"
        );
        let announce_cseq = extract_cseq(&announce_req).expect("announce cseq");
        write_rtsp_response(
            &mut socket,
            announce_cseq,
            &[("Session", "push-session-1;timeout=60")],
            &[],
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {target_url_for_server}/trackID=0 RTSP/1.0")));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "push-session-1;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let record_req = read_rtsp_request(&mut socket).await;
        assert!(record_req.starts_with(&format!("RECORD {target_url_for_server} RTSP/1.0")));
        assert!(record_req.contains("Session: push-session-1"));
        let record_cseq = extract_cseq(&record_req).expect("record cseq");
        write_rtsp_response(
            &mut socket,
            record_cseq,
            &[("Session", "push-session-1")],
            &[],
        )
        .await;
        let _ = announce_tx.send(announce_req);

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(2), socket.read(&mut drain)).await;
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-announce\"\n        enabled: true\n        source_stream_key: \"live/push-source\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let publisher_api = engine.publisher_api();
    let source_stream = StreamKey::new("live", "push-source");
    let (_lease, sink) = publisher_api
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    sink.update_tracks(vec![track])
        .expect("update source tracks");

    timeout(Duration::from_secs(3), announce_rx)
        .await
        .expect("wait announce timeout")
        .expect("announce request");

    sleep(Duration::from_millis(50)).await;
    sink.close().expect("close source sink");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = target_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn push_job_setup_record_then_sends_interleaved_rtp_and_rtcp() {
    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target addr");
    let target_url = format!("rtsp://{target_addr}/live/push-target");
    let target_url_for_server = target_url.clone();
    let (record_ready_tx, record_ready_rx) = tokio::sync::oneshot::channel::<()>();
    let target_server = tokio::spawn(async move {
        let (mut socket, _) = target_listener.accept().await.expect("accept push target");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {target_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
            &[],
        )
        .await;

        let announce_req = read_rtsp_request(&mut socket).await;
        assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        let announce_cseq = extract_cseq(&announce_req).expect("announce cseq");
        write_rtsp_response(
            &mut socket,
            announce_cseq,
            &[("Session", "push-session-2;timeout=60")],
            &[],
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {target_url_for_server}/trackID=0 RTSP/1.0")));
        assert!(setup_req.contains("Transport: RTP/AVP/TCP;unicast;interleaved=0-1"));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "push-session-2;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let record_req = read_rtsp_request(&mut socket).await;
        assert!(record_req.starts_with(&format!("RECORD {target_url_for_server} RTSP/1.0")));
        let record_cseq = extract_cseq(&record_req).expect("record cseq");
        write_rtsp_response(
            &mut socket,
            record_cseq,
            &[("Session", "push-session-2")],
            &[],
        )
        .await;
        let _ = record_ready_tx.send(());

        let (rtp_channel, rtp_payload) = read_interleaved_frame(&mut socket).await;
        assert_eq!(rtp_channel, 0);
        let rtp_packet = cheetah_codec::RtpPacket::parse(&rtp_payload).expect("parse push rtp");
        assert!(!rtp_packet.payload.is_empty());

        let (rtcp_channel, rtcp_payload) = read_interleaved_frame(&mut socket).await;
        assert_eq!(rtcp_channel, 1);
        assert!(
            rtcp_payload.len() >= 8 && rtcp_payload[1] == 200,
            "expected RTCP Sender Report on channel 1"
        );
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-record\"\n        enabled: true\n        source_stream_key: \"live/push-source\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let publisher_api = engine.publisher_api();
    let source_stream = StreamKey::new("live", "push-source");
    let (_lease, sink) = publisher_api
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    track.payload_type = Some(96);
    sink.update_tracks(vec![track])
        .expect("update source tracks");

    timeout(Duration::from_secs(3), record_ready_rx)
        .await
        .expect("wait push record timeout")
        .expect("record ready signal");

    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 90_000),
        bytes::Bytes::from_static(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x21]),
    );
    frame.flags = FrameFlags::KEY | FrameFlags::START_OF_AU | FrameFlags::END_OF_AU;
    let _ = sink
        .push_frame(Arc::new(frame))
        .expect("push source frame to stream");

    sink.close().expect("close source sink");
    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = target_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn push_job_rebuilds_session_when_source_tracks_change() {
    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target addr");
    let target_url = format!("rtsp://{target_addr}/live/push-target");
    let target_url_for_server = target_url.clone();
    let (first_record_tx, first_record_rx) = tokio::sync::oneshot::channel::<()>();
    let (second_record_tx, second_record_rx) = tokio::sync::oneshot::channel::<()>();
    let target_server = tokio::spawn(async move {
        let (mut first_socket, _) = target_listener.accept().await.expect("accept first push");
        handle_push_session(
            &mut first_socket,
            &target_url_for_server,
            "H264/90000",
            "push-session-rebuild-1",
        )
        .await;
        let _ = first_record_tx.send(());

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(3), first_socket.read(&mut drain)).await;

        let (mut second_socket, _) = target_listener.accept().await.expect("accept second push");
        handle_push_session(
            &mut second_socket,
            &target_url_for_server,
            "H265/90000",
            "push-session-rebuild-2",
        )
        .await;
        let _ = second_record_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-rebuild\"\n        enabled: true\n        source_stream_key: \"live/push-source\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let publisher_api = engine.publisher_api();
    let source_stream = StreamKey::new("live", "push-source");
    let (_lease, sink) = publisher_api
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    let mut h264_track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    h264_track.payload_type = Some(96);
    sink.update_tracks(vec![h264_track])
        .expect("update initial source tracks");

    timeout(Duration::from_secs(3), first_record_rx)
        .await
        .expect("wait first record timeout")
        .expect("first record signal");

    let mut h265_track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H265, 90_000);
    h265_track.payload_type = Some(96);
    sink.update_tracks(vec![h265_track])
        .expect("update changed source tracks");

    timeout(Duration::from_secs(5), second_record_rx)
        .await
        .expect("wait second record timeout")
        .expect("second record signal");

    sink.close().expect("close source sink");
    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = target_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn push_job_sends_keepalive_from_session_timeout() {
    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target addr");
    let target_url = format!("rtsp://{target_addr}/live/push-keepalive");
    let target_url_for_server = target_url.clone();
    let (keepalive_tx, keepalive_rx) = tokio::sync::oneshot::channel::<()>();
    let target_server = tokio::spawn(async move {
        let (mut socket, _) = target_listener.accept().await.expect("accept push target");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {target_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD,GET_PARAMETER")],
            &[],
        )
        .await;

        let announce_req = read_rtsp_request(&mut socket).await;
        assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        let announce_cseq = extract_cseq(&announce_req).expect("announce cseq");
        write_rtsp_response(
            &mut socket,
            announce_cseq,
            &[("Session", "push-keepalive-session;timeout=2")],
            &[],
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {target_url_for_server}/trackID=0 RTSP/1.0")));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "push-keepalive-session;timeout=2"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let record_req = read_rtsp_request(&mut socket).await;
        assert!(record_req.starts_with(&format!("RECORD {target_url_for_server} RTSP/1.0")));
        assert!(record_req.contains("Session: push-keepalive-session"));
        let record_cseq = extract_cseq(&record_req).expect("record cseq");
        write_rtsp_response(
            &mut socket,
            record_cseq,
            &[("Session", "push-keepalive-session")],
            &[],
        )
        .await;

        let keepalive_req = read_rtsp_request(&mut socket).await;
        assert!(
            keepalive_req.starts_with(&format!("GET_PARAMETER {target_url_for_server} RTSP/1.0"))
        );
        assert!(keepalive_req.contains("Session: push-keepalive-session"));
        let keepalive_cseq = extract_cseq(&keepalive_req).expect("keepalive cseq");
        write_rtsp_response(
            &mut socket,
            keepalive_cseq,
            &[("Session", "push-keepalive-session")],
            &[],
        )
        .await;
        let _ = keepalive_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-keepalive\"\n        enabled: true\n        source_stream_key: \"live/push-keepalive\"\n        target_url: \"{target_url}\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let publisher_api = engine.publisher_api();
    let source_stream = StreamKey::new("live", "push-keepalive");
    let (_lease, sink) = publisher_api
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    sink.update_tracks(vec![track])
        .expect("update source tracks");

    timeout(Duration::from_secs(4), keepalive_rx)
        .await
        .expect("wait push keepalive timeout")
        .expect("keepalive signal");

    sink.close().expect("close source sink");
    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = target_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn push_job_uses_configured_credentials_after_digest_challenge() {
    let target_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind target listener");
    let target_addr = target_listener.local_addr().expect("target addr");
    let target_url = format!("rtsp://{target_addr}/live/push-auth");
    let target_url_for_server = target_url.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let target_server = tokio::spawn(async move {
        let (mut socket, _) = target_listener.accept().await.expect("accept push target");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {target_url_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
            &[],
        )
        .await;

        let announce_req_1 = read_rtsp_request(&mut socket).await;
        assert!(announce_req_1.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        assert!(
            !announce_req_1.contains("Authorization: "),
            "first ANNOUNCE should not carry Authorization"
        );
        let announce_cseq_1 = extract_cseq(&announce_req_1).expect("announce first cseq");
        assert_eq!(announce_cseq_1, options_cseq + 1);
        write_rtsp_status_response(
            &mut socket,
            401,
            "Unauthorized",
            announce_cseq_1,
            &[(
                "WWW-Authenticate",
                r#"Digest realm="push-auth", nonce="nonce-1", qop="auth", algorithm=MD5"#,
            )],
            &[],
        )
        .await;

        let announce_req_2 = read_rtsp_request(&mut socket).await;
        assert!(announce_req_2.starts_with(&format!("ANNOUNCE {target_url_for_server} RTSP/1.0")));
        assert!(announce_req_2.contains("Authorization: Digest "));
        assert!(announce_req_2.contains("qop=auth"));
        assert!(announce_req_2.contains("nc=00000001"));
        let announce_cseq_2 = extract_cseq(&announce_req_2).expect("announce retry cseq");
        assert_eq!(announce_cseq_2, announce_cseq_1 + 1);
        write_rtsp_response(
            &mut socket,
            announce_cseq_2,
            &[("Session", "push-auth-session;timeout=60")],
            &[],
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {target_url_for_server}/trackID=0 RTSP/1.0")));
        assert!(setup_req.contains("Authorization: Digest "));
        assert!(setup_req.contains("qop=auth"));
        assert!(setup_req.contains("nc=00000002"));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        assert_eq!(setup_cseq, announce_cseq_2 + 1);
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "push-auth-session;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let record_req = read_rtsp_request(&mut socket).await;
        assert!(record_req.starts_with(&format!("RECORD {target_url_for_server} RTSP/1.0")));
        assert!(record_req.contains("Authorization: Digest "));
        assert!(record_req.contains("qop=auth"));
        assert!(record_req.contains("nc=00000003"));
        let record_cseq = extract_cseq(&record_req).expect("record cseq");
        assert_eq!(record_cseq, setup_cseq + 1);
        write_rtsp_response(
            &mut socket,
            record_cseq,
            &[("Session", "push-auth-session")],
            &[],
        )
        .await;
        let _ = ready_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    push_jobs:\n      - name: \"push-auth\"\n        enabled: true\n        source_stream_key: \"live/push-auth\"\n        target_url: \"{target_url}\"\n        username: \"user\"\n        password: \"pass\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let publisher_api = engine.publisher_api();
    let source_stream = StreamKey::new("live", "push-auth");
    let (_lease, sink) = publisher_api
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
    sink.update_tracks(vec![track])
        .expect("update source tracks");

    timeout(Duration::from_secs(3), ready_rx)
        .await
        .expect("wait push auth ready timeout")
        .expect("push auth ready signal");
    sink.close().expect("close source sink");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = target_server.await;
}

async fn read_rtsp_request(socket: &mut tokio::net::TcpStream) -> String {
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
    socket: &mut tokio::net::TcpStream,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    write_rtsp_status_response(socket, 200, "OK", cseq, headers, body).await;
}

async fn write_rtsp_status_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    cseq: u32,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    let mut response = format!("RTSP/1.0 {status} {reason}\r\nCSeq: {cseq}\r\n");
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

async fn read_interleaved_frame(socket: &mut tokio::net::TcpStream) -> (u8, Vec<u8>) {
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

async fn handle_push_session(
    socket: &mut tokio::net::TcpStream,
    target_url: &str,
    expected_video_rtpmap: &str,
    session_id: &str,
) {
    let options_req = read_rtsp_request(socket).await;
    assert!(options_req.starts_with(&format!("OPTIONS {target_url} RTSP/1.0")));
    let options_cseq = extract_cseq(&options_req).expect("options cseq");
    write_rtsp_response(
        socket,
        options_cseq,
        &[("Public", "OPTIONS,ANNOUNCE,SETUP,RECORD")],
        &[],
    )
    .await;

    let announce_req = read_rtsp_request(socket).await;
    assert!(announce_req.starts_with(&format!("ANNOUNCE {target_url} RTSP/1.0")));
    assert!(
        announce_req.contains(expected_video_rtpmap),
        "announce SDP should contain expected rtpmap {expected_video_rtpmap}"
    );
    let announce_cseq = extract_cseq(&announce_req).expect("announce cseq");
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
    let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
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
    assert!(record_req.contains(&format!("Session: {session_id}")));
    let record_cseq = extract_cseq(&record_req).expect("record cseq");
    write_rtsp_response(socket, record_cseq, &[("Session", session_id)], &[]).await;
}
