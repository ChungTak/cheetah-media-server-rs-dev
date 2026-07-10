use std::sync::Arc;

use cheetah_sdk::{
    CancellationToken, ClusterApi, ConfigApplyApi, ConfigProvider, ConfigSchemaRegistry,
    CoreAdaptersApi, DatabaseApi, EngineContext, EventBus, FfmpegApi, HealthApi, MetricsApi,
    ModuleFactory, ModuleManagerApi, PublisherApi, RoomServiceApi, RuntimeApi, SdkError,
    ServiceRegistry, StreamManagerApi, SubscriberApi, SystemEvent, SystemLifecycleEvent,
    TaskSystemApi,
};
use parking_lot::RwLock;

use crate::cluster::LocalCluster;
use crate::core_adapters::LocalCoreAdapters;
use crate::database::InMemoryDatabase;
use crate::event::LocalEventBus;
use crate::ffmpeg::LocalFfmpegService;
use crate::health::HealthService;
use crate::metrics::MetricsRegistry;
use crate::module_manager::ModuleManager;
use crate::proxy::LocalProxyManager;
use crate::room::RoomService;
use crate::service_registry::InMemoryServiceRegistry;
use crate::stream::{DispatcherMode, StreamManager};
use crate::task::TaskSystem;

/// Builder for constructing an [`Engine`] instance.
///
/// Allows configuration of the event bus capacity, ring buffer capacity,
/// dispatcher mode, and the set of module factories before building.
///
/// 用于构建 [`Engine`] 实例的构建器。
///
/// 在构建前允许配置事件总线容量、环形缓冲区容量、调度器模式和模块工厂集合。
pub struct EngineBuilder {
    /// `config_provider` field.
    /// `config_provider` 字段.
    config_provider: Arc<dyn ConfigProvider>,
    /// `config_apply_api` field.
    /// `config_apply_api` 字段.
    config_apply_api: Arc<dyn ConfigApplyApi>,
    /// `runtime_api` field.
    /// `runtime_api` 字段.
    runtime_api: Arc<dyn RuntimeApi>,
    /// `config_schema_registry` field.
    /// `config_schema_registry` 字段.
    config_schema_registry: Option<Arc<dyn ConfigSchemaRegistry>>,
    /// `event_bus_capacity` field of type `usize`.
    /// `event_bus_capacity` 字段，类型为 `usize`.
    event_bus_capacity: usize,
    /// `ring_capacity` field of type `usize`.
    /// `ring_capacity` 字段，类型为 `usize`.
    ring_capacity: usize,
    /// `dispatcher_mode` field of type `DispatcherMode`.
    /// `dispatcher_mode` 字段，类型为 `DispatcherMode`.
    dispatcher_mode: DispatcherMode,
    /// `factories` field.
    /// `factories` 字段.
    factories: Vec<Arc<dyn ModuleFactory>>,
}

impl EngineBuilder {
    /// Create a new builder with the required config and runtime APIs.
    /// 使用必需的配置和运行时 API 创建新构建器。
    pub fn new(
        config_provider: Arc<dyn ConfigProvider>,
        config_apply_api: Arc<dyn ConfigApplyApi>,
        runtime_api: Arc<dyn RuntimeApi>,
    ) -> Self {
        Self {
            config_provider,
            config_apply_api,
            runtime_api,
            config_schema_registry: None,
            event_bus_capacity: 1024,
            ring_capacity: 2048,
            dispatcher_mode: DispatcherMode::PerStream,
            factories: Vec::new(),
        }
    }

    /// Set the event bus capacity and return `self`.
    /// 设置事件总线容量并返回 `self`。
    pub fn with_event_bus_capacity(mut self, capacity: usize) -> Self {
        self.event_bus_capacity = capacity.max(1);
        self
    }

    /// Set the stream ring buffer capacity and return `self`.
    /// 设置流环形缓冲区容量并返回 `self`。
    pub fn with_ring_capacity(mut self, capacity: usize) -> Self {
        self.ring_capacity = capacity.max(128);
        self
    }

    /// Set the dispatcher mode and return `self`.
    /// 设置调度器模式并返回 `self`。
    pub fn with_dispatcher_mode(mut self, mode: DispatcherMode) -> Self {
        self.dispatcher_mode = mode;
        self
    }

    /// Register a config schema registry and return `self`.
    /// 注册配置 schema 注册表并返回 `self`。
    pub fn with_config_schema_registry(mut self, registry: Arc<dyn ConfigSchemaRegistry>) -> Self {
        self.config_schema_registry = Some(registry);
        self
    }

    /// Register a module factory for this engine.
    /// 为本引擎注册一个模块工厂。
    pub fn register_module_factory(mut self, factory: Arc<dyn ModuleFactory>) -> Self {
        self.factories.push(factory);
        self
    }

    /// Build the engine and wire all internal services.
    /// 构建引擎并连接所有内部服务。
    pub fn build(self) -> Result<Engine, SdkError> {
        let event_bus = Arc::new(LocalEventBus::new(self.event_bus_capacity));
        let task_system = Arc::new(TaskSystem::default());
        task_system.set_event_bus(event_bus.clone());

        let stream_manager = Arc::new(StreamManager::new(
            self.dispatcher_mode,
            self.ring_capacity,
            self.runtime_api.clone(),
        ));
        stream_manager.set_event_bus(event_bus.clone());

        let module_manager = Arc::new(ModuleManager::default());
        let room_service = Arc::new(RoomService::default());
        let metrics = Arc::new(MetricsRegistry::default());
        let health = Arc::new(HealthService::default());

        let service_registry = Arc::new(InMemoryServiceRegistry::default());
        let database = Arc::new(InMemoryDatabase::default());
        let proxy_manager = Arc::new(LocalProxyManager::default());
        let cluster = Arc::new(LocalCluster::default());
        let ffmpeg = Arc::new(LocalFfmpegService::default());
        let core_adapters = Arc::new(LocalCoreAdapters::new(stream_manager.clone()));

        if let Some(registry) = &self.config_schema_registry {
            for factory in &self.factories {
                if let Some(schema) = factory.config_schema() {
                    registry.register_module_schema_entry(schema)?;
                }
            }
        }

        for factory in self.factories {
            module_manager.register_factory(factory)?;
        }

        Ok(Engine {
            config_provider: self.config_provider,
            config_apply_api: self.config_apply_api,
            runtime_api: self.runtime_api,
            event_bus,
            task_system,
            stream_manager,
            module_manager,
            room_service,
            metrics,
            health,
            service_registry,
            database,
            proxy_manager,
            cluster,
            ffmpeg,
            core_adapters,
            root_cancel: RwLock::new(CancellationToken::new()),
        })
    }
}

/// Central orchestration of the media server.
///
/// `Engine` owns the runtime-neutral service implementations (stream manager,
/// module manager, task system, etc.) and exposes typed API traits. It is
/// responsible for initializing, starting, and stopping modules.
///
/// 媒体服务器的中央编排器。
///
/// `Engine` 拥有运行时无关的服务实现（流管理器、模块管理器、任务系统等）
/// 并暴露类型化的 API trait。它负责初始化、启动和停止模块。
pub struct Engine {
    /// `config_provider` field.
    /// `config_provider` 字段.
    config_provider: Arc<dyn ConfigProvider>,
    /// `config_apply_api` field.
    /// `config_apply_api` 字段.
    config_apply_api: Arc<dyn ConfigApplyApi>,
    /// `runtime_api` field.
    /// `runtime_api` 字段.
    runtime_api: Arc<dyn RuntimeApi>,
    /// `event_bus` field.
    /// `event_bus` 字段.
    event_bus: Arc<LocalEventBus>,
    /// `task_system` field.
    /// `task_system` 字段.
    task_system: Arc<TaskSystem>,
    /// `stream_manager` field.
    /// `stream_manager` 字段.
    stream_manager: Arc<StreamManager>,
    /// `module_manager` field.
    /// `module_manager` 字段.
    module_manager: Arc<ModuleManager>,
    /// `room_service` field.
    /// `room_service` 字段.
    room_service: Arc<RoomService>,
    /// `metrics` field.
    /// `metrics` 字段.
    metrics: Arc<MetricsRegistry>,
    /// `health` field.
    /// `health` 字段.
    health: Arc<HealthService>,
    /// `service_registry` field.
    /// `service_registry` 字段.
    service_registry: Arc<InMemoryServiceRegistry>,
    /// `database` field.
    /// `database` 字段.
    database: Arc<InMemoryDatabase>,
    /// `proxy_manager` field.
    /// `proxy_manager` 字段.
    proxy_manager: Arc<LocalProxyManager>,
    /// `cluster` field.
    /// `cluster` 字段.
    cluster: Arc<LocalCluster>,
    /// `ffmpeg` field.
    /// `ffmpeg` 字段.
    ffmpeg: Arc<LocalFfmpegService>,
    /// `core_adapters` field.
    /// `core_adapters` 字段.
    core_adapters: Arc<LocalCoreAdapters>,
    /// `root_cancel` field.
    /// `root_cancel` 字段.
    root_cancel: RwLock<CancellationToken>,
}

impl Engine {
    fn context(&self) -> EngineContext {
        EngineContext {
            runtime_api: self.runtime_api.clone(),
            publisher_api: self.stream_manager.clone(),
            subscriber_api: self.stream_manager.clone(),
            core_adapters_api: self.core_adapters.clone(),
            stream_manager_api: self.stream_manager.clone(),
            task_system_api: self.task_system.clone(),
            event_bus: self.event_bus.clone(),
            config_provider: self.config_provider.clone(),
            config_apply_api: self.config_apply_api.clone(),
            module_manager_api: Arc::downgrade(&self.module_manager)
                as std::sync::Weak<dyn ModuleManagerApi>,
            room_service_api: self.room_service.clone(),
            metrics_api: self.metrics.clone(),
            health_api: self.health.clone(),
            service_registry: self.service_registry.clone(),
            database_api: self.database.clone(),
            proxy_manager: self.proxy_manager.clone(),
            cluster_api: self.cluster.clone(),
            ffmpeg_api: self.ffmpeg.clone(),
        }
    }

    /// Initialize and start all modules.
    /// 初始化并启动所有模块。
    pub async fn start(&self) -> Result<(), SdkError> {
        if self.health.is_live() {
            return Err(SdkError::Conflict("engine is already running".to_string()));
        }
        self.health.set_live(true);
        self.health.set_ready(false);

        let context = self.context();

        if let Err(err) = self
            .module_manager
            .init_all(context.clone(), self.config_provider.clone())
            .await
        {
            self.health.set_live(false);
            self.health.set_ready(false);
            self.event_bus
                .publish(SystemEvent::System(SystemLifecycleEvent {
                    component: "engine".to_string(),
                    phase: "start_failed".to_string(),
                    message: Some(format!("init_all: {err}")),
                }));
            return Err(err);
        }
        let child_cancel = {
            let mut root_cancel = self.root_cancel.write();
            if root_cancel.is_cancelled() {
                *root_cancel = CancellationToken::new();
            }
            root_cancel.child_token()
        };
        if let Err(err) = self.module_manager.start_all(&context, child_cancel).await {
            self.health.set_live(false);
            self.health.set_ready(false);
            self.event_bus
                .publish(SystemEvent::System(SystemLifecycleEvent {
                    component: "engine".to_string(),
                    phase: "start_failed".to_string(),
                    message: Some(format!("start_all: {err}")),
                }));
            return Err(err);
        }

        self.health.set_ready(true);
        self.event_bus
            .publish(SystemEvent::System(SystemLifecycleEvent {
                component: "engine".to_string(),
                phase: "started".to_string(),
                message: None,
            }));

        Ok(())
    }

    /// Stop all modules and release engine resources.
    /// 停止所有模块并释放引擎资源。
    pub async fn stop(&self) {
        if !self.health.is_live() {
            return;
        }
        self.health.set_ready(false);
        self.root_cancel.read().cancel();
        let context = self.context();
        self.module_manager.stop_all(&context).await;
        self.health.set_live(false);
        self.event_bus
            .publish(SystemEvent::System(SystemLifecycleEvent {
                component: "engine".to_string(),
                phase: "stopped".to_string(),
                message: None,
            }));
    }

    /// Return the stream manager API.
    /// 返回流管理器 API。
    pub fn stream_manager_api(&self) -> Arc<dyn StreamManagerApi> {
        self.stream_manager.clone()
    }

    /// Return the publisher API.
    /// 返回发布者 API。
    pub fn publisher_api(&self) -> Arc<dyn PublisherApi> {
        self.stream_manager.clone()
    }

    /// Return the subscriber API.
    /// 返回订阅者 API。
    pub fn subscriber_api(&self) -> Arc<dyn SubscriberApi> {
        self.stream_manager.clone()
    }

    /// Return the core adapters API.
    /// 返回核心适配器 API。
    pub fn core_adapters_api(&self) -> Arc<dyn CoreAdaptersApi> {
        self.core_adapters.clone()
    }

    /// Return the module manager API.
    /// 返回模块管理器 API。
    pub fn module_manager_api(&self) -> Arc<dyn ModuleManagerApi> {
        self.module_manager.clone()
    }

    /// Return the task system API.
    /// 返回任务系统 API。
    pub fn task_system_api(&self) -> Arc<dyn TaskSystemApi> {
        self.task_system.clone()
    }

    /// Return the room service API.
    /// 返回房间服务 API。
    pub fn room_service_api(&self) -> Arc<dyn RoomServiceApi> {
        self.room_service.clone()
    }

    /// Return the event bus API.
    /// 返回事件总线 API。
    pub fn event_bus_api(&self) -> Arc<dyn EventBus> {
        self.event_bus.clone()
    }

    /// Return the health API.
    /// 返回健康 API。
    pub fn health_api(&self) -> Arc<dyn HealthApi> {
        self.health.clone()
    }

    /// Return the metrics API.
    /// 返回指标 API。
    pub fn metrics_api(&self) -> Arc<dyn MetricsApi> {
        self.metrics.clone()
    }

    /// Return the read-only config provider.
    /// 返回只读配置提供者。
    pub fn config_provider(&self) -> Arc<dyn ConfigProvider> {
        self.config_provider.clone()
    }

    /// Return the config apply API.
    /// 返回配置应用 API。
    pub fn config_apply_api(&self) -> Arc<dyn ConfigApplyApi> {
        self.config_apply_api.clone()
    }

    /// Return the runtime API.
    /// 返回运行时 API。
    pub fn runtime_api(&self) -> Arc<dyn RuntimeApi> {
        self.runtime_api.clone()
    }

    /// Return the service registry API.
    /// 返回服务注册 API。
    pub fn service_registry_api(&self) -> Arc<dyn ServiceRegistry> {
        self.service_registry.clone()
    }

    /// Return the database API.
    /// 返回数据库 API。
    pub fn database_api(&self) -> Arc<dyn DatabaseApi> {
        self.database.clone()
    }

    /// Return the proxy manager API.
    /// 返回代理管理器 API。
    pub fn proxy_manager_api(&self) -> Arc<dyn cheetah_sdk::ProxyManager> {
        self.proxy_manager.clone()
    }

    /// Return the cluster API.
    /// 返回集群 API。
    pub fn cluster_api(&self) -> Arc<dyn ClusterApi> {
        self.cluster.clone()
    }

    /// Return the FFmpeg API.
    /// 返回 FFmpeg API。
    pub fn ffmpeg_api(&self) -> Arc<dyn FfmpegApi> {
        self.ffmpeg.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use cheetah_config::ConfigStore;
    use cheetah_runtime_tokio::TokioRuntime;
    use cheetah_sdk::{
        CancellationToken, ConfigEffect, HealthApi, Module, ModuleCapability, ModuleConfigChange,
        ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest, ModuleState,
        SdkError,
    };
    use serde_json::json;

    use super::EngineBuilder;

    #[derive(Default)]
    struct ModuleCounters {
        create: AtomicUsize,
        init: AtomicUsize,
        start: AtomicUsize,
        stop: AtomicUsize,
        apply: AtomicUsize,
    }

    struct TestModule {
        info: ModuleInfo,
        state: ModuleState,
        counters: Arc<ModuleCounters>,
        fail_init: bool,
        fail_start: bool,
        apply_effect: ConfigEffect,
    }

    #[async_trait]
    impl Module for TestModule {
        fn info(&self) -> ModuleInfo {
            self.info.clone()
        }

        fn state(&self) -> ModuleState {
            self.state
        }

        async fn init(&mut self, _ctx: ModuleInitContext) -> Result<(), SdkError> {
            self.counters.init.fetch_add(1, Ordering::Relaxed);
            if self.fail_init {
                return Err(SdkError::Internal("forced init failure".to_string()));
            }
            self.state = ModuleState::Initialized;
            Ok(())
        }

        async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
            if self.fail_start {
                return Err(SdkError::Internal("forced start failure".to_string()));
            }
            self.counters.start.fetch_add(1, Ordering::Relaxed);
            self.state = ModuleState::Running;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), SdkError> {
            self.counters.stop.fetch_add(1, Ordering::Relaxed);
            self.state = ModuleState::Stopped;
            Ok(())
        }

        async fn apply_config(
            &mut self,
            _change: ModuleConfigChange,
        ) -> Result<ConfigEffect, SdkError> {
            self.counters.apply.fetch_add(1, Ordering::Relaxed);
            Ok(self.apply_effect)
        }
    }

    struct TestModuleFactory {
        module_id: ModuleId,
        dependencies: Vec<ModuleId>,
        counters: Arc<ModuleCounters>,
        fail_init: bool,
        fail_start: bool,
        apply_effect: ConfigEffect,
    }

    impl TestModuleFactory {
        fn new(
            module_id: &str,
            dependencies: Vec<ModuleId>,
            counters: Arc<ModuleCounters>,
            fail_init: bool,
            fail_start: bool,
            apply_effect: ConfigEffect,
        ) -> Self {
            Self {
                module_id: ModuleId::new(module_id),
                dependencies,
                counters,
                fail_init,
                fail_start,
                apply_effect,
            }
        }
    }

    impl ModuleFactory for TestModuleFactory {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                module_id: self.module_id.clone(),
                display_name: self.module_id.0.clone(),
                dependencies: self.dependencies.clone(),
                config_namespace: self.module_id.0.clone(),
                routes_prefix: format!("/{}", self.module_id.0),
                capabilities: vec![ModuleCapability::BackgroundJob],
            }
        }

        fn create(&self) -> Box<dyn Module> {
            self.counters.create.fetch_add(1, Ordering::Relaxed);
            Box::new(TestModule {
                info: ModuleInfo {
                    module_id: self.module_id.clone(),
                    display_name: self.module_id.0.clone(),
                    state: ModuleState::Created,
                },
                state: ModuleState::Created,
                counters: self.counters.clone(),
                fail_init: self.fail_init,
                fail_start: self.fail_start,
                apply_effect: self.apply_effect,
            })
        }
    }

    #[derive(Default)]
    struct ApplyTrace {
        values: Mutex<Vec<i64>>,
    }

    struct TraceModule {
        info: ModuleInfo,
        state: ModuleState,
        fail_on_next: Option<i64>,
        trace: Arc<ApplyTrace>,
    }

    #[async_trait]
    impl Module for TraceModule {
        fn info(&self) -> ModuleInfo {
            self.info.clone()
        }

        fn state(&self) -> ModuleState {
            self.state
        }

        async fn init(&mut self, _ctx: ModuleInitContext) -> Result<(), SdkError> {
            self.state = ModuleState::Initialized;
            Ok(())
        }

        async fn start(&mut self, _cancel: CancellationToken) -> Result<(), SdkError> {
            self.state = ModuleState::Running;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), SdkError> {
            self.state = ModuleState::Stopped;
            Ok(())
        }

        async fn apply_config(
            &mut self,
            change: ModuleConfigChange,
        ) -> Result<ConfigEffect, SdkError> {
            let next = change
                .next
                .get("v")
                .and_then(|v| v.as_i64())
                .unwrap_or_default();
            self.trace.values.lock().expect("trace lock").push(next);
            if self.fail_on_next == Some(next) {
                return Err(SdkError::Internal(format!(
                    "forced apply failure on {next}"
                )));
            }
            Ok(ConfigEffect::Immediate)
        }
    }

    struct TraceModuleFactory {
        module_id: ModuleId,
        dependencies: Vec<ModuleId>,
        fail_on_next: Option<i64>,
        trace: Arc<ApplyTrace>,
    }

    impl TraceModuleFactory {
        fn new(
            module_id: &str,
            dependencies: Vec<ModuleId>,
            fail_on_next: Option<i64>,
            trace: Arc<ApplyTrace>,
        ) -> Self {
            Self {
                module_id: ModuleId::new(module_id),
                dependencies,
                fail_on_next,
                trace,
            }
        }
    }

    impl ModuleFactory for TraceModuleFactory {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                module_id: self.module_id.clone(),
                display_name: self.module_id.0.clone(),
                dependencies: self.dependencies.clone(),
                config_namespace: self.module_id.0.clone(),
                routes_prefix: format!("/{}", self.module_id.0),
                capabilities: vec![ModuleCapability::BackgroundJob],
            }
        }

        fn create(&self) -> Box<dyn Module> {
            Box::new(TraceModule {
                info: ModuleInfo {
                    module_id: self.module_id.clone(),
                    display_name: self.module_id.0.clone(),
                    state: ModuleState::Created,
                },
                state: ModuleState::Created,
                fail_on_next: self.fail_on_next,
                trace: self.trace.clone(),
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn module_restart_required_recreates_instance() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let counters = Arc::new(ModuleCounters::default());
        let module_id = ModuleId::new("restart-test");
        let factory = Arc::new(TestModuleFactory::new(
            &module_id.0,
            Vec::new(),
            counters.clone(),
            false,
            false,
            ConfigEffect::ModuleRestartRequired,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");
        engine.start().await.expect("engine start");

        let report = engine
            .module_manager_api()
            .apply_module_config_change(ModuleConfigChange {
                module_id: module_id.clone(),
                previous: json!({"v": 0}),
                next: json!({"v": 1}),
                previous_global: Some(json!({})),
                next_global: Some(json!({})),
            })
            .await
            .expect("apply module config");
        assert_eq!(report.effect, ConfigEffect::ModuleRestartRequired);
        assert_eq!(counters.create.load(Ordering::Relaxed), 2);
        assert_eq!(counters.init.load(Ordering::Relaxed), 2);
        assert_eq!(counters.start.load(Ordering::Relaxed), 2);
        assert_eq!(counters.stop.load(Ordering::Relaxed), 1);

        engine.stop().await;
        assert_eq!(counters.stop.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn start_failure_rolls_back_started_modules_and_health() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());

        let upstream_id = ModuleId::new("upstream");
        let upstream_counters = Arc::new(ModuleCounters::default());
        let upstream_factory = Arc::new(TestModuleFactory::new(
            &upstream_id.0,
            Vec::new(),
            upstream_counters.clone(),
            false,
            false,
            ConfigEffect::Immediate,
        ));

        let failing_counters = Arc::new(ModuleCounters::default());
        let failing_factory = Arc::new(TestModuleFactory::new(
            "failing",
            vec![upstream_id.clone()],
            failing_counters.clone(),
            false,
            true,
            ConfigEffect::Immediate,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(upstream_factory)
            .register_module_factory(failing_factory)
            .build()
            .expect("engine build");

        let err = engine.start().await.expect_err("start should fail");
        assert!(matches!(err, SdkError::Internal(_)));
        assert_eq!(upstream_counters.start.load(Ordering::Relaxed), 1);
        assert_eq!(upstream_counters.stop.load(Ordering::Relaxed), 1);
        assert_eq!(failing_counters.start.load(Ordering::Relaxed), 0);
        assert_eq!(failing_counters.stop.load(Ordering::Relaxed), 1);
        let health: Arc<dyn HealthApi> = engine.health_api();
        assert!(!health.is_live());
        assert!(!health.is_ready());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn init_failure_rolls_back_initialized_modules_and_health() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());

        let upstream_id = ModuleId::new("upstream");
        let upstream_counters = Arc::new(ModuleCounters::default());
        let upstream_factory = Arc::new(TestModuleFactory::new(
            &upstream_id.0,
            Vec::new(),
            upstream_counters.clone(),
            false,
            false,
            ConfigEffect::Immediate,
        ));

        let failing_counters = Arc::new(ModuleCounters::default());
        let failing_factory = Arc::new(TestModuleFactory::new(
            "failing",
            vec![upstream_id.clone()],
            failing_counters.clone(),
            true,
            false,
            ConfigEffect::Immediate,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(upstream_factory)
            .register_module_factory(failing_factory)
            .build()
            .expect("engine build");

        let err = engine.start().await.expect_err("start should fail");
        assert!(matches!(err, SdkError::Internal(_)));
        assert_eq!(upstream_counters.init.load(Ordering::Relaxed), 1);
        assert_eq!(upstream_counters.start.load(Ordering::Relaxed), 0);
        assert_eq!(upstream_counters.stop.load(Ordering::Relaxed), 1);
        assert_eq!(failing_counters.init.load(Ordering::Relaxed), 1);
        assert_eq!(failing_counters.start.load(Ordering::Relaxed), 0);
        assert_eq!(failing_counters.stop.load(Ordering::Relaxed), 1);
        let health: Arc<dyn HealthApi> = engine.health_api();
        assert!(!health.is_live());
        assert!(!health.is_ready());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_module_rejects_non_running_state() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let module_id = ModuleId::new("restart-api-test");
        let factory = Arc::new(TestModuleFactory::new(
            &module_id.0,
            Vec::new(),
            Arc::new(ModuleCounters::default()),
            false,
            false,
            ConfigEffect::Immediate,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");
        engine.start().await.expect("engine start");
        engine.stop().await;

        let err = engine
            .module_manager_api()
            .restart_module(&module_id)
            .await
            .expect_err("restart should fail for non-running module");
        assert!(matches!(err, SdkError::Conflict(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn module_restart_required_rebuilds_initialized_module() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let counters = Arc::new(ModuleCounters::default());
        let module_id = ModuleId::new("restart-initialized");
        let factory = Arc::new(TestModuleFactory::new(
            &module_id.0,
            Vec::new(),
            counters.clone(),
            false,
            false,
            ConfigEffect::ModuleRestartRequired,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");

        engine
            .module_manager
            .init_all(engine.context(), engine.config_provider.clone())
            .await
            .expect("module init");

        let report = engine
            .module_manager_api()
            .apply_module_config_change(ModuleConfigChange {
                module_id: module_id.clone(),
                previous: json!({"v": 0}),
                next: json!({"v": 1}),
                previous_global: Some(json!({})),
                next_global: Some(json!({})),
            })
            .await
            .expect("apply module config");
        assert_eq!(report.effect, ConfigEffect::ModuleRestartRequired);
        assert_eq!(counters.create.load(Ordering::Relaxed), 2);
        assert_eq!(counters.init.load(Ordering::Relaxed), 2);
        assert_eq!(counters.start.load(Ordering::Relaxed), 0);
        assert_eq!(counters.stop.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn module_restart_required_rejects_stopped_module() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let module_id = ModuleId::new("restart-stopped");
        let factory = Arc::new(TestModuleFactory::new(
            &module_id.0,
            Vec::new(),
            Arc::new(ModuleCounters::default()),
            false,
            false,
            ConfigEffect::ModuleRestartRequired,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");
        engine.start().await.expect("engine start");
        engine.stop().await;

        let err = engine
            .module_manager_api()
            .apply_module_config_change(ModuleConfigChange {
                module_id,
                previous: json!({"v": 0}),
                next: json!({"v": 1}),
                previous_global: Some(json!({})),
                next_global: Some(json!({})),
            })
            .await
            .expect_err("config apply should fail for stopped module");
        assert!(matches!(err, SdkError::Conflict(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn engine_can_restart_after_stop() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let counters = Arc::new(ModuleCounters::default());
        let module_id = ModuleId::new("restartable");
        let factory = Arc::new(TestModuleFactory::new(
            &module_id.0,
            Vec::new(),
            counters.clone(),
            false,
            false,
            ConfigEffect::Immediate,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");

        engine.start().await.expect("first start");
        engine.stop().await;
        engine.start().await.expect("second start");
        engine.stop().await;

        assert_eq!(counters.start.load(Ordering::Relaxed), 2);
        assert_eq!(counters.stop.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn engine_rejects_double_start_without_stop() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let counters = Arc::new(ModuleCounters::default());
        let factory = Arc::new(TestModuleFactory::new(
            "single-start",
            Vec::new(),
            counters.clone(),
            false,
            false,
            ConfigEffect::Immediate,
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory)
            .build()
            .expect("engine build");

        engine.start().await.expect("first start");
        let err = engine.start().await.expect_err("second start should fail");
        assert!(matches!(err, SdkError::Conflict(_)));
        assert_eq!(counters.start.load(Ordering::Relaxed), 1);

        engine.stop().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn batch_apply_rolls_back_already_applied_module_changes() {
        let runtime = Arc::new(TokioRuntime::new());
        let config = Arc::new(ConfigStore::new());
        let trace_a = Arc::new(ApplyTrace::default());
        let trace_b = Arc::new(ApplyTrace::default());
        let module_a = ModuleId::new("trace-a");
        let module_b = ModuleId::new("trace-b");

        let factory_a = Arc::new(TraceModuleFactory::new(
            &module_a.0,
            Vec::new(),
            None,
            trace_a.clone(),
        ));
        let factory_b = Arc::new(TraceModuleFactory::new(
            &module_b.0,
            vec![module_a.clone()],
            Some(1),
            trace_b.clone(),
        ));

        let engine = EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(factory_a)
            .register_module_factory(factory_b)
            .build()
            .expect("engine build");
        engine.start().await.expect("engine start");

        let err = engine
            .module_manager_api()
            .apply_module_config_changes(vec![
                ModuleConfigChange {
                    module_id: module_a.clone(),
                    previous: json!({"v": 0}),
                    next: json!({"v": 1}),
                    previous_global: Some(json!({})),
                    next_global: Some(json!({})),
                },
                ModuleConfigChange {
                    module_id: module_b.clone(),
                    previous: json!({"v": 0}),
                    next: json!({"v": 1}),
                    previous_global: Some(json!({})),
                    next_global: Some(json!({})),
                },
            ])
            .await
            .expect_err("batch apply should fail");
        assert!(matches!(err, SdkError::Internal(_) | SdkError::Conflict(_)));

        let applied_a = trace_a.values.lock().expect("trace lock").clone();
        let applied_b = trace_b.values.lock().expect("trace lock").clone();
        assert_eq!(
            applied_a,
            vec![1, 0],
            "module a must apply forward config then rollback"
        );
        assert_eq!(
            applied_b,
            vec![1],
            "module b should fail on first apply without rollback re-entry"
        );

        let states = engine.module_manager_api().modules();
        assert!(
            states
                .iter()
                .all(|(_, state)| *state == ModuleState::Running),
            "all modules must remain running after rollback"
        );

        engine.stop().await;
    }
}
