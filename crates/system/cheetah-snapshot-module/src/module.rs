//! Snapshot module lifecycle integration with `cheetah-sdk`.
//!
//! 截图模块与 `cheetah-sdk` 的生命周期集成。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleSchemaRegistration, ModuleState,
    ProviderRegistration, SdkError,
};

use crate::config::SnapshotModuleConfig;
use crate::media_provider::SnapshotMediaProvider;
use crate::registry::SnapshotRegistry;

const MODULE_ID: &str = "snapshot";

/// Factory for creating `SnapshotModule` instances.
///
/// 创建 `SnapshotModule` 实例的工厂。
pub struct SnapshotModuleFactory;

impl ModuleFactory for SnapshotModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Snapshot Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "snapshot".to_string(),
            routes_prefix: "/api/v1/snapshots".to_string(),
            capabilities: vec![ModuleCapability::Subscribe],
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
/// 截图模块实例。
pub struct SnapshotModule {
    state: ModuleState,
    config: SnapshotModuleConfig,
    ctx: Option<EngineContext>,
    registry: Arc<SnapshotRegistry>,
    registration: Option<ProviderRegistration>,
}

impl SnapshotModule {
    /// Create a new module in the `Created` state.
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: SnapshotModuleConfig::default(),
            ctx: None,
            registry: Arc::new(SnapshotRegistry::new()),
            registration: None,
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
        self.config = SnapshotModuleConfig::from_value(ctx.initial_config)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid snapshot config: {e}")))?;
        self.config
            .validate()
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: cheetah_sdk::CancellationToken) -> Result<(), SdkError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| SdkError::Unavailable("snapshot module not initialized".to_string()))?
            .clone();

        let provider = Arc::new(SnapshotMediaProvider::new(
            ctx.clone(),
            self.config.clone(),
            self.registry.clone(),
        ));
        self.registration = Some(ctx.media_services.register_snapshot(provider));
        self.state = ModuleState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(reg) = self.registration.take() {
            if let Some(ctx) = self.ctx.as_ref() {
                ctx.media_services.unregister(&reg);
            }
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = SnapshotModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid snapshot config: {e}")))?;
        new_config
            .validate()
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.config = new_config;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}
