use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtmp_core::{RtmpClientState, RtmpEvent, RtmpMediaType, RtmpUrl};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
};
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{
    CancellationToken, PublisherOptions, StreamKey, StreamManagerApi, SubscriberOptions,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::sleep;
use tokio::time::timeout;

mod common;
use common::*;

#[tokio::test(flavor = "current_thread")]
async fn pull_job_supervisor_starts_and_stops_with_module_lifecycle() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-demo\"\n        enabled: true\n        source_url: \"rtsp://127.0.0.1:8554/live/demo\"\n        target_stream_key: \"live/pull-demo\"\n        transport_preference:\n          - tcp_interleaved\n          - udp\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
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
async fn pull_job_describe_parses_tracks_and_acquires_publisher_lease() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        assert!(describe_req.contains("Accept: application/sdp"));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\nm=audio 0 RTP/AVP 97\r\na=rtpmap:97 MPEG4-GENERIC/48000/2\r\na=fmtp:97 profile-level-id=1;mode=AAC-hbr;config=1190\r\na=control:trackID=1\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;
        let setup_video_req = read_rtsp_request(&mut socket).await;
        assert!(setup_video_req
            .starts_with(&format!("SETUP {source_uri_for_server}/trackID=0 RTSP/1.0")));
        let setup_video_cseq = extract_cseq(&setup_video_req).expect("setup video cseq");
        write_rtsp_response(
            &mut socket,
            setup_video_cseq,
            &[
                ("Session", "pull-session-describe;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let setup_audio_req = read_rtsp_request(&mut socket).await;
        assert!(setup_audio_req
            .starts_with(&format!("SETUP {source_uri_for_server}/trackID=1 RTSP/1.0")));
        let setup_audio_cseq = extract_cseq(&setup_audio_req).expect("setup audio cseq");
        write_rtsp_response(
            &mut socket,
            setup_audio_cseq,
            &[
                ("Session", "pull-session-describe;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.starts_with(&format!("PLAY {source_uri_for_server} RTSP/1.0")));
        assert!(play_req.contains("Session: pull-session-describe"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-session-describe")],
            &[],
        )
        .await;

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(5), socket.read(&mut drain)).await;
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-describe\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-describe\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
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
    let stream_api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let target_stream = StreamKey::new("live", "pull-describe");
    let snapshot = timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(Some(snapshot)) = stream_api.get_stream(&target_stream).await {
                if snapshot.tracks.len() == 2 {
                    break snapshot;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("wait pull snapshot timeout");
    assert_eq!(
        snapshot.tracks.len(),
        2,
        "describe tracks should be published"
    );

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_tcp_interleaved_rtp_ingest_reuses_publish_pipeline() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        assert!(describe_req.contains("Accept: application/sdp"));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {source_uri_for_server}/trackID=0 RTSP/1.0")));
        assert!(setup_req.contains("Transport: RTP/AVP/TCP;unicast;interleaved=0-1"));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "pull-session-1;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.starts_with(&format!("PLAY {source_uri_for_server} RTSP/1.0")));
        assert!(play_req.contains("Session: pull-session-1"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-session-1")],
            &[],
        )
        .await;

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
            .expect("write interleaved rtp");

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(5), socket.read(&mut drain)).await;
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-ingest\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-ingest\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
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
    let stream_api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();
    let subscriber_api = engine.subscriber_api();

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let target_stream = StreamKey::new("live", "pull-ingest");
    timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(Some(snapshot)) = stream_api.get_stream(&target_stream).await {
                if snapshot.tracks.len() == 1 {
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("wait pull ingest stream timeout");

    let mut subscriber = subscriber_api
        .subscribe(target_stream.clone(), SubscriberOptions::default())
        .await
        .expect("subscribe pull target");
    let frame = timeout(Duration::from_secs(2), subscriber.recv())
        .await
        .expect("recv pull frame timeout")
        .expect("recv frame result")
        .expect("frame should exist");
    assert_eq!(frame.track_id.0, 1);
    assert!(
        !frame.payload.is_empty(),
        "ingested frame should not be empty"
    );

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_sends_keepalive_from_session_timeout() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();
    let (keepalive_tx, keepalive_rx) = tokio::sync::oneshot::channel::<()>();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY,GET_PARAMETER")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "pull-keepalive;timeout=2"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.contains("Session: pull-keepalive"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-keepalive")],
            &[],
        )
        .await;

        let keepalive_req = read_rtsp_request(&mut socket).await;
        assert!(
            keepalive_req.starts_with(&format!("GET_PARAMETER {source_uri_for_server} RTSP/1.0"))
        );
        assert!(keepalive_req.contains("Session: pull-keepalive"));
        let keepalive_cseq = extract_cseq(&keepalive_req).expect("keepalive cseq");
        write_rtsp_response(
            &mut socket,
            keepalive_cseq,
            &[("Session", "pull-keepalive")],
            &[],
        )
        .await;
        let _ = keepalive_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-keepalive\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-keepalive\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
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

    timeout(Duration::from_secs(3), keepalive_rx)
        .await
        .expect("wait keepalive timeout")
        .expect("keepalive signal");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_retries_keepalive_with_auth_on_401() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-keepalive-auth");
    let source_uri_for_server = source_uri.clone();
    let (keepalive_tx, keepalive_rx) = tokio::sync::oneshot::channel::<()>();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY,GET_PARAMETER")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "pull-keepalive-auth;timeout=2"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.contains("Session: pull-keepalive-auth"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-keepalive-auth")],
            &[],
        )
        .await;

        let keepalive_req_1 = read_rtsp_request(&mut socket).await;
        assert!(
            keepalive_req_1.starts_with(&format!("GET_PARAMETER {source_uri_for_server} RTSP/1.0"))
        );
        assert!(keepalive_req_1.contains("Session: pull-keepalive-auth"));
        assert!(
            !keepalive_req_1.contains("Authorization: "),
            "first keepalive should not include authorization before challenge"
        );
        let keepalive_cseq_1 = extract_cseq(&keepalive_req_1).expect("keepalive cseq");
        write_rtsp_status_response(
            &mut socket,
            401,
            "Unauthorized",
            keepalive_cseq_1,
            &[("WWW-Authenticate", r#"Basic realm="pull-keepalive-auth""#)],
            &[],
        )
        .await;

        let keepalive_req_2 = read_rtsp_request(&mut socket).await;
        assert!(
            keepalive_req_2.starts_with(&format!("GET_PARAMETER {source_uri_for_server} RTSP/1.0"))
        );
        assert!(keepalive_req_2.contains("Session: pull-keepalive-auth"));
        assert!(keepalive_req_2.contains("Authorization: Basic dXNlcjpwYXNz"));
        let keepalive_cseq_2 = extract_cseq(&keepalive_req_2).expect("keepalive retry cseq");
        assert_eq!(keepalive_cseq_2, keepalive_cseq_1 + 1);
        write_rtsp_response(
            &mut socket,
            keepalive_cseq_2,
            &[("Session", "pull-keepalive-auth")],
            &[],
        )
        .await;
        let _ = keepalive_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-keepalive-auth\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-keepalive-auth\"\n        username: \"user\"\n        password: \"pass\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
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

    timeout(Duration::from_secs(4), keepalive_rx)
        .await
        .expect("wait keepalive auth timeout")
        .expect("keepalive auth signal");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_uses_configured_credentials_after_401_challenge() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-auth");
    let source_uri_for_server = source_uri.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req_1 = read_rtsp_request(&mut socket).await;
        assert!(options_req_1.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        assert!(
            !options_req_1.contains("Authorization: "),
            "initial OPTIONS should not carry Authorization"
        );
        let options_cseq_1 = extract_cseq(&options_req_1).expect("options cseq");
        write_rtsp_status_response(
            &mut socket,
            401,
            "Unauthorized",
            options_cseq_1,
            &[("WWW-Authenticate", r#"Basic realm="pull-auth""#)],
            &[],
        )
        .await;

        let options_req_2 = read_rtsp_request(&mut socket).await;
        assert!(options_req_2.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        assert!(options_req_2.contains("Authorization: Basic dXNlcjpwYXNz"));
        let options_cseq_2 = extract_cseq(&options_req_2).expect("options retry cseq");
        assert_eq!(options_cseq_2, options_cseq_1 + 1);
        write_rtsp_response(
            &mut socket,
            options_cseq_2,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        assert!(describe_req.contains("Authorization: Basic dXNlcjpwYXNz"));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        assert_eq!(describe_cseq, options_cseq_2 + 1);
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-auth\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {source_uri_for_server}/trackID=0 RTSP/1.0")));
        assert!(setup_req.contains("Authorization: Basic dXNlcjpwYXNz"));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        assert_eq!(setup_cseq, describe_cseq + 1);
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "pull-auth-session;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.starts_with(&format!("PLAY {source_uri_for_server} RTSP/1.0")));
        assert!(play_req.contains("Authorization: Basic dXNlcjpwYXNz"));
        assert!(play_req.contains("Session: pull-auth-session"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        assert_eq!(play_cseq, setup_cseq + 1);
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-auth-session")],
            &[],
        )
        .await;
        let _ = ready_tx.send(());
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);
    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-auth\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-auth\"\n        username: \"user\"\n        password: \"pass\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 200\n"
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

    timeout(Duration::from_secs(3), ready_rx)
        .await
        .expect("wait pull auth ready timeout")
        .expect("pull auth ready signal");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_target_occupied_stops_without_retry() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();
    let (allow_describe_tx, allow_describe_rx) = tokio::sync::oneshot::channel::<()>();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener
            .accept()
            .await
            .expect("accept first pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        let _ = allow_describe_rx.await;
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let second_accept = timeout(Duration::from_millis(600), source_listener.accept()).await;
        assert!(second_accept.is_err(), "unexpected second retry connection");
    });

    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let listen = probe.local_addr().expect("probe addr");
    drop(probe);

    let config_yaml = format!(
        "modules:\n  rtsp:\n    listen: \"{listen}\"\n    pull_jobs:\n      - name: \"pull-conflict\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-conflict\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 100\n"
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
    let occupy_key = StreamKey::new("live", "pull-conflict");
    let (_lease, sink) = publisher_api
        .acquire_publisher(occupy_key, PublisherOptions::default())
        .await
        .expect("acquire occupy publisher");
    let _ = allow_describe_tx.send(());
    sleep(Duration::from_millis(700)).await;

    sink.close().expect("close occupy sink");

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
}

#[tokio::test(flavor = "current_thread")]
async fn pull_job_remote_rtsp_source_restreams_to_local_rtsp_and_rtmp_play() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();
    let source_server = tokio::spawn(async move {
        let (mut socket, _) = source_listener.accept().await.expect("accept pull source");
        let options_req = read_rtsp_request(&mut socket).await;
        assert!(options_req.starts_with(&format!("OPTIONS {source_uri_for_server} RTSP/1.0")));
        let options_cseq = extract_cseq(&options_req).expect("options cseq");
        write_rtsp_response(
            &mut socket,
            options_cseq,
            &[("Public", "OPTIONS,DESCRIBE,SETUP,PLAY,GET_PARAMETER")],
            &[],
        )
        .await;

        let describe_req = read_rtsp_request(&mut socket).await;
        assert!(describe_req.starts_with(&format!("DESCRIBE {source_uri_for_server} RTSP/1.0")));
        let describe_cseq = extract_cseq(&describe_req).expect("describe cseq");
        let sdp = b"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=pull-source\r\nt=0 0\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\n";
        write_rtsp_response(
            &mut socket,
            describe_cseq,
            &[("Content-Type", "application/sdp")],
            sdp,
        )
        .await;

        let setup_req = read_rtsp_request(&mut socket).await;
        assert!(setup_req.starts_with(&format!("SETUP {source_uri_for_server}/trackID=0 RTSP/1.0")));
        let setup_cseq = extract_cseq(&setup_req).expect("setup cseq");
        write_rtsp_response(
            &mut socket,
            setup_cseq,
            &[
                ("Session", "pull-restream-session;timeout=60"),
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
            ],
            &[],
        )
        .await;

        let play_req = read_rtsp_request(&mut socket).await;
        assert!(play_req.starts_with(&format!("PLAY {source_uri_for_server} RTSP/1.0")));
        assert!(play_req.contains("Session: pull-restream-session"));
        let play_cseq = extract_cseq(&play_req).expect("play cseq");
        write_rtsp_response(
            &mut socket,
            play_cseq,
            &[("Session", "pull-restream-session")],
            &[],
        )
        .await;

        for idx in 0..24u16 {
            let rtp = build_publish_h264_rtp(
                12_000u16.wrapping_add(idx),
                900_000u32.wrapping_add(u32::from(idx) * 9_000u32),
                0x4455_6677,
            );
            send_interleaved_frame(&mut socket, 0, &rtp).await;
            sleep(Duration::from_millis(25)).await;
        }

        let mut drain = [0u8; 1];
        let _ = timeout(Duration::from_secs(2), socket.read(&mut drain)).await;
    });

    let rtsp_probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe rtsp listen");
    let rtsp_listen = rtsp_probe.local_addr().expect("probe rtsp addr");
    drop(rtsp_probe);
    let rtmp_probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe rtmp listen");
    let rtmp_listen = rtmp_probe.local_addr().expect("probe rtmp addr");
    drop(rtmp_probe);

    let config_yaml = format!(
        "modules:\n  rtmp:\n    listen: \"{rtmp_listen}\"\n  rtsp:\n    listen: \"{rtsp_listen}\"\n    pull_jobs:\n      - name: \"pull-restream\"\n        enabled: true\n        source_url: \"{source_uri}\"\n        target_stream_key: \"live/pull-restream\"\n        transport_preference:\n          - tcp_interleaved\n        retry_backoff_ms: 50\n        max_retry_backoff_ms: 500\n"
    );
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(&config_yaml).expect("load config");

    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(RtspModuleFactory))
        .build()
        .expect("build engine");
    let stream_api: Arc<dyn StreamManagerApi> = engine.stream_manager_api();

    timeout(Duration::from_secs(2), engine.start())
        .await
        .expect("engine start timeout")
        .expect("start engine");

    let target_stream = StreamKey::new("live", "pull-restream");
    timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(Some(snapshot)) = stream_api.get_stream(&target_stream).await {
                if snapshot.tracks.len() == 1 {
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("wait pull restream stream timeout");

    let rtsp_uri = format!("rtsp://{rtsp_listen}/live/pull-restream");
    let mut rtsp_player = connect_with_retry(rtsp_listen).await;
    let describe = build_request("DESCRIBE", &rtsp_uri, 1, None, &[], &[]);
    write_request(&mut rtsp_player, &describe).await;
    let describe_resp = read_response(&mut rtsp_player, "PULL-RESTREAM-RTSP-DESCRIBE").await;
    assert_eq!(describe_resp.status_code, 200);
    let rtsp_session = describe_resp
        .header("Session")
        .expect("rtsp player session")
        .to_string();

    let setup = build_request(
        "SETUP",
        &format!("{rtsp_uri}/trackID=0"),
        2,
        Some(&rtsp_session),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
        &[],
    );
    write_request(&mut rtsp_player, &setup).await;
    let setup_resp = read_response(&mut rtsp_player, "PULL-RESTREAM-RTSP-SETUP").await;
    assert_eq!(setup_resp.status_code, 200);

    let play = build_request("PLAY", &rtsp_uri, 3, Some(&rtsp_session), &[], &[]);
    write_request(&mut rtsp_player, &play).await;
    let play_resp = read_response(&mut rtsp_player, "PULL-RESTREAM-RTSP-PLAY").await;
    assert_eq!(play_resp.status_code, 200);

    let mut rtsp_rtp = None;
    for _ in 0..24 {
        let (channel, payload) =
            read_interleaved_frame(&mut rtsp_player, "PULL-RESTREAM-RTSP-RTP").await;
        if channel != 2 {
            continue;
        }
        if let Some(packet) = cheetah_codec::RtpPacket::parse(&payload) {
            rtsp_rtp = Some(packet);
            break;
        }
    }
    let rtsp_rtp = rtsp_rtp.expect("receive local rtsp restream rtp");
    assert!(
        !rtsp_rtp.payload.is_empty(),
        "rtsp restream rtp payload must not be empty"
    );

    let rtmp_url = RtmpUrl::parse(&format!("rtmp://{rtmp_listen}/live/pull-restream"))
        .expect("parse rtmp url");
    let mut rtmp_player = start_client(
        runtime.clone(),
        rtmp_url,
        RtmpClientMode::Play,
        RtmpClientDriverConfig::default(),
        CancellationToken::new(),
    )
    .expect("start rtmp play client");
    wait_for_rtmp_client_state(
        &mut rtmp_player,
        RtmpClientState::Playing,
        "pull-restream rtmp playing",
    )
    .await;
    recv_h264_video_event(&mut rtmp_player, "pull-restream rtmp media").await;

    rtmp_player.shutdown();
    let _ = rtmp_player.wait().await;

    let teardown = build_request("TEARDOWN", &rtsp_uri, 4, Some(&rtsp_session), &[], &[]);
    write_request(&mut rtsp_player, &teardown).await;
    let teardown_resp = read_response(&mut rtsp_player, "PULL-RESTREAM-RTSP-TEARDOWN").await;
    assert_eq!(teardown_resp.status_code, 200);

    timeout(Duration::from_secs(5), engine.stop())
        .await
        .expect("engine stop timeout");
    let _ = source_server.await;
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

async fn wait_for_rtmp_client_state(
    client: &mut RtmpClientHandle,
    expected: RtmpClientState,
    stage: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting rtmp client state {expected:?} at {stage}"
        );
        let remain = deadline.saturating_duration_since(now);
        let event = timeout(remain, client.recv_event())
            .await
            .expect("wait rtmp state timeout")
            .expect("rtmp event stream closed");
        if let ClientDriverEvent::Core {
            event: RtmpEvent::ClientStateChanged { state },
        } = event
        {
            if state == expected {
                return;
            }
        }
    }
}

async fn recv_h264_video_event(client: &mut RtmpClientHandle, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let now = Instant::now();
        assert!(
            now < deadline,
            "timeout waiting h264 media event at {stage}"
        );
        let remain = deadline.saturating_duration_since(now);
        let event = timeout(remain, client.recv_event())
            .await
            .expect("wait rtmp media timeout")
            .expect("rtmp event stream closed");
        if let ClientDriverEvent::Core {
            event:
                RtmpEvent::MediaData {
                    media_type: RtmpMediaType::Video,
                    payload,
                    ..
                },
        } = event
        {
            if payload.len() > 1 && payload[1] == 0x01 {
                return;
            }
        }
    }
}
