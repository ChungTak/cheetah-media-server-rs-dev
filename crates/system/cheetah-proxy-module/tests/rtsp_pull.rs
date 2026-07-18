//! Real RTSP pull success-path integration tests for the proxy module.
//!
//! These tests start a local RTSP source server, create a pull proxy through the
//! `ProxyApi` facade, and verify that:
//! - the proxy reaches `Connected` state,
//! - at least one frame is delivered to the destination engine stream,
//! - deleting the proxy cancels the background task and removes the entry.
//!
//! 代理模块真实 RTSP 拉流成功路径集成测试。

#[cfg(feature = "rtsp")]
mod common;

#[cfg(feature = "rtsp")]
use std::sync::Arc;
#[cfg(feature = "rtsp")]
use std::time::Duration;

#[cfg(feature = "rtsp")]
use cheetah_config::ConfigStore;
#[cfg(feature = "rtsp")]
use cheetah_engine::EngineBuilder;
#[cfg(feature = "rtsp")]
use cheetah_engine::EngineMediaFacade;
#[cfg(feature = "rtsp")]
use cheetah_media_api::command::{PullProxyRequest, RetryPolicy};
#[cfg(feature = "rtsp")]
use cheetah_media_api::ids::{AppName, MediaKey, ProxyId, StreamName, VhostName};
#[cfg(feature = "rtsp")]
use cheetah_media_api::model::{OutputPolicy, ProxyState};
#[cfg(feature = "rtsp")]
use cheetah_media_api::port::{MediaRequestContext, ProxyApi};
#[cfg(feature = "rtsp")]
use cheetah_media_api::processing::ProcessingPolicy;
#[cfg(feature = "rtsp")]
use cheetah_proxy_module::ProxyModuleFactory;
#[cfg(feature = "rtsp")]
use cheetah_runtime_tokio::TokioRuntime;
#[cfg(feature = "rtsp")]
use cheetah_sdk::{StreamKey, SubscriberOptions};
#[cfg(feature = "rtsp")]
use common::*;
#[cfg(feature = "rtsp")]
use serde_json::json;
#[cfg(feature = "rtsp")]
use tokio::net::TcpListener;
#[cfg(feature = "rtsp")]
use tokio::time::{sleep, timeout};

#[cfg(feature = "rtsp")]
fn make_key() -> MediaKey {
    MediaKey {
        vhost: VhostName("__defaultVhost__".to_string()),
        app: AppName("live".to_string()),
        stream: StreamName("test".to_string()),
        schema: None,
    }
}

#[cfg(feature = "rtsp")]
fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(
            "modules:\n  proxy:\n    ssrf_allowlist_cidrs:\n      - 127.0.0.0/8\n    retry_max: 0\n    connect_timeout_ms: 5000\n",
        )
        .expect("load proxy config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

#[cfg(feature = "rtsp")]
fn pull_request(source_url: &str, destination: MediaKey) -> PullProxyRequest {
    PullProxyRequest {
        source_url: source_url.to_string(),
        destination,
        retry_policy: RetryPolicy::default(),
        heartbeat_ms: None,
        timeout_ms: 10_000,
        processing_policy: ProcessingPolicy::default(),
        output_policy: OutputPolicy::default(),
        record_policy: None,
    }
}

#[cfg(feature = "rtsp")]
async fn wait_for_proxy_state(
    facade: &Arc<EngineMediaFacade>,
    proxy_id: &ProxyId,
    state: ProxyState,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let info = facade
            .get_pull_proxy(&MediaRequestContext::default(), proxy_id)
            .await
            .expect("get proxy");
        if info.state == state {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for proxy state {state:?}; got {:?}",
                info.state
            );
        }
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "rtsp")]
async fn rtsp_pull_proxy_connects_and_delivers_frame() {
    let source_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind source listener");
    let source_addr = source_listener.local_addr().expect("source addr");
    let source_uri = format!("rtsp://{source_addr}/live/pull-source");
    let source_uri_for_server = source_uri.clone();

    let rtp_payload = h264_rtp_packet();
    let source_server = tokio::spawn(async move {
        let (socket, _) = source_listener.accept().await.expect("accept pull source");
        run_interleaved_rtsp_source(socket, source_uri_for_server, Some(rtp_payload)).await;
    });

    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let destination = make_key();
    let info = facade
        .create_pull_proxy(
            &MediaRequestContext::default(),
            pull_request(&source_uri, destination.clone()),
        )
        .await
        .expect("create pull proxy");
    assert_eq!(info.state, ProxyState::Created);

    wait_for_proxy_state(&facade, &info.proxy_id, ProxyState::Connected).await;

    // Verify a frame actually arrived at the destination engine stream.
    let target_stream = StreamKey::new("live", "test");
    let mut subscriber = engine
        .subscriber_api()
        .subscribe(target_stream, SubscriberOptions::default())
        .await
        .expect("subscribe to destination");

    let frame = timeout(Duration::from_secs(5), subscriber.recv())
        .await
        .expect("recv frame timeout")
        .expect("recv frame result")
        .expect("frame should exist");
    assert!(
        !frame.payload.is_empty(),
        "pulled frame payload should not be empty"
    );

    facade
        .delete_pull_proxy(&MediaRequestContext::default(), &info.proxy_id)
        .await
        .expect("delete pull proxy");

    let list = facade
        .list_pull_proxies(&MediaRequestContext::default(), Default::default())
        .await
        .expect("list pull proxies");
    assert_eq!(list.total, 0, "proxy should be removed after delete");

    engine.stop().await;
    let _ = source_server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "rtsp")]
async fn rtsp_pull_proxy_without_allowlist_is_rejected() {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .build()
        .expect("engine build");
    let engine = Arc::new(engine);
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let result = facade
        .create_pull_proxy(
            &MediaRequestContext::default(),
            pull_request("rtsp://127.0.0.1:554/live/stream", make_key()),
        )
        .await;
    assert!(
        result.is_err(),
        "loopback RTSP source should be rejected without allowlist"
    );

    engine.stop().await;
}
