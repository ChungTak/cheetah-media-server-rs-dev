#![allow(unused_imports)]

use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_api::ids::MediaKey;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::processing::{
    CreateProcessingJob, MosaicCell, MosaicLayout, ProcessingJobSpec, VideoMosaicInput,
};
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::ModuleId;

#[cfg(feature = "media-processing-cpu")]
#[tokio::test]
async fn preflight_includes_video_mosaic_when_cpu_feature_enabled() {
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
        report.operations.contains(&"video_mosaic".to_string()),
        "video_mosaic should be advertised when media-processing-cpu is enabled: {:?}",
        report.operations
    );
}

#[cfg(all(
    feature = "media-processing-caption",
    not(feature = "media-processing-cpu")
))]
#[tokio::test]
async fn video_mosaic_create_job_unsupported_without_cpu_feature() {
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
        spec: ProcessingJobSpec::VideoMosaic {
            inputs: vec![
                VideoMosaicInput {
                    source: MediaKey::with_default_vhost("app", "src1", None).unwrap(),
                    cell: MosaicCell {
                        column: 0,
                        row: 0,
                        z_order: 0,
                    },
                    audio_gain_db: None,
                    fit: None,
                    label: None,
                },
                VideoMosaicInput {
                    source: MediaKey::with_default_vhost("app", "src2", None).unwrap(),
                    cell: MosaicCell {
                        column: 1,
                        row: 0,
                        z_order: 0,
                    },
                    audio_gain_db: None,
                    fit: None,
                    label: None,
                },
            ],
            target: MediaKey::with_default_vhost("app", "out", None).unwrap(),
            layout: MosaicLayout {
                columns: 2,
                rows: 1,
                cell_width: 320,
                cell_height: 240,
                background: None,
                frame_rate_num: None,
                frame_rate_den: None,
                bit_rate: None,
                gop_size: None,
                video_codec: None,
                fit: None,
            },
            audio_mix: None,
            overlays: vec![],
        },
    };

    let result = processing
        .create_job(&MediaRequestContext::default(), request)
        .await;
    assert!(
        result.is_err(),
        "VideoMosaic should be unsupported without media-processing-cpu"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not compiled") || err.to_string().contains("unsupported"),
        "unexpected error: {err}"
    );
}
