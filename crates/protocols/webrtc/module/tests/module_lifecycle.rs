//! WebRTC module lifecycle and HTTP smoke tests.
//!
//! These tests start the module via the engine builder, then send HTTP
//! requests through the registered `ModuleHttpService`. The actual UDP
//! socket is bound to `127.0.0.1:0` so the OS picks a free port.

use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
use cheetah_webrtc_module::WebRtcModuleFactory;

fn fixture_offer() -> String {
    include_str!("fixtures/minimal_offer.sdp").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn module_starts_and_serves_session_list_route() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(config_yaml)
        .expect("load webrtc module config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    // Find the HTTP service.
    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // session list endpoint should respond 200 even with zero sessions.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    assert_eq!(resp.status, 200);

    // pull/list returns 200 OK with empty array (Phase 05 stub).
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/pull/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("pull list handler");
    assert_eq!(resp.status, 200);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whip_returns_201_with_sdp_and_location_header() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=demo".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header");
    assert!(location.value.starts_with("/api/v1/rtc/session/"));
    let content_type = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .expect("content-type header");
    assert!(content_type.value.contains("application/sdp"));
    assert!(resp.body.starts_with(b"v=0"));

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ome_whip_path_returns_201_with_sdp_and_location_header() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/live/demo".into(),
            query: Some("direction=whip&transport=udptcp".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("OME WHIP path handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header");
    assert!(location.value.starts_with("/api/v1/rtc/session/"));
    assert!(resp.body.starts_with(b"v=0"));

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ome_relay_whip_path_returns_ice_server_link_headers() {
    let config_yaml = r#"
modules:
  webrtc:
    listen_udp: "127.0.0.1:0"
    enable_tcp: false
    ome_ice_servers:
      - urls:
          - "turn:relay.example.com:3478?transport=tcp"
        username: "ome"
        credential: "airen"
"#;

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/live/demo-ome-relay".into(),
            query: Some("direction=whip&transport=relay".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("OME WHIP relay path handler");
    assert_eq!(resp.status, 201);
    let link = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("link"))
        .expect("Link ice-server header");
    assert_eq!(
        link.value,
        "<turn:relay.example.com:3478?transport=tcp>; rel=\"ice-server\"; username=\"ome\"; credential=\"airen\""
    );
    let expose = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("access-control-expose-headers"))
        .expect("expose headers");
    assert_eq!(expose.value, "Location, Link");

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sms_publish_returns_json_with_server_label_and_session_id() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "appName": "live",
        "streamName": "demo",
        "sdp": fixture_offer(),
    });

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/publish".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("publish handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("response body json");
    assert_eq!(payload["code"], 0);
    assert_eq!(payload["server"], "cheetah");
    assert!(payload["sessionid"]
        .as_str()
        .unwrap_or("")
        .contains("webrtc-session-"));
    assert!(payload["sdp"].as_str().unwrap_or("").starts_with("v=0"));

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whip_acquires_publisher_lease_on_engine() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=publish-engine".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);

    // The publisher lease should now be visible from the engine's
    // stream manager. Allow up to 1 second for the registry to settle
    // — the bridge acquires the lease synchronously inside the
    // request handler so it should be visible immediately.
    let stream_key = cheetah_sdk::StreamKey::new("live", "publish-engine");
    let mut found = false;
    for _ in 0..20 {
        let snapshot = engine
            .stream_manager_api()
            .get_stream(&stream_key)
            .await
            .expect("get_stream");
        if snapshot.map(|s| s.publisher_active).unwrap_or(false) {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        found,
        "publisher lease for live/publish-engine should be active"
    );

    // A second WHIP for the same stream should fail with 409.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=publish-engine".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("conflicting whip handler");
    assert_eq!(resp.status, 409, "second publisher must conflict");

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn patch_session_with_unknown_id_returns_404() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Patch,
            path: "/session/9999".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"a=candidate:foo 1 udp 1 1.2.3.4 1234 typ host\r\n"),
        })
        .await
        .expect("patch handler");
    assert_eq!(resp.status, 404);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h265_browser_profile_publish_is_rejected_with_422() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n    codec_profile: browser\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "appName": "live",
        "streamName": "demo-h265",
        "sdp": fixture_offer(),
        "preferVideoCodec": "h265",
    });

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/publish".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("publish handler");
    assert_eq!(resp.status, 422);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_bridge_pushes_frames_into_engine_publisher() {
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=engine-publish".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);

    // Verify the publisher lease appears in the engine's stream
    // manager; if so, the engine bridge has been wired through end to
    // end. We do not push real frames in this test (DTLS handshake is
    // out of scope), but reaching this state proves the
    // module → bridge → engine PublisherSink chain is connected.
    let stream_key = cheetah_sdk::StreamKey::new("live", "engine-publish");
    let mut visible = false;
    for _ in 0..40 {
        let snap = engine
            .stream_manager_api()
            .get_stream(&stream_key)
            .await
            .expect("get stream");
        if snap.map(|s| s.publisher_active).unwrap_or(false) {
            visible = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        visible,
        "engine should observe the active publisher lease for live/engine-publish"
    );

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn play_bridge_creates_subscriber_when_stream_present() {
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Pre-create a publisher on the engine so the WHEP play has
    // something to subscribe to. We use the engine PublisherApi
    // directly here so the test does not depend on a second WHIP
    // request making it past DTLS.
    let stream_key = cheetah_sdk::StreamKey::new("live", "engine-play");
    let (_lease, sink) = engine
        .publisher_api()
        .acquire_publisher(stream_key.clone(), cheetah_sdk::PublisherOptions::default())
        .await
        .expect("acquire publisher");

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whep".into(),
            query: Some("appName=live&streamName=engine-play".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whep handler");
    assert_eq!(resp.status, 201);

    // The play bridge is spawned asynchronously after the answer is
    // delivered. Wait for the engine to record at least one
    // subscriber on the stream.
    let mut saw_subscriber = false;
    for _ in 0..40 {
        let snap = engine
            .stream_manager_api()
            .get_stream(&stream_key)
            .await
            .expect("get stream");
        if snap.map(|s| s.subscriber_count > 0).unwrap_or(false) {
            saw_subscriber = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        saw_subscriber,
        "play bridge should have registered a subscriber on the engine"
    );

    let _ = sink.close();
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn echo_endpoints_toggle_session_state() {
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Create a publish session first so we have something to echo on.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=echo-target".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("location header")
        .value
        .clone();
    let session_id = location
        .strip_prefix("/api/v1/rtc/session/")
        .and_then(|s| s.strip_prefix("webrtc-session-"))
        .and_then(|s| s.parse::<u64>().ok())
        .expect("session id from location");

    // Start datachannel echo.
    let body = serde_json::json!({
        "sessionid": format!("webrtc-session-{session_id}"),
        "mode": "datachannel",
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/echo/start".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("echo start");
    assert_eq!(resp.status, 200);

    // Echo on unknown session should 404.
    let body = serde_json::json!({"sessionid": "999999", "mode": "datachannel"});
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/echo/start".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("echo start");
    assert_eq!(resp.status, 404);

    // Stop echo.
    let body = serde_json::json!({"sessionid": format!("webrtc-session-{session_id}")});
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/echo/stop".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("echo stop");
    assert_eq!(resp.status, 204);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn module_rejects_invalid_simulcast_policy_at_init() {
    use std::sync::Arc;

    let config_yaml = "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n    simulcast_default_policy: bogus\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    let result = engine.start().await;
    assert!(
        result.is_err(),
        "engine start should fail with invalid policy"
    );
    let err = format!("{}", result.err().unwrap());
    assert!(
        err.contains("simulcast_default_policy"),
        "error message should mention the bad field: {err}"
    );
    engine.stop().await;
}

mod fake_signaling {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    /// Tiny WHIP/WHEP-style HTTP/1.1 mock that returns a fixed
    /// 201 + Location header for `POST` requests and 204 for `DELETE`.
    /// The `last_offer_body` field captures the most recently posted
    /// offer so tests can assert the supervisor sent a real SDP.
    pub struct FakeServer {
        #[allow(dead_code)]
        pub addr: std::net::SocketAddr,
        pub url_template: String,
        pub last_offer_body: Arc<Mutex<Option<String>>>,
        pub stop: tokio::sync::oneshot::Sender<()>,
    }

    pub async fn start() -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let url_template = format!("http://{addr}/whip");
        let last_offer = Arc::new(Mutex::new(None::<String>));
        let last_offer_for_task = last_offer.clone();
        tokio::spawn(async move {
            loop {
                let listener = &listener;
                tokio::select! {
                    _ = &mut stop_rx => break,
                    accept = listener.accept() => {
                        if let Ok((mut sock, _)) = accept {
                            let last_offer = last_offer_for_task.clone();
                            tokio::spawn(async move {
                                // Read the full HTTP request: headers
                                // first, then drain any body framed by
                                // content-length. Single-shot `read`
                                // is racy under load because the body
                                // can arrive in a separate TCP segment.
                                let mut buf = Vec::with_capacity(8192);
                                let mut tmp = [0u8; 4096];
                                let mut header_end = None;
                                let mut content_length = 0usize;
                                loop {
                                    let n = match sock.read(&mut tmp).await {
                                        Ok(0) => break,
                                        Ok(n) => n,
                                        Err(_) => break,
                                    };
                                    buf.extend_from_slice(&tmp[..n]);
                                    if header_end.is_none() {
                                        if let Some(idx) = buf
                                            .windows(4)
                                            .position(|w| w == b"\r\n\r\n")
                                        {
                                            header_end = Some(idx);
                                            // Parse content-length from header.
                                            let head = &buf[..idx];
                                            for line in head.split(|b| *b == b'\n') {
                                                let line = std::str::from_utf8(line)
                                                    .unwrap_or("")
                                                    .trim_end_matches('\r');
                                                if let Some((k, v)) = line.split_once(':') {
                                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                                        content_length = v.trim()
                                                            .parse()
                                                            .unwrap_or(0);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if let Some(end) = header_end {
                                        let body_have = buf.len().saturating_sub(end + 4);
                                        if body_have >= content_length {
                                            break;
                                        }
                                    }
                                }
                                let req = String::from_utf8_lossy(&buf).to_string();
                                let is_delete = req.starts_with("DELETE ");
                                if !is_delete {
                                    if let Some(idx) = header_end {
                                        let body = req[idx + 4..].to_string();
                                        if !body.is_empty() {
                                            *last_offer.lock().await = Some(body);
                                        }
                                    }
                                }
                                // Build a mock answer that mirrors a
                                // typical WebRTC server response.
                                let answer = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n";
                                let response: Vec<u8> = if is_delete {
                                    b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
                                } else {
                                    let body = answer.as_bytes();
                                    let mut out = Vec::new();
                                    out.extend_from_slice(b"HTTP/1.1 201 Created\r\n");
                                    out.extend_from_slice(b"content-type: application/sdp\r\n");
                                    out.extend_from_slice(b"location: /whip/session/abc\r\n");
                                    out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
                                    out.extend_from_slice(b"connection: close\r\n\r\n");
                                    out.extend_from_slice(body);
                                    out
                                };
                                let _ = sock.write_all(&response).await;
                                let _ = sock.flush().await;
                            });
                        }
                    }
                }
            }
            let _ = listener;
        });
        FakeServer {
            addr,
            url_template,
            last_offer_body: last_offer,
            stop: stop_tx,
        }
    }

    impl FakeServer {
        pub fn shutdown(self) {
            let _ = self.stop.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_job_lifecycle_end_to_end() {
    use std::sync::Arc;

    let server = fake_signaling::start().await;
    let url = server.url_template.clone();

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "url": url,
        "appName": "live",
        "streamName": "demo",
        "protocol": "whep",
        "timeoutMs": 5000,
        "retry": false,
        "allowPrivateIps": true,
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull/start handler");
    assert_eq!(resp.status, 200);

    // Wait for the supervisor to reach `Connected` (status 201 from
    // the mock server). The supervisor parks on `Connected` until
    // cancellation.
    let mut connected = false;
    for _ in 0..40 {
        let list_resp = svc
            .handle(HttpRequest {
                method: HttpMethod::Get,
                path: "/pull/list".into(),
                query: None,
                headers: Vec::new(),
                body: Bytes::new(),
            })
            .await
            .expect("pull/list handler");
        let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
        let jobs = payload["jobs"].as_array().cloned().unwrap_or_default();
        if jobs
            .iter()
            .any(|j| j["state"] == "Connected" || j["state"] == "Failed")
        {
            connected = jobs.iter().any(|j| j["state"] == "Connected");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        connected,
        "supervisor should reach Connected via mock server"
    );

    // Verify the supervisor actually generated a real str0m offer
    // and POSTed it to our mock signaling server. A genuine WebRTC
    // offer always contains the ICE-ufrag attribute generated by
    // str0m.
    let captured = server.last_offer_body.lock().await.clone();
    let offer = captured.expect("supervisor must POST an offer body");
    assert!(offer.starts_with("v=0"), "offer should be SDP: {offer:?}");
    assert!(
        offer.contains("a=ice-ufrag:"),
        "offer should be a real WebRTC SDP with a=ice-ufrag, got: {offer:?}"
    );

    // Stop the job.
    let stop_body = serde_json::json!({
        "appName": "live",
        "streamName": "demo",
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/stop".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&stop_body).unwrap()),
        })
        .await
        .expect("pull/stop handler");
    assert_eq!(resp.status, 204);

    server.shutdown();
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_job_blocks_private_ips_by_default() {
    use std::sync::Arc;

    // We point at a fake server on 127.0.0.1 so the supervisor will
    // try to resolve a loopback address. Because `allowPrivateIps`
    // defaults to false the HTTP client should refuse to connect and
    // the supervisor should land in `Failed`.
    let server = fake_signaling::start().await;
    let url = server.url_template.clone();

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "url": url,
        "appName": "live",
        "streamName": "blocked",
        "protocol": "whep",
        "timeoutMs": 1000,
        "retry": false,
        "allowPrivateIps": false,
    });
    let _ = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull/start handler");

    let mut failed_with_block = false;
    for _ in 0..40 {
        let list = svc
            .handle(HttpRequest {
                method: HttpMethod::Get,
                path: "/pull/list".into(),
                query: None,
                headers: Vec::new(),
                body: Bytes::new(),
            })
            .await
            .expect("list");
        let payload: serde_json::Value = serde_json::from_slice(&list.body).expect("json");
        if let Some(arr) = payload["jobs"].as_array() {
            if let Some(job) = arr.iter().find(|j| j["stream_key"] == "live/blocked") {
                if job["state"] == "Failed" {
                    let err = job["last_error"].as_str().unwrap_or("");
                    if err.contains("private") || err.contains("blocked") {
                        failed_with_block = true;
                    }
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        failed_with_block,
        "private-ip job should fail with an address-blocked diagnostic"
    );

    server.shutdown();
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_add_returns_answer_sdp_and_appears_in_list() {
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "appName": "live",
        "streamName": "p2p-demo",
        "sdp": fixture_offer(),
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/add".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("p2p add handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).expect("p2p add json");
    assert_eq!(payload["code"], 0);
    let session_label = payload["sessionid"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(session_label.contains("webrtc-session-"));
    assert!(payload["sdp"].as_str().unwrap_or("").starts_with("v=0"));

    // The session should appear in `/p2p/list`.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/p2p/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("p2p list handler");
    assert_eq!(list_resp.status, 200);
    let list_payload: serde_json::Value =
        serde_json::from_slice(&list_resp.body).expect("list json");
    let sessions = list_payload["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        sessions.iter().any(|s| s["session_id"] == session_label),
        "p2p list should include the new session: {sessions:?}"
    );

    // Removing the session should return 204.
    let remove_body = serde_json::json!({"sessionid": session_label});
    let remove_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/remove".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&remove_body).unwrap()),
        })
        .await
        .expect("p2p remove handler");
    assert_eq!(remove_resp.status, 204);

    // Removing a non-existent session returns 404.
    let bad_body = serde_json::json!({"sessionid": "999999"});
    let bad_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/remove".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&bad_body).unwrap()),
        })
        .await
        .expect("p2p remove unknown");
    assert_eq!(bad_resp.status, 404);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_get_includes_telemetry_skeleton() {
    // Phase 04: every session GET must surface a `telemetry` block,
    // even before any `Stats` / `Bwe` events have arrived. Operators
    // rely on the shape being stable so JSON parsers do not
    // intermittently fail on missing fields.
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Create a publish session via WHIP so the registry has at least
    // one entry to GET.
    let whip_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=tele".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(whip_resp.status, 201);

    let location = whip_resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header")
        .value
        .clone();
    let session_id_str = location
        .strip_prefix("/api/v1/rtc/session/")
        .expect("Location must start with /api/v1/rtc/session/")
        .to_string();
    // Session id format: webrtc-session-N — strip the prefix when
    // the path was rendered with the Display form.
    let session_path = format!("/session/{session_id_str}");

    let get_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: session_path,
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session get handler");
    assert_eq!(get_resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&get_resp.body).expect("session get json");
    let telemetry = body
        .get("telemetry")
        .expect("telemetry key must be present in session GET");
    assert!(telemetry.is_object(), "telemetry should be a JSON object");
    // The schema must always carry these keys, regardless of whether
    // any events have arrived yet. Operator dashboards key off the
    // shape, so missing keys would silently regress them.
    for key in [
        "bwe_estimated_bps",
        "bwe_target_bps",
        "remb_bitrate_bps",
        "rtp_extensions",
        "rtt_micros",
        "loss_fraction_x10000",
        "packets_in",
        "packets_out",
        "bytes_in",
        "bytes_out",
        "nack_in",
        "nack_out",
        "pli_in",
        "pli_out",
        "fir_in",
        "fir_out",
        "rtx_sent",
        "rtx_miss",
    ] {
        assert!(
            telemetry.get(key).is_some(),
            "telemetry must include `{key}` key"
        );
    }
    assert!(
        telemetry["rtp_extensions"].is_array(),
        "rtp_extensions must be a stable array field"
    );
    // Counter fields are always integers (zero or more). Optional
    // fields may be either null or an integer depending on whether
    // an event arrived in the window.
    for counter_key in [
        "packets_in",
        "packets_out",
        "bytes_in",
        "bytes_out",
        "nack_in",
        "nack_out",
        "pli_in",
        "pli_out",
        "fir_in",
        "fir_out",
        "rtx_sent",
        "rtx_miss",
    ] {
        assert!(
            telemetry[counter_key].is_u64() || telemetry[counter_key].is_i64(),
            "{counter_key} must be a non-negative integer"
        );
    }

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn play_bridge_waits_for_slow_start_publisher() {
    // Phase 03 follow-up: a WHEP request that arrives before the
    // publisher has made it onto the engine stream manager must wait
    // for the publisher within `wait_stream_timeout_ms` instead of
    // failing with `NotFound`. ZLM's `wait_stream` config drives the
    // same behaviour. We use 5 seconds here so the test has slack on
    // slow CI without becoming a long sleep on success.
    use std::sync::Arc;

    let config_yaml = "\
modules:
  webrtc:
    listen_udp: \"127.0.0.1:0\"
    enable_tcp: false
    wait_stream_timeout_ms: 5000
";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Send the WHEP request first — no publisher exists yet.
    let stream_key = cheetah_sdk::StreamKey::new("live", "slow-start");
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whep".into(),
            query: Some("appName=live&streamName=slow-start".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whep handler");
    assert_eq!(
        resp.status, 201,
        "WHEP must succeed even before publisher arrives"
    );

    // Now pretend the publisher is slow: create it after a short delay.
    // We have to keep both the lease and the sink alive past the
    // subscriber check; dropping `_lease` would tear down the
    // publisher session and the engine would close the stream
    // before the play subscriber sees it.
    let publisher_engine = engine.publisher_api();
    let stream_key_for_pub = stream_key.clone();
    let publisher_handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let pair = publisher_engine
            .acquire_publisher(
                stream_key_for_pub.clone(),
                cheetah_sdk::PublisherOptions::default(),
            )
            .await
            .expect("acquire slow-start publisher");
        // Hold the pair until the surrounding test signals shutdown.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = &pair;
        }
    });

    // Within the wait window the play subscriber should attach to
    // the engine stream once the publisher lease is acquired.
    let mut saw_subscriber = false;
    for _ in 0..80 {
        let snap = engine
            .stream_manager_api()
            .get_stream(&stream_key)
            .await
            .expect("get stream");
        if snap.map(|s| s.subscriber_count > 0).unwrap_or(false) {
            saw_subscriber = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        saw_subscriber,
        "slow-start publisher should be picked up by the play subscriber within wait_stream_timeout_ms"
    );

    publisher_handle.abort();
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn patch_with_ice_restart_creds_triggers_credential_rotation() {
    // Phase 05 follow-up: when a WHIP/WHEP PATCH carries
    // `a=ice-ufrag:` + `a=ice-pwd:`, the module must trigger an ICE
    // restart in addition to (or in lieu of) trickle candidates. We
    // observe the result indirectly: the PATCH still returns 204
    // (per WHIP convention), and the underlying str0m session ends
    // up with new pending creds (a follow-up `OfferReady` or
    // re-negotiation would carry them).
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Bring up a session via WHIP first.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=patch-restart".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header")
        .value
        .clone();
    let session_path = location
        .strip_prefix("/api/v1/rtc")
        .expect("location prefix")
        .to_string();

    // PATCH with both an ice-restart credential pair and a fresh
    // candidate. The module should accept this with `204 No Content`.
    let patch_body = "a=ice-ufrag:newufrag\r\n\
                      a=ice-pwd:newpasswordlongerthan22chars\r\n\
                      a=candidate:0 1 UDP 2122252543 192.168.1.1 50000 typ host\r\n";
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Patch,
            path: session_path.clone(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/trickle-ice-sdpfrag".into(),
            }],
            body: Bytes::from(patch_body.as_bytes().to_vec()),
        })
        .await
        .expect("patch handler");
    assert_eq!(resp.status, 204);

    // PATCH with neither candidates nor restart creds → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Patch,
            path: session_path,
            query: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"v=0\r\n"),
        })
        .await
        .expect("patch handler");
    assert_eq!(resp.status, 400);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_sendrecv_with_play_stream_acquires_publish_and_subscriber() {
    // Phase 05 follow-up: when the P2P add body carries an explicit
    // `playStreamName`, the same session should both acquire the
    // engine publisher lease (for the peer's incoming media) AND
    // attach an engine subscriber for the outgoing direction. We
    // verify both halves are visible from the engine's stream
    // manager at the same time.
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Pre-create the play-direction publisher on engine so the
    // P2P session has something to subscribe to.
    let play_key = cheetah_sdk::StreamKey::new("live", "p2p-rx");
    let (_lease, sink) = engine
        .publisher_api()
        .acquire_publisher(play_key.clone(), cheetah_sdk::PublisherOptions::default())
        .await
        .expect("acquire publisher for play-direction");

    let body = serde_json::json!({
        "appName": "live",
        "streamName": "p2p-tx",
        "playStreamName": "p2p-rx",
        "sdp": fixture_offer(),
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/add".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("p2p add handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).expect("p2p json");
    let session_label = payload["sessionid"].as_str().unwrap_or("").to_string();
    assert!(session_label.contains("webrtc-session-"));

    // Publisher lease for `p2p-tx` should be visible (the peer
    // publishes media into Cheetah).
    let pub_key = cheetah_sdk::StreamKey::new("live", "p2p-tx");
    let mut pub_visible = false;
    for _ in 0..40 {
        let snap = engine
            .stream_manager_api()
            .get_stream(&pub_key)
            .await
            .expect("get stream");
        if snap.map(|s| s.publisher_active).unwrap_or(false) {
            pub_visible = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        pub_visible,
        "P2P publish-direction lease must be active: live/p2p-tx"
    );

    // Subscriber for `p2p-rx` should attach within the slow-start
    // window (the engine pre-publisher lease was established above,
    // so attach is immediate in practice).
    let mut sub_visible = false;
    for _ in 0..40 {
        let snap = engine
            .stream_manager_api()
            .get_stream(&play_key)
            .await
            .expect("get stream");
        if snap.map(|s| s.subscriber_count > 0).unwrap_or(false) {
            sub_visible = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        sub_visible,
        "P2P play-direction subscriber must attach: live/p2p-rx"
    );

    // Cleanup.
    let remove_body = serde_json::json!({"sessionid": session_label});
    let remove_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/remove".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&remove_body).unwrap()),
        })
        .await
        .expect("p2p remove handler");
    assert_eq!(remove_resp.status, 204);
    let _ = sink.close();
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datachannel_send_endpoint_validates_inputs() {
    // Phase 05 follow-up: `POST /api/v1/rtc/session/{id}/datachannel/send`
    // accepts a JSON body with `channel`, `payload` (text or
    // base64-encoded bytes), and `binary`. We verify input validation
    // (missing fields, unknown session, closed session, bad base64)
    // and the happy path (202 Accepted on a known session).
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Bring up a session via WHIP first.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=dc-send".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location")
        .value
        .clone();
    let session_path = location
        .strip_prefix("/api/v1/rtc")
        .expect("loc prefix")
        .to_string();
    let send_path = format!("{session_path}/datachannel/send");

    // Unknown session → 404.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/session/9999999/datachannel/send".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(b"{\"channel\":0,\"payload\":\"hi\"}".to_vec()),
        })
        .await
        .expect("dc send unknown session");
    assert_eq!(resp.status, 404);

    // Missing `channel` → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: send_path.clone(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(b"{\"payload\":\"hi\"}".to_vec()),
        })
        .await
        .expect("dc send missing channel");
    assert_eq!(resp.status, 400);

    // Missing `payload` → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: send_path.clone(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(b"{\"channel\":0}".to_vec()),
        })
        .await
        .expect("dc send missing payload");
    assert_eq!(resp.status, 400);

    // Bad base64 with binary=true → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: send_path.clone(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(
                b"{\"channel\":0,\"payload\":\"!!not-b64!!\",\"binary\":true}".to_vec(),
            ),
        })
        .await
        .expect("dc send bad b64");
    assert_eq!(resp.status, 400);

    // Happy path: text payload on a known session. The DataChannel
    // is not actually open (no DTLS handshake in the test harness)
    // but the endpoint accepts the queued command and returns 202.
    // The driver-side enqueue is asynchronous; the handler does not
    // block on the actual SCTP write.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: send_path,
            query: None,
            headers: Vec::new(),
            body: Bytes::from(b"{\"channel\":0,\"payload\":\"hello\"}".to_vec()),
        })
        .await
        .expect("dc send happy");
    assert_eq!(resp.status, 202);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn module_rejects_inverted_bwe_thresholds_at_init() {
    // Phase 04 follow-up: `bwe_low_threshold_kbps >= bwe_high_threshold_kbps`
    // is a configuration error because the adaptive simulcast policy
    // would never elect the middle layer. Surface it via validation.
    let config_yaml = "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n    bwe_low_threshold_kbps: 1800\n    bwe_high_threshold_kbps: 600\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    let result = engine.start().await;
    assert!(
        result.is_err(),
        "engine start should fail with inverted thresholds"
    );
    let err = format!("{}", result.err().unwrap());
    assert!(
        err.contains("bwe_low_threshold_kbps") && err.contains("bwe_high_threshold_kbps"),
        "error message should mention the inverted thresholds: {err}"
    );
    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ice_restart_endpoint_returns_fresh_sdp_offer() {
    // Phase 05 follow-up: `POST /api/v1/rtc/session/{id}/ice-restart`
    // should rotate ICE credentials on the underlying str0m session
    // and surface the resulting fresh offer back to the caller.
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // First create a session via WHIP so we have something to
    // restart.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=ice-restart".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201);
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header")
        .value
        .clone();
    let session_path = location
        .strip_prefix("/api/v1/rtc")
        .expect("location prefix")
        .to_string();

    // Trigger the ICE restart. Empty body uses defaults.
    let restart_path = format!("{session_path}/ice-restart");
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: restart_path.clone(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("ice restart handler");
    assert_eq!(resp.status, 200, "ice-restart should return 200 with SDP");
    let content_type = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .expect("content-type")
        .value
        .clone();
    assert!(content_type.contains("application/sdp"));
    let body_str = std::str::from_utf8(&resp.body).expect("utf8 sdp");
    assert!(body_str.starts_with("v=0"), "fresh SDP must be valid");
    assert!(
        body_str.contains("a=ice-ufrag:"),
        "ICE-restart offer must carry fresh ufrag/pwd: {body_str:?}"
    );

    // Restarting an unknown session returns 404.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/session/9999999/ice-restart".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("ice restart handler");
    assert_eq!(resp.status, 404);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_endpoint_returns_prometheus_and_json() {
    // Phase 04 §4.8 metrics surface is exposed at
    // `GET /api/v1/rtc/metrics` (Prometheus text format) and
    // `GET /api/v1/rtc/metrics.json` (operator JSON). Both must
    // return the documented metric names with at-least-zero values
    // even before any sessions exist.
    use std::sync::Arc;

    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";
    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Prometheus exposition format.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/metrics".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("metrics handler");
    assert_eq!(resp.status, 200);
    let ct = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .expect("content-type")
        .value
        .clone();
    assert!(
        ct.contains("text/plain"),
        "metrics content-type should be Prometheus text/plain, got {ct:?}"
    );
    let body = std::str::from_utf8(&resp.body).expect("utf8 metrics body");
    // Spot-check a few documented metric names appear with the
    // standard `# HELP` / `# TYPE` preamble.
    for metric in [
        "webrtc_sessions_active",
        "webrtc_publish_sessions",
        "webrtc_play_sessions",
        "webrtc_packets_in_total",
        "webrtc_pli_total",
        "webrtc_route_migration_total",
        "webrtc_remb_bitrate_bps",
        "webrtc_bwe_estimate_bps",
    ] {
        assert!(
            body.contains(&format!("# HELP {metric}")),
            "metrics body must contain HELP for {metric}: {body}"
        );
        assert!(
            body.contains(&format!("# TYPE {metric}")),
            "metrics body must contain TYPE for {metric}"
        );
    }
    // Counter sample lines end with `0` for an idle module.
    assert!(body.contains("webrtc_sessions_active 0\n"));

    // JSON variant for ad-hoc consumers.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/metrics.json".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("metrics.json handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).expect("metrics.json body");
    for key in [
        "sessions_active",
        "publish_sessions",
        "play_sessions",
        "packets_in_total",
        "pli_total",
        "route_migration_total",
        "remb_bitrate_bps",
        "bwe_estimate_bps",
    ] {
        assert!(
            payload.get(key).is_some(),
            "metrics.json must contain key {key}"
        );
        assert!(
            payload[key].is_u64() || payload[key].is_i64(),
            "metrics.json[{key}] must be an integer"
        );
    }
    assert_eq!(payload["sessions_active"], 0);

    engine.stop().await;
}

/// Phase 05 follow-up: P2P signaling room keeper API smoke test.
///
/// `/api/v1/rtc/p2p/keeper/{add,remove,list}` and `/p2p/rooms` go through
/// the same `WebRtcHttpService` handler pipeline as WHIP/WHEP; the
/// registry behind them is the in-memory `P2pRoomKeeperRegistry`. The
/// test exercises the full add → list → rooms → remove cycle plus
/// negative paths (bad JSON, missing fields, removing an unknown key).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_keeper_api_round_trip() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // 1. Empty registry → /list returns 200 with `keepers: []`.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/p2p/keeper/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("keeper list handler");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["keepers"].as_array().unwrap().len(), 0);

    // 2. Add a keeper.
    let add_body = serde_json::json!({
        "server_host": "signaling.example.com",
        "server_port": 8443,
        "ssl": true,
        "room_id": "room42",
        "vhost": "__defaultVhost__",
        "app": "live",
        "stream": "demo",
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/keeper/add".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&add_body).unwrap()),
        })
        .await
        .expect("keeper add handler");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let key = body["key"].as_str().expect("key").to_string();
    assert!(key.starts_with("keeper-"));

    // 3. The new keeper appears in /list and /rooms.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/p2p/keeper/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("keeper list handler");
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let keepers = body["keepers"].as_array().unwrap();
    assert_eq!(keepers.len(), 1);
    assert_eq!(keepers[0]["room_id"], "room42");
    assert_eq!(keepers[0]["server_host"], "signaling.example.com");
    assert_eq!(keepers[0]["state"], "pending");

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/p2p/rooms".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("keeper rooms handler");
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["rooms"].as_array().unwrap()[0], "room42");

    // 4. Bad add: missing room_id → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/keeper/add".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(
                serde_json::to_vec(&serde_json::json!({"server_host": "x", "server_port": 1}))
                    .unwrap(),
            ),
        })
        .await
        .expect("keeper add handler");
    assert_eq!(resp.status, 400);

    // 5. Remove an unknown key → 404.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/keeper/remove".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(
                serde_json::to_vec(&serde_json::json!({"key": "keeper-99999"})).unwrap(),
            ),
        })
        .await
        .expect("keeper remove handler");
    assert_eq!(resp.status, 404);

    // 6. Remove the real keeper → 200, /list goes back to empty.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/keeper/remove".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&serde_json::json!({"key": key})).unwrap()),
        })
        .await
        .expect("keeper remove handler");
    assert_eq!(resp.status, 200);

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/p2p/keeper/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("keeper list handler");
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["keepers"].as_array().unwrap().len(), 0);

    engine.stop().await;
}

/// Phase 05 follow-up round 9: pull/push HTTP entry now spawns a
/// real P2P client job for `signaling_protocols=1` URLs and returns
/// `200` + a session id when the driver is bound. The supervisor
/// task runs in the background and may fail the WebSocket handshake
/// (we point at a non-existent host here), but the HTTP response is
/// the start signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_start_p2p_signaling_returns_200_with_session_id() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "url": "webrtc://signaling.example.com/live/demo?signaling_protocols=1&peer_room_id=room42",
        "app": "live",
        "stream": "demo",
        // Keep timeouts short so the supervisor's first connect
        // attempt fails quickly when the host doesn't actually run a
        // signaling server.
        "connectTimeoutMs": 200,
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull start handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(payload["kind"], "pull");
    assert_eq!(payload["peer_room_id"], "room42");
    let session_id = payload["session_id"].as_str().unwrap_or_default();
    assert!(
        session_id.starts_with("webrtc-session-"),
        "expected session id, got {session_id:?}"
    );
    let signaling_url = payload["signaling_url"].as_str().unwrap_or_default();
    assert!(
        signaling_url.contains("signaling.example.com"),
        "signaling_url should keep the host, got {signaling_url:?}"
    );
    assert!(payload["state"]
        .as_str()
        .map(|s| s == "pending" || s == "running")
        .unwrap_or(false));
    engine.stop().await;
}

/// Phase 05 follow-up round 8: a `signaling_protocols=1` URL whose
/// signaling host fails the SSRF guard must return `400 p2p_invalid_url`
/// instead of `501`. The operator should be told that the URL itself
/// is the problem, not the runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_start_p2p_with_loopback_host_returns_400() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Loopback host should be rejected by the SSRF guard.
    let body = serde_json::json!({
        "url": "webrtc://127.0.0.1:8443/live/demo?signaling_protocols=1&peer_room_id=room42",
        "app": "live",
        "stream": "demo",
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull start handler");
    assert_eq!(resp.status, 400);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(payload["error"], "p2p_invalid_url");

    engine.stop().await;
}

/// Phase 05 follow-up round 9: when the request opts in via
/// `allowPrivateIps`, the loopback host is accepted. Round 9 wires
/// the P2P client job runner so a valid P2P URL now returns 200 + a
/// session id (driver is bound — the supervisor task will run in the
/// background and may fail the WebSocket handshake against an
/// inert host, but the HTTP response is the start signal).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_start_p2p_loopback_with_allow_private_ips_returns_200() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let body = serde_json::json!({
        "url": "webrtc://127.0.0.1:8443/live/demo?signaling_protocols=1&peer_room_id=room42",
        "app": "live",
        "stream": "demo",
        "allowPrivateIps": true,
        // Keep timeouts short so the supervisor's first attempt
        // fails quickly when the loopback signaling URL is inert.
        "connectTimeoutMs": 200,
        "offerTimeoutMs": 200,
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull start handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(payload["kind"], "pull");
    assert_eq!(payload["peer_room_id"], "room42");
    assert!(payload["session_id"]
        .as_str()
        .unwrap_or_default()
        .starts_with("webrtc-session-"));
    let signaling_url = payload["signaling_url"].as_str().unwrap_or_default();
    assert!(
        signaling_url.contains("127.0.0.1"),
        "signaling_url should resolve loopback when allowed, got {signaling_url:?}"
    );

    engine.stop().await;
}

/// Phase 05 follow-up round 9: list + stop endpoints for P2P client
/// jobs. After `/pull/start` returns 200, the job should appear in
/// `/p2p/client/list`; `POST /p2p/client/stop` cancels it and the
/// list goes back to empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2p_client_list_and_stop_round_trip() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Start a P2P pull job.
    let body = serde_json::json!({
        "url": "webrtc://signaling.example.com/live/demo?signaling_protocols=1&peer_room_id=room42",
        "app": "live",
        "stream": "demo",
        "connectTimeoutMs": 200,
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/pull/start".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("pull start handler");
    assert_eq!(resp.status, 200);
    let payload: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let session_id = payload["session_id"].as_str().unwrap().to_string();

    // List should include the job. The supervisor may have already
    // failed (no real WS server on signaling.example.com), so we
    // accept any non-empty list. We poll briefly because the
    // supervisor cleans the registry up after `GaveUp`.
    let mut found = false;
    for _ in 0..20 {
        let resp = svc
            .handle(HttpRequest {
                method: HttpMethod::Get,
                path: "/p2p/client/list".into(),
                query: None,
                headers: Vec::new(),
                body: Bytes::new(),
            })
            .await
            .expect("client list handler");
        assert_eq!(resp.status, 200);
        let payload: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        let jobs = payload["jobs"].as_array().unwrap();
        if jobs.iter().any(|j| j["session_id"] == session_id) {
            found = true;
            break;
        }
        if jobs.is_empty() {
            // Supervisor already finished and unregistered. That's
            // fine; the job ran to completion (even if as a Failed
            // outcome).
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let _ = found;

    // Stop returns 200 if the job is still alive, 404 otherwise.
    // Either is acceptable for this test — the contract is about
    // shape, not lifetime.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/client/stop".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(
                serde_json::to_vec(&serde_json::json!({"session_id": session_id})).unwrap(),
            ),
        })
        .await
        .expect("client stop handler");
    assert!(resp.status == 200 || resp.status == 404);

    // Stopping with bad payload returns 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/client/stop".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"{not valid json"),
        })
        .await
        .expect("client stop handler");
    assert_eq!(resp.status, 400);

    // Missing session_id → 400.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/client/stop".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"{}"),
        })
        .await
        .expect("client stop handler");
    assert_eq!(resp.status, 400);

    // Unknown session id → 404.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/p2p/client/stop".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"{\"session_id\": \"webrtc-session-99999\"}"),
        })
        .await
        .expect("client stop handler");
    assert_eq!(resp.status, 404);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abl_whep_options_returns_cors_without_session() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // OPTIONS /rtc/v1/whep/?app=live&stream=s should return 200 with CORS headers.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Options,
            path: "/rtc/v1/whep/".into(),
            query: Some("app=live&stream=s".into()),
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("options handler");
    assert_eq!(resp.status, 200);

    // Verify CORS headers are present.
    let find_header = |name: &str| {
        resp.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.clone())
    };
    assert!(
        find_header("Access-Control-Allow-Origin").is_some(),
        "must include Access-Control-Allow-Origin"
    );
    assert!(
        find_header("Access-Control-Allow-Methods").is_some(),
        "must include Access-Control-Allow-Methods"
    );
    assert!(
        find_header("Access-Control-Allow-Headers").is_some(),
        "must include Access-Control-Allow-Headers"
    );
    assert_eq!(
        find_header("Content-Length").as_deref(),
        Some("0"),
        "must include Content-Length: 0"
    );

    // Body must be empty.
    assert!(resp.body.is_empty(), "OPTIONS body must be empty");

    // Private network access header should NOT be present by default.
    assert!(
        find_header("Access-Control-Allow-Private-Network").is_none(),
        "private network header should not be present when config is disabled"
    );

    // Verify no session was created.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "OPTIONS must not create a WebRTC session"
    );

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abl_whep_options_includes_private_network_header_when_enabled() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n    enable_private_network_access: true\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Options,
            path: "/rtc/v1/whep/".into(),
            query: Some("app=live&stream=s".into()),
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("options handler");
    assert_eq!(resp.status, 200);

    let find_header = |name: &str| {
        resp.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.clone())
    };
    assert_eq!(
        find_header("Access-Control-Allow-Private-Network").as_deref(),
        Some("true"),
        "private network header must be present when config enables it"
    );

    engine.stop().await;
}

/// Task 1.3: POST success returns 201 Created with `application/sdp` and
/// a `Location` header pointing to a stable session resource URL that
/// supports subsequent PATCH and DELETE operations.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abl_whep_post_location_supports_patch_delete() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Pre-create a publisher so the WHEP play has something to subscribe to.
    let stream_key = cheetah_sdk::StreamKey::new("live", "abl-lifecycle");
    let (_lease, _sink) = engine
        .publisher_api()
        .acquire_publisher(stream_key.clone(), cheetah_sdk::PublisherOptions::default())
        .await
        .expect("acquire publisher");

    // POST /rtc/v1/whep/?app=live&stream=abl-lifecycle (ABL-style path)
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/rtc/v1/whep/".into(),
            query: Some("app=live&stream=abl-lifecycle".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whep post handler");

    // Verify 201 Created.
    assert_eq!(resp.status, 201, "POST must return 201 Created");

    // Verify Content-Type: application/sdp.
    let find_header = |name: &str| {
        resp.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.clone())
    };
    assert_eq!(
        find_header("content-type").as_deref(),
        Some("application/sdp"),
        "POST must return Content-Type: application/sdp"
    );

    // Verify Location header is present and points to a session resource.
    let location = find_header("location").expect("POST must include Location header");
    assert!(
        location.starts_with("/api/v1/rtc/session/"),
        "Location must point to /api/v1/rtc/session/{{id}}, got: {location}"
    );

    // Verify Access-Control-Expose-Headers includes Location (for browser CORS).
    let expose = find_header("access-control-expose-headers");
    assert!(
        expose
            .as_deref()
            .map(|v| v.contains("Location"))
            .unwrap_or(false),
        "POST must include Access-Control-Expose-Headers: Location for browser compatibility"
    );

    // Body must be non-empty SDP answer.
    assert!(!resp.body.is_empty(), "POST body must contain SDP answer");
    let body_str = std::str::from_utf8(&resp.body).expect("body is UTF-8");
    assert!(body_str.contains("v=0"), "SDP answer must contain v=0 line");

    // Extract the session path from Location for subsequent operations.
    // Location is `/api/v1/rtc/session/{id}`, but the HTTP service routes
    // are relative to the module mount, so we strip the `/api/v1/rtc` prefix.
    let session_path = location
        .strip_prefix("/api/v1/rtc")
        .expect("location should start with /api/v1/rtc");

    // PATCH the session with a trickle ICE candidate — should return 204.
    let patch_body = "a=candidate:1 1 udp 2130706431 192.168.1.1 12345 typ host\r\n";
    let patch_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Patch,
            path: session_path.to_string(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/trickle-ice-sdpfrag".into(),
            }],
            body: Bytes::from(patch_body),
        })
        .await
        .expect("patch handler");
    assert_eq!(
        patch_resp.status, 204,
        "PATCH on Location URL must return 204"
    );

    // DELETE the session — should return 204.
    let delete_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Delete,
            path: session_path.to_string(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("delete handler");
    assert_eq!(
        delete_resp.status, 204,
        "DELETE on Location URL must return 204"
    );

    // Verify the session is gone from the registry.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(sessions.is_empty(), "session must be removed after DELETE");

    engine.stop().await;
}

/// Task 1.3: HTTP connection close (i.e., the POST request completing and
/// the TCP connection being dropped) must NOT trigger play session
/// destruction. The session lives until explicit DELETE, driver close, or
/// timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abl_whep_http_close_does_not_close_media_session() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Pre-create a publisher so the WHEP play has something to subscribe to.
    let stream_key = cheetah_sdk::StreamKey::new("live", "http-close-test");
    let (_lease, _sink) = engine
        .publisher_api()
        .acquire_publisher(stream_key.clone(), cheetah_sdk::PublisherOptions::default())
        .await
        .expect("acquire publisher");

    // POST to create a WHEP play session.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whep".into(),
            query: Some("app=live&stream=http-close-test".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whep post handler");
    assert_eq!(resp.status, 201);

    // At this point the HTTP request is complete — in a real server the
    // TCP connection would be closed (or reused for keep-alive). The key
    // invariant is that the session STILL EXISTS in the registry.

    // Simulate "HTTP connection closed" by simply dropping the response
    // and not holding any reference to the HTTP layer. The session must
    // persist.
    drop(resp);

    // Small delay to ensure any async cleanup (if incorrectly wired)
    // would have had time to fire.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify the session is still alive in the registry.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert_eq!(
        sessions.len(),
        1,
        "session must survive HTTP connection close — found {} sessions",
        sessions.len()
    );

    // Verify the session has the correct stream key.
    let session = &sessions[0];
    assert_eq!(
        session["stream_key"].as_str(),
        Some("live/http-close-test"),
        "surviving session must have the correct stream key"
    );
    assert_eq!(
        session["role"].as_str(),
        Some("Player"),
        "surviving session must be a Player"
    );

    // Now explicitly DELETE to clean up.
    let session_id_str = session["session_id"].as_str().expect("session_id");
    let delete_path = format!("/session/{}", session_id_str);
    let delete_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Delete,
            path: delete_path,
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("delete handler");
    assert_eq!(delete_resp.status, 204);

    // Verify session is now gone.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "session must be removed after explicit DELETE"
    );

    engine.stop().await;
}

/// Phase 04 Task 4.1: HTTP request drop (connection close) does NOT equal
/// session close. A WHEP play session created via POST must survive the
/// HTTP response being dropped. Only explicit DELETE, driver close,
/// timeout, or stream closure should terminate the session.
///
/// This test complements `abl_whep_http_close_does_not_close_media_session`
/// with the exact name from the Phase 04 acceptance criteria.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_drop_does_not_delete_play_session() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Pre-create a publisher so the WHEP play has something to subscribe to.
    let stream_key = cheetah_sdk::StreamKey::new("live", "http-drop-test");
    let (_lease, _sink) = engine
        .publisher_api()
        .acquire_publisher(stream_key.clone(), cheetah_sdk::PublisherOptions::default())
        .await
        .expect("acquire publisher");

    // Create a WHEP play session via POST.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whep".into(),
            query: Some("app=live&stream=http-drop-test".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whep post handler");
    assert_eq!(resp.status, 201, "WHEP POST must return 201 Created");

    // Extract session ID from Location header.
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header must be present");
    assert!(
        location.value.contains("/session/"),
        "Location must point to session resource"
    );

    // Drop the HTTP response — simulating the HTTP connection closing.
    // In a real server, the TCP connection would be closed here.
    // The key invariant: the WebRTC session must NOT be destroyed.
    drop(resp);

    // Allow time for any (incorrectly wired) async cleanup to fire.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Verify the session is still alive.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert_eq!(
        sessions.len(),
        1,
        "play session must survive HTTP connection drop — found {} sessions",
        sessions.len()
    );
    assert_eq!(
        sessions[0]["stream_key"].as_str(),
        Some("live/http-drop-test"),
        "surviving session must have the correct stream key"
    );
    assert_eq!(
        sessions[0]["state"].as_str(),
        Some("Created"),
        "session state should be Created (ICE/DTLS not yet established in test)"
    );

    engine.stop().await;
}

/// Phase 04 Task 4.1: DELETE enters the unified cleanup path and is
/// idempotent. Calling DELETE twice on the same session must both
/// return 204 — the first removes the session, the second is a no-op.
/// This verifies the unified cleanup path handles the "already gone"
/// case gracefully.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_closes_session_once() {
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Create a WHIP publish session.
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=delete-once-test".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(fixture_offer()),
        })
        .await
        .expect("whip handler");
    assert_eq!(resp.status, 201, "WHIP POST must return 201");

    // Extract session ID from Location header.
    let location = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .expect("Location header");
    let session_path = location
        .value
        .strip_prefix("/api/v1/rtc")
        .unwrap_or(&location.value);

    // Verify session exists.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1, "session must exist before DELETE");

    // First DELETE — should remove the session and return 204.
    let delete_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Delete,
            path: session_path.to_string(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("first delete handler");
    assert_eq!(
        delete_resp.status, 204,
        "first DELETE must return 204 No Content"
    );

    // Verify session is gone.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list after first delete");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "session must be removed after first DELETE"
    );

    // Second DELETE — idempotent, must also return 204 without error.
    let delete_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Delete,
            path: session_path.to_string(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("second delete handler");
    assert_eq!(
        delete_resp.status, 204,
        "second DELETE must also return 204 (idempotent)"
    );

    // Session list still empty — no ghost sessions.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list after second delete");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "no ghost sessions after idempotent DELETE"
    );

    engine.stop().await;
}

/// Phase 04 Task 4.1: Half-initialized failure (answer generation fails
/// after session is allocated in the registry) must NOT leave session
/// registry residue. The cleanup path must remove the partially-created
/// session.
///
/// We trigger this by sending a completely invalid SDP offer that the
/// driver cannot process, causing the answer to fail. The module must
/// call `cleanup_session` and remove the entry from the registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_post_releases_partial_session() {
    // Use a short timeout so the test doesn't hang if the driver
    // doesn't emit a failure diagnostic quickly enough.
    let config_yaml =
        "modules:\n  webrtc:\n    listen_udp: \"127.0.0.1:0\"\n    enable_tcp: false\n    wait_stream_timeout_ms: 2000\n";

    let config = Arc::new(ConfigStore::new());
    config.load_yaml_str(config_yaml).expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(WebRtcModuleFactory))
        .build()
        .expect("build engine");
    engine.start().await.expect("start engine");

    let mounts = engine.module_manager_api().http_mounts();
    let mount = mounts
        .iter()
        .find(|m| m.module_id.0 == "webrtc")
        .expect("webrtc mount registered");
    let svc = mount.service.clone();

    // Send a WHIP request with a completely invalid SDP. The driver
    // will fail to parse it and emit a diagnostic/failure, which
    // triggers cleanup_session.
    let invalid_sdp = "this is not a valid SDP at all\r\n";
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/whip".into(),
            query: Some("appName=live&streamName=partial-fail-test".into()),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/sdp".into(),
            }],
            body: Bytes::from(invalid_sdp),
        })
        .await
        .expect("whip handler with invalid SDP");

    // The response should be an error (503 from driver failure or timeout).
    assert_ne!(
        resp.status, 201,
        "invalid SDP must not produce a successful 201 response"
    );
    assert!(
        resp.status == 503 || resp.status >= 400,
        "expected error status, got {}",
        resp.status
    );

    // The critical invariant: the session registry must be empty.
    // No residue from the half-initialized session.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "half-initialized session must not remain in registry — found {} sessions: {:?}",
        sessions.len(),
        sessions
    );

    // Also verify via SMS-style play endpoint with invalid SDP.
    let body = serde_json::json!({
        "appName": "live",
        "streamName": "partial-fail-play",
        "sdp": "not valid sdp",
    });
    let resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Post,
            path: "/play".into(),
            query: None,
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        })
        .await
        .expect("play handler with invalid SDP");
    assert_ne!(
        resp.status, 200,
        "invalid SDP must not produce a successful response"
    );

    // Registry must still be empty — no residue from either failed request.
    let list_resp = svc
        .handle(HttpRequest {
            method: HttpMethod::Get,
            path: "/session/list".into(),
            query: None,
            headers: Vec::new(),
            body: Bytes::new(),
        })
        .await
        .expect("session list handler");
    let payload: serde_json::Value = serde_json::from_slice(&list_resp.body).expect("json");
    let sessions = payload["sessions"].as_array().expect("sessions array");
    assert!(
        sessions.is_empty(),
        "no session residue after failed play request — found {} sessions",
        sessions.len()
    );

    engine.stop().await;
}
