use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_sdk::{
    CancellationToken, ConfigEffect, ConfigProvider, EngineContext, HttpRouteMount,
    ModuleConfigApplyReport, ModuleConfigChange, ModuleEvent, ModuleEventKind, ModuleFactory,
    ModuleId, ModuleInitContext, ModuleManagerApi, ModuleManifest, ModuleState, SdkError,
    SystemEvent,
};
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// Owned module instance held by the module manager.
///
/// 模块管理器持有的模块实例。
struct ModuleRecord {
    module: Box<dyn cheetah_sdk::Module>,
}

#[derive(Clone)]
/// Runtime snapshot captured during `init_all` and used for rebuilds.
///
/// 在 `init_all` 期间捕获、用于重建的运行时快照。
struct RuntimeState {
    context: EngineContext,
    config: Arc<dyn ConfigProvider>,
    root_cancel: Option<CancellationToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Target lifecycle state for a module rebuild.
///
/// 模块重建的目标生命周期状态。
enum RebuildTarget {
    Initialized,
    Running,
}

#[derive(Default)]
/// Owns module factories, lifecycles, HTTP mounts, and dependency ordering.
///
/// 拥有模块工厂、生命周期、HTTP 挂载和依赖顺序。
pub struct ModuleManager {
    factories: RwLock<HashMap<ModuleId, Arc<dyn ModuleFactory>>>,
    manifests: RwLock<HashMap<ModuleId, ModuleManifest>>,
    records: Mutex<HashMap<ModuleId, ModuleRecord>>,
    states: DashMap<ModuleId, ModuleState>,
    http_mounts: RwLock<HashMap<ModuleId, HttpRouteMount>>,
    runtime: RwLock<Option<RuntimeState>>,
}

impl ModuleManager {
    /// Register a module factory and record its manifest.
    ///
    /// 注册模块工厂并记录其清单。
    pub fn register_factory(&self, factory: Arc<dyn ModuleFactory>) -> Result<(), SdkError> {
        let manifest = factory.manifest();
        let module_id = manifest.module_id.clone();
        let mut factories = self.factories.write();

        if factories.contains_key(&module_id) {
            return Err(SdkError::AlreadyExists(format!("module {}", module_id)));
        }

        self.manifests.write().insert(module_id.clone(), manifest);
        factories.insert(module_id.clone(), factory);
        self.states.insert(module_id, ModuleState::Created);
        Ok(())
    }

    fn manifests(&self) -> Vec<ModuleManifest> {
        self.manifests.read().values().cloned().collect()
    }

    fn manifest_of(&self, module_id: &ModuleId) -> Option<ModuleManifest> {
        self.manifests.read().get(module_id).cloned()
    }

    fn factory_of(&self, module_id: &ModuleId) -> Option<Arc<dyn ModuleFactory>> {
        self.factories.read().get(module_id).cloned()
    }

    fn state_of(&self, module_id: &ModuleId) -> Result<ModuleState, SdkError> {
        self.states
            .get(module_id)
            .map(|v| *v)
            .ok_or_else(|| SdkError::NotFound(format!("module {}", module_id)))
    }

    /// Compute module initialization order using Kahn's topological sort.
    ///
    /// 使用 Kahn 拓扑排序计算模块初始化顺序。
    fn topo_order(&self) -> Result<Vec<ModuleId>, SdkError> {
        let manifests = self.manifests();
        let mut indegree: HashMap<ModuleId, usize> = HashMap::new();
        let mut graph: HashMap<ModuleId, Vec<ModuleId>> = HashMap::new();

        for manifest in &manifests {
            indegree.entry(manifest.module_id.clone()).or_insert(0);
        }

        for manifest in &manifests {
            for dep in &manifest.dependencies {
                if !indegree.contains_key(dep) {
                    return Err(SdkError::InvalidArgument(format!(
                        "module {} depends on missing module {}",
                        manifest.module_id, dep
                    )));
                }
                graph
                    .entry(dep.clone())
                    .or_default()
                    .push(manifest.module_id.clone());
                *indegree.entry(manifest.module_id.clone()).or_insert(0) += 1;
            }
        }

        let mut queue = VecDeque::new();
        for (module_id, degree) in &indegree {
            if *degree == 0 {
                queue.push_back(module_id.clone());
            }
        }

        let mut order = Vec::new();
        while let Some(module_id) = queue.pop_front() {
            order.push(module_id.clone());
            if let Some(edges) = graph.get(&module_id) {
                for next in edges {
                    if let Some(degree) = indegree.get_mut(next) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(next.clone());
                        }
                    }
                }
            }
        }

        if order.len() != indegree.len() {
            return Err(SdkError::Conflict(
                "module dependency cycle detected".to_string(),
            ));
        }

        Ok(order)
    }

    /// Create `ModuleRecord`s for every registered factory if not already present.
    ///
    /// 为每个已注册工厂创建 `ModuleRecord`（若不存在）。
    async fn ensure_records(&self) {
        let factories = self.factories.read().clone();
        let mut records = self.records.lock().await;

        for (module_id, factory) in factories {
            records.entry(module_id).or_insert_with(|| ModuleRecord {
                module: factory.create(),
            });
        }
    }

    /// Remove and return a module record, preventing concurrent lifecycle access.
    ///
    /// 移除并返回模块记录，避免并发生命周期访问。
    async fn take_record(&self, module_id: &ModuleId) -> Result<ModuleRecord, SdkError> {
        let mut records = self.records.lock().await;
        records
            .remove(module_id)
            .ok_or_else(|| SdkError::NotFound(format!("module {}", module_id)))
    }

    /// Return a module record to the map after lifecycle work completes.
    ///
    /// 生命周期操作完成后将模块记录放回映射。
    async fn put_record(&self, module_id: ModuleId, record: ModuleRecord) {
        let mut records = self.records.lock().await;
        records.insert(module_id, record);
    }

    /// Publish a module state event to the event bus.
    ///
    /// 向事件总线发布模块状态事件。
    fn publish_state(&self, context: &EngineContext, module_id: &ModuleId, kind: ModuleEventKind) {
        let state = self.states.get(module_id).map(|entry| *entry.value());
        context.event_bus.publish(SystemEvent::Module(ModuleEvent {
            module_id: module_id.0.clone(),
            kind,
            state,
            effect: None,
            error: None,
        }));
    }

    /// Publish a module failure event with the phase that produced the error.
    ///
    /// 发布模块失败事件，并说明产生错误的阶段。
    fn publish_failed(
        &self,
        context: &EngineContext,
        module_id: &ModuleId,
        phase: &str,
        err: &SdkError,
    ) {
        let state = self.states.get(module_id).map(|entry| *entry.value());
        context.event_bus.publish(SystemEvent::Module(ModuleEvent {
            module_id: module_id.0.clone(),
            kind: ModuleEventKind::Failed,
            state,
            effect: None,
            error: Some(format!("{phase}: {err}")),
        }));
    }

    /// Publish a module config-applied event with the resulting effect.
    ///
    /// 发布模块配置已应用事件，并附带产生的效果。
    fn publish_config_applied(
        &self,
        context: &EngineContext,
        module_id: &ModuleId,
        effect: ConfigEffect,
    ) {
        let state = self.states.get(module_id).map(|entry| *entry.value());
        context.event_bus.publish(SystemEvent::Module(ModuleEvent {
            module_id: module_id.0.clone(),
            kind: ModuleEventKind::ConfigApplied,
            state,
            effect: Some(effect),
            error: None,
        }));
    }

    /// Return the captured runtime state, or fail if `init_all` has not been called.
    ///
    /// 返回已捕获的运行时状态；若未调用 `init_all` 则失败。
    fn load_runtime(&self) -> Result<RuntimeState, SdkError> {
        self.runtime.read().clone().ok_or_else(|| {
            SdkError::Unavailable("module runtime context not initialized".to_string())
        })
    }

    /// Refresh the HTTP route mount for a module after `init`.
    ///
    /// 在 `init` 后刷新模块的 HTTP 路由挂载。
    fn update_http_mount(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        module: &dyn cheetah_sdk::Module,
    ) {
        if let Some(service) = module.http_service() {
            let mount = HttpRouteMount {
                module_id: module_id.clone(),
                prefix: manifest.routes_prefix.clone(),
                routes: module.http_routes(),
                service,
            };
            self.http_mounts.write().insert(module_id.clone(), mount);
        } else {
            self.http_mounts.write().remove(module_id);
        }
    }

    /// Initialize a module record and install its HTTP routes.
    ///
    /// 初始化模块记录并安装其 HTTP 路由。
    async fn init_record(
        &self,
        module_id: &ModuleId,
        record: &mut ModuleRecord,
        context: &EngineContext,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<(), SdkError> {
        let manifest = self
            .manifest_of(module_id)
            .ok_or_else(|| SdkError::NotFound(format!("manifest for module {}", module_id)))?;
        let init_ctx = ModuleInitContext {
            manifest: manifest.clone(),
            engine: context.clone(),
            initial_config: config.module(module_id),
        };
        record.module.init(init_ctx).await?;
        self.update_http_mount(module_id, &manifest, record.module.as_ref());
        self.states
            .insert(module_id.clone(), ModuleState::Initialized);
        self.publish_state(context, module_id, ModuleEventKind::Initialized);
        Ok(())
    }

    /// Start a module with a child cancellation token.
    ///
    /// 用子取消 token 启动模块。
    async fn start_record(
        &self,
        module_id: &ModuleId,
        record: &mut ModuleRecord,
        context: &EngineContext,
        root_cancel: &CancellationToken,
    ) -> Result<(), SdkError> {
        record.module.start(root_cancel.child_token()).await?;
        self.states.insert(module_id.clone(), ModuleState::Running);
        self.publish_state(context, module_id, ModuleEventKind::Started);
        Ok(())
    }

    /// Stop a module and clean up its HTTP mount.
    ///
    /// 停止模块并清理其 HTTP 挂载。
    async fn stop_record(
        &self,
        module_id: &ModuleId,
        record: &mut ModuleRecord,
        context: &EngineContext,
    ) -> Result<(), SdkError> {
        self.states.insert(module_id.clone(), ModuleState::Stopping);
        self.publish_state(context, module_id, ModuleEventKind::Stopping);
        let stop_res = record.module.stop().await;
        self.http_mounts.write().remove(module_id);
        match stop_res {
            Ok(()) => {
                self.states.insert(module_id.clone(), ModuleState::Stopped);
                self.publish_state(context, module_id, ModuleEventKind::Stopped);
                Ok(())
            }
            Err(err) => {
                self.states.insert(module_id.clone(), ModuleState::Failed);
                self.publish_failed(context, module_id, "stop", &err);
                Err(err)
            }
        }
    }

    /// Stop initialized modules in reverse order when `init_all` fails.
    ///
    /// `init_all` 失败时按相反顺序停止已初始化模块。
    async fn rollback_initialized(
        &self,
        initialized: &[ModuleId],
        context: &EngineContext,
    ) -> Option<String> {
        let mut failures = Vec::new();
        for module_id in initialized.iter().rev() {
            let mut record = match self.take_record(module_id).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Err(err) = self.stop_record(module_id, &mut record, context).await {
                failures.push(format!("{module_id}: {err}"));
            }
            self.put_record(module_id.clone(), record).await;
        }
        if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        }
    }

    /// Stop started modules in reverse order when `start_all` fails.
    ///
    /// `start_all` 失败时按相反顺序停止已启动模块。
    async fn rollback_started(
        &self,
        started: &[ModuleId],
        context: &EngineContext,
    ) -> Option<String> {
        let mut failures = Vec::new();
        for module_id in started.iter().rev() {
            let mut record = match self.take_record(module_id).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Err(err) = self.stop_record(module_id, &mut record, context).await {
                failures.push(format!("{module_id}: {err}"));
            }
            self.put_record(module_id.clone(), record).await;
        }
        if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        }
    }

    /// Stop, recreate, and reinitialize/restart a module to apply config changes.
    ///
    /// 停止、重新创建并重新初始化/启动模块以应用配置变更。
    async fn rebuild_module(
        &self,
        module_id: &ModuleId,
        target: RebuildTarget,
    ) -> Result<(), SdkError> {
        self.ensure_records().await;
        let runtime = self.load_runtime()?;
        let factory = self
            .factory_of(module_id)
            .ok_or_else(|| SdkError::NotFound(format!("module factory {}", module_id)))?;

        let mut old_record = self.take_record(module_id).await?;
        if let Err(err) = self
            .stop_record(module_id, &mut old_record, &runtime.context)
            .await
        {
            self.put_record(module_id.clone(), old_record).await;
            return Err(SdkError::Internal(format!(
                "module rebuild stop failed: {err}"
            )));
        }

        let mut new_record = ModuleRecord {
            module: factory.create(),
        };
        if let Err(err) = self
            .init_record(
                module_id,
                &mut new_record,
                &runtime.context,
                runtime.config.clone(),
            )
            .await
        {
            self.publish_failed(&runtime.context, module_id, "rebuild-init", &err);
            self.states.insert(module_id.clone(), ModuleState::Failed);
            self.put_record(module_id.clone(), new_record).await;
            return Err(err);
        }

        if target == RebuildTarget::Running {
            let root_cancel = runtime.root_cancel.clone().ok_or_else(|| {
                SdkError::Unavailable("root cancellation token not available".to_string())
            })?;
            if let Err(err) = self
                .start_record(module_id, &mut new_record, &runtime.context, &root_cancel)
                .await
            {
                self.publish_failed(&runtime.context, module_id, "rebuild-start", &err);
                let cleanup_err = self
                    .stop_record(module_id, &mut new_record, &runtime.context)
                    .await
                    .err();
                self.states.insert(module_id.clone(), ModuleState::Failed);
                self.put_record(module_id.clone(), new_record).await;
                return Err(match cleanup_err {
                    Some(stop_err) => SdkError::Internal(format!(
                        "module rebuild start failed: {err}; cleanup failed: {stop_err}"
                    )),
                    None => err,
                });
            }
        }

        self.put_record(module_id.clone(), new_record).await;
        Ok(())
    }

    /// Initialize all modules in dependency order.
    ///
    /// 按依赖顺序初始化所有模块。
    pub async fn init_all(
        &self,
        context: EngineContext,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<(), SdkError> {
        self.ensure_records().await;
        *self.runtime.write() = Some(RuntimeState {
            context: context.clone(),
            config: config.clone(),
            root_cancel: None,
        });

        let order = self.topo_order()?;
        let mut initialized = Vec::new();

        for module_id in order {
            let mut record = self.take_record(&module_id).await?;
            match self
                .init_record(&module_id, &mut record, &context, config.clone())
                .await
            {
                Ok(()) => {
                    initialized.push(module_id.clone());
                    self.put_record(module_id, record).await;
                }
                Err(err) => {
                    self.publish_failed(&context, &module_id, "init", &err);
                    let cleanup_err = self
                        .stop_record(&module_id, &mut record, &context)
                        .await
                        .err();
                    self.states.insert(module_id.clone(), ModuleState::Failed);
                    self.put_record(module_id.clone(), record).await;
                    let rollback_err = self.rollback_initialized(&initialized, &context).await;
                    let mut detail = format!("{err}");
                    if let Some(stop_err) = cleanup_err {
                        detail.push_str(&format!("; cleanup failed: {stop_err}"));
                    }
                    if let Some(extra) = rollback_err {
                        detail.push_str(&format!("; rollback failed: {extra}"));
                    }
                    return Err(SdkError::Internal(detail));
                }
            }
        }

        Ok(())
    }

    /// Start all initialized modules in dependency order.
    ///
    /// 按依赖顺序启动所有已初始化模块。
    pub async fn start_all(
        &self,
        context: &EngineContext,
        root_cancel: CancellationToken,
    ) -> Result<(), SdkError> {
        {
            let mut runtime = self.runtime.write();
            let state = runtime.as_mut().ok_or_else(|| {
                SdkError::Unavailable("module runtime context not initialized".to_string())
            })?;
            state.root_cancel = Some(root_cancel.clone());
        }

        let order = self.topo_order()?;
        let mut started = Vec::new();

        for module_id in order {
            let mut record = self.take_record(&module_id).await?;
            match self
                .start_record(&module_id, &mut record, context, &root_cancel)
                .await
            {
                Ok(()) => {
                    started.push(module_id.clone());
                    self.put_record(module_id, record).await;
                }
                Err(err) => {
                    self.publish_failed(context, &module_id, "start", &err);
                    let cleanup_err = self
                        .stop_record(&module_id, &mut record, context)
                        .await
                        .err();
                    self.states.insert(module_id.clone(), ModuleState::Failed);
                    self.put_record(module_id.clone(), record).await;
                    let rollback_err = self.rollback_started(&started, context).await;
                    if let Some(runtime) = self.runtime.write().as_mut() {
                        runtime.root_cancel = None;
                    }
                    let mut detail = format!("{err}");
                    if let Some(stop_err) = cleanup_err {
                        detail.push_str(&format!("; cleanup failed: {stop_err}"));
                    }
                    if let Some(extra) = rollback_err {
                        detail.push_str(&format!("; rollback failed: {extra}"));
                    }
                    return Err(SdkError::Internal(detail));
                }
            }
        }

        Ok(())
    }

    /// Stop all modules in reverse dependency order.
    ///
    /// 按依赖顺序的逆序停止所有模块。
    pub async fn stop_all(&self, context: &EngineContext) {
        let mut order = match self.topo_order() {
            Ok(v) => v,
            Err(_) => self
                .states
                .iter()
                .map(|entry| entry.key().clone())
                .collect::<Vec<_>>(),
        };
        order.reverse();

        for module_id in order {
            let mut record = match self.take_record(&module_id).await {
                Ok(record) => record,
                Err(_) => continue,
            };
            let _ = self.stop_record(&module_id, &mut record, context).await;
            self.put_record(module_id, record).await;
        }

        self.http_mounts.write().clear();
        if let Some(runtime) = self.runtime.write().as_mut() {
            runtime.root_cancel = None;
        }
    }

    /// Rollback already-applied config changes when a later change fails.
    ///
    /// 当后续配置变更失败时回滚已应用的变更。
    async fn rollback_applied_changes(&self, applied: &[ModuleConfigChange]) -> Option<String> {
        let mut failures = Vec::new();
        for change in applied.iter().rev() {
            let rollback_change = ModuleConfigChange {
                module_id: change.module_id.clone(),
                previous: change.next.clone(),
                next: change.previous.clone(),
                previous_global: change.next_global.clone(),
                next_global: change.previous_global.clone(),
            };
            if let Err(err) = self.apply_module_config_change(rollback_change).await {
                failures.push(format!("{}: {err}", change.module_id));
            }
        }
        if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        }
    }
}

/// `ModuleManagerApi` implementation: lifecycle, config, and HTTP mounts.
///
/// `ModuleManagerApi` 实现：生命周期、配置与 HTTP 挂载。
#[async_trait]
impl ModuleManagerApi for ModuleManager {
    fn modules(&self) -> Vec<(ModuleId, ModuleState)> {
        let mut out: Vec<_> = self
            .states
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect();
        out.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        out
    }

    fn http_mounts(&self) -> Vec<HttpRouteMount> {
        self.http_mounts.read().values().cloned().collect()
    }

    /// Apply a single module config change and rebuild if the module requests restart.
    ///
    /// 应用单个模块配置变更；若模块请求重启则重建。
    async fn apply_module_config_change(
        &self,
        change: ModuleConfigChange,
    ) -> Result<ModuleConfigApplyReport, SdkError> {
        let module_id = change.module_id.clone();
        let mut record = self.take_record(&module_id).await?;
        let apply = record.module.apply_config(change).await;
        self.put_record(module_id.clone(), record).await;

        let effect = match apply {
            Ok(v) => v,
            Err(err) => {
                if let Some(runtime) = self.runtime.read().as_ref() {
                    self.publish_failed(&runtime.context, &module_id, "apply_config", &err);
                }
                return Err(err);
            }
        };

        if effect == ConfigEffect::ModuleRestartRequired {
            let state = self.state_of(&module_id)?;
            let target = match state {
                ModuleState::Running => RebuildTarget::Running,
                ModuleState::Initialized => RebuildTarget::Initialized,
                _ => {
                    let err = SdkError::Conflict(format!(
                        "module {} is in state {:?}, restart required config can only apply to Running or Initialized module",
                        module_id, state
                    ));
                    if let Some(runtime) = self.runtime.read().as_ref() {
                        self.publish_failed(&runtime.context, &module_id, "config-restart", &err);
                    }
                    return Err(err);
                }
            };
            self.rebuild_module(&module_id, target).await?;
        }

        if let Some(runtime) = self.runtime.read().as_ref() {
            self.publish_config_applied(&runtime.context, &module_id, effect);
        }

        Ok(ModuleConfigApplyReport { module_id, effect })
    }

    /// Apply ordered module config changes and rollback on first failure.
    ///
    /// 按顺序应用模块配置变更，第一次失败时回滚。
    async fn apply_module_config_changes(
        &self,
        changes: Vec<ModuleConfigChange>,
    ) -> Result<Vec<ModuleConfigApplyReport>, SdkError> {
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_id = HashMap::new();
        for change in changes {
            by_id.insert(change.module_id.clone(), change);
        }

        let mut ordered_changes = Vec::new();
        let order = self.topo_order()?;
        for module_id in order {
            if let Some(change) = by_id.remove(&module_id) {
                ordered_changes.push(change);
            }
        }
        for (_, change) in by_id {
            ordered_changes.push(change);
        }

        let mut out = Vec::new();
        let mut applied = Vec::new();
        for change in ordered_changes {
            match self.apply_module_config_change(change.clone()).await {
                Ok(report) => {
                    applied.push(change);
                    out.push(report);
                }
                Err(err) => {
                    let rollback_err = self.rollback_applied_changes(&applied).await;
                    return match rollback_err {
                        Some(rollback_err) => Err(SdkError::Internal(format!(
                            "apply module config changes failed: {err}; rollback failed: {rollback_err}"
                        ))),
                        None => Err(err),
                    };
                }
            }
        }
        Ok(out)
    }

    /// Restart a single module in `Running` state.
    ///
    /// 重启处于 `Running` 状态的单个模块。
    async fn restart_module(&self, module_id: &ModuleId) -> Result<(), SdkError> {
        let state = self.state_of(module_id)?;
        if state != ModuleState::Running {
            return Err(SdkError::Conflict(format!(
                "restart requires Running state, current state is {:?}",
                state
            )));
        }
        self.rebuild_module(module_id, RebuildTarget::Running).await
    }

    /// Restart multiple modules in dependency order.
    ///
    /// 按依赖顺序重启多个模块。
    async fn restart_modules(&self, module_ids: Vec<ModuleId>) -> Result<(), SdkError> {
        if module_ids.is_empty() {
            return Ok(());
        }

        let mut wanted: HashSet<_> = module_ids.into_iter().collect();
        for module_id in &wanted {
            let state = self.state_of(module_id)?;
            if state != ModuleState::Running {
                return Err(SdkError::Conflict(format!(
                    "restart requires Running state, module {} is {:?}",
                    module_id, state
                )));
            }
        }

        let order = self.topo_order()?;
        for module_id in order {
            if wanted.remove(&module_id) {
                self.rebuild_module(&module_id, RebuildTarget::Running)
                    .await?;
            }
        }
        if let Some(unknown) = wanted.into_iter().next() {
            return Err(SdkError::NotFound(format!("module {}", unknown)));
        }
        Ok(())
    }
}
