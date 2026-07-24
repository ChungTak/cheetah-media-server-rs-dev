//! RTP ingress worker: translate RtpCore events into engine publishes.
//!
//! RTP 入站 worker：将 RtpCore 事件转换为引擎发布。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;

use cheetah_codec::TrackInfo;
use cheetah_rtp_core::{RtpCoreEvent, RtpSessionCloseReason, RtpTransportMode};
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle};
use cheetah_sdk::media_api::event::{EventHeader, MediaEvent, RtpSessionTimeout, SessionClosed};
use cheetah_sdk::media_api::ids::{MediaKey, RtpSessionId, SessionId};
use cheetah_sdk::media_api::model::{CloseReason, RtpSessionKind, RtpSessionState, SessionKind};
use cheetah_sdk::{
    CancellationToken, EngineContext, OneShotSender, ProtocolEvent, PublishLease, PublisherOptions,
    PublisherSink, StreamKey, SystemEvent,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{debug, error, info, warn};

use crate::orchestrator::RtpSessionOrchestrator;

fn hash_endpoint(addr: &SocketAddr) -> String {
    let mut h = DefaultHasher::new();
    addr.to_string().hash(&mut h);
    format!("{:x}", h.finish())
}

/// Per-session state for an RTP ingress publisher.
///
/// RTP 入站发布者的每个会话状态。
struct ActiveIngressSession {
    _lease: PublishLease,
    sink: Box<dyn PublisherSink>,
    _tracks: Vec<TrackInfo>,
    /// Stream key used to publish into the engine.
    stream_key: StreamKey,
    /// Whether the media-online event has already been emitted for this session.
    online_reported: bool,
    /// Bounded cache of frames that arrived before the publisher was ready / authenticated.
    /// ZLM-style behaviour: see `vendor-ref/ZLMediaKit/src/Rtp/RtpProcess.cpp` `_cached_func`.
    pending_frames: std::collections::VecDeque<Arc<cheetah_codec::AVFrame>>,
    pending_frames_capacity: usize,
    publisher_ready: bool,
}

/// Drive the RTP driver event loop, translating ingress events into engine publishes.
///
/// 驱动 RTP 驱动事件循环，将入站事件转换为引擎发布。
pub(crate) async fn run_ingress_worker(
    ctx: EngineContext,
    handle: Arc<RtpDriverHandle>,
    orchestrator: Arc<RtpSessionOrchestrator>,
    cancel: CancellationToken,
    publish_frame_cache_capacity: usize,
    ready: Option<OneShotSender>,
) {
    let mut sessions: HashMap<String, ActiveIngressSession> = HashMap::new();

    // Signal that the ingress worker has been polled and is ready to consume
    // driver events. This prevents races where the first `open_*` call returns
    // before the worker has entered its event loop.
    if let Some(ready) = ready {
        let _ = ready.send();
    }

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let event_fut = handle.recv_event().fuse();
        pin_mut!(cancel_fut, event_fut);

        let event = select_biased! {
            _ = cancel_fut => break,
            ev = event_fut => match ev {
                Some(e) => e,
                None => break,
            },
        };

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                transport_mode,
            } => {
                info!("RTP ingress session created: key={session_key}, ssrc={ssrc}, payload={payload_mode:?}, transport={transport_mode:?}");
                let sk = parse_session_key(&session_key);

                // Only receiver-side sessions publish into the engine. Sender sessions
                // pull frames through a separate subscriber/egress path and must not
                // claim the publish lease here.
                if transport_mode == RtpTransportMode::SendOnly {
                    continue;
                }

                match ctx
                    .publisher_api
                    .acquire_publisher(sk.clone(), PublisherOptions::default())
                    .await
                {
                    Ok((lease, sink)) => {
                        sessions.insert(
                            session_key,
                            ActiveIngressSession {
                                _lease: lease,
                                sink,
                                _tracks: Vec::new(),
                                stream_key: sk.clone(),
                                online_reported: false,
                                pending_frames: std::collections::VecDeque::new(),
                                pending_frames_capacity: publish_frame_cache_capacity,
                                publisher_ready: true,
                            },
                        );
                    }
                    Err(e) => {
                        error!("RTP acquire_publisher failed for {sk}: {e}");
                        // A receiver that cannot secure the publish lease before the first
                        // frame must not publish into the engine; tear it down cleanly.
                        // Send the stop command directly: the orchestrator entry may not
                        // exist yet because `SessionCreated` is emitted before the provider
                        // inserts the session record. `SessionClosed` will remove the entry.
                        handle
                            .send_command(RtpDriverCommand::StopSession(session_key))
                            .await;
                    }
                }
            }
            RtpCoreEvent::TrackFound {
                session_key,
                tracks,
            } => {
                if let Some(session) = sessions.get_mut(&session_key) {
                    debug!("RTP tracks found for {session_key}: {tracks:?}");
                    let _ = session.sink.update_tracks(tracks.clone());
                    session._tracks = tracks;
                }
            }
            RtpCoreEvent::Frame {
                session_key,
                frame,
                source_addr,
            } => {
                if let Some(session) = sessions.get_mut(&session_key) {
                    let frame_arc = Arc::new(frame);
                    if session.publisher_ready {
                        // Drain any frames buffered while waiting for publisher readiness.
                        while let Some(buffered) = session.pending_frames.pop_front() {
                            let _ = session.sink.push_frame(buffered);
                        }
                        let _ = session.sink.push_frame(frame_arc);

                        if !session.online_reported {
                            session.online_reported = true;
                            let session_id =
                                cheetah_sdk::media_api::ids::RtpSessionId(session_key.clone());
                            if let Some(addr) = source_addr {
                                let _ = orchestrator.set_session_remote_endpoint(&session_id, addr);
                            } else {
                                let _ = orchestrator
                                    .set_session_state(&session_id, RtpSessionState::Connected);
                            }
                            ctx.event_bus.publish(SystemEvent::Protocol(ProtocolEvent {
                                protocol: "rtp".to_string(),
                                event_type: "media_online".to_string(),
                                payload: serde_json::json!({
                                    "session_key": session_key,
                                    "stream_key": {
                                        "namespace": session.stream_key.namespace,
                                        "path": session.stream_key.path,
                                    },
                                }),
                            }));
                        }
                    } else if session.pending_frames_capacity > 0 {
                        if session.pending_frames.len() >= session.pending_frames_capacity {
                            session.pending_frames.pop_front();
                        }
                        session.pending_frames.push_back(frame_arc);
                    }
                }
            }
            RtpCoreEvent::SessionUpdated { .. } => {
                // Update acknowledgements are consumed by the driver loop; the module
                // learns about successful updates through the orchestrator snapshot.
            }
            RtpCoreEvent::SessionStateChanged {
                session_key,
                old_state,
                new_state,
            } => {
                debug!(
                    "RTP session state changed: key={session_key}, {old_state:?} -> {new_state:?}"
                );
            }
            RtpCoreEvent::SessionUpdateFailed {
                session_key,
                reason,
            } => {
                warn!("RTP session update failed: key={session_key}, reason={reason}");
            }
            RtpCoreEvent::FormatChanged {
                session_key,
                payload_type,
                old_payload_mode,
                new_payload_mode,
            } => {
                warn!("RTP payload format changed: key={session_key}, pt={payload_type}, {old_payload_mode:?} -> {new_payload_mode:?}");
                // The core keeps the session alive and re-initializes the demuxer for the new
                // format. Re-acquire a publisher for the same stream key so subsequent
                // TrackFound / Frame events under the new format are published.
                if let Some(old) = sessions.remove(&session_key) {
                    let _ = old.sink.close();
                    let sk = parse_session_key(&session_key);
                    match ctx
                        .publisher_api
                        .acquire_publisher(sk.clone(), PublisherOptions::default())
                        .await
                    {
                        Ok((lease, sink)) => {
                            sessions.insert(
                                session_key,
                                ActiveIngressSession {
                                    _lease: lease,
                                    sink,
                                    _tracks: Vec::new(),
                                    stream_key: sk,
                                    online_reported: false,
                                    pending_frames: std::collections::VecDeque::new(),
                                    pending_frames_capacity: publish_frame_cache_capacity,
                                    publisher_ready: true,
                                },
                            );
                        }
                        Err(e) => {
                            error!("RTP re-acquire_publisher failed for {sk}: {e}");
                            // Without a publisher the stream would stay alive in the core but
                            // discard every incoming frame. Tear it down cleanly.
                            let _ = orchestrator.stop_session_by_key(&session_key).await;
                        }
                    }
                } else {
                    warn!("RTP FormatChanged for unknown session: {session_key}");
                }
            }
            RtpCoreEvent::SourceChanged {
                session_key,
                old,
                new,
            } => {
                info!(
                    "RTP source address rebind: key={session_key}, old={}, new={}",
                    hash_endpoint(&old),
                    hash_endpoint(&new),
                );
                // Keep the orchestrator's remote_endpoint in sync so talkback/feedback
                // is sent to the new source address after a validated rebind.
                if let Err(e) = orchestrator
                    .set_session_remote_endpoint(&RtpSessionId(session_key.clone()), new)
                {
                    warn!("Failed to update remote endpoint after source rebind: {e}");
                }
            }
            RtpCoreEvent::SessionClosed {
                session_key,
                reason,
            } => {
                let is_timeout = matches!(
                    reason,
                    RtpSessionCloseReason::IdleTimeout | RtpSessionCloseReason::RrTimeout
                );
                info!("RTP ingress session closed: key={session_key}, reason={reason}");
                let id = RtpSessionId(session_key.clone());
                let closed_session = {
                    let mut guard = orchestrator.sessions.lock();
                    let session = guard.get(&id).cloned();
                    guard.remove(&id);
                    session
                };
                if let Some(session) = sessions.remove(&session_key) {
                    let _ = session.sink.close();
                }

                let event_type = if is_timeout {
                    "rtp_session_timeout"
                } else {
                    "rtp_session_closed"
                };
                ctx.event_bus.publish(SystemEvent::Protocol(ProtocolEvent {
                    protocol: "rtp".to_string(),
                    event_type: event_type.to_string(),
                    payload: serde_json::json!({
                        "session_key": session_key,
                        "reason": reason.to_string(),
                    }),
                }));

                let close_reason = match reason {
                    RtpSessionCloseReason::Stopped => CloseReason::Normal,
                    RtpSessionCloseReason::IdleTimeout => CloseReason::Idle,
                    RtpSessionCloseReason::RrTimeout => CloseReason::Timeout,
                    RtpSessionCloseReason::Bye => CloseReason::Other("rtcp_bye".to_string()),
                    RtpSessionCloseReason::UnresolvablePayloadType { .. }
                    | RtpSessionCloseReason::PayloadModeOscillation { .. } => {
                        CloseReason::Unsupported
                    }
                    RtpSessionCloseReason::ConnectionClosed => {
                        CloseReason::Other("connection_closed".to_string())
                    }
                };
                let kind = closed_session
                    .as_ref()
                    .map(|s| rtp_session_kind_to_session_kind(s.kind))
                    .or_else(|| rtp_session_kind_from_session_key(&session_key));
                let media_key = closed_session
                    .as_ref()
                    .map(|s| s.media_key.clone())
                    .or_else(|| media_key_from_session_key(&session_key));
                let now_ms = (ctx.runtime_api.now().as_micros() / 1000) as i64;
                if !is_timeout {
                    if let (Some(kind), Some(media_key)) = (kind, media_key) {
                        let _ =
                            ctx.media_event_bus
                                .publish(MediaEvent::SessionClosed(SessionClosed {
                                    header: EventHeader {
                                        event_id: format!(
                                            "rtp-session-closed-{session_key}-{now_ms}"
                                        ),
                                        occurred_at: now_ms,
                                        sequence: None,
                                        media_key: Some(media_key),
                                        source: "rtp-module".to_string(),
                                        correlation_id: Some(session_key.clone()),
                                    },
                                    kind,
                                    session_id: SessionId(session_key.clone()),
                                    reason: close_reason.clone(),
                                }));
                    }
                }

                if let Some(rtp_session) = closed_session.filter(|_| is_timeout) {
                    let _ = ctx.media_event_bus.publish(MediaEvent::RtpSessionTimeout(
                        RtpSessionTimeout {
                            header: EventHeader {
                                event_id: format!("rtp-timeout-{session_key}-{now_ms}"),
                                occurred_at: now_ms,
                                sequence: None,
                                media_key: Some(rtp_session.media_key),
                                source: "rtp-module".to_string(),
                                correlation_id: Some(session_key),
                            },
                            session_id: rtp_session.session_id,
                            local_port: rtp_session.local_port,
                            tcp_mode: rtp_session.tcp_mode,
                            reuse_port: rtp_session.reuse_port,
                            ssrc: rtp_session.ssrc,
                        },
                    ));
                }
            }
        }
    }

    for (_, session) in sessions {
        let _ = session.sink.close();
    }
}

/// Parse `session_key` into a `StreamKey`.
///
/// Modern orchestrator keys use `{kind}:{namespace}:{path}` so the `session_id`
/// remains a single URL path segment. Legacy 2/3-segment slash forms are still
/// accepted for pull jobs and backward compatibility.
///
/// 将 `session_key` 解析为 `StreamKey`。
/// 新版编排器键使用 `{kind}:{namespace}:{path}`，使 `session_id` 在 URL path 中保持单一段；
/// 对旧的 2/3 段斜杠形式仍兼容，用于 pull 任务。
fn parse_session_key(key: &str) -> StreamKey {
    // Modern orchestrator keys are `{kind}:{namespace}:{path}` and always start
    // with a known kind prefix. Legacy pull/slash keys fall back to '/'.
    let is_modern = key.starts_with("recv:")
        || key.starts_with("send:")
        || key.starts_with("pull:")
        || key.starts_with("talk:");
    let sep = if is_modern { ':' } else { '/' };
    let mut it = key.splitn(3, sep);
    match (it.next(), it.next(), it.next()) {
        (Some(_kind), Some(ns), Some(path)) => StreamKey::new(ns, path),
        (Some(ns), Some(path), None) => StreamKey::new(ns, path),
        (Some(path), None, None) => StreamKey::new("live", path),
        _ => StreamKey::new("live", key),
    }
}

fn rtp_session_kind_to_session_kind(kind: RtpSessionKind) -> SessionKind {
    match kind {
        RtpSessionKind::Receiver => SessionKind::RtpReceiver,
        RtpSessionKind::Sender | RtpSessionKind::Talk => SessionKind::RtpSender,
    }
}

fn rtp_session_kind_from_session_key(key: &str) -> Option<SessionKind> {
    if key.starts_with("recv:") {
        Some(SessionKind::RtpReceiver)
    } else if key.starts_with("send:") || key.starts_with("pull:") {
        Some(SessionKind::RtpSender)
    } else if key.starts_with("talk:") {
        // Talkback is a duplex sender session from the event consumer's point of view.
        Some(SessionKind::RtpSender)
    } else {
        None
    }
}

fn media_key_from_session_key(key: &str) -> Option<MediaKey> {
    let stream_key = parse_session_key(key);
    MediaKey::with_default_vhost(&stream_key.namespace, &stream_key.path, None).ok()
}
