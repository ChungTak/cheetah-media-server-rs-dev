use std::collections::{BTreeSet, HashMap};
use std::env;
use std::sync::Arc;

use cheetah_sdk::config::{
    ConfigAdminApi, ConfigApplyApi, ConfigApplyOutcome, ConfigApplyResult, ConfigEffect,
    ConfigProvider, ConfigRollbackToken, ConfigSchemaRegistry, ConfigValidator, ConfigValueChange,
    ModuleConfigChange, RegisteredSchema,
};
use cheetah_sdk::{ConfigEvent, EventBus, ModuleId, SdkError, SystemEvent};
use parking_lot::RwLock;
use serde_json::{Map, Value};

#[derive(Clone)]
/// Registered schema metadata and an optional validator closure.
///
/// 已注册的 schema 元数据与可选的校验器闭包。
struct SchemaEntry {
    schema_name: String,
    validator: Option<ConfigValidator>,
}

#[derive(Default)]
/// Layered configuration state: default, file, env, and runtime patches.
///
/// 分层配置状态：默认值、文件、环境变量与运行时补丁。
struct ConfigState {
    version: u64,
    global_default: Value,
    global_file: Value,
    global_env: Value,
    global_runtime: Value,
    module_default: HashMap<ModuleId, Value>,
    module_file: HashMap<ModuleId, Value>,
    module_env: HashMap<ModuleId, Value>,
    module_runtime: HashMap<ModuleId, Value>,
    global_schema: Option<SchemaEntry>,
    module_schemas: HashMap<ModuleId, SchemaEntry>,
}

/// In-memory configuration store with layered precedence and schema validation.
///
/// 内存配置存储，支持分层优先级与 schema 校验。
#[derive(Default)]
pub struct ConfigStore {
    inner: RwLock<ConfigState>,
    event_bus: RwLock<Option<Arc<dyn EventBus>>>,
}

impl ConfigStore {
    /// Create a new empty config store.
    ///
    /// 创建新的空配置存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the event bus used for config change notifications.
    ///
    /// 附加用于配置变更通知的事件总线。
    pub fn set_event_bus(&self, event_bus: Arc<dyn EventBus>) {
        *self.event_bus.write() = Some(event_bus);
    }

    /// Publish a config event to the event bus if configured.
    ///
    /// 若已配置，则向事件总线发布配置事件。
    fn publish_config_event(
        &self,
        scope: String,
        version: u64,
        effect: Option<ConfigEffect>,
        rolled_back: bool,
    ) {
        if let Some(event_bus) = self.event_bus.read().as_ref() {
            event_bus.publish(SystemEvent::Config(ConfigEvent {
                scope,
                version,
                effect,
                rolled_back,
            }));
        }
    }

    /// Register the default config object for a module.
    ///
    /// 注册模块的默认配置对象。
    pub fn register_module_default(&self, module_id: ModuleId, value: Value) {
        let mut state = self.inner.write();
        state.module_default.insert(module_id, value);
        state.version += 1;
    }

    /// Set the global default config object.
    ///
    /// 设置全局默认配置对象。
    pub fn set_global_default(&self, value: Value) {
        let mut state = self.inner.write();
        state.global_default = value;
        state.version += 1;
    }

    /// Parse a YAML string and load the `global` and `modules` sections.
    ///
    /// 解析 YAML 字符串并加载 `global` 与 `modules` 节。
    pub fn load_yaml_str(&self, yaml: &str) -> Result<(), SdkError> {
        let parsed: Value = serde_yaml::from_str(yaml)
            .map_err(|e| SdkError::InvalidArgument(format!("yaml parse error: {e}")))?;

        let mut state = self.inner.write();
        if let Some(global) = parsed.get("global") {
            state.global_file = global.clone();
        }

        if let Some(modules) = parsed.get("modules").and_then(Value::as_object) {
            for (module, value) in modules {
                state
                    .module_file
                    .insert(ModuleId::new(module.clone()), value.clone());
            }
        }
        state.version += 1;
        Ok(())
    }

    /// Load environment variables matching `<prefix>GLOBAL__*` and `<prefix>MODULE__<module>__*`.
    ///
    /// 加载匹配 `<prefix>GLOBAL__*` 与 `<prefix>MODULE__<module>__*` 的环境变量。
    pub fn load_env(&self, prefix: &str) {
        let mut state = self.inner.write();
        let global_prefix = format!("{prefix}GLOBAL__");
        let module_prefix = format!("{prefix}MODULE__");

        for (key, value) in env::vars() {
            if let Some(path) = key.strip_prefix(&global_prefix) {
                let path = parse_path(path);
                insert_path(&mut state.global_env, &path, env_value_to_json(&value));
                continue;
            }

            if let Some(rest) = key.strip_prefix(&module_prefix) {
                let mut segs = rest.split("__");
                if let Some(module) = segs.next() {
                    let module_id = ModuleId::new(module.to_lowercase());
                    let remain = segs.collect::<Vec<_>>().join("__");
                    let path = parse_path(&remain);
                    let entry = state
                        .module_env
                        .entry(module_id)
                        .or_insert_with(|| Value::Object(Map::new()));
                    insert_path(entry, &path, env_value_to_json(&value));
                }
            }
        }

        state.version += 1;
    }

    /// Compute the effective global value by merging default, file, env, and runtime layers.
    ///
    /// 通过合并 default、file、env 与 runtime 层计算最终全局值。
    fn effective_global(state: &ConfigState) -> Value {
        let mut out = state.global_default.clone();
        merge_value(&mut out, state.global_file.clone());
        merge_value(&mut out, state.global_env.clone());
        merge_value(&mut out, state.global_runtime.clone());
        out
    }

    /// Compute the effective module value by merging default, file, env, and runtime layers.
    ///
    /// 通过合并 default、file、env 与 runtime 层计算最终模块值。
    fn effective_module(state: &ConfigState, module_id: &ModuleId) -> Value {
        let mut out = state
            .module_default
            .get(module_id)
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));

        if let Some(v) = state.module_file.get(module_id).cloned() {
            merge_value(&mut out, v);
        }
        if let Some(v) = state.module_env.get(module_id).cloned() {
            merge_value(&mut out, v);
        }
        if let Some(v) = state.module_runtime.get(module_id).cloned() {
            merge_value(&mut out, v);
        }
        out
    }

    /// Collect all module IDs that have a default, file, env, runtime, or schema entry.
    ///
    /// 收集所有在 default、file、env、runtime 或 schema 中有条目的模块 ID。
    fn module_ids(state: &ConfigState) -> Vec<ModuleId> {
        let mut out = BTreeSet::new();
        for id in state.module_default.keys() {
            out.insert(id.clone());
        }
        for id in state.module_file.keys() {
            out.insert(id.clone());
        }
        for id in state.module_env.keys() {
            out.insert(id.clone());
        }
        for id in state.module_runtime.keys() {
            out.insert(id.clone());
        }
        for id in state.module_schemas.keys() {
            out.insert(id.clone());
        }
        out.into_iter().collect()
    }

    /// Run the global schema validator against the proposed value.
    ///
    /// 用全局 schema 校验器校验候选值。
    fn validate_global(state: &ConfigState, value: &Value) -> Result<(), SdkError> {
        if let Some(schema) = &state.global_schema {
            if let Some(validator) = &schema.validator {
                validator(value).map_err(SdkError::InvalidArgument)?;
            }
        }
        Ok(())
    }

    /// Run a module schema validator against the proposed value.
    ///
    /// 用模块 schema 校验器校验候选值。
    fn validate_module(
        state: &ConfigState,
        module_id: &ModuleId,
        value: &Value,
    ) -> Result<(), SdkError> {
        if let Some(schema) = state.module_schemas.get(module_id) {
            if let Some(validator) = &schema.validator {
                validator(value).map_err(SdkError::InvalidArgument)?;
            }
        }
        Ok(())
    }
}

/// `ConfigProvider` implementation: expose effective global and module values.
///
/// `ConfigProvider` 实现：暴露最终全局与模块值。
impl ConfigProvider for ConfigStore {
    fn global(&self) -> Value {
        let state = self.inner.read();
        Self::effective_global(&state)
    }

    fn module(&self, module_id: &ModuleId) -> Value {
        let state = self.inner.read();
        Self::effective_module(&state, module_id)
    }

    fn version(&self) -> u64 {
        self.inner.read().version
    }
}

/// `ConfigSchemaRegistry` implementation: register schemas and defaults.
///
/// `ConfigSchemaRegistry` 实现：注册 schema 与默认值。
impl ConfigSchemaRegistry for ConfigStore {
    fn register_global_schema(
        &self,
        schema_name: &str,
        default_value: Value,
        validator: Option<ConfigValidator>,
    ) -> Result<(), SdkError> {
        let mut state = self.inner.write();
        state.global_default = default_value;
        state.global_schema = Some(SchemaEntry {
            schema_name: schema_name.to_string(),
            validator,
        });
        state.version += 1;
        Ok(())
    }

    fn register_module_schema(
        &self,
        module_id: ModuleId,
        schema_name: &str,
        default_value: Value,
        validator: Option<ConfigValidator>,
    ) -> Result<(), SdkError> {
        let mut state = self.inner.write();
        state
            .module_default
            .insert(module_id.clone(), default_value);
        state.module_schemas.insert(
            module_id,
            SchemaEntry {
                schema_name: schema_name.to_string(),
                validator,
            },
        );
        state.version += 1;
        Ok(())
    }

    fn list_schemas(&self) -> Vec<RegisteredSchema> {
        let state = self.inner.read();
        let mut out = Vec::new();
        if let Some(schema) = &state.global_schema {
            out.push(RegisteredSchema {
                scope: "global".to_string(),
                schema_name: schema.schema_name.clone(),
            });
        }
        for (module_id, schema) in &state.module_schemas {
            out.push(RegisteredSchema {
                scope: format!("module:{}", module_id.0),
                schema_name: schema.schema_name.clone(),
            });
        }
        out
    }
}

/// `ConfigApplyApi` implementation: apply patches with validation and rollback tokens.
///
/// `ConfigApplyApi` 实现：在校验后应用补丁并生成回滚 token。
impl ConfigApplyApi for ConfigStore {
    /// Apply a global runtime patch and emit per-module config changes.
    ///
    /// 应用全局运行时补丁，并为每个模块生成配置变更。
    fn apply_global_patch(
        &self,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyOutcome, SdkError> {
        let mut state = self.inner.write();
        let previous_global = Self::effective_global(&state);
        let previous_global_runtime = state.global_runtime.clone();
        let previous_module_runtime = Self::module_ids(&state)
            .into_iter()
            .map(|module_id| {
                (
                    module_id.clone(),
                    state.module_runtime.get(&module_id).cloned(),
                )
            })
            .collect::<Vec<_>>();
        let previous_modules = Self::module_ids(&state)
            .into_iter()
            .map(|module_id| {
                (
                    module_id.clone(),
                    Self::effective_module(&state, &module_id),
                    previous_global.clone(),
                )
            })
            .collect::<Vec<_>>();

        merge_value(&mut state.global_runtime, patch);
        let next_global = Self::effective_global(&state);
        if let Err(err) = Self::validate_global(&state, &next_global) {
            state.global_runtime = previous_global_runtime;
            return Err(err);
        }

        let mut module_changes = Vec::new();
        for (module_id, previous_module, previous_global_snapshot) in previous_modules {
            let next_module = Self::effective_module(&state, &module_id);
            if let Err(err) = Self::validate_module(&state, &module_id, &next_module) {
                state.global_runtime = previous_global_runtime;
                return Err(err);
            }
            module_changes.push(ModuleConfigChange {
                module_id,
                previous: previous_module,
                next: next_module,
                previous_global: Some(previous_global_snapshot.clone()),
                next_global: Some(next_global.clone()),
            });
        }

        state.version += 1;
        let version = state.version;
        drop(state);
        self.publish_config_event("global".to_string(), version, Some(effect), false);
        Ok(ConfigApplyOutcome {
            version,
            effect,
            global_change: Some(ConfigValueChange {
                previous: previous_global,
                next: next_global,
            }),
            module_changes,
            rollback_token: Some(ConfigRollbackToken {
                previous_global_runtime: Some(previous_global_runtime),
                previous_module_runtime,
            }),
        })
    }

    /// Apply a module runtime patch and emit the resulting config change.
    ///
    /// 应用模块运行时补丁并生成对应的配置变更。
    fn apply_module_patch(
        &self,
        module_id: &ModuleId,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyOutcome, SdkError> {
        let mut state = self.inner.write();
        let previous_global = Self::effective_global(&state);
        let previous_module = Self::effective_module(&state, module_id);

        let previous_runtime = state.module_runtime.get(module_id).cloned();
        let entry = state
            .module_runtime
            .entry(module_id.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        merge_value(entry, patch);
        let next_module = Self::effective_module(&state, module_id);

        if let Err(err) = Self::validate_module(&state, module_id, &next_module) {
            match previous_runtime {
                Some(value) => {
                    state.module_runtime.insert(module_id.clone(), value);
                }
                None => {
                    state.module_runtime.remove(module_id);
                }
            }
            return Err(err);
        }

        state.version += 1;
        let version = state.version;
        drop(state);
        self.publish_config_event(
            format!("module:{}", module_id.0),
            version,
            Some(effect),
            false,
        );
        Ok(ConfigApplyOutcome {
            version,
            effect,
            global_change: None,
            module_changes: vec![ModuleConfigChange {
                module_id: module_id.clone(),
                previous: previous_module,
                next: next_module,
                previous_global: Some(previous_global.clone()),
                next_global: Some(previous_global),
            }],
            rollback_token: Some(ConfigRollbackToken {
                previous_global_runtime: None,
                previous_module_runtime: vec![(module_id.clone(), previous_runtime)],
            }),
        })
    }

    /// Restore runtime values from a rollback token.
    ///
    /// 用回滚 token 恢复运行时值。
    fn rollback(&self, token: ConfigRollbackToken) -> Result<(), SdkError> {
        let mut state = self.inner.write();
        if let Some(previous_global_runtime) = token.previous_global_runtime {
            state.global_runtime = previous_global_runtime;
        }
        for (module_id, previous_runtime) in token.previous_module_runtime {
            match previous_runtime {
                Some(value) => {
                    state.module_runtime.insert(module_id, value);
                }
                None => {
                    state.module_runtime.remove(&module_id);
                }
            }
        }
        state.version += 1;
        let version = state.version;
        drop(state);
        self.publish_config_event("rollback".to_string(), version, None, true);
        Ok(())
    }
}

/// `ConfigAdminApi` implementation: administrative entry points for config patches.
///
/// `ConfigAdminApi` 实现：配置补丁的管理入口。
impl ConfigAdminApi for ConfigStore {
    fn patch_global(
        &self,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyResult, SdkError> {
        let outcome = self.apply_global_patch(patch, effect)?;
        Ok(ConfigApplyResult {
            version: outcome.version,
            effect: outcome.effect,
        })
    }

    fn patch_module(
        &self,
        module_id: &ModuleId,
        patch: Value,
        effect: ConfigEffect,
    ) -> Result<ConfigApplyResult, SdkError> {
        let outcome = self.apply_module_patch(module_id, patch, effect)?;
        Ok(ConfigApplyResult {
            version: outcome.version,
            effect: outcome.effect,
        })
    }
}

/// Deep-merge `patch` into `base`; objects are merged recursively, other values are replaced.
///
/// 将 `patch` 深度合并到 `base`；对象递归合并，其他值直接替换。
fn merge_value(base: &mut Value, patch: Value) {
    match (base, patch) {
        (_, Value::Null) => {}
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                if let Some(existing) = base_map.get_mut(&k) {
                    merge_value(existing, v);
                } else {
                    base_map.insert(k, v);
                }
            }
        }
        (base_value, patch_value) => {
            *base_value = patch_value;
        }
    }
}

/// Convert an environment variable string into a JSON bool/number/string value.
///
/// 将环境变量字符串转换为 JSON 布尔/数字/字符串值。
fn env_value_to_json(input: &str) -> Value {
    if input.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if input.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(v) = input.parse::<i64>() {
        return Value::Number(v.into());
    }
    if let Ok(v) = input.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(v) {
            return Value::Number(num);
        }
    }
    Value::String(input.to_string())
}

/// Split a `__`-separated path and normalize each segment to lowercase ASCII.
///
/// 按 `__` 拆分路径并将每段规范化为小写 ASCII。
fn parse_path(path: &str) -> Vec<String> {
    path.split("__")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Insert a value into a nested JSON object by a path of keys, creating objects as needed.
///
/// 按路径将值插入嵌套 JSON 对象，必要时创建中间对象。
fn insert_path(root: &mut Value, path: &[String], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }

    if !root.is_object() {
        *root = Value::Object(Map::new());
    }

    let mut cursor = root;
    for key in &path[..path.len() - 1] {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        let Some(map) = cursor.as_object_mut() else {
            return;
        };
        cursor = map
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }

    let Some(leaf) = path.last() else {
        return;
    };
    let Some(map) = cursor.as_object_mut() else {
        return;
    };
    map.insert(leaf.clone(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_generates_changes() {
        let store = ConfigStore::new();
        store
            .register_global_schema("global", serde_json::json!({"a": 1}), None)
            .expect("schema");
        store
            .register_module_schema(
                ModuleId::new("noop-test"),
                "noop",
                serde_json::json!({"enabled": true}),
                None,
            )
            .expect("module schema");

        let outcome = store
            .apply_global_patch(serde_json::json!({"a": 2}), ConfigEffect::Immediate)
            .expect("global patch");
        assert_eq!(
            outcome
                .global_change
                .expect("global change")
                .next
                .get("a")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert_eq!(outcome.module_changes.len(), 1);
    }

    #[test]
    fn rollback_restores_previous_runtime_values() {
        let store = ConfigStore::new();
        store
            .register_global_schema("global", serde_json::json!({"a": 1}), None)
            .expect("schema");
        store
            .register_module_schema(
                ModuleId::new("noop-test"),
                "noop",
                serde_json::json!({"enabled": true}),
                None,
            )
            .expect("module schema");

        let outcome = store
            .apply_global_patch(serde_json::json!({"a": 9}), ConfigEffect::Immediate)
            .expect("global patch");
        assert_eq!(
            store.global().get("a").and_then(Value::as_i64),
            Some(9),
            "global should be patched before rollback"
        );
        store
            .rollback(outcome.rollback_token.expect("rollback token"))
            .expect("rollback");
        assert_eq!(store.global().get("a").and_then(Value::as_i64), Some(1));
    }
}
