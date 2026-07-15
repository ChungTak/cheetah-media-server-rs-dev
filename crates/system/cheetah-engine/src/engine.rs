use std::sync::Arc;

use cheetah_sdk::{
    CancellationToken, ClusterApi, ConfigApplyApi, ConfigProvider, ConfigSchemaRegistry,
    CoreAdaptersApi, DatabaseApi, EngineContext, EventBus, FfmpegApi, HealthApi, MediaDataPlaneApi,
    MediaFileStoreApi, MediaServices, MediaSessionDirectoryApi, MetricsApi, ModuleFactory,
    ModuleManagerApi, PublisherApi, RoomServiceApi, RuntimeApi, SdkError, ServiceRegistry,
    StreamManagerApi, SubscriberApi, SystemEvent, SystemLifecycleEvent, TaskSystemApi,
};
use parking_lot::RwLock;

use crate::cluster::LocalCluster;
use crate::core_adapters::LocalCoreAdapters;
use crate::database::InMemoryDatabase;
use crate::event::LocalEventBus;
use crate::ffmpeg::LocalFfmpegService;
use crate::health::HealthService;
use crate::media_provider::{
    EngineMediaDataPlane, EngineMediaFacade, EngineMediaFileStore, EngineMediaSessionDirectory,
    StreamMediaProvider,
};
use crate::metrics::MetricsRegistry;
use crate::module_manager::ModuleManager;
use crate::proxy::LocalProxyManager;
use crate::room::RoomService;
use crate::service_registry::InMemoryServiceRegistry;
use crate::stream::{DispatcherMode, StreamManager};
use crate::task::TaskSystem;

/// Builder for assembling an `Engine` with injected runtime, config, and modules.
///
/// The builder wires all in-memory engine services (event bus, task system, stream
/// manager, module manager, room, metrics, health, registry, database, proxy, cluster,
/// ffmpeg, core adapters) before returning a ready `Engine`.
///
/// 组装 `Engine` 的构建器，用于注入运行时、配置和模块。
///
/// 构建器在返回就绪的 `Engine` 之前连接所有内存引擎服务（事件总线、任务系统、流管理器、
/// 模块管理器、房间、指标、健康、注册表、数据库、代理、集群、ffmpeg、核心适配器）。
pub struct EngineBuilder {
    config_provider: Arc<dyn ConfigProvider>,
    config_apply_api: Arc<dyn ConfigApplyApi>,
    runtime_api: Arc<dyn RuntimeApi>,
    config_schema_registry: Option<Arc<dyn ConfigSchemaRegistry>>,
    event_bus_capacity: usize,
    ring_capacity: usize,
    dispatcher_mode: DispatcherMode,
    factories: Vec<Arc<dyn ModuleFactory>>,
}

impl EngineBuilder {
    /// Create a builder with the three required runtime/config dependencies.
    ///
    /// 用三个必需依赖创建构建器。
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

    /// Set the event bus bounded queue capacity.
    ///
    /// 设置事件总线有界队列容量。
    pub fn with_event_bus_capacity(mut self, capacity: usize) -> Self {
        self.event_bus_capacity = capacity.max(1);
        self
    }

    /// Set the per-stream ring buffer capacity used for GOP bootstrap.
    ///
    /// 设置用于 GOP 引导的每流环形缓冲区容量。
    pub fn with_ring_capacity(mut self, capacity: usize) -> Self {
        self.ring_capacity = capacity.max(128);
        self
    }

    /// Set the frame dispatcher mode (`PerStream` or `SharedPool`).
    ///
    /// 设置帧分发器模式（`PerStream` 或 `SharedPool`）。
    pub fn with_dispatcher_mode(mut self, mode: DispatcherMode) -> Self {
        self.dispatcher_mode = mode;
        self
    }

    /// Register the config schema registry used for module schema validation.
    ///
    /// 注册用于模块 schema 校验的配置 schema 注册表。
    pub fn with_config_schema_registry(mut self, registry: Arc<dyn ConfigSchemaRegistry>) -> Self {
        self.config_schema_registry = Some(registry);
        self
    }

    /// Register a module factory before building the engine.
    ///
    /// 在构建引擎前注册模块工厂。
    pub fn register_module_factory(mut self, factory: Arc<dyn ModuleFactory>) -> Self {
        self.factories.push(factory);
        self
    }

    /// Build the engine and wire all services.
    ///
    /// This registers module schemas, registers module factories, and creates the
    /// engine service graph. The engine is not started until `Engine::start` is called.
    ///
    /// 构建引擎并连接所有服务。
    ///
    /// 这会注册模块 schema、注册模块工厂并创建引擎服务图。直到调用 `Engine::start` 引擎才会启动。
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

        let publisher_api: Arc<dyn PublisherApi> = stream_manager.clone();
        let subscriber_api: Arc<dyn SubscriberApi> = stream_manager.clone();
        let session_directory: Arc<dyn MediaSessionDirectoryApi> =
            Arc::new(EngineMediaSessionDirectory::new());
        let media_data_plane: Arc<dyn MediaDataPlaneApi> = Arc::new(EngineMediaDataPlane::new(
            publisher_api.clone(),
            subscriber_api.clone(),
        ));
        let stream_provider = StreamMediaProvider::new(
            stream_manager.clone(),
            media_data_plane.clone(),
            session_directory.clone(),
        );

        let media_services = MediaServices::unavailable();
        media_services
            .register_control(Arc::new(stream_provider.clone())
                as Arc<dyn cheetah_media_api::port::MediaControlApi>);
        media_services.register_publish_subscribe(
            Arc::new(stream_provider) as Arc<dyn cheetah_media_api::port::PublishSubscribeApi>
        );
        let media_file_store: Arc<dyn MediaFileStoreApi> = Arc::new(EngineMediaFileStore::new());
        let media_event_bus = Arc::new(crate::media_provider::LocalMediaEventBus::new(
            self.runtime_api.clone(),
        ));
        let media_facade = Arc::new(EngineMediaFacade::new(
            media_services.clone(),
            media_event_bus.clone() as Arc<dyn cheetah_media_api::event::MediaEventBusApi>,
        ));

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
            media_facade,
            media_services,
            session_directory,
            media_data_plane,
            media_file_store,
            media_event_bus,
            root_cancel: RwLock::new(CancellationToken::new()),
        })
    }
}

/// The top-level orchestrator that owns engine services and module lifecycle.
///
/// `Engine` provides accessor methods for the public SDK APIs and is responsible for
/// `start`/`stop`/`apply_config` coordination. It uses a `CancellationToken` tree to
/// propagate shutdown to modules.
///
/// 顶层编排器，拥有引擎服务并管理模块生命周期。
///
/// `Engine` 提供公共 SDK API 的访问方法，并负责 `start`/`stop`/`apply_config` 协调。
/// 它使用 `CancellationToken` 树向模块传播关闭。
pub struct Engine {
    config_provider: Arc<dyn ConfigProvider>,
    config_apply_api: Arc<dyn ConfigApplyApi>,
    runtime_api: Arc<dyn RuntimeApi>,
    event_bus: Arc<LocalEventBus>,
    task_system: Arc<TaskSystem>,
    stream_manager: Arc<StreamManager>,
    module_manager: Arc<ModuleManager>,
    room_service: Arc<RoomService>,
    metrics: Arc<MetricsRegistry>,
    health: Arc<HealthService>,
    service_registry: Arc<InMemoryServiceRegistry>,
    database: Arc<InMemoryDatabase>,
    proxy_manager: Arc<LocalProxyManager>,
    cluster: Arc<LocalCluster>,
    ffmpeg: Arc<LocalFfmpegService>,
    core_adapters: Arc<LocalCoreAdapters>,
    media_facade: Arc<crate::media_provider::EngineMediaFacade>,
    media_services: MediaServices,
    session_directory: Arc<dyn MediaSessionDirectoryApi>,
    media_data_plane: Arc<dyn MediaDataPlaneApi>,
    media_file_store: Arc<dyn MediaFileStoreApi>,
    media_event_bus: Arc<crate::media_provider::LocalMediaEventBus>,
    root_cancel: RwLock<CancellationToken>,
}

impl Engine {
    /// Build an `EngineContext` snapshot from the current service set.
    ///
    /// 从当前服务集合构建 `EngineContext` 快照。
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
            media_services: self.media_services.clone(),
            media_session_directory: self.session_directory.clone(),
            media_data_plane: self.media_data_plane.clone(),
            media_file_store: self.media_file_store.clone(),
            media_event_bus: self.media_event_bus.clone(),
        }
    }

    /// Initialize and start all registered modules.
    ///
    /// Marks the engine live, initializes modules in topological order, creates a child
    /// cancellation token, starts modules, then marks ready. On failure, live/ready are
    /// cleared and an event is published.
    ///
    /// 初始化并启动所有已注册模块。
    ///
    /// 标记引擎存活，按拓扑顺序初始化模块，创建子取消 token，启动模块，然后标记就绪。
    /// 失败时清除 live/ready 并发布事件。
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

    /// Stop all modules and clear the engine ready/live flags.
    ///
    /// 停止所有模块并清除引擎就绪/存活标志。
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

    pub fn stream_manager_api(&self) -> Arc<dyn StreamManagerApi> {
        self.stream_manager.clone()
    }

    pub fn publisher_api(&self) -> Arc<dyn PublisherApi> {
        self.stream_manager.clone()
    }

    pub fn subscriber_api(&self) -> Arc<dyn SubscriberApi> {
        self.stream_manager.clone()
    }

    pub fn core_adapters_api(&self) -> Arc<dyn CoreAdaptersApi> {
        self.core_adapters.clone()
    }

    pub fn module_manager_api(&self) -> Arc<dyn ModuleManagerApi> {
        self.module_manager.clone()
    }

    pub fn task_system_api(&self) -> Arc<dyn TaskSystemApi> {
        self.task_system.clone()
    }

    pub fn room_service_api(&self) -> Arc<dyn RoomServiceApi> {
        self.room_service.clone()
    }

    pub fn event_bus_api(&self) -> Arc<dyn EventBus> {
        self.event_bus.clone()
    }

    pub fn health_api(&self) -> Arc<dyn HealthApi> {
        self.health.clone()
    }

    pub fn metrics_api(&self) -> Arc<dyn MetricsApi> {
        self.metrics.clone()
    }

    pub fn config_provider(&self) -> Arc<dyn ConfigProvider> {
        self.config_provider.clone()
    }

    pub fn config_apply_api(&self) -> Arc<dyn ConfigApplyApi> {
        self.config_apply_api.clone()
    }

    pub fn runtime_api(&self) -> Arc<dyn RuntimeApi> {
        self.runtime_api.clone()
    }

    pub fn service_registry_api(&self) -> Arc<dyn ServiceRegistry> {
        self.service_registry.clone()
    }

    pub fn database_api(&self) -> Arc<dyn DatabaseApi> {
        self.database.clone()
    }

    pub fn proxy_manager_api(&self) -> Arc<dyn cheetah_sdk::ProxyManager> {
        self.proxy_manager.clone()
    }

    pub fn cluster_api(&self) -> Arc<dyn ClusterApi> {
        self.cluster.clone()
    }

    pub fn ffmpeg_api(&self) -> Arc<dyn FfmpegApi> {
        self.ffmpeg.clone()
    }

    pub fn media_facade(&self) -> Arc<crate::media_provider::EngineMediaFacade> {
        self.media_facade.clone()
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
