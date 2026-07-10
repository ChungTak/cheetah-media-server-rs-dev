use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SdkError;
use crate::ids::ModuleId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigEffect {
    /// 立即生效，不要求会话或模块重建。
    Immediate,
    /// 仅对新会话生效，已有会话保持旧配置。
    NewSessionsOnly,
    /// 需要模块重启后才能完全生效。
    ModuleRestartRequired,
    /// 需要引擎重启后才能完全生效。
    EngineRestartRequired,
}

#[derive(Debug, Clone)]
pub struct ConfigApplyResult {
    /// 配置应用后的版本号。
    pub version: u64,
    /// 本次配置变更影响级别。
    pub effect: ConfigEffect,
}

#[derive(Debug, Clone)]
pub struct ConfigValueChange {
    /// 变更前值。
    pub previous: Value,
    /// 变更后值。
    pub next: Value,
}

#[derive(Debug, Clone)]
pub struct ModuleConfigChange {
    /// 发生变更的 module 标识。
    pub module_id: ModuleId,
    /// 变更前的 module 运行时配置。
    pub previous: Value,
    /// 变更后的 module 运行时配置。
    pub next: Value,
    /// 变更前生效的全局配置快照（如有）。
    pub previous_global: Option<Value>,
    /// 变更后生效的全局配置快照（如有）。
    pub next_global: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ConfigApplyOutcome {
    /// 配置应用后的版本号。
    pub version: u64,
    /// 聚合后的影响级别。
    pub effect: ConfigEffect,
    /// 全局配置变更详情（如有）。
    pub global_change: Option<ConfigValueChange>,
    /// 各 module 配置变更详情。
    pub module_changes: Vec<ModuleConfigChange>,
    /// 回滚令牌；支持时返回用于恢复到变更前状态。
    pub rollback_token: Option<ConfigRollbackToken>,
}

#[derive(Debug, Clone)]
pub struct RegisteredSchema {
    /// schema 注册作用域（global 或 module:<id>）。
    pub scope: String,
    /// schema 名称。
    pub schema_name: String,
}

#[derive(Clone)]
pub struct ModuleSchemaRegistration {
    /// 目标 module 标识。
    pub module_id: ModuleId,
    /// schema 名称。
    pub schema_name: String,
    /// schema 默认配置值。
    pub default_value: Value,
    /// 可选校验器，用于拒绝非法配置。
    pub validator: Option<ConfigValidator>,
}

#[derive(Debug, Clone)]
pub struct ConfigRollbackToken {
    /// 变更前全局运行时配置快照。
    pub previous_global_runtime: Option<Value>,
    /// 变更前各 module 运行时配置快照。
    pub previous_module_runtime: Vec<(ModuleId, Option<Value>)>,
}

pub type ConfigValidator = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

pub trait ConfigSchema:
    Default + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + Sync + 'static
{
    fn schema_name() -> &'static str;

    fn default_json() -> Value;

    fn validate(_value: &Self) -> Result<(), String> {
        Ok(())
    }
}

pub trait ConfigProvider: Send + Sync {
    fn global(&self) -> Value;
    fn module(&self, module_id: &ModuleId) -> Value;
    fn version(&self) -> u64;
}

pub trait ConfigSchemaRegistry: Send + Sync {
    fn register_global_schema(
        &self,
        schema_name: &str,
        default_value: Value,
        validator: Option<ConfigValidator>,
    ) -> Result<(), SdkError>;

    fn register_module_schema(
        &self,
        module_id: ModuleId,
        schema_name: &str,
        default_value: Value,
        validator: Option<ConfigValidator>,
    ) -> Result<(), SdkError>;

    fn register_module_schema_entry(
        &self,
        entry: ModuleSchemaRegistration,
    ) -> Result<(), SdkError> {
        self.register_module_schema(
            entry.module_id,
            &entry.schema_name,
            entry.default_value,
            entry.validator,
        )
    }

    fn list_schemas(&self) -> Vec<RegisteredSchema>;
}

pub trait ConfigApplyApi: Send + Sync {
    fn apply_global_patch(
        &self,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyOutcome, SdkError>;

    fn apply_module_patch(
        &self,
        module_id: &ModuleId,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyOutcome, SdkError>;

    fn rollback(&self, token: ConfigRollbackToken) -> Result<(), SdkError>;
}

pub trait ConfigAdminApi: Send + Sync {
    fn patch_global(
        &self,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyResult, SdkError>;

    fn patch_module(
        &self,
        module_id: &ModuleId,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyResult, SdkError>;
}
