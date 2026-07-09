use std::sync::Arc;
use std::time::Duration;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_rtmp_module::RtmpModuleFactory;
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleId, ModuleState};

#[tokio::test(flavor = "current_thread")]
async fn push_job_missing_source_keeps_module_running_and_stoppable() {
    let runtime = Arc::new(TokioRuntime::new());
    let config = Arc::new(ConfigStore::new());
    config
        .load_yaml_str(
            r#"
modules:
  rtmp:
    enabled: true
    listen: "127.0.0.1:0"
    push_jobs:
      - name: "missing-source"
        enabled: true
        source_stream_key: "live/missing"
        target_url: "rtmp://127.0.0.1/live/out"
"#,
        )
        .expect("load rtmp test config");

    let engine = EngineBuilder::new(config.clone(), config.clone(), runtime)
        .with_config_schema_registry(config.clone())
        .register_module_factory(Arc::new(RtmpModuleFactory))
        .build()
        .expect("build engine");

    engine.start().await.expect("start engine");
    tokio::time::sleep(Duration::from_millis(120)).await;

    let running_state = engine
        .module_manager_api()
        .modules()
        .into_iter()
        .find_map(|(module_id, state)| {
            if module_id == ModuleId::new("rtmp") {
                Some(state)
            } else {
                None
            }
        })
        .expect("rtmp module state");
    assert_eq!(running_state, ModuleState::Running);
    assert!(engine.health_api().is_live());
    assert!(engine.health_api().is_ready());

    engine.stop().await;

    let stopped_state = engine
        .module_manager_api()
        .modules()
        .into_iter()
        .find_map(|(module_id, state)| {
            if module_id == ModuleId::new("rtmp") {
                Some(state)
            } else {
                None
            }
        })
        .expect("rtmp module state after stop");
    assert_eq!(stopped_state, ModuleState::Stopped);
    assert!(!engine.health_api().is_live());
    assert!(!engine.health_api().is_ready());
}
