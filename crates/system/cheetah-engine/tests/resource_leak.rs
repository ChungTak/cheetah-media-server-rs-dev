use std::sync::Arc;

use cheetah_config::ConfigStore;
use cheetah_engine::EngineBuilder;
use cheetah_media_module::{NativeMediaModuleFactory, ZlmMediaModuleFactory};
use cheetah_runtime_tokio::TokioRuntime;
use cheetah_sdk::{ModuleId, TaskKind, TaskOutcome};

fn make_engine() -> Arc<cheetah_engine::Engine> {
    let runtime = Arc::new(TokioRuntime::new());
    let config = Arc::new(ConfigStore::new());
    Arc::new(
        EngineBuilder::new(config.clone(), config, runtime)
            .build()
            .expect("engine build"),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn clean_engine_has_no_leaks_after_stop() {
    let engine = make_engine();
    engine.start().await.expect("engine start");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.is_clean(),
        "a fresh started engine should have no leaks before stop: {report:?}"
    );

    engine.stop().await;

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.is_clean(),
        "engine stop should leave no orphan tasks or streams: {report:?}"
    );
    assert!(
        report.running_module_ids.is_empty(),
        "engine stop should stop all modules: {report:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn leak_report_detects_unfinished_task() {
    let engine = make_engine();

    let task_id = engine
        .task_system_api()
        .create_task(None, TaskKind::Task, "resource-leak-test", "leaky-task")
        .expect("create task");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.active_task_ids.contains(&task_id.to_string()),
        "unfinished task should appear in leak report: {report:?}"
    );

    engine
        .task_system_api()
        .finish(task_id, TaskOutcome::Succeeded)
        .expect("finish task");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.active_task_ids.is_empty(),
        "finished task should no longer leak: {report:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_task_is_not_leaked() {
    let engine = make_engine();

    let task_id = engine
        .task_system_api()
        .create_task(
            None,
            TaskKind::Work,
            "resource-leak-test",
            "cancellable-task",
        )
        .expect("create task");
    let token = engine.task_system_api().token(task_id).expect("task token");

    // Start some work tied to the task token.
    let worker = tokio::spawn(async move {
        loop {
            if token.is_cancelled() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    engine
        .task_system_api()
        .cancel(task_id, Some("test cancellation"))
        .expect("cancel task");

    // Wait for the worker to observe cancellation.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .expect("worker finishes after cancellation");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(
        report.is_clean(),
        "cancelled task should not be reported as leaked: {report:?}"
    );
}

fn make_engine_with_media_modules() -> Arc<cheetah_engine::Engine> {
    let runtime = Arc::new(TokioRuntime::new());
    let config = Arc::new(ConfigStore::new());
    Arc::new(
        EngineBuilder::new(config.clone(), config, runtime)
            .register_module_factory(Arc::new(NativeMediaModuleFactory))
            .register_module_factory(Arc::new(ZlmMediaModuleFactory))
            .build()
            .expect("engine build"),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn restart_native_module_is_leak_free() {
    let engine = make_engine_with_media_modules();
    engine.start().await.expect("engine start");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(report.is_clean(), "before restart: {report:?}");

    engine
        .module_manager_api()
        .restart_module(&ModuleId::new("media-http-native"))
        .await
        .expect("restart native module");

    let report = engine.resource_leak_report().await.expect("leak report");
    assert!(report.is_clean(), "after native module restart: {report:?}");
    assert!(
        report
            .running_module_ids
            .contains(&"media-http-native".to_string()),
        "native module should be running after restart: {report:?}"
    );
}
