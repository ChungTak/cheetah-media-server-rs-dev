use cheetah_sdk::{
    ConfigEffect, EngineContext, Module, ModuleCapability, ModuleConfigChange, ModuleFactory,
    ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState, SdkError,
};
use std::sync::Arc;

use crate::config::WebhookDispatcherConfig;
use crate::decision::WebhookDecisionClient;
use crate::dispatcher::WebhookDispatcher;
use crate::security::WebhookUrlPolicy;
use crate::sender::{RuntimeHttpClient, WebhookSender};
use crate::translator::ZlmWebhookTranslator;

const MODULE_ID: &str = "webhook-dispatcher";
const DISPATCHER_QUEUE_CAPACITY: usize = 1024;

/// Factory for the webhook dispatcher module.
///
/// webhook 分发器模块工厂。
pub struct WebhookModuleFactory;

impl ModuleFactory for WebhookModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Webhook Dispatcher".to_string(),
            dependencies: Vec::new(),
            config_namespace: "media.webhook".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![ModuleCapability::BackgroundJob],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(WebhookModule::new())
    }
}

/// Module that runs the outbound webhook dispatcher.
///
/// 运行出站 webhook 分发器的模块。
pub struct WebhookModule {
    state: ModuleState,
    ctx: Option<EngineContext>,
    dispatcher: Option<WebhookDispatcher>,
    decision_client: Option<WebhookDecisionClient>,
    handle: Option<crate::dispatcher::WebhookDispatcherHandle>,
}

impl WebhookModule {
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            ctx: None,
            dispatcher: None,
            decision_client: None,
            handle: None,
        }
    }

    fn build_dispatcher(config: WebhookDispatcherConfig, ctx: &EngineContext) -> WebhookDispatcher {
        WebhookDispatcher::new(
            config,
            ctx.media_event_bus.clone(),
            ctx.runtime_api.clone(),
            Self::sender(ctx),
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        )
    }

    fn build_decision_client(
        config: WebhookDispatcherConfig,
        ctx: &EngineContext,
    ) -> WebhookDecisionClient {
        WebhookDecisionClient::new(
            config,
            Self::sender(ctx),
            Arc::new(ZlmWebhookTranslator),
            WebhookUrlPolicy::default(),
        )
    }

    fn sender(ctx: &EngineContext) -> Arc<dyn WebhookSender> {
        Arc::new(RuntimeHttpClient::new(ctx.runtime_api.clone()))
    }
}

impl Default for WebhookModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Module for WebhookModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "Webhook Dispatcher".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        let config = if ctx.initial_config.is_null() {
            WebhookDispatcherConfig::default()
        } else {
            serde_json::from_value(ctx.initial_config)
                .map_err(|e| SdkError::InvalidArgument(e.to_string()))?
        };
        let decision_client = Self::build_decision_client(config.clone(), &ctx.engine);
        ctx.engine
            .media_services
            .register_webhook(Arc::new(decision_client.clone()));
        self.dispatcher = Some(Self::build_dispatcher(config, &ctx.engine));
        self.decision_client = Some(decision_client);
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, _cancel: cheetah_sdk::CancellationToken) -> Result<(), SdkError> {
        let dispatcher = self
            .dispatcher
            .as_ref()
            .ok_or_else(|| SdkError::InvalidArgument("dispatcher not initialized".to_string()))?;
        let handle = dispatcher
            .start(DISPATCHER_QUEUE_CAPACITY)
            .map_err(|e| SdkError::Internal(e.to_string()))?;
        self.handle = Some(handle);
        self.state = ModuleState::Running;
        // The dispatcher runs as a background task and is stopped from
        // `WebhookModule::stop`. Returning immediately keeps the engine startup
        // pipeline moving.
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(handle) = self.handle.take() {
            handle.stop();
        }
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config: WebhookDispatcherConfig = serde_json::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if let Some(dispatcher) = self.dispatcher.as_ref() {
            dispatcher.set_config(new_config.clone());
        }
        if let Some(client) = self.decision_client.as_ref() {
            client.set_config(new_config);
        }
        Ok(ConfigEffect::Immediate)
    }
}
