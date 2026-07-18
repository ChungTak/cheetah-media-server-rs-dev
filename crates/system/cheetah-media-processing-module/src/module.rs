//! Module lifecycle for `cheetah-media-processing-module`.

use std::sync::Arc;
#[cfg(feature = "media-processing-caption")]
use std::sync::Mutex;

use async_trait::async_trait;
#[cfg(feature = "media-processing-caption")]
use cheetah_codec::MonoTime;
#[cfg(feature = "media-processing-caption")]
use cheetah_sdk::JoinHandle;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange,
    ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, ProviderRegistration, SdkError,
};
use tracing::info;

use crate::config::MediaProcessingModuleConfig;

const MODULE_ID: &str = "media-processing";

/// Factory for creating [`MediaProcessingModule`] instances.
pub struct MediaProcessingModuleFactory;

impl ModuleFactory for MediaProcessingModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Media Processing Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "media_processing".to_string(),
            routes_prefix: "/api/v1/media-processing".to_string(),
            capabilities: vec![ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(MediaProcessingModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "media-processing-module".to_string(),
            default_value: MediaProcessingModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg = MediaProcessingModuleConfig::from_value(value.clone())
                    .map_err(|e| e.to_string())?;
                cfg.validate()
            })),
        })
    }
}

/// Media processing module instance.
pub struct MediaProcessingModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    config: MediaProcessingModuleConfig,
    image_process_registration: Option<ProviderRegistration>,
    processing_registration: Option<ProviderRegistration>,
    metrics_cancel: Option<CancellationToken>,
    #[cfg(feature = "media-processing-caption")]
    processing_provider: Option<Arc<crate::provider::MediaProcessingProvider>>,
    #[cfg(feature = "media-processing-caption")]
    metrics_handle: Option<Mutex<Box<dyn JoinHandle>>>,
}

impl MediaProcessingModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            config: MediaProcessingModuleConfig::default(),
            image_process_registration: None,
            processing_registration: None,
            metrics_cancel: None,
            #[cfg(feature = "media-processing-caption")]
            processing_provider: None,
            #[cfg(feature = "media-processing-caption")]
            metrics_handle: None,
        }
    }
}

impl Default for MediaProcessingModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for MediaProcessingModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Media Processing Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let cfg = MediaProcessingModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        cfg.validate().map_err(SdkError::InvalidArgument)?;
        self.config = cfg;
        self.ctx = Some(ctx.engine.clone());

        #[cfg(feature = "media-processing-image")]
        {
            let provider = Arc::new(crate::provider::ImageProcessProvider::new(
                ctx.engine.runtime_api.clone(),
                Some(ctx.engine.media_file_store.clone()),
                self.config.clone(),
            ));

            let mut capabilities = cheetah_media_api::MediaCapabilitySet::empty();
            capabilities.add(cheetah_media_api::MediaCapability::ImageProcessing, 1);
            let reason = if cfg!(feature = "media-processing-image-overlay") {
                "jpeg input/output; crop/resize/fit/rotate/flip/pad/csc/resize-pad/text/blend"
            } else {
                "jpeg input/output; crop/resize/fit/rotate/flip/pad/csc/resize-pad/text"
            };
            capabilities.set_reason(cheetah_media_api::MediaCapability::ImageProcessing, reason);

            self.image_process_registration = Some(
                ctx.engine
                    .media_services
                    .register_image_process_with_capabilities(provider, capabilities),
            );
        }

        #[cfg(feature = "media-processing-caption")]
        {
            let provider = Arc::new(crate::provider::MediaProcessingProvider::new(
                ctx.engine.clone(),
                self.config.clone(),
            ));
            self.processing_provider = Some(provider.clone());

            let capabilities = crate::provider::MediaProcessingProvider::default_capabilities();
            self.processing_registration = Some(
                ctx.engine
                    .media_services
                    .register_processing_with_capabilities(provider, capabilities),
            );
        }

        info!("media processing module initialized");
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;

        #[cfg(feature = "media-processing-caption")]
        if let Some(provider) = self.processing_provider.clone() {
            let Some(ctx) = self.ctx.as_ref() else {
                return Ok(());
            };
            let runtime = ctx.runtime_api.clone();
            let metrics_cancel = _cancel.child_token();
            let loop_cancel = metrics_cancel.clone();
            let runtime_for_loop = runtime.clone();
            self.metrics_cancel = Some(metrics_cancel);
            self.metrics_handle = Some(Mutex::new(runtime.spawn(Box::pin(async move {
                let publish_interval_us = 5_000_000;
                loop {
                    provider.publish_job_metrics();
                    let deadline = MonoTime::from_micros(
                        runtime_for_loop
                            .now()
                            .as_micros()
                            .saturating_add(publish_interval_us),
                    );
                    let mut timer = runtime_for_loop.sleep_until(deadline);
                    timer.wait().await;
                    if loop_cancel.is_cancelled() {
                        break;
                    }
                }
            }))));
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(reg) = self.image_process_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        #[cfg(feature = "media-processing-caption")]
        if let Some(provider) = self.processing_provider.take() {
            provider.cancel_all().await;
        }
        if let Some(cancel) = self.metrics_cancel.take() {
            cancel.cancel();
        }
        #[cfg(feature = "media-processing-caption")]
        if let Some(handle) = self.metrics_handle.take() {
            handle.lock().unwrap().abort();
        }
        if let Some(reg) = self.processing_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = MediaProcessingModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        new_cfg.validate().map_err(SdkError::InvalidArgument)?;
        if new_cfg != self.config {
            self.config = new_cfg;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }
}
