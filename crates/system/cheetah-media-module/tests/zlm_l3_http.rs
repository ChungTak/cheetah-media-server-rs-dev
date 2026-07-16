//! L3 black-box tests for the ZLM-compatible adapter.
//!
//! These tests start the full control-plane HTTP server on a free port and
//! interact with it only through an independent TCP client.

use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::{DispatcherMode, EngineBuilder};
use cheetah_media_module::ZlmMediaModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn build_config() -> Arc<ConfigStore> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(serde_json::json!({
        "media": {
            "zlm": { "auth": { "mode": "none" } }
        }
    }));
    config
}

fn make_engine(config: Arc<ConfigStore>) -> Arc<cheetah_engine::Engine> {
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_dispatcher_mode(DispatcherMode::PerStream)
        .with_config_schema_registry(config)
        .register_module_factory(Arc::new(ZlmMediaModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn control_state(
    engine: &cheetah_engine::Engine,
    config: Arc<ConfigStore>,
) -> cheetah_control::ControlState {
    cheetah_control::ControlState {
        health: engine.health_api(),
        metrics: engine.metrics_api(),
        modules: engine.module_manager_api(),
        streams: engine.stream_manager_api(),
        tasks: engine.task_system_api(),
        config: config.clone(),
        config_apply: engine.config_apply_api(),
        config_schemas: config,
        service_registry: engine.service_registry_api(),
    }
}

async fn start_server() -> (SocketAddr, Arc<cheetah_engine::Engine>) {
    let config = build_config();
    let engine = make_engine(config.clone());
    engine.start().await.expect("engine start");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let app = cheetah_control::router(control_state(&engine, config));
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    (addr, engine)
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to server");
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path,
        addr.port()
    );
    stream.write_all(request.as_bytes()).await.expect("write");

    let mut buf = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let n = stream.read(&mut temp).await.expect("read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&temp[..n]);
    }

    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if let Some(idx) = text.find("\r\n\r\n") {
        let headers = &text[..idx];
        let body_start = idx + 4;
        let body = if let Some(cl) = headers.lines().find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(|v| v.to_string())
        }) {
            if let Ok(len) = cl.trim().parse::<usize>() {
                text[body_start..body_start + len.min(text.len() - body_start)].to_string()
            } else {
                text[body_start..].to_string()
            }
        } else {
            text[body_start..].to_string()
        };
        return (status, body);
    }

    (status, text)
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_l3_get_api_list() {
    let (addr, _engine) = start_server().await;

    let (status, body) = http_get(addr, "/index/api/getApiList").await;
    assert_eq!(status, 200, "body: {body}");

    let value: Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(value["code"], 0);
    let apis = value["data"]["apis"].as_array().expect("apis array");
    assert!(!apis.is_empty(), "api list should not be empty");
    let paths: Vec<&str> = apis.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(paths.contains(&"/api/getApiList"));
    assert!(paths.contains(&"/api/getMediaList"));
}

#[tokio::test(flavor = "current_thread")]
async fn zlm_l3_version_and_empty_media_list() {
    let (addr, _engine) = start_server().await;

    let (status, body) = http_get(addr, "/index/api/version").await;
    assert_eq!(status, 200, "body: {body}");
    let value: Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(value["code"], 0);
    assert!(value["data"]["branchName"].is_string());

    let (status, body) = http_get(
        addr,
        "/index/api/getMediaList?vhost=__defaultVhost__&app=live&stream=none",
    )
    .await;
    assert_eq!(status, 200, "body: {body}");
    let value: Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(value["code"], 0);
    assert!(value["data"].as_array().unwrap().is_empty());
}
