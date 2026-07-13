//! In-memory engine implementation: service registry, stream dispatch, module lifecycle, and task system.
//!
//! 内存引擎实现：服务注册表、流分发、模块生命周期和任务系统。

mod cluster;
mod core_adapters;
mod database;
mod engine;
mod event;
mod ffmpeg;
mod health;
mod media_provider;
mod metrics;
mod module_manager;
mod proxy;
mod room;
mod service_registry;
mod stream;
mod task;

pub use cluster::LocalCluster;
pub use core_adapters::LocalCoreAdapters;
pub use database::InMemoryDatabase;
pub use engine::{Engine, EngineBuilder};
pub use event::LocalEventBus;
pub use ffmpeg::LocalFfmpegService;
pub use health::HealthService;
pub use media_provider::{EngineMediaFacade, StreamMediaProvider};
pub use metrics::MetricsRegistry;
pub use module_manager::ModuleManager;
pub use proxy::LocalProxyManager;
pub use room::RoomService;
pub use service_registry::InMemoryServiceRegistry;
pub use stream::{DispatcherMode, StreamManager};
pub use task::TaskSystem;
