use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange,
    ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, ProviderRegistration, SdkError,
};
use tracing::info;

use crate::config::ProxyModuleConfig;
use crate::media_provider::ProxyMediaProvider;
use crate::registry::ProxyRegistry;

const MODULE_ID: &str = "proxy";

/// Factory for creating [`ProxyModule`] instances.
///
/// 创建 [`ProxyModule`] 实例的工厂。
pub struct ProxyModuleFactory;

impl ModuleFactory for ProxyModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Proxy Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "proxy".to_string(),
            routes_prefix: "/api/v1/proxy".to_string(),
            capabilities: vec![ModuleCapability::BackgroundJob, ModuleCapability::HttpApi],
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
    ctx: Option<EngineContext>,
    config: ProxyModuleConfig,
    registry: Arc<ProxyRegistry>,
    media_services_registration: Option<ProviderRegistration>,
}

impl ProxyModule {
    /// Create a new module in the `Created` state.
    ///
    /// 在 `Created` 状态下创建新模块。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            config: ProxyModuleConfig::default(),
            registry: Arc::new(ProxyRegistry::new(ProxyModuleConfig::default().max_proxies)),
            media_services_registration: None,
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
        self.config = ProxyModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine.clone());
        self.registry = Arc::new(ProxyRegistry::new(self.config.max_proxies));

        let provider = Arc::new(ProxyMediaProvider::new(
            ctx.engine.clone(),
            self.registry.clone(),
            self.config.clone(),
        )?);

        let mut capabilities = cheetah_media_api::MediaCapabilitySet::empty();
        let mut proxy_operations = vec![
            "create_pull".to_string(),
            "delete_pull".to_string(),
            "list_pull".to_string(),
            "create_push".to_string(),
            "delete_push".to_string(),
        ];
        if ctx.engine.ffmpeg_api.is_available() {
            proxy_operations.push("create_ffmpeg".to_string());
            proxy_operations.push("delete_ffmpeg".to_string());
        }
        capabilities.add_with_operations(
            cheetah_media_api::MediaCapability::Proxy,
            1,
            proxy_operations,
        );

        self.media_services_registration = Some(
            ctx.engine
                .media_services
                .register_proxy_with_capabilities(provider, capabilities),
        );

        info!("proxy module initialized");
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
        self.registry.cancel_all();
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_cfg = ProxyModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_cfg != self.config {
            self.config = new_cfg;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }
}
