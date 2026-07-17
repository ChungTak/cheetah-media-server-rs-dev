use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::ModuleId;

#[tokio::test]
async fn module_registers_image_process_only_when_feature_enabled() {
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

    let has_image_process = engine.media_services().image_process().is_some();

    #[cfg(feature = "media-processing-image")]
    assert!(
        has_image_process,
        "image processing provider should be registered"
    );

    #[cfg(not(feature = "media-processing-image"))]
    assert!(
        !has_image_process,
        "image processing provider should not be registered"
    );
}
