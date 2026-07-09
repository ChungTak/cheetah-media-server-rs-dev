//! Client pull/push job registry.
//!
//! Each job represents a long-running supervisor that:
//! 1. Asks the local WebRTC core to produce an SDP offer
//!    (via `WebRtcDriverCommand::CreateOffer`).
//! 2. POSTs the offer to a remote WHIP / WHEP / SMS-style endpoint.
//! 3. Feeds the remote answer back via
//!    `WebRtcDriverCommand::ApplyRemoteAnswer`.
//! 4. Optionally records the response `Location` header for later
//!    `DELETE` on stop.
//!
//! The supervisor is bounded by:
//! * Per-job HTTP timeouts (10 s default).
//! * Exponential retry backoff up to a configured max.
//! * SSRF: by default only public DNS-resolved IPs are allowed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_sdk::{CancellationToken, EngineContext, RuntimeApi, StreamKey};
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcSessionId, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{WebRtcDriverCommand, WebRtcDriverHandle};
use parking_lot::Mutex;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, info};

use crate::http::AnswerDispatcher;
use crate::http_client::{HttpClientRequest, WhipWhepHttpClient};
use crate::session::WebRtcSessionIdAllocator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcJobKind {
    Pull,
    Push,
}

impl WebRtcJobKind {
    pub fn label(self) -> &'static str {
        match self {
            WebRtcJobKind::Pull => "pull",
            WebRtcJobKind::Push => "push",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebRtcSignalingProtocol {
    Whip,
    Whep,
}

#[derive(Debug, Clone)]
pub struct WebRtcClientJobSpec {
    pub kind: WebRtcJobKind,
    pub stream_key: StreamKey,
    pub url: String,
    pub protocol: WebRtcSignalingProtocol,
    pub timeout: Duration,
    pub retry: bool,
    pub retry_initial_backoff: Duration,
    pub retry_max_backoff: Duration,
    pub max_retries: u32,
    pub max_response_bytes: usize,
    pub allow_private_ips: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcJobState {
    Pending,
    Connecting,
    Connected,
    Failed,
    Stopped,
}

#[derive(Debug, Error)]
pub enum WebRtcJobError {
    #[error("job already exists for stream {0}")]
    Conflict(String),
    #[error("job not found: {0}")]
    NotFound(String),
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("driver error: {0}")]
    Driver(String),
}

/// Per-job runtime state surfaced to HTTP `*list` endpoints.
#[derive(Debug, Clone)]
pub struct WebRtcJobSnapshot {
    pub kind: WebRtcJobKind,
    pub stream_key: String,
    pub url: String,
    pub state: WebRtcJobState,
    pub last_error: Option<String>,
    pub retry_count: u32,
    pub remote_session_location: Option<String>,
    pub local_session_id: Option<WebRtcSessionId>,
}

#[derive(Default)]
pub struct WebRtcJobRegistry {
    pull: HashMap<String, JobEntry>,
    push: HashMap<String, JobEntry>,
}

struct JobEntry {
    snapshot: Arc<Mutex<WebRtcJobSnapshot>>,
    cancel: CancellationToken,
}

impl WebRtcJobRegistry {
    pub fn list(&self, kind: WebRtcJobKind) -> Vec<WebRtcJobSnapshot> {
        let map = match kind {
            WebRtcJobKind::Pull => &self.pull,
            WebRtcJobKind::Push => &self.push,
        };
        map.values().map(|e| e.snapshot.lock().clone()).collect()
    }

    pub fn cancel_all(&mut self) {
        for (_, entry) in self.pull.drain() {
            entry.cancel.cancel();
        }
        for (_, entry) in self.push.drain() {
            entry.cancel.cancel();
        }
    }

    pub fn stop(&mut self, kind: WebRtcJobKind, stream_key: &str) -> bool {
        let map = match kind {
            WebRtcJobKind::Pull => &mut self.pull,
            WebRtcJobKind::Push => &mut self.push,
        };
        match map.remove(stream_key) {
            Some(entry) => {
                entry.cancel.cancel();
                true
            }
            None => false,
        }
    }

    fn insert(
        &mut self,
        kind: WebRtcJobKind,
        stream_key: String,
        snapshot: Arc<Mutex<WebRtcJobSnapshot>>,
        cancel: CancellationToken,
    ) -> Result<(), WebRtcJobError> {
        let map = match kind {
            WebRtcJobKind::Pull => &mut self.pull,
            WebRtcJobKind::Push => &mut self.push,
        };
        if let Some(existing) = map.get(&stream_key) {
            // Allow replacing entries that have already terminated
            // (Failed / Stopped). Active entries (Pending / Connecting
            // / Connected) are treated as a conflict and the caller
            // gets a 409.
            let state = existing.snapshot.lock().state;
            let terminal = matches!(state, WebRtcJobState::Failed | WebRtcJobState::Stopped);
            if !terminal {
                return Err(WebRtcJobError::Conflict(stream_key));
            }
            // Cancel the old (already-finished) supervisor's token
            // for cleanliness; this is a no-op if it was already
            // cancelled.
            existing.cancel.cancel();
        }
        map.insert(stream_key, JobEntry { snapshot, cancel });
        Ok(())
    }
}

/// Spawn a supervised pull/push job.
///
/// Spawn a supervised pull/push job.
///
/// The supervisor performs the full WebRTC client handshake:
///   1. Allocate a local session id.
///   2. Subscribe to the answer-dispatcher for that id (the offer
///      arrives as `OfferReady` from the driver and is delivered via
///      the same dispatcher path that handles WHIP/WHEP server-side
///      answers).
///   3. Issue `WebRtcDriverCommand::CreateOffer` to ask the local
///      WebRTC core to produce an SDP offer.
///   4. POST the offer to the remote signaling endpoint, parse the
///      response, and apply the remote answer through
///      `WebRtcDriverCommand::ApplyRemoteAnswer`.
///   5. Park on cancel; on cancel, send `DELETE` to the resource
///      `Location` and `StopSession` to release driver state.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_job(
    registry: Arc<Mutex<WebRtcJobRegistry>>,
    ctx: EngineContext,
    driver: Arc<WebRtcDriverHandle>,
    http_client: WhipWhepHttpClient,
    dispatcher: Arc<AnswerDispatcher>,
    allocator: Arc<WebRtcSessionIdAllocator>,
    spec: WebRtcClientJobSpec,
    parent_cancel: CancellationToken,
) -> Result<WebRtcJobSnapshot, WebRtcJobError> {
    let stream_key_str = format!("{}", spec.stream_key);
    let snapshot = Arc::new(Mutex::new(WebRtcJobSnapshot {
        kind: spec.kind,
        stream_key: stream_key_str.clone(),
        url: spec.url.clone(),
        state: WebRtcJobState::Pending,
        last_error: None,
        retry_count: 0,
        remote_session_location: None,
        local_session_id: None,
    }));
    let job_cancel = parent_cancel.child_token();
    {
        let mut guard = registry.lock();
        guard.insert(
            spec.kind,
            stream_key_str.clone(),
            snapshot.clone(),
            job_cancel.clone(),
        )?;
    }

    let initial = snapshot.lock().clone();
    let runtime_api = ctx.runtime_api.clone();
    runtime_api.spawn(Box::pin(async move {
        run_supervisor(
            spec,
            snapshot,
            http_client,
            driver,
            dispatcher,
            allocator,
            ctx,
            job_cancel,
        )
        .await;
    }));
    Ok(initial)
}

#[allow(clippy::too_many_arguments)]
async fn run_supervisor(
    spec: WebRtcClientJobSpec,
    snapshot: Arc<Mutex<WebRtcJobSnapshot>>,
    http_client: WhipWhepHttpClient,
    driver: Arc<WebRtcDriverHandle>,
    dispatcher: Arc<AnswerDispatcher>,
    allocator: Arc<WebRtcSessionIdAllocator>,
    ctx: EngineContext,
    cancel: CancellationToken,
) {
    let max_retries = if spec.retry {
        spec.max_retries.max(1)
    } else {
        1
    };
    let mut backoff = spec.retry_initial_backoff.max(Duration::from_millis(100));
    let max_backoff = spec.retry_max_backoff.max(backoff);
    for attempt in 0..max_retries {
        if cancel.is_cancelled() {
            mark(&snapshot, |s| {
                s.state = WebRtcJobState::Stopped;
            });
            return;
        }
        mark(&snapshot, |s| {
            s.state = WebRtcJobState::Connecting;
            s.retry_count = attempt;
        });

        let attempt_outcome = run_attempt(
            &spec,
            &snapshot,
            &http_client,
            &driver,
            &dispatcher,
            &allocator,
            &ctx.runtime_api,
            &cancel,
        )
        .await;

        match attempt_outcome {
            AttemptOutcome::Connected {
                session_id,
                location,
            } => {
                mark(&snapshot, |s| {
                    s.state = WebRtcJobState::Connected;
                    s.last_error = None;
                    s.remote_session_location = location.clone();
                    s.local_session_id = Some(session_id);
                });
                info!(
                    "WebRTC {} job {} connected: session={} location={:?}",
                    spec.kind.label(),
                    spec.stream_key,
                    session_id,
                    location
                );
                cancel.cancelled().await;
                mark(&snapshot, |s| {
                    s.state = WebRtcJobState::Stopped;
                });

                // Stop the local driver session and DELETE the
                // remote resource if the server published a Location.
                driver
                    .send_command(WebRtcDriverCommand::StopSession {
                        session_id,
                        reason: WebRtcCloseReason::Normal,
                    })
                    .await;

                if let Some(loc) = location {
                    let absolute = absolute_location(&spec.url, &loc);
                    let req = HttpClientRequest::new_delete(absolute);
                    let _ = http_client.send(req).await;
                }
                return;
            }
            AttemptOutcome::Stopped => {
                mark(&snapshot, |s| {
                    s.state = WebRtcJobState::Stopped;
                });
                return;
            }
            AttemptOutcome::Failed { reason, transient } => {
                debug!(
                    "WebRTC {} job {} attempt {} failed: {reason}",
                    spec.kind.label(),
                    spec.stream_key,
                    attempt
                );
                mark(&snapshot, |s| {
                    s.state = WebRtcJobState::Failed;
                    s.last_error = Some(reason);
                });
                if !spec.retry || attempt + 1 >= max_retries || !transient {
                    return;
                }
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => {
                mark(&snapshot, |s| { s.state = WebRtcJobState::Stopped; });
                return;
            }
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

enum AttemptOutcome {
    Connected {
        session_id: WebRtcSessionId,
        location: Option<String>,
    },
    Stopped,
    Failed {
        reason: String,
        transient: bool,
    },
}

#[allow(clippy::too_many_arguments)]
async fn run_attempt(
    spec: &WebRtcClientJobSpec,
    snapshot: &Arc<Mutex<WebRtcJobSnapshot>>,
    http_client: &WhipWhepHttpClient,
    driver: &Arc<WebRtcDriverHandle>,
    dispatcher: &Arc<AnswerDispatcher>,
    allocator: &Arc<WebRtcSessionIdAllocator>,
    runtime: &Arc<dyn RuntimeApi>,
    cancel: &CancellationToken,
) -> AttemptOutcome {
    let session_id = allocator.allocate();
    mark(snapshot, |s| {
        s.local_session_id = Some(session_id);
    });

    let waiter = dispatcher.subscribe(session_id);
    let role = match spec.kind {
        WebRtcJobKind::Pull => WebRtcSessionRole::Publisher,
        WebRtcJobKind::Push => WebRtcSessionRole::Player,
    };
    let offer_direction = match spec.kind {
        WebRtcJobKind::Pull => WebRtcOfferDirection::RecvOnly,
        WebRtcJobKind::Push => WebRtcOfferDirection::SendOnly,
    };
    let offer_spec = WebRtcOfferSpec {
        video_direction: Some(offer_direction),
        audio_direction: Some(offer_direction),
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

    // Wait for the driver to surface the local SDP offer.
    let offer_sdp = match wait_dispatcher(waiter, spec.timeout, runtime, cancel).await {
        WaitOutcome::Sdp(sdp) => sdp,
        WaitOutcome::Failure(reason) => {
            // The driver couldn't produce an offer; treat as transient
            // unless cancelled. StopSession is a no-op if the session
            // never landed.
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal(reason.clone()),
                })
                .await;
            return AttemptOutcome::Failed {
                reason: format!("create offer failed: {reason}"),
                transient: true,
            };
        }
        WaitOutcome::Cancelled => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Normal,
                })
                .await;
            return AttemptOutcome::Stopped;
        }
        WaitOutcome::Timeout => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::HandshakeTimeout,
                })
                .await;
            return AttemptOutcome::Failed {
                reason: "driver did not produce an SDP offer in time".into(),
                transient: true,
            };
        }
    };

    // POST the offer to the remote endpoint.
    let mut req = HttpClientRequest::new_post_sdp(spec.url.clone(), Bytes::from(offer_sdp));
    req.timeout = spec.timeout;
    req.max_response_bytes = spec.max_response_bytes;
    req.allow_private_ips = spec.allow_private_ips;
    let resp = match http_client.send(req).await {
        Ok(resp) => resp,
        Err(err) => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal(err.to_string()),
                })
                .await;
            return AttemptOutcome::Failed {
                reason: err.to_string(),
                transient: true,
            };
        }
    };
    if !(200..300).contains(&resp.status) {
        driver
            .send_command(WebRtcDriverCommand::StopSession {
                session_id,
                reason: WebRtcCloseReason::Internal(format!("remote status {}", resp.status)),
            })
            .await;
        let body_preview = String::from_utf8_lossy(&resp.body[..resp.body.len().min(256)]);
        // 4xx is permanent (config error / auth / bad SDP); 5xx is
        // transient. 3xx falls through to "transient" because we do
        // not follow redirects automatically.
        let transient = !(400..500).contains(&resp.status);
        return AttemptOutcome::Failed {
            reason: format!("remote status={} body={}", resp.status, body_preview),
            transient,
        };
    }
    let location = resp.header("location").map(|s| s.to_string());
    let answer_sdp = match std::str::from_utf8(&resp.body) {
        Ok(s) => s.to_string(),
        Err(_) => {
            driver
                .send_command(WebRtcDriverCommand::StopSession {
                    session_id,
                    reason: WebRtcCloseReason::Internal("non-utf8 sdp body".into()),
                })
                .await;
            return AttemptOutcome::Failed {
                reason: "remote returned non-utf8 SDP body".into(),
                transient: false,
            };
        }
    };

    // Apply the remote answer to the local session.
    driver
        .send_command(WebRtcDriverCommand::ApplyRemoteAnswer {
            session_id,
            remote_sdp: answer_sdp,
        })
        .await;

    AttemptOutcome::Connected {
        session_id,
        location,
    }
}

enum WaitOutcome {
    Sdp(String),
    Failure(String),
    Cancelled,
    Timeout,
}

async fn wait_dispatcher(
    waiter: tokio::sync::oneshot::Receiver<crate::http::AnswerOutcome>,
    timeout: Duration,
    _runtime: &Arc<dyn RuntimeApi>,
    cancel: &CancellationToken,
) -> WaitOutcome {
    let timeout = timeout
        .max(Duration::from_secs(1))
        .min(Duration::from_secs(60));
    tokio::select! {
        _ = cancel.cancelled() => WaitOutcome::Cancelled,
        res = tokio::time::timeout(timeout, waiter) => match res {
            Ok(Ok(crate::http::AnswerOutcome::Sdp(sdp))) => WaitOutcome::Sdp(sdp),
            Ok(Ok(crate::http::AnswerOutcome::Failed(reason))) => WaitOutcome::Failure(reason),
            Ok(Err(_)) => WaitOutcome::Failure("dispatcher channel closed".into()),
            Err(_) => WaitOutcome::Timeout,
        },
    }
}

fn absolute_location(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    if let Some(authority_end) = base
        .strip_prefix("https://")
        .map(|rest| (true, rest))
        .or_else(|| base.strip_prefix("http://").map(|rest| (false, rest)))
    {
        let (https, rest) = authority_end;
        let scheme = if https { "https" } else { "http" };
        let authority = rest.split('/').next().unwrap_or(rest);
        if location.starts_with('/') {
            format!("{scheme}://{authority}{location}")
        } else {
            format!("{scheme}://{authority}/{location}")
        }
    } else {
        location.to_string()
    }
}

fn mark(snapshot: &Arc<Mutex<WebRtcJobSnapshot>>, f: impl FnOnce(&mut WebRtcJobSnapshot)) {
    let mut guard = snapshot.lock();
    f(&mut guard);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_location_prefers_existing_scheme() {
        assert_eq!(
            absolute_location("https://e.com/whip", "https://other/x"),
            "https://other/x"
        );
        assert_eq!(
            absolute_location("https://e.com/whip", "/x?session=1"),
            "https://e.com/x?session=1"
        );
        assert_eq!(
            absolute_location("http://e.com/whip", "session/123"),
            "http://e.com/session/123"
        );
    }

    #[test]
    fn registry_rejects_duplicate_stream_keys() {
        let mut reg = WebRtcJobRegistry::default();
        let snap = Arc::new(Mutex::new(WebRtcJobSnapshot {
            kind: WebRtcJobKind::Pull,
            stream_key: "live/demo".into(),
            url: "https://example.com".into(),
            state: WebRtcJobState::Pending,
            last_error: None,
            retry_count: 0,
            remote_session_location: None,
            local_session_id: None,
        }));
        let cancel = CancellationToken::new();
        reg.insert(
            WebRtcJobKind::Pull,
            "live/demo".into(),
            snap.clone(),
            cancel.clone(),
        )
        .unwrap();
        let err = reg
            .insert(
                WebRtcJobKind::Pull,
                "live/demo".into(),
                snap,
                CancellationToken::new(),
            )
            .unwrap_err();
        assert!(matches!(err, WebRtcJobError::Conflict(_)));
    }

    #[test]
    fn registry_replaces_terminal_state_entries() {
        // Once a job lands in Failed or Stopped, a fresh `/start` for
        // the same stream key should succeed (the old supervisor has
        // already exited; the entry is just an audit-trail).
        let mut reg = WebRtcJobRegistry::default();
        let old_snap = Arc::new(Mutex::new(WebRtcJobSnapshot {
            kind: WebRtcJobKind::Push,
            stream_key: "live/replay".into(),
            url: "https://example.com".into(),
            state: WebRtcJobState::Failed,
            last_error: Some("connection refused".into()),
            retry_count: 1,
            remote_session_location: None,
            local_session_id: None,
        }));
        reg.insert(
            WebRtcJobKind::Push,
            "live/replay".into(),
            old_snap,
            CancellationToken::new(),
        )
        .expect("insert old terminal entry");

        let new_snap = Arc::new(Mutex::new(WebRtcJobSnapshot {
            kind: WebRtcJobKind::Push,
            stream_key: "live/replay".into(),
            url: "https://example.com".into(),
            state: WebRtcJobState::Pending,
            last_error: None,
            retry_count: 0,
            remote_session_location: None,
            local_session_id: None,
        }));
        reg.insert(
            WebRtcJobKind::Push,
            "live/replay".into(),
            new_snap.clone(),
            CancellationToken::new(),
        )
        .expect("replace terminal entry");

        let listed = reg.list(WebRtcJobKind::Push);
        assert_eq!(listed.len(), 1);
        assert!(matches!(listed[0].state, WebRtcJobState::Pending));
    }
}
