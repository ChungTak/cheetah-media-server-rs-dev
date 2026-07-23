//! GB28181 module factory and implementation.
//!
//! GB28181 模块工厂与实现。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::media_api::model::SessionKind;
use cheetah_sdk::media_api::rtp_session::RtpSessionRef;

/// Tracked GB28181 session state stored under a `app/stream` session key.
pub(crate) type GbSessionEntry = (String, RtpSessionRef, SessionKind);
use cheetah_sdk::{
    CancellationToken, ConfigEffect, EngineContext, HttpMethod, HttpRouteDescriptor, Module,
    ModuleCapability, ModuleConfigChange, ModuleFactory, ModuleHttpService, ModuleId, ModuleInfo,
    ModuleInitContext, ModuleManifest, ModuleSchemaRegistration, ModuleState, SdkError,
};
use parking_lot::Mutex;
use serde_json::Value;

use crate::config::{ControlOwner, Gb28181ModuleConfig};
use crate::http_service::GbHttpService;

const MODULE_ID: &str = "gb28181";

/// Factory for creating GB28181 modules.
///
/// GB28181 模块工厂。
pub struct Gb28181ModuleFactory;

/// `Gb28181ModuleFactory` implementation.
///
/// `Gb28181ModuleFactory` 实现。
impl ModuleFactory for Gb28181ModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "GB28181 Module".to_string(),
            dependencies: vec![ModuleId::new("rtp")], // Depends on rtp module for media delivery
            config_namespace: "gb28181".to_string(),
            routes_prefix: "/api/v1/gb28181".to_string(),
            capabilities: vec![
                ModuleCapability::Publish,
                ModuleCapability::Subscribe,
                ModuleCapability::HttpApi,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(Gb28181Module::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "gb28181-module".to_string(),
            default_value: Gb28181ModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let config = Gb28181ModuleConfig::from_value(value.clone())
                    .map_err(|err| err.to_string())?;
                config.validate()
            })),
        })
    }
}

/// GB28181 module runtime state.
///
/// GB28181 模块运行时状态。
pub struct Gb28181Module {
    state: ModuleState,
    config: Gb28181ModuleConfig,
    ctx: Option<EngineContext>,
    cancel_token: Option<CancellationToken>,
    active_sessions: Arc<Mutex<HashMap<String, GbSessionEntry>>>,
}

/// `Gb28181Module` constructor.
///
/// `Gb28181Module` 构造器。
impl Gb28181Module {
    /// Create a new GB28181 module instance.
    ///
    /// 创建新的 GB28181 模块实例。
    pub fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Gb28181ModuleConfig::default(),
            ctx: None,
            cancel_token: None,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// `Default` forward to `Gb28181Module::new`.
///
/// `Default` 转发到 `Gb28181Module::new`。
impl Default for Gb28181Module {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns true when the signaling control plane is enabled and in a rollout
/// mode that can drive mutations for GB resources.
///
/// 当信号控制面已启用且处于可驱动 GB 资源变更的灰度/生产阶段时返回 true。
fn signaling_controls_gb(signaling_cfg: &Value) -> bool {
    let enabled = signaling_cfg
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return false;
    }
    matches!(
        signaling_cfg.get("rollout").and_then(Value::as_str),
        Some("canary") | Some("production")
    )
}

/// `Module` lifecycle and HTTP API for GB28181.
///
/// GB28181 的 `Module` 生命周期与 HTTP API。
#[async_trait]
impl Module for Gb28181Module {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "GB28181 Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = Gb28181ModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        // A disabled module never binds the local listener, so there is no
        // dual-owner risk. Only enforce ownership when the module is enabled.
        if self.config.enabled {
            let signaling_cfg = ctx
                .engine
                .config_provider
                .module(&ModuleId::new("signaling_control_plane"));

            match self.config.control_owner {
                ControlOwner::Signaling => {
                    if !signaling_cfg
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        return Err(SdkError::InvalidArgument(
                            "gb28181.control_owner=signaling requires signaling_control_plane.enabled=true"
                                .to_string(),
                        ));
                    }
                }
                ControlOwner::Local => {
                    if signaling_controls_gb(&signaling_cfg) {
                        return Err(SdkError::InvalidArgument(
                            "gb28181.control_owner=local conflicts with signaling_control_plane canary/production rollout"
                                .to_string(),
                        ));
                    }
                }
            }
        }

        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        if !self.config.enabled {
            // Module is disabled; nothing to run.
            self.state = ModuleState::Running;
            cancel.cancelled().await;
            return Ok(());
        }

        self.ctx.clone().ok_or_else(|| {
            SdkError::InvalidArgument(
                "Gb28181Module::start called before init (engine context missing)".to_string(),
            )
        })?;

        self.state = ModuleState::Running;
        self.cancel_token = Some(cancel.clone());

        // The module only exposes the structured media REST API. SIP/SDP signaling is handled
        // by an external control plane, so no local listener or driver is started here.
        cancel.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        // Drop any tracked active sessions so the module restarts from a clean state.
        self.active_sessions.lock().clear();
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = Gb28181ModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_config != self.config {
            self.config = new_config;
            return Ok(ConfigEffect::ModuleRestartRequired);
        }
        Ok(ConfigEffect::Immediate)
    }

    fn http_routes(&self) -> Vec<HttpRouteDescriptor> {
        if self.config.control_owner == ControlOwner::Signaling {
            return Vec::new();
        }
        vec![
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/recv/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/recv/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/send/create".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/send/stop".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/talk/start".to_string(),
            },
            HttpRouteDescriptor {
                method: HttpMethod::Post,
                path: "/talk/stop".to_string(),
            },
        ]
    }

    fn http_service(&self) -> Option<Arc<dyn ModuleHttpService>> {
        if self.config.control_owner == ControlOwner::Signaling {
            return None;
        }
        let engine = self.ctx.clone()?;
        Some(Arc::new(GbHttpService::new(
            engine,
            self.active_sessions.clone(),
            self.config.default_media_port,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signaling_controls_gb_only_in_active_rollout() {
        assert!(!signaling_controls_gb(
            &serde_json::json!({"enabled": false})
        ));
        assert!(!signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "register_only"
        })));
        assert!(!signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "shadow_query"
        })));
        assert!(signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "canary"
        })));
        assert!(signaling_controls_gb(&serde_json::json!({
            "enabled": true,
            "rollout": "production"
        })));
    }

    #[test]
    fn signaling_owner_disables_http_routes() {
        let mut module = Gb28181Module::new();
        module.config.control_owner = ControlOwner::Signaling;
        assert!(module.http_routes().is_empty());
        assert!(module.http_service().is_none());
    }

    #[test]
    fn local_owner_keeps_http_routes() {
        let module = Gb28181Module::new();
        assert_eq!(module.config.control_owner, ControlOwner::Local);
        assert_eq!(module.http_routes().len(), 6);
    }
}
