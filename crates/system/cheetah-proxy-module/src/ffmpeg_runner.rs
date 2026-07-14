//! FFmpeg proxy runner.
//!
//! Monitors a submitted `FfmpegJob` and maps its outcome to proxy registry state
//! and media events. Cancellation is forwarded to `FfmpegApi`.
//!
//! FFmpeg 代理运行器。监控已提交的 `FfmpegJob`，将其结果映射为代理注册表状态
//! 和媒体事件。取消操作会转发给 `FfmpegApi`。

use std::sync::Arc;

use cheetah_media_api::event::{EventHeader, MediaEvent, MediaEventSender};
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::ProxyState;
use cheetah_sdk::{CancellationToken, FfmpegApi, FfmpegJobOutcome};
use futures::{pin_mut, select_biased, FutureExt};

use crate::media_provider::{generate_id, now_ms};
use crate::registry::ProxyRegistry;

/// Wait for an FFmpeg job to finish, honouring cancellation.
///
/// 等待 FFmpeg 任务完成，并响应取消。
pub async fn run(
    registry: ProxyRegistry,
    event_sender: Option<Arc<dyn MediaEventSender>>,
    ffmpeg_api: Arc<dyn FfmpegApi>,
    proxy_id: ProxyId,
    cancel: CancellationToken,
) {
    let job_id = proxy_id.0.clone();

    let cancel_fut = cancel.cancelled().fuse();
    let wait_fut = ffmpeg_api.wait_job(&job_id).fuse();
    pin_mut!(cancel_fut, wait_fut);

    let outcome = select_biased! {
        _ = cancel_fut => {
            let _ = ffmpeg_api.cancel_job(&job_id).await;
            match ffmpeg_api.wait_job(&job_id).await {
                Ok(o) => o,
                Err(_) => FfmpegJobOutcome::Cancelled,
            }
        }
        outcome = wait_fut => {
            match outcome {
                Ok(o) => o,
                Err(e) => FfmpegJobOutcome::Failed(format!("failed to wait for ffmpeg job: {e}")),
            }
        }
    };

    let (state, last_error) = match outcome {
        FfmpegJobOutcome::Succeeded => (ProxyState::Stopped, None),
        FfmpegJobOutcome::Failed(m) => (ProxyState::Failed, Some(m)),
        FfmpegJobOutcome::Cancelled => (ProxyState::Stopped, None),
        FfmpegJobOutcome::Timeout => (ProxyState::Failed, Some("ffmpeg job timed out".to_string())),
    };

    registry.update_state(&proxy_id, state, last_error.clone());

    if let Some(sender) = event_sender {
        if let Some(info) = registry.get(&proxy_id) {
            let header = EventHeader {
                event_id: generate_id(),
                occurred_at: now_ms(),
                sequence: None,
                media_key: Some(info.destination.clone()),
                source: info.source.clone(),
                correlation_id: None,
            };
            let _ = sender.send(MediaEvent::ProxyStateChanged(
                cheetah_media_api::event::ProxyStateChanged {
                    header,
                    proxy_id: info.proxy_id.clone(),
                    state,
                    last_error,
                },
            ));
        }
    }
}
