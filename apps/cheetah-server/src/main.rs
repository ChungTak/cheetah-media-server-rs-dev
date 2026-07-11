//! `cheetah-server` application binary entrypoint.
//!
//! This crate is the top-level executable that wires the tokio runtime, global configuration,
//! feature modules, and the control plane into a running media engine. It is intentionally thin:
//! all protocol, media, and control logic lives in the lower-level engine and module crates.
//!
//! `cheetah-server` 是顶层可执行程序，负责将 tokio 运行时、全局配置、特性模块和控制面组合成运行的媒体引擎。
//! 它本身刻意保持精简：所有协议、媒体和控制逻辑均位于下层的引擎与模块 crate 中。
//!
//! Lifecycle overview:
//! 1. Install rustls crypto provider and initialize tracing.
//! 2. Load global config (defaults + YAML file + env overrides).
//! 3. Build the engine with enabled module factories and start it.
//! 4. Register the `control` service and spawn the HTTP control server.
//! 5. Wait for shutdown signal, then stop the engine and abort the control task.
//!
//! 生命周期概览：
//! 1. 安装 rustls 加密提供方并初始化 tracing。
//! 2. 加载全局配置（默认值 + YAML 文件 + 环境变量覆盖）。
//! 3. 构建引擎，注册已启用的模块工厂，并启动引擎。
//! 4. 注册 `control` 服务并启动 HTTP 控制服务。
//! 5. 等待关闭信号，随后停止引擎并终止控制任务。

use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_control::{spawn_server, ControlState};
use cheetah_engine::{DispatcherMode, EngineBuilder};
#[cfg(feature = "fmp4")]
use cheetah_fmp4_module::Fmp4ModuleFactory;
#[cfg(feature = "gb28181")]
use cheetah_gb28181_module::Gb28181ModuleFactory;
#[cfg(feature = "hls")]
use cheetah_hls_module::HlsModuleFactory;
#[cfg(feature = "http-flv")]
use cheetah_http_flv_module::HttpFlvModuleFactory;
#[cfg(feature = "mp4")]
use cheetah_mp4_module::Mp4ModuleFactory;
#[cfg(feature = "record")]
use cheetah_record_module::RecordModuleFactory;
#[cfg(feature = "rtmp")]
use cheetah_rtmp_module::RtmpModuleFactory;
#[cfg(feature = "rtp")]
use cheetah_rtp_module::RtpModuleFactory;
#[cfg(feature = "rtsp")]
use cheetah_rtsp_module::RtspModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ConfigProvider, ConfigSchemaRegistry, ServiceDescriptor};
#[cfg(feature = "srt")]
use cheetah_srt_module::SrtModuleFactory;
#[cfg(feature = "ts")]
use cheetah_ts_module::TsModuleFactory;
#[cfg(feature = "webrtc")]
use cheetah_webrtc_module::WebRtcModuleFactory;
use serde_json::Value;
use tracing::{error, info};

/// Main entrypoint of the cheetah media server.
///
/// Orchestrates initialization, engine startup, control-plane startup, and graceful shutdown.
/// Returns `anyhow::Result<()>` so that startup errors propagate as a non-zero exit code.
///
/// 媒体服务器主入口。
/// 负责初始化、引擎启动、控制面启动与优雅关闭。
/// 启动错误以 `anyhow::Result<()>` 传播，失败时返回非零退出码。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider before any TLS operations.
    // 在任何 TLS 操作前安装 rustls 加密提供方。
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    #[cfg(feature = "tokio-console")]
    console_subscriber::init();

    #[cfg(not(feature = "tokio-console"))]
    // Initialize the default tracing subscriber; `RUST_LOG` controls verbosity.
    // 初始化默认 tracing subscriber；`RUST_LOG` 环境变量控制日志级别。
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Create the runtime abstraction and the layered config store.
    // 创建运行时抽象与分层配置存储。
    let runtime = Arc::new(TokioRuntime::new());
    let config = Arc::new(ConfigStore::new());

    // Register the global schema with default control listen address before loading user config.
    // 在加载用户配置前，先注册包含默认控制监听地址的全局 schema。
    config
        .register_global_schema(
            "global",
            serde_json::json!({
                "control": {
                    "listen": "127.0.0.1:8891"
                }
            }),
            None,
        )
        .expect("register global schema");

    // Load YAML config from `CHEETAH_CONFIG` if set; errors are logged but not fatal.
    // 若设置了 `CHEETAH_CONFIG`，则从 YAML 文件加载配置；错误会被记录，但不会导致启动失败。
    if let Ok(path) = std::env::var("CHEETAH_CONFIG") {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if let Err(err) = config.load_yaml_str(&content) {
                    error!(%path, %err, "failed to load yaml config");
                }
            }
            Err(err) => error!(%path, %err, "failed to read config file"),
        }
    }

    // Overlay `M7S_` prefixed environment variables for backward compatibility.
    // 加载以 `M7S_` 为前缀的环境变量，以保持向后兼容。
    config.load_env("M7S_");

    // Build the engine with the runtime, config, and per-stream dispatcher.
    // 使用运行时、配置与按流分发器构建引擎。
    let mut builder = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_dispatcher_mode(DispatcherMode::PerStream)
        .with_config_schema_registry(config.clone());

    // Register enabled protocol / feature module factories via conditional compilation.
    // 通过条件编译注册已启用的协议/特性模块工厂。
    #[cfg(feature = "rtmp")]
    {
        builder = builder.register_module_factory(Arc::new(RtmpModuleFactory));
    }
    #[cfg(feature = "rtsp")]
    {
        builder = builder.register_module_factory(Arc::new(RtspModuleFactory));
    }
    #[cfg(feature = "http-flv")]
    {
        builder = builder.register_module_factory(Arc::new(HttpFlvModuleFactory));
    }

    #[cfg(feature = "hls")]
    {
        builder = builder.register_module_factory(Arc::new(HlsModuleFactory));
    }

    #[cfg(feature = "ts")]
    {
        builder = builder.register_module_factory(Arc::new(TsModuleFactory));
    }

    #[cfg(feature = "srt")]
    {
        builder = builder.register_module_factory(Arc::new(SrtModuleFactory));
    }

    #[cfg(feature = "rtp")]
    {
        builder = builder.register_module_factory(Arc::new(RtpModuleFactory));
    }

    #[cfg(feature = "gb28181")]
    {
        builder = builder.register_module_factory(Arc::new(Gb28181ModuleFactory));
    }

    #[cfg(feature = "fmp4")]
    {
        builder = builder.register_module_factory(Arc::new(Fmp4ModuleFactory));
    }

    #[cfg(feature = "mp4")]
    {
        builder = builder.register_module_factory(Arc::new(Mp4ModuleFactory));
    }

    #[cfg(feature = "record")]
    {
        builder = builder.register_module_factory(Arc::new(RecordModuleFactory));
    }

    #[cfg(feature = "webrtc")]
    {
        builder = builder.register_module_factory(Arc::new(WebRtcModuleFactory));
    }

    // Build the engine and wire the config store to the event bus.
    // 构建引擎，并将配置存储连接到事件总线。
    let engine = builder.build()?;
    config.set_event_bus(engine.event_bus_api());

    // Register the control HTTP service in the service registry.
    // 在服务注册表中注册 control HTTP 服务。
    engine
        .service_registry_api()
        .register(ServiceDescriptor {
            name: "control".to_string(),
            endpoint: "http".to_string(),
            metadata: Default::default(),
        })
        .expect("register service");

    // Start all registered modules and the engine dispatch loop.
    // 启动所有已注册模块与引擎分发循环。
    engine.start().await?;

    // Resolve the control plane listen address from global config.
    // 从全局配置解析控制面监听地址。
    let control_addr = resolve_control_addr(config.global())?;

    // Assemble the control state object and spawn the control server task.
    // 组装控制状态对象并启动控制服务任务。
    let control_state = ControlState {
        health: engine.health_api(),
        metrics: engine.metrics_api(),
        modules: engine.module_manager_api(),
        streams: engine.stream_manager_api(),
        tasks: engine.task_system_api(),
        config: engine.config_provider(),
        config_apply: engine.config_apply_api(),
        config_schemas: config.clone(),
        service_registry: engine.service_registry_api(),
    };

    let control_task = spawn_server(control_addr, control_state);
    info!(%control_addr, "cheetah control started");

    // Block on Ctrl+C / SIGINT, then begin shutdown.
    // 阻塞等待 Ctrl+C / SIGINT 信号，随后开始关闭。
    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");

    // Give modules up to 5 seconds to stop gracefully, then proceed anyway.
    // 给模块最多 5 秒进行优雅停止，超时仍继续关闭流程。
    if tokio::time::timeout(std::time::Duration::from_secs(5), engine.stop())
        .await
        .is_err()
    {
        info!("graceful shutdown timed out, forcing exit");
    }
    control_task.abort();
    info!("shutdown complete");

    // Force process exit to avoid hanging on background task cleanup.
    // 强制进程退出，避免后台任务清理导致挂起。
    std::process::exit(0);
}

/// Resolve the control plane listen address from the `global` config object.
///
/// Prefers `global.control.listen` if present, then falls back to `CHEETAH_CONTROL_ADDR`,
/// and finally defaults to `127.0.0.1:8891`. Invalid addresses produce a clear anyhow error.
///
/// 从 `global` 配置对象解析控制面监听地址。
/// 优先使用 `global.control.listen`，其次回退到 `CHEETAH_CONTROL_ADDR`，最后默认 `127.0.0.1:8891`。
/// 非法地址会返回明确的 anyhow 错误。
fn resolve_control_addr(global: Value) -> anyhow::Result<SocketAddr> {
    if let Some(addr) = global
        .get("control")
        .and_then(|v| v.get("listen"))
        .and_then(Value::as_str)
    {
        return addr
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid control.listen: {e}"));
    }

    std::env::var("CHEETAH_CONTROL_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8891".to_string())
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid control addr: {e}"))
}
