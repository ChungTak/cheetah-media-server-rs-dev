use std::sync::Arc;

use bytes::Bytes;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_rtp_module::RtpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ConfigApplyApi, ConfigEffect, HttpMethod, HttpRequest, ModuleId};
use serde_json::json;

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({
        "media": {
            "zlm": {
                "auth": { "mode": "none" }
            }
        },
        "rtp": {
            "enabled": true
        }
    }));
    config
        .apply_module_patch(
            &ModuleId::new("rtp"),
            json!({
                "enabled": true,
                "listen_udp": "127.0.0.1:0",
                "listen_tcp": "127.0.0.1:0"
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
async fn zlm_rtp_multiplex_and_check_control() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let mount = engine
        .module_manager_api()
        .http_mounts()
        .into_iter()
        .find(|m| m.module_id.0 == "media-http-zlm")
        .expect("zlm mount");
    let service = mount.service.clone();

    let open = post(
        "/api/openRtpServerMultiplex",
        json!({
            "vhost": "__defaultVhost__",
            "app": "live",
            "stream": "rtp-multiplex"
        }),
    );
    let resp = service
        .handle(open)
        .await
        .expect("open rtp server multiplex");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "openRtpServerMultiplex failed: {body}");
    assert!(body["port"].is_u64(), "port missing: {body}");

    let session_id = body["session_id"]
        .as_str()
        .expect("session_id in response")
        .to_string();

    let pause = post("/api/pauseRtpCheck", json!({"session_id": &session_id}));
    let resp = service.handle(pause).await.expect("pause rtp check");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "pauseRtpCheck failed: {body}");
    assert_eq!(body["check_paused"], true);

    let resume = post("/api/resumeRtpCheck", json!({"session_id": session_id}));
    let resp = service.handle(resume).await.expect("resume rtp check");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "resumeRtpCheck failed: {body}");
    assert_eq!(body["check_paused"], false);

    let update = post(
        "/api/updateRtpServerSSRC",
        json!({"session_id": session_id, "ssrc": 12345}),
    );
    let resp = service
        .handle(update)
        .await
        .expect("updateRtpServerSSRC response");
    let body = body_json(&resp);
    assert_eq!(body["code"], 0, "updateRtpServerSSRC failed: {body}");
    assert_eq!(body["session_id"], session_id);
    assert_eq!(body["ssrc"], 12345);
}
