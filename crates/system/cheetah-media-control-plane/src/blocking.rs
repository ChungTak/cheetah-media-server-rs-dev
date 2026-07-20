//! Helpers for running blocking work on `RuntimeApi::spawn_blocking` and
//! retrieving the result without exposing Tokio types.
//!
//! 在 `RuntimeApi::spawn_blocking` 上运行阻塞任务并取回结果的辅助函数。

use std::sync::{Arc, Mutex};

use cheetah_runtime_api::{OneShotRecvError, RuntimeApi, SpawnError};

use crate::error::ControlPlaneError;

/// Run `task` on the runtime's blocking pool and return its result.
///
/// `RuntimeApi::spawn_blocking` is fire-and-forget, so the result is passed
/// back through a shared `Arc<Mutex<Option<T>>>` guarded by a oneshot signal.
/// The caller must `await` the returned future on an async runtime.
pub async fn blocking_call<R, F>(
    runtime: &dyn RuntimeApi,
    name: &str,
    task: F,
) -> Result<R, ControlPlaneError>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let result: Arc<Mutex<Option<R>>> = Arc::new(Mutex::new(None));
    let result_clone = result.clone();

    let (tx, mut rx) = runtime.oneshot();
    let closure = move || {
        let value = task();
        *result_clone.lock().expect("blocking result mutex poisoned") = Some(value);
        let _ = tx.send();
    };

    runtime.spawn_blocking(name, Box::new(closure))?;

    match rx.recv().await {
        Ok(()) => {}
        Err(OneShotRecvError) => return Err(ControlPlaneError::RuntimeShutdown),
    }

    let mut guard = result.lock().expect("blocking result mutex poisoned");
    let value = guard.take().ok_or_else(|| {
        ControlPlaneError::Internal("blocking task did not set result".to_string())
    })?;
    drop(guard);
    Ok(value)
}

impl From<SpawnError> for ControlPlaneError {
    fn from(e: SpawnError) -> Self {
        ControlPlaneError::RuntimeError(e.to_string())
    }
}
