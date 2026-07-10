//! Glue between [`P2pJob`] and [`WebRtcDriverHandle`].
//!
//! The bridge is the runtime-aware piece that:
//!
//! 1. Allocates a local WebRTC session and asks the driver for an SDP
//!    offer (`WebRtcDriverCommand::CreateOffer`).
//! 2. Waits for `OfferReady` via [`AnswerDispatcher`] and feeds it to
//!    `P2pJob::apply(LocalOfferReady)`.
//! 3. Translates each [`P2pJobAction`] into a transport `send` and/or
//!    a driver command.
//! 4. Forwards inbound [`P2pTransportEvent::Message`] frames to the
//!    job as the matching `P2pJobInput::*` variant.
//! 5. Stops cleanly when the parent cancellation token fires or the
//!    job reaches a terminal state.
//!
//! The bridge is generic over `P2pTransport` so production code uses
//! a real WebSocket transport while tests use [`InMemoryTransport`].
//! The `WebRtcDriverHandle` connection point is hidden behind a
//! [`P2pDriverSink`] trait so the unit tests don't have to spin up
//! a real UDP driver.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::MonoTime;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcSessionId, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{WebRtcDriverCommand, WebRtcDriverHandle};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use parking_lot::Mutex;
use thiserror::Error;

use super::buffer::PendingCandidate;
use super::job::{P2pJob, P2pJobAction, P2pJobConfig, P2pJobInput, P2pJobKind, P2pJobState};
use super::message::{P2pMessage, P2pMessageError};
use super::transport::{P2pTransport, P2pTransportError, P2pTransportEvent};

/// A sink for driver commands. Production uses [`WebRtcDriverHandle`];
/// tests can plug in a recorder.
#[async_trait]
pub trait P2pDriverSink: Send + Sync {
    async fn send_command(&self, cmd: WebRtcDriverCommand);
}

/// Subscriber for the driver's `OfferReady` / `AnswerReady` channel.
/// The bridge calls `subscribe(session_id)` once before issuing the
/// `CreateOffer` command.
#[async_trait]
pub trait P2pOfferWaiter: Send + Sync {
    /// Return a single-shot future that resolves with the SDP offer
    /// produced by the driver, or with a failure reason.
    async fn wait_for_offer(
        &self,
        session_id: WebRtcSessionId,
        timeout: Duration,
    ) -> Result<String, String>;
}

#[async_trait]
impl P2pDriverSink for Arc<WebRtcDriverHandle> {
    async fn send_command(&self, cmd: WebRtcDriverCommand) {
        WebRtcDriverHandle::send_command(self, cmd).await;
    }
}

/// Blanket impl so callers can pass `Arc<T>` for any sink type. This
/// keeps tests ergonomic (they share a `RecordingDriverSink` between
/// the bridge and the asserter) without duplicating impls.
#[async_trait]
impl<T: P2pDriverSink + ?Sized> P2pDriverSink for Arc<T> {
    async fn send_command(&self, cmd: WebRtcDriverCommand) {
        T::send_command(self, cmd).await;
    }
}

/// `AnswerDispatcher`-backed waiter. Production code constructs this
/// directly from the module-owned dispatcher.
pub struct DispatcherOfferWaiter {
    /// Per-session subscription factory. Returns a future that
    /// resolves once the driver produces an offer (or the underlying
    /// channel is dropped, surfaced as `Failed`).
    subscribe:
        Box<dyn Fn(WebRtcSessionId) -> BoxFuture<'static, DispatcherOfferOutcome> + Send + Sync>,
    /// Runtime handle for the offer-wait timeout.
    runtime: Arc<dyn RuntimeApi>,
}

/// Dispatcher-side outcome. Mirrors the existing [`crate::http::AnswerOutcome`]
/// shape so adapters can be written without re-exposing private types.
#[derive(Debug, Clone)]
pub enum DispatcherOfferOutcome {
    Sdp(String),
    Failed(String),
}

impl DispatcherOfferWaiter {
    pub fn new<F>(runtime: Arc<dyn RuntimeApi>, subscribe: F) -> Self
    where
        F: Fn(WebRtcSessionId) -> BoxFuture<'static, DispatcherOfferOutcome>
            + Send
            + Sync
            + 'static,
    {
        Self {
            subscribe: Box::new(subscribe),
            runtime,
        }
    }
}

#[async_trait]
impl P2pOfferWaiter for DispatcherOfferWaiter {
    async fn wait_for_offer(
        &self,
        session_id: WebRtcSessionId,
        timeout: Duration,
    ) -> Result<String, String> {
        let timeout_us = u64::try_from(timeout.as_micros()).unwrap_or(u64::MAX);
        let deadline =
            MonoTime::from_micros(self.runtime.now().as_micros().saturating_add(timeout_us));
        let mut timer = self.runtime.sleep_until(deadline);
        let waiter = (self.subscribe)(session_id).fuse();
        let wait = timer.wait().fuse();
        futures::pin_mut!(waiter, wait);
        futures::select_biased! {
            outcome = waiter => match outcome {
                DispatcherOfferOutcome::Sdp(sdp) => Ok(sdp),
                DispatcherOfferOutcome::Failed(reason) => Err(reason),
            },
            _ = wait => Err("driver did not produce an offer in time".into()),
        }
    }
}

/// Source of driver lifecycle events for the bridge.
///
/// The bridge calls [`subscribe`](Self::subscribe) once before issuing
/// `CreateOffer`. The returned receiver yields a single
/// [`BridgeLifecycleEvent`] per session id and is dropped once the
/// bridge sees a terminal event or the bridge itself exits.
///
/// Production wires this up to the existing `WebRtcDriverEvent::Core`
/// stream in `module.rs::run_driver_event_worker`. Tests use the
/// in-crate [`StaticLifecycleSource`] to replay events on demand.
#[async_trait]
pub trait BridgeLifecycleSource: Send + Sync {
    /// Subscribe to lifecycle events for the given session. The
    /// returned channel must yield at most one event per state and
    /// is closed when the source has nothing more to deliver.
    async fn subscribe(&self, session_id: WebRtcSessionId) -> mpsc::Receiver<BridgeLifecycleEvent>;
}

/// Lifecycle events the bridge cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeLifecycleEvent {
    /// Driver reports `Lifecycle::Connected` — ICE+DTLS+SRTP up.
    Connected,
    /// Driver reports the session was closed before connect.
    Closed { reason: String },
}

/// Bridge configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pBridgeConfig {
    pub job: P2pJobConfig,
    /// Local session id. Must be unique across the driver. Callers
    /// allocate via `WebRtcSessionIdAllocator`.
    pub session_id: WebRtcSessionId,
    /// Time the bridge waits for `OfferReady` after sending
    /// `CreateOffer`. Mirrors the WHIP/WHEP pull job timeout.
    pub offer_timeout: Duration,
}

/// Run-time outcome of a single bridge run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2pBridgeOutcome {
    /// Job reached a terminal `Bye` / `Failed` state, the transport
    /// closed cleanly, or the cancellation token fired.
    Completed { final_state: P2pJobState },
    /// Bridge couldn't even produce an offer.
    OfferFailed { reason: String },
    /// Transport blew up before / during delivery.
    TransportError { reason: P2pTransportError },
    /// Outbound message refused by the schema validator.
    Encode { reason: P2pMessageError },
}

#[derive(Debug, Clone, Error)]
pub enum P2pBridgeError {
    #[error("bridge already finished")]
    AlreadyFinished,
}

/// Drive a `P2pJob` from start to finish.
///
/// Convenience wrapper over [`run_bridge_with_lifecycle`] for callers
/// that don't have a lifecycle source. The job will stay in
/// `AwaitingAnswer` after the answer arrives because nothing emits
/// `DriverConnected`; that's fine for the WHIP/WHEP-style flows where
/// the bridge tears down on `bye` rather than waiting for connect.
pub async fn run_bridge<T, S, W>(
    config: P2pBridgeConfig,
    transport: T,
    driver: S,
    waiter: Arc<W>,
    cancel: CancellationToken,
) -> P2pBridgeOutcome
where
    T: P2pTransport,
    S: P2pDriverSink,
    W: P2pOfferWaiter + ?Sized + 'static,
{
    run_bridge_with_lifecycle::<T, S, W, NoopLifecycleSource>(
        config, transport, driver, waiter, None, cancel,
    )
    .await
}

/// Outcome of the bridge main-loop multi-wait, resolved before any arm
/// side effect runs so the borrow on `lifecycle_rx` is released.
enum BridgeStep {
    Cancelled,
    Lifecycle(Option<BridgeLifecycleEvent>),
    Transport(Result<P2pTransportEvent, P2pTransportError>),
}

/// Helper used inside `run_bridge_with_lifecycle`'s multi-wait to make
/// the lifecycle arm pend forever when no receiver is held (or after
/// the channel closed). Returning `Option<...>` lets the select arm
/// distinguish "real event" / "channel closed" without a busy loop on
/// an empty channel.
async fn recv_lifecycle(
    rx: &mut Option<mpsc::Receiver<BridgeLifecycleEvent>>,
) -> Option<BridgeLifecycleEvent> {
    match rx.as_mut() {
        Some(channel) => channel.next().await,
        None => std::future::pending().await,
    }
}

/// Marker source used when the caller doesn't supply lifecycle events.
pub struct NoopLifecycleSource;

#[async_trait]
impl BridgeLifecycleSource for NoopLifecycleSource {
    async fn subscribe(
        &self,
        _session_id: WebRtcSessionId,
    ) -> mpsc::Receiver<BridgeLifecycleEvent> {
        // Empty channel: the sender is dropped immediately so
        // `next()` returns `None`. The bridge handles this by
        // dropping the receiver and falling back to a never-ready
        // future — see `take_lifecycle_rx` in `run_bridge_with_lifecycle`.
        let (_tx, rx) = mpsc::channel(1);
        rx
    }
}

/// Drive a `P2pJob` from start to finish, optionally observing driver
/// lifecycle events.
///
/// The function:
///
/// * issues `CreateOffer` to the driver,
/// * awaits the resulting offer SDP,
/// * subscribes to lifecycle events (when a source is supplied),
/// * pumps `transport.recv()`, the lifecycle channel, and the cancel
///   token in parallel,
/// * translates [`P2pJobAction`] into outbound `transport.send` and
///   driver commands.
///
/// On exit the bridge sends `StopSession` to the driver if the session
/// was ever activated, and closes the transport.
pub async fn run_bridge_with_lifecycle<T, S, W, L>(
    config: P2pBridgeConfig,
    transport: T,
    driver: S,
    waiter: Arc<W>,
    lifecycle: Option<Arc<L>>,
    cancel: CancellationToken,
) -> P2pBridgeOutcome
where
    T: P2pTransport,
    S: P2pDriverSink,
    W: P2pOfferWaiter + ?Sized + 'static,
    L: BridgeLifecycleSource + ?Sized + 'static,
{
    let mut job = P2pJob::new(config.job.clone());
    let session_id = config.session_id;

    // Subscribe to lifecycle events *before* issuing CreateOffer so
    // we don't race the driver's `Connected` event. When no source
    // is supplied we use `None` and skip the lifecycle arm; when the
    // source closes its channel we also drop the receiver so the
    // select arm stops waking up on a perpetual `None`.
    let mut lifecycle_rx: Option<mpsc::Receiver<BridgeLifecycleEvent>> = match lifecycle.as_ref() {
        Some(source) => Some(source.subscribe(session_id).await),
        None => None,
    };

    // Phase 1: ask the driver for an offer.
    let role = match config.job.kind {
        P2pJobKind::Pull => WebRtcSessionRole::Publisher,
        P2pJobKind::Push => WebRtcSessionRole::Player,
    };
    let direction = match config.job.kind {
        P2pJobKind::Pull => WebRtcOfferDirection::RecvOnly,
        P2pJobKind::Push => WebRtcOfferDirection::SendOnly,
    };
    let offer_spec = WebRtcOfferSpec {
        video_direction: Some(direction),
        audio_direction: Some(direction),
        data_channel: false,
    };
    driver
        .send_command(WebRtcDriverCommand::CreateOffer {
            session_id,
            role,
            spec: offer_spec,
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        })
        .await;

    let offer_sdp = match waiter
        .wait_for_offer(session_id, config.offer_timeout)
        .await
    {
        Ok(sdp) => sdp,
        Err(reason) => {
            // `StopSession` is a no-op if the session never landed,
            // but keep it so we don't leak driver state when the
            // driver started the session and only the offer event
            // raced.
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal(reason.clone()),
                })
                .await;
            transport.close().await;
            return P2pBridgeOutcome::OfferFailed { reason };
        }
    };

    // Phase 2: feed the offer to the job and execute the resulting
    // actions (a `SendCheckIn`).
    let actions = match job.apply(P2pJobInput::LocalOfferReady { sdp: offer_sdp }) {
        Ok(actions) => actions,
        Err(err) => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal(err.to_string()),
                })
                .await;
            transport.close().await;
            return P2pBridgeOutcome::OfferFailed {
                reason: err.to_string(),
            };
        }
    };
    if let Err(err) = execute_actions(&actions, &transport, &driver, session_id).await {
        transport.close().await;
        return err;
    }

    // Phase 3: main loop. Pump the transport while watching the cancel
    // token. Apply each inbound message to the job and execute its
    // resulting actions.
    loop {
        if cancel.is_cancelled() {
            let _ = job.apply(P2pJobInput::LocalBye {
                reason: Some("cancelled".into()),
            });
            break;
        }
        if matches!(job.state(), P2pJobState::Bye | P2pJobState::Failed) {
            break;
        }

        // Runtime-neutral multi-wait: pin each arm's future, fuse it,
        // and let `select_biased!` poll cancel → lifecycle → transport
        // in priority order (matching the previous `biased` select).
        // The chosen arm is reduced to a `BridgeStep` before any arm
        // body runs so the borrow on `lifecycle_rx` is released and we
        // can reassign it in the `None` case.
        let step = {
            let cancelled = cancel.cancelled().fuse();
            let life = recv_lifecycle(&mut lifecycle_rx).fuse();
            let recv = transport.recv().fuse();
            futures::pin_mut!(cancelled, life, recv);
            futures::select_biased! {
                _ = cancelled => BridgeStep::Cancelled,
                ev = life => BridgeStep::Lifecycle(ev),
                res = recv => BridgeStep::Transport(res),
            }
        };
        let event = match step {
            BridgeStep::Cancelled => {
                let _ = job.apply(P2pJobInput::LocalBye {
                    reason: Some("cancelled".into()),
                });
                break;
            }
            BridgeStep::Lifecycle(lifecycle_event) => match lifecycle_event {
                Some(BridgeLifecycleEvent::Connected) => {
                    let actions = match job.apply(P2pJobInput::DriverConnected) {
                        Ok(a) => a,
                        Err(err) => {
                            tracing::debug!(target: "webrtc::p2p::bridge", "job rejected DriverConnected: {err}");
                            continue;
                        }
                    };
                    if let Err(err) =
                        execute_actions(&actions, &transport, &driver, session_id).await
                    {
                        transport.close().await;
                        return err;
                    }
                    continue;
                }
                Some(BridgeLifecycleEvent::Closed { reason }) => {
                    let _ = job.apply(P2pJobInput::TransportError(reason.clone()));
                    driver
                        .send_command(WebRtcDriverCommand::StopSession {
                            session_id,
                            reason: WebRtcCloseReason::Internal(reason.clone()),
                        })
                        .await;
                    transport.close().await;
                    return P2pBridgeOutcome::TransportError {
                        reason: P2pTransportError::Io(reason),
                    };
                }
                None => {
                    // Source closed — drop the receiver so this arm
                    // never wakes again.
                    lifecycle_rx = None;
                    continue;
                }
            },
            BridgeStep::Transport(res) => match res {
                Ok(e) => e,
                Err(err) => {
                    let _ = job.apply(P2pJobInput::TransportError(err.to_string()));
                    driver
                        .send_command(WebRtcDriverCommand::StopSession {
                            session_id,
                            reason: WebRtcCloseReason::Internal(err.to_string()),
                        })
                        .await;
                    transport.close().await;
                    return P2pBridgeOutcome::TransportError { reason: err };
                }
            },
        };

        let inputs = match event {
            P2pTransportEvent::Message(msg) => message_to_inputs(msg),
            P2pTransportEvent::Closed => {
                vec![P2pJobInput::TransportError("transport closed".into())]
            }
            P2pTransportEvent::Error(reason) => vec![P2pJobInput::TransportError(reason)],
        };

        for input in inputs {
            let actions = match job.apply(input) {
                Ok(a) => a,
                Err(err) => {
                    // State machine rejected the input — surface as a
                    // diagnostic and continue. The runner is purely
                    // informational here.
                    tracing::debug!(target: "webrtc::p2p::bridge", "job rejected input: {err}");
                    continue;
                }
            };
            if let Err(err) = execute_actions(&actions, &transport, &driver, session_id).await {
                transport.close().await;
                return err;
            }
        }
    }

    // Phase 4: shutdown. Issue `StopSession` so the driver releases
    // any state it built up, then close the transport.
    driver
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    transport.close().await;
    P2pBridgeOutcome::Completed {
        final_state: job.state(),
    }
}

/// Translate a `P2pMessage` into `P2pJobInput`s. Unknown / `Error` /
/// `Ping` etc. become diagnostics on the job side; we only forward
/// the messages the state machine actually understands.
fn message_to_inputs(msg: P2pMessage) -> Vec<P2pJobInput> {
    match msg {
        P2pMessage::Answer { sdp, .. } => vec![P2pJobInput::RemoteAnswer { sdp }],
        P2pMessage::Candidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => vec![P2pJobInput::RemoteCandidate(PendingCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
        })],
        P2pMessage::Bye { reason, .. } => vec![P2pJobInput::RemoteBye { reason }],
        P2pMessage::CheckIn { sdp: Some(sdp), .. } => {
            // ZLM piggy-backs the answer on `check_in_ok` rather than
            // `check_in`, but some forks reuse the same envelope.
            // Treat an SDP-bearing `check_in` as an answer for
            // robustness.
            vec![P2pJobInput::RemoteAnswer { sdp }]
        }
        P2pMessage::CheckInOk { sdp: Some(sdp), .. } => {
            vec![P2pJobInput::RemoteAnswer { sdp }]
        }
        // The rest (ping/pong/error/room_list/check_in/check_in_ok
        // without sdp/unknown) are not state-machine inputs. The
        // state machine surfaces diagnostics via its own actions.
        _ => Vec::new(),
    }
}

/// Execute a batch of [`P2pJobAction`] in sequence.
async fn execute_actions<T, S>(
    actions: &[P2pJobAction],
    transport: &T,
    driver: &S,
    session_id: WebRtcSessionId,
) -> Result<(), P2pBridgeOutcome>
where
    T: P2pTransport,
    S: P2pDriverSink,
{
    for action in actions {
        match action {
            P2pJobAction::SendCheckIn { message } | P2pJobAction::SendBye { message } => {
                if let Err(err) = transport.send(message.clone()).await {
                    return Err(P2pBridgeOutcome::TransportError { reason: err });
                }
            }
            P2pJobAction::ApplyRemoteAnswer { sdp } => {
                driver
                    .send_command(WebRtcDriverCommand::ApplyRemoteAnswer {
                        session_id,
                        remote_sdp: sdp.clone(),
                    })
                    .await;
            }
            P2pJobAction::AddRemoteCandidate(candidate) => {
                driver
                    .send_command(WebRtcDriverCommand::AddRemoteCandidate {
                        session_id,
                        candidate: candidate.candidate.clone(),
                    })
                    .await;
            }
            P2pJobAction::Diagnostic(message) => {
                tracing::debug!(target: "webrtc::p2p::bridge", session_id = %session_id, "{message}");
            }
            P2pJobAction::Fatal(reason) => {
                driver
                    .send_command(WebRtcDriverCommand::StopSession {
                        session_id,
                        reason: WebRtcCloseReason::Internal(reason.clone()),
                    })
                    .await;
                return Err(P2pBridgeOutcome::TransportError {
                    reason: P2pTransportError::Io(reason.clone()),
                });
            }
        }
    }
    Ok(())
}

/// Test-only sink that records every command. Public to keep the
/// integration test module thin.
#[derive(Debug, Default)]
pub struct RecordingDriverSink {
    pub commands: Mutex<Vec<WebRtcDriverCommand>>,
}

#[async_trait]
impl P2pDriverSink for RecordingDriverSink {
    async fn send_command(&self, cmd: WebRtcDriverCommand) {
        self.commands.lock().push(cmd);
    }
}

/// Test-only waiter that returns a fixed offer SDP after a small
/// delay so the bridge actually awaits.
pub struct StaticOfferWaiter {
    pub sdp: String,
}

#[async_trait]
impl P2pOfferWaiter for StaticOfferWaiter {
    async fn wait_for_offer(
        &self,
        _session_id: WebRtcSessionId,
        _timeout: Duration,
    ) -> Result<String, String> {
        Ok(self.sdp.clone())
    }
}

/// Test-only waiter that always fails after `delay`. Used to verify
/// the bridge emits a `OfferFailed` outcome and stops the session.
pub struct FailingOfferWaiter {
    pub reason: String,
}

#[async_trait]
impl P2pOfferWaiter for FailingOfferWaiter {
    async fn wait_for_offer(
        &self,
        _session_id: WebRtcSessionId,
        _timeout: Duration,
    ) -> Result<String, String> {
        Err(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::message::{P2pMessage, P2pMessageHeader, P2pStreamTuple};
    use crate::p2p::transport::InMemoryTransport;

    fn job_cfg(kind: P2pJobKind) -> P2pJobConfig {
        P2pJobConfig {
            kind,
            stream: P2pStreamTuple {
                vhost: "v".into(),
                app: "live".into(),
                stream: "demo".into(),
            },
            local_room_id: "ringing".into(),
            peer_room_id: "room42".into(),
            transport_id: "tr1".into(),
            pending_candidate_cap: 4,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_pull_happy_path_drives_full_lifecycle() {
        let (local, remote) = InMemoryTransport::pair(8);
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter {
            sdp: "v=0\r\noffer".into(),
        });
        let cancel = CancellationToken::new();
        let cfg = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Pull),
            session_id: WebRtcSessionId::new(101),
            offer_timeout: Duration::from_millis(500),
        };

        // Spawn a fake remote signaling server that replies with an
        // `answer` once the bridge sends its `check_in`, then a
        // `bye` to drive the bridge to its terminal state. Using
        // `bye` instead of cancel guarantees the answer is processed
        // before shutdown.
        let remote_handle = tokio::spawn(async move {
            match remote.recv().await.unwrap() {
                P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
                    let echo_header = P2pMessageHeader {
                        room_id: header.peer_id,
                        peer_id: header.room_id,
                        transport_id: header.transport_id,
                    };
                    remote
                        .send(P2pMessage::Answer {
                            header: echo_header.clone(),
                            sdp: "v=0\r\nanswer".into(),
                        })
                        .await
                        .unwrap();
                    remote
                        .send(P2pMessage::Bye {
                            header: echo_header,
                            reason: Some("done".into()),
                        })
                        .await
                        .unwrap();
                    // Wait for the bridge's outbound `bye` so the
                    // transport stays open until shutdown.
                    let _ = remote.recv().await;
                }
                other => panic!("expected check_in, got {other:?}"),
            }
        });

        let outcome = run_bridge(cfg, local, driver.clone(), waiter, cancel).await;
        remote_handle.await.unwrap();

        match outcome {
            P2pBridgeOutcome::Completed { final_state } => {
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        let cmds = driver.commands.lock().clone();
        // CreateOffer + ApplyRemoteAnswer + StopSession are mandatory.
        assert!(
            matches!(cmds.first(), Some(WebRtcDriverCommand::CreateOffer { .. })),
            "first command must be CreateOffer"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, WebRtcDriverCommand::ApplyRemoteAnswer { .. })),
            "ApplyRemoteAnswer should be present"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, WebRtcDriverCommand::StopSession { .. })),
            "StopSession should be issued on shutdown"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_offer_failure_stops_session_and_returns_offer_failed() {
        let (local, _remote) = InMemoryTransport::pair(4);
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(FailingOfferWaiter {
            reason: "driver did not respond".into(),
        });
        let cancel = CancellationToken::new();
        let cfg = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Pull),
            session_id: WebRtcSessionId::new(202),
            offer_timeout: Duration::from_millis(50),
        };
        let outcome = run_bridge(cfg, local, driver.clone(), waiter, cancel).await;
        match outcome {
            P2pBridgeOutcome::OfferFailed { reason } => {
                assert!(reason.contains("driver"));
            }
            other => panic!("expected OfferFailed, got {other:?}"),
        }
        // Session must be stopped even though the offer never landed.
        let cmds = driver.commands.lock().clone();
        assert!(cmds
            .iter()
            .any(|c| matches!(c, WebRtcDriverCommand::StopSession { .. })));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_remote_bye_marks_job_bye() {
        let (local, remote) = InMemoryTransport::pair(4);
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter { sdp: "v=0".into() });
        let cancel = CancellationToken::new();
        let cfg = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Push),
            session_id: WebRtcSessionId::new(303),
            offer_timeout: Duration::from_millis(50),
        };
        let remote_task = tokio::spawn(async move {
            // Eat the bridge's `check_in`.
            let _ = remote.recv().await;
            // Reply with an immediate `bye` (without an answer).
            remote
                .send(P2pMessage::Bye {
                    header: P2pMessageHeader::default(),
                    reason: Some("no thanks".into()),
                })
                .await
                .unwrap();
        });
        let outcome = run_bridge(cfg, local, driver.clone(), waiter, cancel).await;
        remote_task.await.unwrap();
        match outcome {
            P2pBridgeOutcome::Completed { final_state } => {
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_cancellation_stops_session() {
        let (local, remote) = InMemoryTransport::pair(4);
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter { sdp: "v=0".into() });
        let cancel = CancellationToken::new();
        let cancel_outer = cancel.clone();
        let cfg = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Pull),
            session_id: WebRtcSessionId::new(404),
            offer_timeout: Duration::from_millis(50),
        };
        let remote_task = tokio::spawn(async move {
            // Wait for the bridge's check-in then idle. The cancel
            // token will tear the bridge down before any answer.
            let _ = remote.recv().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_outer.cancel();
            // Drain whatever the bridge emits during shutdown.
            for _ in 0..4 {
                let _ = remote.recv().await;
            }
        });
        let outcome = run_bridge(cfg, local, driver.clone(), waiter, cancel).await;
        remote_task.await.unwrap();
        match outcome {
            P2pBridgeOutcome::Completed { final_state } => {
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        // StopSession was issued.
        let cmds = driver.commands.lock().clone();
        assert!(cmds
            .iter()
            .any(|c| matches!(c, WebRtcDriverCommand::StopSession { .. })));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_with_lifecycle_advances_job_to_connected() {
        // Verifies that when a `BridgeLifecycleSource` emits
        // `Connected` after the answer arrives, the job transitions
        // to `Connected` instead of staying in `AwaitingAnswer`.
        use crate::p2p::bridge::{
            run_bridge_with_lifecycle, BridgeLifecycleEvent, BridgeLifecycleSource,
        };
        use futures::SinkExt;

        struct OneShotLifecycle {
            tx: tokio::sync::Mutex<Option<mpsc::Sender<BridgeLifecycleEvent>>>,
        }

        #[async_trait]
        impl BridgeLifecycleSource for OneShotLifecycle {
            async fn subscribe(
                &self,
                _session_id: WebRtcSessionId,
            ) -> mpsc::Receiver<BridgeLifecycleEvent> {
                let (tx, rx) = mpsc::channel(1);
                *self.tx.lock().await = Some(tx);
                rx
            }
        }

        let (local, remote) = InMemoryTransport::pair(8);
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter {
            sdp: "v=0\r\noffer".into(),
        });
        let lifecycle = Arc::new(OneShotLifecycle {
            tx: tokio::sync::Mutex::new(None),
        });
        let cancel = CancellationToken::new();
        let cfg = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Pull),
            session_id: WebRtcSessionId::new(2024),
            offer_timeout: Duration::from_millis(500),
        };

        let lifecycle_for_bridge = lifecycle.clone();
        let bridge_handle = tokio::spawn(async move {
            run_bridge_with_lifecycle::<_, _, StaticOfferWaiter, OneShotLifecycle>(
                cfg,
                local,
                driver,
                waiter,
                Some(lifecycle_for_bridge),
                cancel,
            )
            .await
        });

        // Server: read check_in, reply with answer, then send
        // `Connected` lifecycle, then bye to terminate.
        match remote.recv().await.unwrap() {
            P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
                let echo_header = P2pMessageHeader {
                    room_id: header.room_id.clone(),
                    peer_id: header.peer_id.clone(),
                    transport_id: header.transport_id.clone(),
                };
                remote
                    .send(P2pMessage::Answer {
                        header: echo_header.clone(),
                        sdp: "v=0\r\nanswer".into(),
                    })
                    .await
                    .unwrap();
                // The bridge subscribes before issuing CreateOffer,
                // so the tx slot is already populated.
                let mut lifecycle_tx = lifecycle.tx.lock().await.clone().expect("subscribed");
                lifecycle_tx
                    .send(BridgeLifecycleEvent::Connected)
                    .await
                    .unwrap();
                remote
                    .send(P2pMessage::Bye {
                        header: echo_header,
                        reason: Some("done".into()),
                    })
                    .await
                    .unwrap();
                let _ = remote.recv().await; // drain bridge's outbound bye
            }
            other => panic!("expected check_in, got {other:?}"),
        }

        let outcome = bridge_handle.await.unwrap();
        match outcome {
            P2pBridgeOutcome::Completed { final_state } => {
                // After Connected → Bye, the job ends in Bye.
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }
}
