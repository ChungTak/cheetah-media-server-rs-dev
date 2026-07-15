use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
use serde_json::json;

fn make_engine(global: serde_json::Value) -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(global);
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn get(path: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query: None,
        headers: vec![],
        body: Bytes::new(),
    }
}

fn bearer_get(path: &str, token: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query: None,
        headers: vec![HttpHeader {
            name: "authorization".to_string(),
            value: format!("Bearer {token}"),
        }],
        body: Bytes::new(),
    }
}

fn body_json(resp: &cheetah_sdk::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

async fn zlm_service() -> Arc<dyn cheetah_sdk::ModuleHttpService> {
    let engine = make_engine(json!({
        "media": {
            "zlm": {
                "auth": { "mode": "none" }
            }
        }
    }));
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    mount.service.clone()
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_version_and_api_list_are_available() {
    let service = zlm_service().await;

    let resp = service.handle(get("/api/version")).await.expect("version");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "version failed: {body}");
    assert!(
        body["data"]["version"].as_str().is_some(),
        "version missing: {body}"
    );

    let resp = service
        .handle(get("/api/getApiList"))
        .await
        .expect("api list");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "api list failed: {body}");
    let apis = body["data"]["apis"].as_array().expect("apis array");
    assert!(
        apis.iter().any(|v| v.as_str() == Some("/api/version")),
        "version route missing from api list: {body}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_l3_and_l4_routes_return_501() {
    let service = zlm_service().await;

    let resp = service
        .handle(get("/api/getThreadsLoad"))
        .await
        .expect("threads load");
    let body = body_json(&resp);
    assert_eq!(
        body["code"], -501,
        "L3 route should be capability-gated: {body}"
    );

    let resp = service
        .handle(get("/api/searchOnvifDevice"))
        .await
        .expect("search onvif");
    let body = body_json(&resp);
    assert_eq!(
        body["code"], -501,
        "L4 route should be capability-gated: {body}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_l3_requires_admin_scope() {
    let engine = make_engine(json!({
        "media": {
            "zlm": {
                "auth": { "mode": "token" }
            },
            "native": {
                "tokens": {
                    "user-token": { "principal": "user", "scopes": ["media.read"] }
                }
            }
        }
    }));
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    let resp = service
        .handle(bearer_get("/api/getThreadsLoad", "user-token"))
        .await
        .expect("threads load with read token");
    let body = body_json(&resp);
    assert_eq!(
        body["code"], -100,
        "L3 route should require admin scope: {body}"
    );
}
