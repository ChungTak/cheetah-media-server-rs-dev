use std::any::Any;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SdkError;
use crate::ids::ModuleId;

/// Effect level of a configuration change.
///
/// 配置变更的影响级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigEffect {
    /// Effective immediately; no session or module rebuild required.
    ///
    /// 立即生效，不要求会话或模块重建。
    Immediate,
    /// Only affects new sessions; existing sessions keep the old config.
    ///
    /// 仅对新会话生效，已有会话保持旧配置。
    NewSessionsOnly,
    /// Requires the module to restart before taking full effect.
    ///
    /// 需要模块重启后才能完全生效。
    ModuleRestartRequired,
    /// Requires the engine to restart before taking full effect.
    ///
    /// 需要引擎重启后才能完全生效。
    EngineRestartRequired,
}

/// Result of applying a configuration patch.
///
/// 配置补丁应用结果。
#[derive(Debug, Clone)]
pub struct ConfigApplyResult {
    /// 配置应用后的版本号。
    pub version: u64,
    /// 本次配置变更影响级别。
    pub effect: ConfigEffect,
}

/// Value change snapshot for a single config field.
///
/// 单个配置字段的值变化快照。
#[derive(Debug, Clone)]
pub struct ConfigValueChange {
    /// 变更前值。
    pub previous: Value,
    /// 变更后值。
    pub next: Value,
}

/// Config change for a specific module, including global snapshot context.
///
/// 特定 module 的配置变更，包含全局配置快照上下文。
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

/// Aggregated outcome of applying one or more config patches.
///
/// 应用一个或多个配置补丁后的聚合结果。
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

/// Metadata for a registered config schema.
///
/// 已注册配置 schema 的元数据。
#[derive(Debug, Clone)]
pub struct RegisteredSchema {
    /// schema 注册作用域（global 或 module:<id>）。
    pub scope: String,
    /// schema 名称。
    pub schema_name: String,
}

/// Schema registration for a module, including optional validation.
///
/// module 的 schema 注册，包含可选校验器。
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

/// Snapshot used to roll back a configuration change.
///
/// 用于回滚配置变更的快照。
#[derive(Debug, Clone)]
pub struct ConfigRollbackToken {
    /// 变更前全局运行时配置快照。
    pub previous_global_runtime: Option<Value>,
    /// 变更前各 module 运行时配置快照。
    pub previous_module_runtime: Vec<(ModuleId, Option<Value>)>,
}

pub type ConfigValidator = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

/// Typed config schema that can be registered and validated.
///
/// 可注册和校验的强类型配置 schema。
pub trait ConfigSchema:
    Default + serde::Serialize + for<'de> serde::Deserialize<'de> + Send + Sync + 'static
{
    fn schema_name() -> &'static str;

    fn default_json() -> Value;

    fn validate(_value: &Self) -> Result<(), String> {
        Ok(())
    }
}

/// Read access to global and per-module runtime config.
///
/// 全局和 module 运行时配置的读取接口。
pub trait ConfigProvider: Send + Sync + Any {
    fn global(&self) -> Value;
    fn module(&self, module_id: &ModuleId) -> Value;
    fn version(&self) -> u64;
}

/// Registry for global and module config schemas.
///
/// 全局和 module 配置 schema 的注册表。
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

/// Apply and rollback runtime config patches.
///
/// 应用和回滚运行时配置补丁。
pub trait ConfigApplyApi: Send + Sync + Any {
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

/// Admin-level entry point for applying config patches.
///
/// 应用配置补丁的管理入口。
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
