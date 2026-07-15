use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{HttpHeader, HttpMethod, HttpRequest};
use serde_json::json;

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "zlm": {
                "auth": {
                    "mode": "session",
                    "session": {
                        "username": "admin",
                        "password": "secret123",
                        "cookie_name": "zlm_session",
                        "session_ttl_sec": 3600
                    }
                }
            }
        }
    }));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn get(path: &str, headers: Vec<HttpHeader>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        path: path.to_string(),
        query: None,
        headers,
        body: Bytes::new(),
    }
}

fn post(path: &str, body: serde_json::Value, headers: Vec<HttpHeader>) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        path: path.to_string(),
        query: None,
        headers,
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    }
}

fn body_json(resp: &cheetah_sdk::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!({}))
}

fn find_cookie_token(resp: &cheetah_sdk::HttpResponse, cookie_name: &str) -> Option<String> {
    let header = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("set-cookie"))?
        .value
        .clone();
    for pair in header.split(';') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next()?.trim();
        if key.eq_ignore_ascii_case(cookie_name) {
            return kv.next().map(|v| v.trim().to_string());
        }
    }
    None
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_session_auth_flow() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    // Protected route without session returns -100.
    let resp = service
        .handle(get("/api/version", vec![]))
        .await
        .expect("version");
    let body = body_json(&resp);
    assert_eq!(body["code"], -100, "version should require auth: {body}");

    // Invalid credentials rejected.
    let resp = service
        .handle(post(
            "/api/login",
            json!({"username": "admin", "password": "wrong"}),
            vec![],
        ))
        .await
        .expect("login");
    let body = body_json(&resp);
    assert_eq!(body["code"], -100, "invalid login: {body}");

    // Successful login returns a session cookie.
    let resp = service
        .handle(post(
            "/api/login",
            json!({"username": "admin", "password": "secret123"}),
            vec![],
        ))
        .await
        .expect("login");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "login failed: {body}");
    let token = find_cookie_token(&resp, "zlm_session").expect("session cookie");

    // Protected route with cookie succeeds.
    let resp = service
        .handle(get(
            "/api/version",
            vec![HttpHeader {
                name: "Cookie".to_string(),
                value: format!("zlm_session={token}"),
            }],
        ))
        .await
        .expect("version with cookie");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "version with cookie failed: {body}");

    // Logout succeeds.
    let resp = service
        .handle(post(
            "/api/logout",
            json!({}),
            vec![HttpHeader {
                name: "Cookie".to_string(),
                value: format!("zlm_session={token}"),
            }],
        ))
        .await
        .expect("logout");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "logout failed: {body}");

    // Cookie is now invalid.
    let resp = service
        .handle(get(
            "/api/version",
            vec![HttpHeader {
                name: "Cookie".to_string(),
                value: format!("zlm_session={token}"),
            }],
        ))
        .await
        .expect("version after logout");
    let body = body_json(&resp);
    assert_eq!(
        body["code"], -100,
        "token should be invalid after logout: {body}"
    );
}
