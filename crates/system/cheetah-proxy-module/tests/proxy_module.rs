use std::sync::Arc;

use async_trait::async_trait;
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest, RetryPolicy,
};
use cheetah_media_api::ids::{AppName, MediaKey, ProxyId, StreamName, VhostName};
use cheetah_media_api::model::{OutputPolicy, ProxyKind, TranscodePolicy};
use cheetah_media_api::port::{MediaFacade, MediaRequestContext, ProxyApi};
use cheetah_media_api::MediaCapability;
use cheetah_proxy_module::ProxyModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{FfmpegApi, FfmpegJobHandle, FfmpegJobSpec, FfmpegJobStatus, SdkError};
use serde_json::json;

fn make_key() -> MediaKey {
    MediaKey {
        vhost: VhostName("__defaultVhost__".to_string()),
        app: AppName("live".to_string()),
        stream: StreamName("test".to_string()),
        schema: None,
    }
}

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

fn pull_request(url: &str) -> PullProxyRequest {
    PullProxyRequest {
        source_url: url.to_string(),
        destination: make_key(),
        retry_policy: RetryPolicy::default(),
        heartbeat_ms: None,
        timeout_ms: 10_000,
        transcode_policy: TranscodePolicy::default(),
        output_policy: OutputPolicy::default(),
        record_policy: None,
    }
}

fn push_request(url: &str) -> PushProxyRequest {
    PushProxyRequest {
        source_media_key: make_key(),
        destination_url: url.to_string(),
        protocol: "rtmp".to_string(),
        retry_policy: RetryPolicy::default(),
        protocol_options: Default::default(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn proxy_provider_is_registered_and_advertises_capability() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let caps = engine.media_facade().capabilities();
    assert!(caps.has(cheetah_media_api::MediaCapability::Proxy));
}

#[tokio::test(flavor = "current_thread")]
async fn create_and_list_pull_proxy() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let info = facade
        .create_pull_proxy(&ctx, pull_request("http://example.com/live.flv"))
        .await
        .expect("create pull proxy");
    assert_eq!(info.kind, ProxyKind::Pull);

    let page = facade
        .list_pull_proxies(&ctx, ProxyQuery::default())
        .await
        .expect("list pull proxies");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].proxy_id, info.proxy_id);
}

#[tokio::test(flavor = "current_thread")]
async fn create_and_list_push_proxy() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let info = facade
        .create_push_proxy(&ctx, push_request("rtmp://example.com/live/push"))
        .await
        .expect("create push proxy");
    assert_eq!(info.kind, ProxyKind::Push);

    let page = facade
        .list_push_proxies(&ctx, ProxyQuery::default())
        .await
        .expect("list push proxies");
    assert_eq!(page.total, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn delete_proxy_removes_it_from_list() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let info = facade
        .create_pull_proxy(&ctx, pull_request("http://example.com/live.flv"))
        .await
        .expect("create");

    facade
        .delete_pull_proxy(&ctx, &info.proxy_id)
        .await
        .expect("delete");

    let page = facade
        .list_pull_proxies(&ctx, ProxyQuery::default())
        .await
        .expect("list");
    assert_eq!(page.total, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn delete_wrong_kind_returns_not_found() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let info = facade
        .create_pull_proxy(&ctx, pull_request("http://example.com/live.flv"))
        .await
        .expect("create pull proxy");

    let result = facade.delete_push_proxy(&ctx, &info.proxy_id).await;
    assert!(result.is_err(), "deleting a pull proxy as push should fail");

    let page = facade
        .list_pull_proxies(&ctx, ProxyQuery::default())
        .await
        .expect("list");
    assert_eq!(page.total, 1, "pull proxy should still exist");
}

#[tokio::test(flavor = "current_thread")]
async fn get_proxy_by_id() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let info = facade
        .create_pull_proxy(&ctx, pull_request("http://example.com/live.flv"))
        .await
        .expect("create");

    let got = facade
        .get_pull_proxy(&ctx, &info.proxy_id)
        .await
        .expect("get");
    assert_eq!(got.proxy_id, info.proxy_id);
}

#[tokio::test(flavor = "current_thread")]
async fn get_missing_proxy_returns_not_found() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let result = facade
        .get_pull_proxy(&ctx, &ProxyId("no-such-proxy".to_string()))
        .await;
    assert!(result.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn internal_proxy_targets_are_rejected() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let forbidden = [
        "http://127.0.0.1:8891/live.flv",
        "http://localhost/live.flv",
        "http://[::1]/live.flv",
        "http://[::ffff:127.0.0.1]/live.flv",
        "http://169.254.169.254/latest/meta-data",
        "http://10.0.0.1/live.flv",
        "http://192.168.1.1/live.flv",
        "http://172.16.0.1/live.flv",
        "http://[fd00::1]:8080/live.flv",
        "http://[fe80::1]/live.flv",
        "http://[::1]/live.flv",
    ];
    for url in forbidden {
        let mut req = pull_request(url);
        req.source_url = url.to_string();
        let result = facade.create_pull_proxy(&ctx, req).await;
        assert!(
            result.is_err(),
            "should reject internal target {url}: {result:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn public_proxy_targets_are_accepted() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    for url in [
        "http://example.com/live.flv",
        "rtmp://example.com/live/push",
    ] {
        let result = facade.create_pull_proxy(&ctx, pull_request(url)).await;
        assert!(
            result.is_ok(),
            "should accept public target {url}: {result:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_url_scheme_is_rejected() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let mut req = pull_request("ftp://example.com/live.flv");
    req.source_url = "ftp://example.com/live.flv".to_string();
    let result = facade.create_pull_proxy(&ctx, req).await;
    assert!(result.is_err(), "ftp scheme should be rejected");
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_allowlist_fails_engine_start() {
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str("modules:\n  proxy:\n    ssrf_allowlist_cidrs:\n      - not-a-cidr\n")
        .expect("load proxy config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .build()
        .expect("engine build");

    let err = engine.start().await.expect_err("engine start should fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("invalid ssrf_allowlist_cidrs") || msg.contains("InvalidArgument"),
        "expected invalid allowlist error, got {msg}"
    );
}

/// FFmpeg API stub that reports itself as unavailable.
struct NoopFfmpegApi;

#[async_trait]
impl FfmpegApi for NoopFfmpegApi {
    async fn submit(
        &self,
        _job_id: String,
        _spec: FfmpegJobSpec,
    ) -> Result<FfmpegJobHandle, SdkError> {
        unimplemented!("noop ffmpeg submit")
    }

    async fn get(&self, _job_id: &str) -> Result<FfmpegJobStatus, SdkError> {
        unimplemented!("noop ffmpeg get")
    }

    async fn list(&self) -> Vec<FfmpegJobStatus> {
        unimplemented!("noop ffmpeg list")
    }

    async fn wait(&self, _job_id: &str) -> Result<FfmpegJobStatus, SdkError> {
        unimplemented!("noop ffmpeg wait")
    }

    async fn cancel(&self, _job_id: &str) -> Result<(), SdkError> {
        unimplemented!("noop ffmpeg cancel")
    }

    async fn remove(&self, _job_id: &str) -> Result<(), SdkError> {
        unimplemented!("noop ffmpeg remove")
    }

    fn is_available(&self) -> bool {
        false
    }
}

fn make_engine_with_ffmpeg(ffmpeg: Arc<dyn FfmpegApi>) -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(json!({}));
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .with_ffmpeg_api(ffmpeg)
        .build()
        .expect("engine build");
    Arc::new(engine)
}

#[tokio::test(flavor = "current_thread")]
async fn proxy_capability_includes_ffmpeg_when_executor_available() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let caps = engine.media_facade().capabilities();
    let proxy_ops = caps
        .operations
        .get(&MediaCapability::Proxy)
        .expect("proxy operations should be advertised");
    assert!(proxy_ops.contains(&"create_pull".to_string()));
    assert!(proxy_ops.contains(&"delete_pull".to_string()));
    assert!(proxy_ops.contains(&"create_ffmpeg".to_string()));
    assert!(proxy_ops.contains(&"delete_ffmpeg".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn proxy_capability_omits_ffmpeg_when_executor_unavailable() {
    let engine = make_engine_with_ffmpeg(Arc::new(NoopFfmpegApi));
    engine.start().await.expect("engine start");

    let caps = engine.media_facade().capabilities();
    let proxy_ops = caps
        .operations
        .get(&MediaCapability::Proxy)
        .expect("proxy operations should be advertised");
    assert!(proxy_ops.contains(&"create_pull".to_string()));
    assert!(!proxy_ops.contains(&"create_ffmpeg".to_string()));
    assert!(!proxy_ops.contains(&"delete_ffmpeg".to_string()));

    let facade = engine.media_facade();
    let ctx = MediaRequestContext::default();
    let req = FfmpegProxyRequest {
        source_url: "rtmp://example.com/live/source".to_string(),
        destination: make_key(),
        input_options: Vec::new(),
        output_options: Vec::new(),
        transcode_policy: TranscodePolicy::default(),
        output_policy: OutputPolicy::default(),
    };
    let result = facade.create_ffmpeg_proxy(&ctx, req).await;
    assert!(
        result.is_err(),
        "create_ffmpeg_proxy should fail when ffmpeg executor is unavailable"
    );
}
