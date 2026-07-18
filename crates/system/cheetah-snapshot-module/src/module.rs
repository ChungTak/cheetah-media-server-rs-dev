use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange,
    ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, ProviderRegistration, SdkError,
};
use tracing::info;

use cheetah_media_processing_module::config::MediaProcessingModuleConfig;
use cheetah_media_processing_module::ImageProcessProvider;

use crate::config::SnapshotModuleConfig;
use crate::media_provider::SnapshotMediaProvider;
use crate::registry::SnapshotRegistry;

const MODULE_ID: &str = "snapshot";

/// Factory for creating [`SnapshotModule`] instances.
///
/// 创建 [`SnapshotModule`] 实例的工厂。
pub struct SnapshotModuleFactory;

impl ModuleFactory for SnapshotModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Snapshot Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "snapshot".to_string(),
            routes_prefix: "/api/v1/snapshots".to_string(),
            capabilities: vec![ModuleCapability::Subscribe, ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(SnapshotModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "snapshot-module".to_string(),
            default_value: SnapshotModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg =
                    SnapshotModuleConfig::from_value(value.clone()).map_err(|e| e.to_string())?;
                cfg.validate()
            })),
        })
    }
}

/// Snapshot module instance.
///
/// 快照模块实例。
pub struct SnapshotModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    config: SnapshotModuleConfig,
    registry: Arc<SnapshotRegistry>,
    media_services_registration: Option<ProviderRegistration>,
    image_process_registration: Option<ProviderRegistration>,
}

impl SnapshotModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            config: SnapshotModuleConfig::default(),
            registry: Arc::new(SnapshotRegistry::new(
                SnapshotModuleConfig::default().max_snapshots,
            )),
            media_services_registration: None,
            image_process_registration: None,
        }
    }
}

impl Default for SnapshotModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for SnapshotModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Snapshot Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = SnapshotModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine.clone());
        self.registry = Arc::new(SnapshotRegistry::new(self.config.max_snapshots));

        let provider = Arc::new(SnapshotMediaProvider::new(
            ctx.engine.clone(),
            self.registry.clone(),
            self.config.clone(),
        ));

        let mut capabilities = cheetah_media_api::MediaCapabilitySet::empty();
        capabilities.add(cheetah_media_api::MediaCapability::Snapshot, 1);
        capabilities.set_reason(
            cheetah_media_api::MediaCapability::Snapshot,
            "avcodec-rs-backed image processing",
        );

        self.media_services_registration = Some(
            ctx.engine
                .media_services
                .register_snapshot_with_capabilities(provider, capabilities),
        );

        // Register an ImageProcessApi provider if no other module (e.g. the
        // dedicated media-processing module) has already done so.
        if ctx.engine.media_services.image_process().is_none() {
            let provider = Arc::new(ImageProcessProvider::new(
                ctx.engine.runtime_api.clone(),
                Some(ctx.engine.media_file_store.clone()),
                MediaProcessingModuleConfig::default(),
            ));
            self.image_process_registration =
                Some(ctx.engine.media_services.register_image_process(provider));
        }

        info!("snapshot module initialized");
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(reg) = self.media_services_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        if let Some(reg) = self.image_process_registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = SnapshotModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_cfg != self.config {
            self.config = new_cfg;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }
}
