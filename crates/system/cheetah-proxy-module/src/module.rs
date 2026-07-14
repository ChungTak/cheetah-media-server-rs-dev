//! Proxy module lifecycle integration with `cheetah-sdk`.
//!
//! 代理模块与 `cheetah-sdk` 的生命周期集成。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleSchemaRegistration, ModuleState,
    ProviderRegistration, SdkError,
};

use crate::config::ProxyModuleConfig;
use crate::media_provider::ProxyMediaProvider;
use crate::registry::ProxyRegistry;

const MODULE_ID: &str = "proxy";

/// Factory for creating `ProxyModule` instances.
///
/// 创建 `ProxyModule` 实例的工厂。
pub struct ProxyModuleFactory;

impl ModuleFactory for ProxyModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Proxy Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "proxy".to_string(),
            routes_prefix: "/api/v1/proxies".to_string(),
            capabilities: vec![ModuleCapability::HttpApi, ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(ProxyModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "proxy-module".to_string(),
            default_value: ProxyModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let cfg =
                    ProxyModuleConfig::from_value(value.clone()).map_err(|e| e.to_string())?;
                cfg.validate()
            })),
        })
    }
}

/// Proxy module instance.
///
/// 代理模块实例。
pub struct ProxyModule {
    state: ModuleState,
    config: ProxyModuleConfig,
    ctx: Option<EngineContext>,
    registry: Arc<ProxyRegistry>,
    registration: Option<ProviderRegistration>,
}

impl ProxyModule {
    /// Create a new module in the `Created` state.
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: ProxyModuleConfig::default(),
            ctx: None,
            registry: Arc::new(ProxyRegistry::default()),
            registration: None,
        }
    }
}

impl Default for ProxyModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ProxyModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Proxy Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = ProxyModuleConfig::from_value(ctx.initial_config)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid proxy config: {e}")))?;
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
            .ok_or_else(|| SdkError::Unavailable("proxy module not initialized".to_string()))?
            .clone();

        self.registry = Arc::new(ProxyRegistry::new(self.config.max_total_proxies));

        let provider = Arc::new(ProxyMediaProvider::new(&ctx, &self.config));
        self.registration = Some(ctx.media_services.register_proxy(provider));
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
        let new_config = ProxyModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(format!("invalid proxy config: {e}")))?;
        new_config
            .validate()
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.config = new_config;
        Ok(ConfigEffect::ModuleRestartRequired)
    }
}
