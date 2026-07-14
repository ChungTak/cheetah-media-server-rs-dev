//! Shared egress helpers for RTP senders and pull jobs.
//!
//! RTP 发送端与拉流任务的共享出口辅助函数。

use std::sync::Arc;
use std::time::Duration;

use cheetah_rtp_core::RtpSendFrame;
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle, RtpSocketReuse};
use cheetah_sdk::media_api::ids::RtpSessionId;
use cheetah_sdk::media_api::model::RtpSessionState;
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, EngineContext, ProtocolEvent, StreamKey, SubscriberOptions,
    SystemEvent,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{debug, warn};

use crate::orchestrator::RtpSessionOrchestrator;

/// Sleep until `duration` or cancellation, returning true if cancelled.
///
/// 睡眠直到 `duration` 或取消，若被取消返回 true。
pub(crate) async fn sleep_or_cancel(
    runtime_api: &dyn cheetah_runtime_api::RuntimeApi,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let now = runtime_api.now().as_micros();
    let delta = duration.as_micros() as u64;
    let deadline = cheetah_codec::MonoTime::from_micros(now.saturating_add(delta));
    let mut timer = runtime_api.sleep_until(deadline);
    let cancel_fut = cancel.cancelled().fuse();
    let wait_fut = timer.wait().fuse();
    pin_mut!(cancel_fut, wait_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = wait_fut => false,
    }
}

/// Wait for a stream to appear in the engine, respecting timeout and cancellation.
///
/// 等待引擎中的流出现，遵守超时与取消。
pub(crate) async fn wait_for_stream(
    ctx: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Option<cheetah_sdk::StreamSnapshot> {
    let start = ctx.runtime_api.now().as_micros();
    let timeout_us = timeout.as_micros() as u64;

    loop {
        if cancel.is_cancelled() {
            return None;
        }
        if let Ok(Some(snapshot)) = ctx.stream_manager_api.get_stream(stream_key).await {
            return Some(snapshot);
        }
        let elapsed = ctx.runtime_api.now().as_micros().saturating_sub(start);
        if elapsed >= timeout_us {
            return None;
        }
        if sleep_or_cancel(ctx.runtime_api.as_ref(), cancel, Duration::from_millis(100)).await {
            return None;
        }
    }
}

/// Subscribe to an engine stream and fan out frames to one or more RTP target sessions.
///
/// On the first successfully delivered frame, every target session is promoted to
/// `Connected` and a single `media_online` protocol event is published.
///
/// 订阅引擎流并将每帧扇出到一个或多个 RTP 目标会话。
/// 首帧成功发送后，每个目标会话进入 Connected 并发布一次 media_online 事件。
pub(crate) async fn run_egress_session(
    engine: EngineContext,
    driver_handle: Arc<RtpDriverHandle>,
    session_keys: Vec<String>,
    stream_key: StreamKey,
    cancel: CancellationToken,
    orchestrator: Option<Arc<RtpSessionOrchestrator>>,
) {
    if session_keys.is_empty() {
        return;
    }
    let Some(_snapshot) =
        wait_for_stream(&engine, &stream_key, &cancel, Duration::from_millis(5000)).await
    else {
        debug!("Egress session wait stream timeout: {}", stream_key);
        return;
    };

    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            stream_key.clone(),
            SubscriberOptions {
                queue_capacity: 256,
                bootstrap_policy: BootstrapPolicy::live_tail(150, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!("Egress session subscribe failed: {e}");
            return;
        }
    };

    let mut first_frame = true;
    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let frame_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, frame_fut);

        let frame = select_biased! {
            _ = cancel_fut => break,
            res = frame_fut => match res {
                Ok(Some(f)) => f,
                Ok(None) | Err(_) => break,
            }
        };

        if first_frame {
            first_frame = false;
            if let Some(o) = orchestrator.as_ref() {
                for sk in &session_keys {
                    let _ =
                        o.set_session_state(&RtpSessionId(sk.clone()), RtpSessionState::Connected);
                    engine
                        .event_bus
                        .publish(SystemEvent::Protocol(ProtocolEvent {
                            protocol: "rtp".to_string(),
                            event_type: "media_online".to_string(),
                            payload: serde_json::json!({
                                "session_key": sk,
                                "stream_key": {
                                    "namespace": stream_key.namespace,
                                    "path": stream_key.path,
                                },
                                "direction": "egress",
                            }),
                        }));
                }
            }
        }

        // Fan out the same frame to every configured target session.
        for sk in &session_keys {
            let cmd = RtpDriverCommand::SendFrame(Box::new(RtpSendFrame {
                session_key: sk.clone(),
                frame: (*frame).clone(),
            }));
            driver_handle.send_command(cmd).await;
        }
    }

    let _ = subscriber.close().await;
}

/// Helper to choose socket reuse semantics from a boolean flag.
///
/// 从布尔标志选择 socket 复用语义。
pub(crate) fn reuse_from_flag(reuse: bool) -> RtpSocketReuse {
    if reuse {
        RtpSocketReuse::Reuse
    } else {
        RtpSocketReuse::Exclusive
    }
}
