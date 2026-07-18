#![allow(unused_imports)]

use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{
    AudioCodec, AudioMixInput, AudioTarget, CreateProcessingJob, ProcessingJobId, ProcessingJobSpec,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::ModuleId;

#[cfg(feature = "media-processing-cpu")]
#[tokio::test]
async fn preflight_includes_audio_mix_when_cpu_feature_enabled() {
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

    let report = processing
        .preflight(&MediaRequestContext::default())
        .await
        .expect("preflight");

    assert!(
        report.operations.contains(&"audio_mix".to_string()),
        "audio_mix should be advertised when media-processing-cpu is enabled: {:?}",
        report.operations
    );
}

#[cfg(all(
    feature = "media-processing-caption",
    not(feature = "media-processing-cpu")
))]
#[tokio::test]
async fn audio_mix_create_job_unsupported_without_cpu_feature() {
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
        spec: ProcessingJobSpec::AudioMix {
            inputs: vec![
                AudioMixInput {
                    source: MediaKey::with_default_vhost("app", "src1", None).unwrap(),
                    gain_db: None,
                },
                AudioMixInput {
                    source: MediaKey::with_default_vhost("app", "src2", None).unwrap(),
                    gain_db: None,
                },
            ],
            target: MediaKey::with_default_vhost("app", "out", None).unwrap(),
            output: AudioTarget {
                codec: AudioCodec::Aac,
                sample_rate: Some(8_000),
                channels: Some(1),
                bit_rate: Some(64_000),
            },
        },
    };

    let result = processing
        .create_job(&MediaRequestContext::default(), request)
        .await;
    assert!(
        result.is_err(),
        "AudioMix should be unsupported without media-processing-cpu"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not compiled") || err.to_string().contains("unsupported"),
        "unexpected error: {err}"
    );
}
