#![cfg(feature = "media-processing-caption")]

use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::{
    track::{CodecExtradata, MediaKind, TrackId, TrackInfo, TrackReadiness},
    CodecId, Rational32,
};
use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::auth::{
    MediaResourceGrant, MediaResourceSelector, MediaScope, Pattern, Principal,
};
use cheetah_media_api::error::MediaErrorCode;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{
    CaptionConfig, CreateProcessingJob, ProcessingJobSpec, ProcessingJobState,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleId, PublisherOptions, StreamKey};

#[cfg(feature = "media-processing-caption")]
fn ctx(principal: Option<Principal>) -> MediaRequestContext {
    MediaRequestContext {
        request_id: cheetah_media_api::ids::RequestId("test".to_string()),
        correlation_id: None,
        principal,
        source_adapter: "test".to_string(),
        trace_context: None,
        deadline: None,
        idempotency_key: None,
        mutation: None,
    }
}

#[cfg(feature = "media-processing-caption")]
fn principal(name: &str, scopes: Vec<MediaScope>, grants: Vec<MediaResourceGrant>) -> Principal {
    Principal {
        identity: name.to_string(),
        scopes,
        resource_grants: grants,
    }
}

#[cfg(feature = "media-processing-caption")]
struct PublisherGuard {
    _sink: Box<dyn cheetah_sdk::PublisherSink>,
    _lease: cheetah_sdk::PublishLease,
}

#[cfg(feature = "media-processing-caption")]
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

#[cfg(feature = "media-processing-caption")]
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

#[cfg(feature = "media-processing-caption")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn processing_job_rejects_reserved_derived_namespace() {
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

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let request = CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::CaptionExtract {
            source: MediaKey::with_default_vhost("app", "src", None).unwrap(),
            target: MediaKey::with_default_vhost("__cheetah_derived", "out", None).unwrap(),
            caption: CaptionConfig {
                source_streams: vec![],
                languages: vec![],
            },
        },
    };

    let err = processing
        .create_job(&ctx(None), request)
        .await
        .expect_err("reserved namespace target should be rejected");
    assert_eq!(err.code, MediaErrorCode::InvalidArgument);
    assert!(err.to_string().contains("__cheetah_derived"));

    engine.stop().await;
}

#[cfg(feature = "media-processing-caption")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn processing_job_owner_isolation_filters_crud() {
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

    let source_key = StreamKey::new("app", "src");
    let _guard = start_source_publisher(&engine, source_key.clone()).await;
    wait_for_stream(&engine, source_key).await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let alice = principal(
        "alice",
        vec![MediaScope::MediaRead],
        vec![MediaResourceGrant {
            selector: MediaResourceSelector {
                vhost: Pattern::Wildcard,
                app: Pattern::Exact("app".to_string()),
                stream: Pattern::Exact("src".to_string()),
            },
            scopes: vec![MediaScope::MediaControl],
        }],
    );
    let bob = principal("bob", vec![MediaScope::MediaRead], vec![]);

    let request = CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::CaptionExtract {
            source: MediaKey::with_default_vhost("app", "src", None).unwrap(),
            target: MediaKey::with_default_vhost("app", "out", None).unwrap(),
            caption: CaptionConfig {
                source_streams: vec![],
                languages: vec![],
            },
        },
    };

    let job = processing
        .create_job(&ctx(Some(alice.clone())), request.clone())
        .await
        .expect("create caption job");
    assert_eq!(job.owner, Some("alice".to_string()));
    assert_eq!(job.state, ProcessingJobState::Running);

    // Owner can read.
    let got = processing
        .get_job(&ctx(Some(alice.clone())), &job.job_id)
        .await
        .expect("alice get_job");
    assert_eq!(got.owner, Some("alice".to_string()));

    // Non-owner cannot read.
    let err = processing
        .get_job(&ctx(Some(bob.clone())), &job.job_id)
        .await
        .expect_err("bob get_job should fail");
    assert_eq!(err.code, MediaErrorCode::PermissionDenied);

    // Non-owner list is empty.
    let list = processing
        .list_jobs(&ctx(Some(bob.clone())), Default::default())
        .await
        .expect("bob list_jobs");
    assert!(list.items.is_empty());

    // Owner list contains the job.
    let list = processing
        .list_jobs(&ctx(Some(alice.clone())), Default::default())
        .await
        .expect("alice list_jobs");
    assert_eq!(list.items.len(), 1);

    // Non-owner cannot stop.
    let err = processing
        .stop_job(&ctx(Some(bob.clone())), &job.job_id)
        .await
        .expect_err("bob stop_job should fail");
    assert_eq!(err.code, MediaErrorCode::PermissionDenied);

    // Owner can stop and delete.
    let stopped = processing
        .stop_job(&ctx(Some(alice.clone())), &job.job_id)
        .await
        .expect("alice stop_job");
    assert_eq!(stopped.state, ProcessingJobState::Stopped);

    processing
        .delete_job(&ctx(Some(alice.clone())), &job.job_id)
        .await
        .expect("alice delete_job");

    engine.stop().await;
}

#[cfg(feature = "media-processing-caption")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn processing_job_admin_can_access_any_job() {
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

    let source_key = StreamKey::new("app", "src");
    let _guard = start_source_publisher(&engine, source_key.clone()).await;
    wait_for_stream(&engine, source_key).await;

    let processing = engine
        .media_services()
        .processing()
        .expect("processing provider registered");

    let alice = principal("alice", vec![MediaScope::MediaRead], vec![]);
    let admin = principal("admin", vec![MediaScope::ServerAdmin], vec![]);

    let request = CreateProcessingJob {
        idempotency_key: None,
        deadline_ms: None,
        spec: ProcessingJobSpec::CaptionExtract {
            source: MediaKey::with_default_vhost("app", "src", None).unwrap(),
            target: MediaKey::with_default_vhost("app", "out", None).unwrap(),
            caption: CaptionConfig {
                source_streams: vec![],
                languages: vec![],
            },
        },
    };

    let job = processing
        .create_job(&ctx(Some(alice.clone())), request)
        .await
        .expect("create caption job");

    processing
        .get_job(&ctx(Some(admin.clone())), &job.job_id)
        .await
        .expect("admin can get_job");

    processing
        .stop_job(&ctx(Some(admin.clone())), &job.job_id)
        .await
        .expect("admin can stop_job");

    processing
        .delete_job(&ctx(Some(admin.clone())), &job.job_id)
        .await
        .expect("admin can delete_job");

    engine.stop().await;
}
