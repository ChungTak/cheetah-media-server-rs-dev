#![cfg(feature = "media-processing-cpu")]

use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ConfigApplyApi, ConfigEffect, ModuleId};
use serde_json::json;

fn make_engine_with_config(config: Arc<ConfigStore>) -> Arc<cheetah_engine::Engine> {
    let runtime = Arc::new(TokioRuntime::new());
    Arc::new(
        EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(Arc::new(MediaProcessingModuleFactory))
            .build()
            .expect("engine build"),
    )
}

fn media_processing_change(
    outcome: &cheetah_sdk::ConfigApplyOutcome,
) -> Option<cheetah_sdk::ModuleConfigChange> {
    outcome
        .module_changes
        .iter()
        .find(|c| c.module_id.0 == "media-processing")
        .cloned()
}

#[tokio::test(flavor = "current_thread")]
async fn profile_change_requires_module_restart() {
    let config = Arc::new(ConfigStore::new());
    config.register_module_default(
        ModuleId::new("media-processing"),
        MediaProcessingModuleConfig::default_json(),
    );

    let engine = make_engine_with_config(config.clone());
    engine.start().await.expect("engine start");

    // software profile is only valid when avcodec-profile-software is compiled.
    // When it is not, lowering max_encoded_frame_bytes still forces a restart.
    let patch = if cfg!(feature = "avcodec-profile-software") {
        json!({ "profile": "software" })
    } else {
        json!({ "max_encoded_frame_bytes": 1024 })
    };
    let outcome = config
        .apply_module_patch(
            &ModuleId::new("media-processing"),
            patch,
            ConfigEffect::ModuleRestartRequired,
        )
        .expect("patch config");
    let change = media_processing_change(&outcome).expect("media-processing config change");

    let report = engine
        .module_manager_api()
        .apply_module_config_change(change)
        .await
        .expect("apply config");
    assert_eq!(report.effect, ConfigEffect::ModuleRestartRequired);

    engine.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn max_concurrent_jobs_increase_is_immediate() {
    let config = Arc::new(ConfigStore::new());
    config.register_module_default(
        ModuleId::new("media-processing"),
        MediaProcessingModuleConfig::default_json(),
    );

    let engine = make_engine_with_config(config.clone());
    engine.start().await.expect("engine start");

    let outcome = config
        .apply_module_patch(
            &ModuleId::new("media-processing"),
            json!({ "max_concurrent_jobs": 128 }),
            ConfigEffect::Immediate,
        )
        .expect("patch config");
    let change = media_processing_change(&outcome).expect("media-processing config change");

    let report = engine
        .module_manager_api()
        .apply_module_config_change(change)
        .await
        .expect("apply config");
    assert_eq!(report.effect, ConfigEffect::Immediate);

    engine.stop().await;
}
