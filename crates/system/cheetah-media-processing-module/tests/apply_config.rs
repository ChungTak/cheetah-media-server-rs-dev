use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::MediaProcessingModule;
use cheetah_sdk::{ConfigEffect, Module, ModuleConfigChange};

fn change(next: MediaProcessingModuleConfig) -> ModuleConfigChange {
    ModuleConfigChange {
        module_id: cheetah_sdk::ModuleId::new("media-processing"),
        previous: serde_json::json!({}),
        next: serde_json::to_value(next).expect("config serializes"),
        previous_global: None,
        next_global: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn identical_config_is_immediate() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig::default();
    assert_eq!(
        module.apply_config(change(cfg.clone())).await.unwrap(),
        ConfigEffect::Immediate
    );
}

#[cfg(feature = "avcodec-profile-software")]
#[tokio::test(flavor = "current_thread")]
async fn profile_change_requires_restart() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig {
        profile: "software".to_string(),
        ..Default::default()
    };
    assert_eq!(
        module.apply_config(change(cfg)).await.unwrap(),
        ConfigEffect::ModuleRestartRequired
    );
}

#[tokio::test(flavor = "current_thread")]
async fn max_concurrent_jobs_increase_is_immediate_when_no_usage() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig {
        max_concurrent_jobs: 128,
        ..Default::default()
    };
    assert_eq!(
        module.apply_config(change(cfg)).await.unwrap(),
        ConfigEffect::Immediate
    );
}

#[tokio::test(flavor = "current_thread")]
async fn max_image_width_decrease_is_immediate_when_no_usage() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig {
        max_image_width: 100,
        ..Default::default()
    };
    assert_eq!(
        module.apply_config(change(cfg)).await.unwrap(),
        ConfigEffect::Immediate
    );
}

#[tokio::test(flavor = "current_thread")]
async fn max_encoded_frame_bytes_decrease_requires_restart() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig {
        max_encoded_frame_bytes: 1024,
        ..Default::default()
    };
    assert_eq!(
        module.apply_config(change(cfg)).await.unwrap(),
        ConfigEffect::ModuleRestartRequired
    );
}

#[tokio::test(flavor = "current_thread")]
async fn max_overlay_font_size_decrease_requires_restart() {
    let mut module = MediaProcessingModule::new();
    let cfg = MediaProcessingModuleConfig {
        max_overlay_font_size: 64,
        ..Default::default()
    };
    assert_eq!(
        module.apply_config(change(cfg)).await.unwrap(),
        ConfigEffect::ModuleRestartRequired
    );
}
