use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpMethod, HttpRequest};
use serde_json::json;

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "zlm": {
                "auth": { "mode": "none" }
            }
        }
    }));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
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

fn get(path: &str, query: Option<String>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query,
        headers: vec![],
        body: Bytes::new(),
    }
}

fn body_json(resp: &cheetah_sdk::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_proxy_l1_lifecycle() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    let add = post(
        "/api/addStreamProxy",
        json!({
            "url": "http://example.com/live.flv",
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "test"
        }),
    );
    let resp = service.handle(add).await.expect("add stream proxy");
    assert_eq!(resp.status, 200);
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "add stream proxy failed: {body}");
    let key = body["data"]["key"]
        .as_str()
        .expect("key in response")
        .to_string();

    let list = get("/api/listStreamProxy", None);
    let resp = service.handle(list).await.expect("list stream proxy");
    let body = body_json(&resp);
    assert_eq!(body["data"]["total"], 1);

    let get_info = get("/api/getProxyInfo", Some(format!("key={key}")));
    let resp = service.handle(get_info).await.expect("get proxy info");
    let body = body_json(&resp);
    assert_eq!(body["data"]["key"], key);

    let del = post("/api/delStreamProxy", json!({"key": key}));
    let resp = service.handle(del).await.expect("del stream proxy");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);

    let list = get("/api/listStreamProxy", None);
    let resp = service.handle(list).await.expect("list after delete");
    let body = body_json(&resp);
    assert_eq!(body["data"]["total"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_pusher_proxy_l1_lifecycle() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    let add = post(
        "/api/addStreamPusherProxy",
        json!({
            "dst_url": "rtmp://example.com/live/push",
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "test"
        }),
    );
    let resp = service.handle(add).await.expect("add pusher proxy");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "add pusher proxy failed: {body}");
    let key = body["data"]["key"]
        .as_str()
        .expect("key in response")
        .to_string();

    let list = get("/api/listStreamPusherProxy", None);
    let resp = service.handle(list).await.expect("list pusher proxy");
    let body = body_json(&resp);
    assert_eq!(body["data"]["total"], 1);

    let get_info = get("/api/getProxyPusherInfo", Some(format!("key={key}")));
    let resp = service.handle(get_info).await.expect("get pusher info");
    let body = body_json(&resp);
    assert_eq!(body["data"]["key"], key);

    let del = post("/api/delStreamPusherProxy", json!({"key": key}));
    let resp = service.handle(del).await.expect("del pusher proxy");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0);
}
