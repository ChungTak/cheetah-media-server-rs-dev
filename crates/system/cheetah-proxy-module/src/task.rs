use std::pin::Pin;
use std::sync::Arc;

use cheetah_codec::MonoTime;
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::ProxyState;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_sdk::{TaskId, TaskKind, TaskOutcome, TaskSystemApi};
use futures::future::{select, Either};
use futures::Future;
use tracing::{debug, error, trace};

use crate::config::ProxyModuleConfig;
use crate::registry::ProxyRegistry;

/// Spawn a background proxy task and return its task id.
///
/// 派生后台代理任务并返回其任务 id。
pub fn spawn_proxy_task(
    runtime_api: Arc<dyn RuntimeApi>,
    task_system_api: Arc<dyn TaskSystemApi>,
    registry: Arc<ProxyRegistry>,
    proxy_id: ProxyId,
    config: ProxyModuleConfig,
) -> Result<TaskId, cheetah_sdk::SdkError> {
    let task_id = task_system_api.create_task(None, TaskKind::Task, "proxy", "proxy-session")?;
    let cancel = task_system_api.token(task_id)?;

    let fut = Box::pin(proxy_session_loop(
        runtime_api.clone(),
        task_system_api.clone(),
        registry,
        proxy_id,
        config,
        cancel,
        task_id,
    ));

    runtime_api.spawn(fut);
    Ok(task_id)
}

async fn proxy_session_loop(
    runtime_api: Arc<dyn RuntimeApi>,
    task_system_api: Arc<dyn TaskSystemApi>,
    registry: Arc<ProxyRegistry>,
    proxy_id: ProxyId,
    config: ProxyModuleConfig,
    cancel: CancellationToken,
    task_id: TaskId,
) {
    trace!(proxy_id = %proxy_id.0, "proxy session started");

    registry.update_state(&proxy_id, ProxyState::Connecting);

    let connect_deadline =
        MonoTime::from_micros(runtime_api.now().as_micros() + config.connect_timeout_ms * 1_000);

    let mut timer = runtime_api.sleep_until(connect_deadline);
    match wait_first(timer.wait(), Box::pin(cancel.cancelled())).await {
        WaitOutcome::First => {}
        WaitOutcome::Second => {
            transition_to_stopped(&registry, &proxy_id, &task_system_api, task_id, None);
            return;
        }
    }

    // Placeholder: the data plane would open the connector here.
    // For now we mark the proxy as connected and wait for cancellation.
    registry.update_state(&proxy_id, ProxyState::Connected);
    registry.update_error(&proxy_id, None);
    debug!(proxy_id = %proxy_id.0, "proxy connected (data plane stub)");

    loop {
        let heartbeat_deadline = MonoTime::from_micros(runtime_api.now().as_micros() + 1_000_000);
        let mut timer = runtime_api.sleep_until(heartbeat_deadline);

        match wait_first(timer.wait(), Box::pin(cancel.cancelled())).await {
            WaitOutcome::First => {}
            WaitOutcome::Second => {
                transition_to_stopped(&registry, &proxy_id, &task_system_api, task_id, None);
                return;
            }
        }

        if cancel.is_cancelled() {
            transition_to_stopped(&registry, &proxy_id, &task_system_api, task_id, None);
            return;
        }
    }
}

enum WaitOutcome {
    First,
    Second,
}

/// Wait for whichever of two differently-typed futures resolves first.
///
/// 等待两个不同类型 future 中先完成者。
async fn wait_first(
    first: Pin<Box<dyn Future<Output = ()> + Send + '_>>,
    second: Pin<Box<dyn Future<Output = ()> + Send + '_>>,
) -> WaitOutcome {
    match select(first, second).await {
        Either::Left(_) => WaitOutcome::First,
        Either::Right(_) => WaitOutcome::Second,
    }
}

fn transition_to_stopped(
    registry: &ProxyRegistry,
    proxy_id: &ProxyId,
    task_system_api: &Arc<dyn TaskSystemApi>,
    task_id: TaskId,
    error: Option<String>,
) {
    registry.update_state(proxy_id, ProxyState::Stopped);
    let outcome = if let Some(msg) = error {
        let _ = registry.update_error(proxy_id, Some(msg.clone()));
        TaskOutcome::Failed(msg)
    } else {
        TaskOutcome::Succeeded
    };
    if let Err(e) = task_system_api.finish(task_id, outcome) {
        error!(task_id = %task_id.0, "failed to finish proxy task: {e}");
    }
    trace!(proxy_id = %proxy_id.0, "proxy session stopped");
}
