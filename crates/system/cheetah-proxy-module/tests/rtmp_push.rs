#[cfg(feature = "rtmp")]
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(feature = "rtmp")]
use std::time::Duration;

#[cfg(feature = "rtmp")]
use bytes::Bytes;
#[cfg(feature = "rtmp")]
use cheetah_codec::{
    AVFrame, CodecExtradata, CodecId, FrameFlags, FrameFormat, MediaKind, Timebase, TrackId,
    TrackInfo, TrackReadiness,
};
use cheetah_config::ConfigStore;
#[cfg(feature = "rtmp")]
use cheetah_engine::{EngineBuilder, EngineMediaFacade};
#[cfg(feature = "rtmp")]
use cheetah_media_api::command::{PushProxyRequest, RetryPolicy};
#[cfg(feature = "rtmp")]
use cheetah_media_api::ids::MediaKey;
#[cfg(feature = "rtmp")]
use cheetah_media_api::model::ProxyState;
#[cfg(feature = "rtmp")]
use cheetah_media_api::port::{MediaRequestContext, ProxyApi};
#[cfg(feature = "rtmp")]
use cheetah_proxy_module::ProxyModuleFactory;
#[cfg(feature = "rtmp")]
use cheetah_rtmp_module::RtmpModuleFactory;
#[cfg(feature = "rtmp")]
use cheetah_runtime_tokio::TokioRuntime;
#[cfg(feature = "rtmp")]
use cheetah_sdk::{PublisherOptions, StreamKey, SubscriberOptions};
#[cfg(feature = "rtmp")]
use tokio::time::{sleep, timeout};

#[cfg(feature = "rtmp")]
fn h264_track() -> TrackInfo {
    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![Bytes::from_static(&[0x67, 0x42, 0x00, 0x1f])],
        pps: vec![Bytes::from_static(&[0x68, 0xce, 0x3c, 0x80])],
        avcc: None,
    };
    track.readiness = TrackReadiness::Ready;
    track
}

#[cfg(feature = "rtmp")]
fn h264_frame() -> AVFrame {
    let payload = Bytes::from_static(&[
        0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x2f, 0xff, 0xff, 0x00, 0x04, 0x00, 0x00,
        0x04, 0x01,
    ]);
    let mut frame = AVFrame::new(
        TrackId(0),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 1_000),
        payload,
    );
    frame.flags = FrameFlags::KEY;
    frame
}

#[cfg(feature = "rtmp")]
fn make_engine() -> Arc<cheetah_engine::Engine> {
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(
            "modules:\n  rtmp:\n    enabled: true\n    listen: \"127.0.0.1:0\"\n  proxy:\n    ssrf_allowlist_cidrs:\n      - 127.0.0.0/8\n    retry_max: 3\n    retry_delay_ms: 500\n    connect_timeout_ms: 10000\n",
        )
        .expect("load config");
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config, runtime)
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .register_module_factory(Arc::new(ProxyModuleFactory))
        .build()
        .expect("engine build");
    Arc::new(engine)
}

#[cfg(feature = "rtmp")]
fn push_request(source_media_key: MediaKey, destination_url: String) -> PushProxyRequest {
    PushProxyRequest {
        source_media_key,
        destination_url,
        protocol: "rtmp".to_string(),
        retry_policy: RetryPolicy::default(),
        protocol_options: Default::default(),
    }
}

#[cfg(feature = "rtmp")]
fn parse_endpoint_addr(endpoint: &str) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let Some((_scheme, rest)) = endpoint.split_once("://") else {
        return Err(format!("invalid endpoint: {endpoint}").into());
    };
    Ok(rest.parse::<SocketAddr>()?)
}

#[cfg(feature = "rtmp")]
async fn wait_for_proxy_state(
    facade: &Arc<EngineMediaFacade>,
    proxy_id: &cheetah_media_api::ids::ProxyId,
    state: ProxyState,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let info = facade
            .get_push_proxy(&MediaRequestContext::default(), proxy_id)
            .await
            .expect("get proxy");
        if info.state == state {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for proxy state {state:?}; got {:?} error {:?}",
                info.state, info.last_error
            );
        }
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "rtmp")]
async fn rtmp_push_proxy_connects_and_delivers_frame() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let rtmp_endpoint = engine
        .service_registry_api()
        .get("rtmp")
        .expect("rtmp service registered")
        .endpoint;
    let rtmp_addr = parse_endpoint_addr(&rtmp_endpoint).expect("rtmp addr");
    let destination_url = format!("rtmp://{rtmp_addr}/live/test");

    let source_key = MediaKey::with_default_vhost("live", "source", None).expect("source key");
    let target_stream = StreamKey::new("live", "test");
    let source_stream = StreamKey::new("live", "source");

    // Pre-register the source stream with tracks so the proxy subscriber can
    // attach before any frame is produced.
    let (_lease, publisher) = engine
        .publisher_api()
        .acquire_publisher(source_stream, PublisherOptions::default())
        .await
        .expect("acquire source publisher");
    publisher
        .update_tracks(vec![h264_track()])
        .expect("update tracks");

    let facade = engine.media_facade();
    let info = facade
        .create_push_proxy(
            &MediaRequestContext::default(),
            push_request(source_key, destination_url),
        )
        .await
        .expect("create push proxy");
    assert_eq!(info.state, ProxyState::Created);

    wait_for_proxy_state(&facade, &info.proxy_id, ProxyState::Connected).await;

    publisher
        .push_frame(Arc::new(h264_frame()))
        .expect("push frame");

    let mut subscriber = engine
        .subscriber_api()
        .subscribe(target_stream, SubscriberOptions::default())
        .await
        .expect("subscribe target");
    let frame = timeout(Duration::from_secs(5), subscriber.recv())
        .await
        .expect("recv timeout")
        .expect("recv result")
        .expect("frame should exist");
    assert!(
        !frame.payload.is_empty(),
        "pushed frame payload should not be empty"
    );

    facade
        .delete_push_proxy(&MediaRequestContext::default(), &info.proxy_id)
        .await
        .expect("delete push proxy");

    let list = facade
        .list_push_proxies(&MediaRequestContext::default(), Default::default())
        .await
        .expect("list push proxies");
    assert_eq!(list.total, 0, "proxy should be removed after delete");

    engine.stop().await;
}
