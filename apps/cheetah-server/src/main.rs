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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider before any TLS operations.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let runtime = Arc::new(TokioRuntime::new());
    let config = Arc::new(ConfigStore::new());
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
    config.load_env("M7S_");

    let mut builder = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_dispatcher_mode(DispatcherMode::PerStream)
        .with_config_schema_registry(config.clone());

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

    let engine = builder.build()?;
    config.set_event_bus(engine.event_bus_api());

    engine
        .service_registry_api()
        .register(ServiceDescriptor {
            name: "control".to_string(),
            endpoint: "http".to_string(),
            metadata: Default::default(),
        })
        .expect("register service");

    engine.start().await?;

    let control_addr = resolve_control_addr(config.global())?;

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

    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received");

    // Give modules up to 5 seconds to stop gracefully, then force exit.
    if tokio::time::timeout(std::time::Duration::from_secs(5), engine.stop())
        .await
        .is_err()
    {
        info!("graceful shutdown timed out, forcing exit");
    }
    control_task.abort();
    info!("shutdown complete");

    // Force process exit to avoid hanging on background task cleanup.
    std::process::exit(0);
}

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
