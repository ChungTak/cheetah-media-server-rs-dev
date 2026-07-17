use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::{NativeMediaModuleFactory, ZlmMediaModuleFactory};
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_rtp_module::RtpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ConfigApplyApi, ConfigEffect, HttpMethod, HttpRequest, ModuleId};
use serde_json::json;

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "native": { "auth": { "mode": "none" } },
            "zlm": { "auth": { "mode": "none" } }
        }
    }));
    config
        .apply_module_patch(
            &ModuleId::new("rtp"),
            json!({
                "enabled": true,
                "listen_udp": "127.0.0.1:0",
                "listen_tcp": "127.0.0.1:0",
                "rtcp_listen_udp": "127.0.0.1:0"
            }),
            ConfigEffect::Immediate,
        )
        .expect("apply rtp config patch");
    let runtime = Arc::new(TokioRuntime::new());
    let schema_registry = config.clone();
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .with_config_schema_registry(schema_registry)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .register_module_factory(Arc::new(RtpModuleFactory))
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .register_module_factory(Arc::new(NativeMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn get(path: &str, query: Option<String>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query,
        headers: vec![],
        body: Bytes::new(),
    }
}

fn post(path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        query: None,
        headers: vec![],
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

fn patch(path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Patch,
        path: path.to_string(),
        query: None,
        headers: vec![],
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

fn delete(path: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Delete,
        path: path.to_string(),
        query: None,
        headers: vec![],
        body: Bytes::new(),
    }
}

fn body_json(resp: &cheetah_sdk::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

#[tokio::test(flavor = "current_thread")]
async fn native_rtp_receiver_http_lifecycle() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-native")
        .expect("native mount");
    let service = mount.service.clone();

    // 1. Open a passive UDP receiver and update it.
    let open = post(
        "/rtp/receivers",
        json!({
            "media_key": {
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "native-rtp-passive"
            },
            "ssrc": 1000,
            "payload_type": 96,
            "codec_hint": "ps"
        }),
    );
    let resp = service.handle(open).await.expect("open receiver");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "open receiver failed: {body}");
    let passive_id = body["session_id"].as_str().expect("session_id in response");
    assert_eq!(body["kind"], "receiver");
    assert_eq!(body["state"], "listening");
    assert!(body["local_port"].is_u64());

    let get_session = get(&format!("/rtp/sessions/{passive_id}"), None);
    let resp = service.handle(get_session).await.expect("get session");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "get session failed: {body}");
    assert_eq!(body["session_id"], passive_id);

    let update = patch(
        &format!("/rtp/sessions/{passive_id}"),
        json!({"expected_generation": 1, "ssrc": 3000, "payload_type": 97}),
    );
    let resp = service.handle(update).await.expect("update session");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "update session failed: {body}");
    assert_eq!(body["ssrc"], 3000);
    assert_eq!(body["payload_type"], 97);
    assert_eq!(body["generation"], 2);

    // 2. Open an active TCP receiver and connect it to a local listener.
    let open = post(
        "/rtp/receivers",
        json!({
            "media_key": {
                "vhost": "__defaultVhost__",
                "app": "live",
                "stream": "native-rtp-active"
            },
            "ssrc": 4000,
            "payload_type": 96,
            "codec_hint": "ps",
            "tcp_mode": "active"
        }),
    );
    let resp = service.handle(open).await.expect("open active receiver");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "open active receiver failed: {body}");
    let active_id = body["session_id"].as_str().expect("session_id in response");
    assert_eq!(body["state"], "created");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let dest = listener.local_addr().unwrap();
    let _accept_task = tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let connect = post(
        &format!("/rtp/receivers/{active_id}/connect"),
        json!({"remote_endpoint": dest.to_string(), "ssrc": 5000}),
    );
    let resp = service.handle(connect).await.expect("connect receiver");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "connect receiver failed: {body}");
    assert_eq!(body["remote_endpoint"], dest.to_string());
    assert_eq!(body["generation"], 2);

    let get_session = get(&format!("/rtp/sessions/{active_id}"), None);
    let resp = service
        .handle(get_session)
        .await
        .expect("get active session");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "get active session failed: {body}");
    assert_eq!(body["session_id"], active_id);
    assert_eq!(body["remote_endpoint"], dest.to_string());

    // 3. List and delete.
    let list = get("/rtp/sessions", None);
    let resp = service.handle(list).await.expect("list sessions");
    let body = body_json(&resp);
    assert_eq!(resp.status, 200, "list sessions failed: {body}");
    assert_eq!(body["total"], 2);

    let del = delete(&format!("/rtp/sessions/{passive_id}"));
    let resp = service.handle(del).await.expect("delete passive session");
    assert_eq!(resp.status, 204, "delete passive session failed: {resp:?}");

    let del = delete(&format!("/rtp/sessions/{active_id}"));
    let resp = service.handle(del).await.expect("delete active session");
    assert_eq!(resp.status, 204, "delete active session failed: {resp:?}");

    let get_session = get(&format!("/rtp/sessions/{passive_id}"), None);
    let resp = service.handle(get_session).await;
    assert!(resp.is_err() || resp.unwrap().status == 404);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_rtp_compat_adapter_maps_server_and_sender() {
    let engine = make_engine();
    engine.start().await.expect("engine start");
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    let open = post(
        "/api/openRtpServer",
        json!({
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "zlm-adapter",
            "ssrc": 1111,
            "payload_type": 96
        }),
    );
    let resp = service.handle(open).await.expect("openRtpServer");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "openRtpServer failed: {body}");
    let _session_id = body["session_id"].as_str().expect("session_id");
    let port = body["port"].as_u64().expect("port");
    assert!(port > 0);

    let info = get(
        "/api/getRtpInfo",
        Some("?vhost=__defaultVhost__&app=live&stream=zlm-adapter".to_string()),
    );
    let resp = service.handle(info).await.expect("getRtpInfo");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "getRtpInfo failed: {body}");
    assert_eq!(body["exist"], true);
    assert_eq!(body["localPort"], port);

    let list = get("/api/listRtpServer", None);
    let resp = service.handle(list).await.expect("listRtpServer");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "listRtpServer failed: {body}");
    assert!(body["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| { i["streamId"].as_str().unwrap_or("") == "__defaultVhost__/live/zlm-adapter" }));

    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind udp");
    let dest = socket.local_addr().unwrap();

    let start = post(
        "/api/startSendRtp",
        json!({
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "zlm-adapter-send",
            "ssrc": 2222,
            "payload_type": 96,
            "dst_ip": dest.ip().to_string(),
            "dst_port": dest.port()
        }),
    );
    let resp = service.handle(start).await.expect("startSendRtp");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "startSendRtp failed: {body}");
    let _sender_id = body["session_id"].as_str().expect("session_id");

    let list = get("/api/listRtpSender", None);
    let resp = service.handle(list).await.expect("listRtpSender");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "listRtpSender failed: {body}");
    assert!(body["data"].as_array().unwrap().iter().any(|i| {
        i["streamId"].as_str().unwrap_or("") == "__defaultVhost__/live/zlm-adapter-send"
    }));

    let stop = post(
        "/api/stopSendRtp",
        json!({
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "zlm-adapter-send"
        }),
    );
    let resp = service.handle(stop).await.expect("stopSendRtp");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "stopSendRtp failed: {body}");

    let close = post(
        "/api/closeRtpServer",
        json!({
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "zlm-adapter"
        }),
    );
    let resp = service.handle(close).await.expect("closeRtpServer");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "closeRtpServer failed: {body}");
    assert_eq!(body["hit"], 1);

    let info = get(
        "/api/getRtpInfo",
        Some("?vhost=__defaultVhost__&app=live&stream=zlm-adapter".to_string()),
    );
    let resp = service.handle(info).await.expect("getRtpInfo after close");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "getRtpInfo after close failed: {body}");
    assert_eq!(body["exist"], false);

    engine.stop().await;
}
