#![cfg(feature = "media-processing-caption")]

use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::{
    track::{CodecExtradata, MediaKind, TrackId, TrackInfo, TrackReadiness},
    CodecId, Rational32,
};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{CaptionConfig, CreateProcessingJob, ProcessingJobSpec};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleId, PublisherOptions, StreamKey};

fn ctx(idempotency_key: Option<&str>, deadline: Option<i64>) -> MediaRequestContext {
    MediaRequestContext {
        request_id: cheetah_media_api::ids::RequestId("test".to_string()),
        correlation_id: None,
        principal: None,
        source_adapter: "test".to_string(),
        trace_context: None,
        deadline,
        idempotency_key: idempotency_key.map(|s| s.to_string()),
    }
}

fn caption_request(source: &str, target: &str) -> CreateProcessingJob {
    CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::CaptionExtract {
            source: MediaKey::with_default_vhost("app", source, None).unwrap(),
            target: MediaKey::with_default_vhost("app", target, None).unwrap(),
            caption: CaptionConfig {
                source_streams: vec![],
                languages: vec![],
            },
        },
    }
}

async fn build_engine() -> cheetah_engine::Engine {
    let config = Arc::new(ConfigStore::new());
    config.set_global_default(serde_json::json!({}));
    config.register_module_default(
        ModuleId::new("media-processing"),
        MediaProcessingModuleConfig::default_json(),
    );
    let runtime = Arc::new(TokioRuntime::new());
    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .register_module_factory(Arc::new(MediaProcessingModuleFactory))
        .build()
        .expect("engine build");
    engine.start().await.expect("engine start");
    engine
}

struct PublisherGuard {
    _sink: Box<dyn cheetah_sdk::PublisherSink>,
    _lease: cheetah_sdk::PublishLease,
}

async fn start_source_publisher(engine: &cheetah_engine::Engine, key: StreamKey) -> PublisherGuard {
    let publisher_api = engine.publisher_api();
    let (lease, sink) = publisher_api
        .acquire_publisher(key.clone(), PublisherOptions::default())
        .await
        .expect("acquire source publisher");

    let mut track = TrackInfo::new(TrackId(0), MediaKind::Video, CodecId::H264, 90_000);
    track.extradata = CodecExtradata::H264 {
        sps: vec![bytes::Bytes::from_static(&[0; 10])],
        pps: vec![bytes::Bytes::from_static(&[0; 6])],
        avcc: None,
    };
    track.readiness = TrackReadiness::Ready;
    track.width = Some(160);
    track.height = Some(120);
    track.fps = Some(Rational32::new(30, 1));
    sink.update_tracks(vec![track])
        .expect("update source track");

    PublisherGuard {
        _sink: sink,
        _lease: lease,
    }
}

async fn wait_for_stream(engine: &cheetah_engine::Engine, key: StreamKey) {
    let sm = engine.stream_manager_api();
    for _ in 0..40 {
        if let Ok(Some(snapshot)) = sm.get_stream(&key).await {
            if snapshot.publisher_active && !snapshot.tracks.is_empty() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stream {key} did not become active in time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_deadline_rejected_before_allocation() {
    let engine = build_engine().await;
    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let err = processing
        .create_job(&ctx(None, Some(0)), caption_request("src", "out"))
        .await
        .expect_err("expired deadline should be rejected");
    assert_eq!(err.code, MediaErrorCode::Timeout);
    assert!(err.to_string().contains("deadline"));

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_create_returns_same_job() {
    let engine = build_engine().await;
    let source_key = StreamKey::new("app", "src");
    let _guard = start_source_publisher(&engine, source_key.clone()).await;
    wait_for_stream(&engine, source_key).await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let key = "idem-1";
    let req = CreateProcessingJob {
        idempotency_key: Some(key.to_string()),
        ..caption_request("src", "out")
    };

    let job1 = processing
        .create_job(&ctx(Some(key), None), req.clone())
        .await
        .expect("first idempotent create");

    let job2 = processing
        .create_job(&ctx(Some(key), None), req)
        .await
        .expect("second idempotent create");

    assert_eq!(job1.job_id, job2.job_id);
    assert_eq!(job1.owner, job2.owner);

    engine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_create_conflict_on_different_spec() {
    let engine = build_engine().await;
    let source_key = StreamKey::new("app", "src2");
    let _guard = start_source_publisher(&engine, source_key.clone()).await;
    wait_for_stream(&engine, source_key).await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let key = "idem-conflict";
    let req1 = CreateProcessingJob {
        idempotency_key: Some(key.to_string()),
        ..caption_request("src2", "out2")
    };
    processing
        .create_job(&ctx(Some(key), None), req1)
        .await
        .expect("first create");

    let mut req2 = caption_request("src2", "out3");
    req2.idempotency_key = Some(key.to_string());

    let err = processing
        .create_job(&ctx(Some(key), None), req2)
        .await
        .expect_err("same key with different spec should conflict");
    assert_eq!(err.code, MediaErrorCode::Conflict);

    engine.stop().await;
}
