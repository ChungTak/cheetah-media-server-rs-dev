//! Shared egress helpers for RTP senders and pull jobs.
//!
//! RTP 发送端与拉流任务的共享出口辅助函数。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cheetah_codec::{FrameFlags, FrameSideData, MediaKind, Timebase};
use cheetah_rtp_core::RtpSendFrame;
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle, RtpSocketReuse};
use cheetah_sdk::media_api::ids::RtpSessionId;
use cheetah_sdk::media_api::model::{RtpSessionKind, RtpSessionState};
use cheetah_sdk::media_api::rtp_session::PlaybackRange;
use cheetah_sdk::{
    BackpressurePolicy, BootstrapPolicy, CancellationToken, EngineContext, ProtocolEvent,
    StreamKey, SubscriberOptions, SystemEvent,
};
use futures::{pin_mut, select_biased, FutureExt};
use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::orchestrator::RtpSessionOrchestrator;

/// Shared map used to track active egress workers by logical session key.
///
/// 用于按逻辑 session key 追踪活动 egress worker 的共享映射。
pub(crate) type ActiveEgressMap = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// Removes the active-egress tracking entry for a session key when dropped.
///
/// 当退出作用域时移除对应 session key 的活动 egress 追踪项。
pub(crate) struct EgressCleanup(Option<(ActiveEgressMap, String)>);

impl EgressCleanup {
    pub(crate) fn new(map: ActiveEgressMap, key: String) -> Self {
        Self(Some((map, key)))
    }
}

impl Drop for EgressCleanup {
    fn drop(&mut self) {
        if let Some((map, key)) = self.0.take() {
            map.lock().remove(&key);
        }
    }
}

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

/// Choose a bootstrap policy for an egress worker based on the session kind.
///
/// VoiceTalk (bidirectional audio) is often audio-only, so waiting for a video
/// random-access point would block forever. Other egress paths start from the
/// most recent tail to minimize latency.
///
/// 根据会话类型为 egress worker 选择引导策略。
/// VoiceTalk（双向对讲）通常只有音频，等待视频随机访问点会永久阻塞；
/// 其他 egress 路径从最近的尾部开始以降低延迟。
fn bootstrap_policy_for_sessions(
    orchestrator: Option<&Arc<RtpSessionOrchestrator>>,
    session_keys: &[String],
) -> BootstrapPolicy {
    if let Some(orchestrator) = orchestrator {
        let sessions = orchestrator.sessions.lock();
        if session_keys.iter().any(|k| {
            sessions
                .get(&RtpSessionId(k.clone()))
                .is_some_and(|s| s.kind == RtpSessionKind::Talk)
        }) {
            return BootstrapPolicy::none();
        }
    }
    BootstrapPolicy::live_tail(150, None)
}

/// Subscribe to an engine stream and fan out frames to one or more RTP target sessions.
///
/// On the first successfully delivered frame, every target session is promoted to
/// `Connected` and a single `media_online` protocol event is published.
///
/// 订阅引擎流并将每帧扇出到一个或多个 RTP 目标会话。
/// 首帧成功发送后，每个目标会话进入 Connected 并发布一次 media_online 事件。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_egress_session(
    engine: EngineContext,
    driver_handle: Arc<RtpDriverHandle>,
    session_keys: Vec<String>,
    stream_key: StreamKey,
    cancel: CancellationToken,
    orchestrator: Option<Arc<RtpSessionOrchestrator>>,
    cleanup: Option<EgressCleanup>,
    mut subscriber_options: SubscriberOptions,
    talkback_max_latency_ms: u32,
    playback_range: Option<PlaybackRange>,
) {
    // The cleanup guard removes the active-egress tracking entry on any exit,
    // including natural completion, cancellation, and errors.
    let _cleanup = cleanup;
    if session_keys.is_empty() {
        return;
    }
    let Some(_snapshot) =
        wait_for_stream(&engine, &stream_key, &cancel, Duration::from_millis(5000)).await
    else {
        debug!("Egress session wait stream timeout: {}", stream_key);
        return;
    };

    let is_talk = orchestrator.as_ref().is_some_and(|o| {
        let sessions = o.sessions.lock();
        session_keys.iter().any(|k| {
            sessions
                .get(&RtpSessionId(k.clone()))
                .is_some_and(|s| s.kind == RtpSessionKind::Talk)
        })
    });

    let bootstrap_policy = bootstrap_policy_for_sessions(orchestrator.as_ref(), &session_keys);
    subscriber_options.bootstrap_policy = bootstrap_policy;
    // For talkback, use the supplied bounded queue + drop policy to keep latency low.
    if is_talk {
        subscriber_options.backpressure = BackpressurePolicy::DropDroppableFirst;
    }
    let mut subscriber = match engine
        .subscriber_api
        .subscribe(stream_key.clone(), subscriber_options)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!("Egress session subscribe failed: {e}");
            return;
        }
    };

    let max_latency_us = (talkback_max_latency_ms as u64) * 1000;
    let playback_start_us = playback_range
        .as_ref()
        .map(|r| r.start_ms.saturating_mul(1000))
        .unwrap_or(0);
    let playback_end_us = playback_range
        .as_ref()
        .and_then(|r| (r.end_ms?).checked_mul(1000));
    let mut first_frame = true;
    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let frame_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, frame_fut);

        let mut frame = select_biased! {
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

        // Playback range end: stop the egress once the frame presentation time reaches
        // or exceeds the requested end. The driver session is stopped so the receiver
        // sees a normal close rather than an abrupt socket teardown.
        if let Some(end_us) = playback_end_us {
            if frame.pts_us >= end_us {
                if let Some(o) = orchestrator.as_ref() {
                    for sk in &session_keys {
                        let _ = o.stop_rtp_session(&RtpSessionId(sk.clone())).await;
                    }
                }
                break;
            }
        }

        // Playback timeline normalization: shift timestamps so the output RTP timeline
        // starts from 0 (or the requested start) and grows monotonically. The original
        // source time is preserved as frame side data for logging/A/V sync.
        if playback_range.is_some() {
            let frame = Arc::make_mut(&mut frame);
            let source_pts_us = frame.pts_us;
            let source_dts_us = frame.dts_us;
            let pts_us = source_pts_us.saturating_sub(playback_start_us).max(0);
            let dts_us = source_dts_us.saturating_sub(playback_start_us).max(0);
            frame.pts_us = pts_us;
            frame.dts_us = dts_us;
            frame.pts = Timebase::from_micros(frame.timebase, pts_us);
            frame.dts = Timebase::from_micros(frame.timebase, dts_us);
            frame.side_data.push(FrameSideData::Metadata {
                key: "playback.source_pts_us".to_string(),
                value: source_pts_us.to_string(),
            });
            frame.side_data.push(FrameSideData::Metadata {
                key: "playback.start_us".to_string(),
                value: playback_start_us.to_string(),
            });
        }

        // Late/drop policy: talkback audio frames that are older than the configured
        // latency budget are dropped unless they are key/config frames. This isolates a
        // slow downstream device from the upstream publisher.
        if is_talk && max_latency_us > 0 {
            let now_us = engine.runtime_api.now().as_micros();
            let age_us = now_us.saturating_sub(frame.pts_us as u64);
            let droppable =
                frame.flags.contains(FrameFlags::DROPPABLE) || frame.media_kind == MediaKind::Audio;
            if age_us > max_latency_us && droppable {
                warn!(
                    "Dropping late talkback frame for {} (age {} ms > {} ms)",
                    stream_key,
                    age_us / 1000,
                    talkback_max_latency_ms
                );
                continue;
            }
        }

        // Fan out the same frame to every configured target session.
        for sk in &session_keys {
            let cmd = RtpDriverCommand::SendFrame(Box::new(RtpSendFrame {
                session_key: sk.clone(),
                frame: (*frame).clone(),
            }));
            if is_talk {
                // Non-blocking send for talkback: if the driver is saturated, drop the
                // frame for this target instead of stalling the whole fan-out.
                if !driver_handle.try_send_command(cmd) {
                    warn!(
                        "Driver queue full; dropping talkback frame for session {}",
                        sk
                    );
                }
            } else {
                driver_handle.send_command(cmd).await;
            }
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
