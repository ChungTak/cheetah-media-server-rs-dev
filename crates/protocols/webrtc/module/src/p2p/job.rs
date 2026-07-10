//! Pure-state P2P job state machine.
//!
//! The state machine sequences a single P2P session lifecycle:
//!
//! ```text
//! Pending --offer ready--> Offering --check_in/offer sent--> Awaiting Answer
//!                                                                   |
//!                                                                   v
//!                                                              answer applied
//!                                                                   |
//!                                                                   v
//!                                                                Connected
//!                                                                   |
//!                                                                   v
//!                                                                  Bye
//! ```
//!
//! Inputs are deliberately small ADT variants so a transport runner
//! can drive the state machine without owning network state. Outputs
//! are the actions a runner should take (send a message, push a
//! candidate to the driver, surface an error, …).
//!
//! This separation matches the rest of the codebase: protocol logic
//! is Sans-I/O, transport runners glue it to async I/O.

use thiserror::Error;

use super::buffer::{
    BufferState, PendingCandidate, PendingCandidateBuffer, PushOutcome,
    PENDING_CANDIDATE_DEFAULT_CAP,
};
use super::message::{P2pDirection, P2pMessage, P2pMessageHeader, P2pStreamTuple};

/// Job kind drives the SDP direction in the offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pJobKind {
    /// `Pull` variant.
    /// `Pull` 变体.
    Pull,
    /// `Push` variant.
    /// `Push` 变体.
    Push,
}

impl From<P2pJobKind> for P2pDirection {
    fn from(value: P2pJobKind) -> Self {
        match value {
            P2pJobKind::Pull => P2pDirection::Pull,
            P2pJobKind::Push => P2pDirection::Push,
        }
    }
}

/// Lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pJobState {
    /// Initial: no offer produced yet.
    Pending,
    /// Local SDP offer has been produced; waiting to send it.
    Offering,
    /// Offer sent; waiting for the remote answer.
    AwaitingAnswer,
    /// Answer applied; session is up.
    Connected,
    /// Job finalized (graceful close).
    Bye,
    /// Job finalized with error.
    Failed,
}

/// `P2pJobError` enumeration.
/// `P2pJobError` 枚举.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum P2pJobError {
    /// `InvalidTransition` variant.
    /// `InvalidTransition` 变体.
    #[error("invalid transition: cannot apply {what} in state {state:?}")]
    InvalidTransition {
        what: &'static str,
        state: P2pJobState,
    },
    /// `PeerProtocol` variant.
    /// `PeerProtocol` 变体.
    #[error("inconsistent peer message: {0}")]
    PeerProtocol(String),
}

/// Inputs the runner feeds the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum P2pJobInput {
    /// The driver produced a local offer (response to `CreateOffer`).
    LocalOfferReady { sdp: String },
    /// The remote answer SDP arrived.
    RemoteAnswer { sdp: String },
    /// A trickle ICE candidate from the remote.
    RemoteCandidate(PendingCandidate),
    /// The driver reports the WebRTC session is connected.
    DriverConnected,
    /// User asked for graceful close.
    LocalBye { reason: Option<String> },
    /// The peer sent a `bye`.
    RemoteBye { reason: Option<String> },
    /// Transport-level error (e.g. WebSocket dropped).
    TransportError(String),
}

/// Side-effect requests the runner must perform.
#[derive(Debug, Clone, PartialEq)]
pub enum P2pJobAction {
    /// Send `check_in` (with the offer piggy-backed) to the remote.
    SendCheckIn { message: P2pMessage },
    /// Apply the remote SDP answer to the WebRTC driver.
    ApplyRemoteAnswer { sdp: String },
    /// Trickle a remote candidate into the WebRTC driver. Already
    /// dedup-checked by the buffer.
    AddRemoteCandidate(PendingCandidate),
    /// Send a `bye` message on the wire.
    SendBye { message: P2pMessage },
    /// Surface a diagnostic to the operator. Non-fatal.
    Diagnostic(String),
    /// Surface a fatal error. The runner stops the job.
    Fatal(String),
}

/// Configuration knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pJobConfig {
    /// `kind` field of type `P2pJobKind`.
    /// `kind` 字段，类型为 `P2pJobKind`.
    pub kind: P2pJobKind,
    /// `stream` field of type `P2pStreamTuple`.
    /// `stream` 字段，类型为 `P2pStreamTuple`.
    pub stream: P2pStreamTuple,
    /// `local_room_id` field of type `String`.
    /// `local_room_id` 字段，类型为 `String`.
    pub local_room_id: String,
    /// `peer_room_id` field of type `String`.
    /// `peer_room_id` 字段，类型为 `String`.
    pub peer_room_id: String,
    /// `transport_id` field of type `String`.
    /// `transport_id` 字段，类型为 `String`.
    pub transport_id: String,
    /// `pending_candidate_cap` field of type `usize`.
    /// `pending_candidate_cap` 字段，类型为 `usize`.
    pub pending_candidate_cap: usize,
}

impl Default for P2pJobConfig {
    fn default() -> Self {
        Self {
            kind: P2pJobKind::Pull,
            stream: P2pStreamTuple::default(),
            local_room_id: String::new(),
            peer_room_id: String::new(),
            transport_id: String::new(),
            pending_candidate_cap: PENDING_CANDIDATE_DEFAULT_CAP,
        }
    }
}

/// Pure state machine.
#[derive(Debug)]
pub struct P2pJob {
    /// `config` field of type `P2pJobConfig`.
    /// `config` 字段，类型为 `P2pJobConfig`.
    config: P2pJobConfig,
    /// `state` field of type `P2pJobState`.
    /// `state` 字段，类型为 `P2pJobState`.
    state: P2pJobState,
    /// `pending` field of type `PendingCandidateBuffer`.
    /// `pending` 字段，类型为 `PendingCandidateBuffer`.
    pending: PendingCandidateBuffer,
    /// `last_error` field.
    /// `last_error` 字段.
    last_error: Option<String>,
}

impl P2pJob {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(config: P2pJobConfig) -> Self {
        let cap = config.pending_candidate_cap.max(1);
        let pending = PendingCandidateBuffer::new(cap).expect("non-zero cap is guaranteed above");
        Self {
            config,
            state: P2pJobState::Pending,
            pending,
            last_error: None,
        }
    }

    /// `state` function.
    /// `state` 函数.
    pub fn state(&self) -> P2pJobState {
        self.state
    }

    /// `last_error` function.
    /// `last_error` 函数.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// `config` function.
    /// `config` 函数.
    pub fn config(&self) -> &P2pJobConfig {
        &self.config
    }

    /// `pending_state` function.
    /// `pending_state` 函数.
    pub fn pending_state(&self) -> BufferState {
        self.pending.state()
    }

    /// Number of candidates currently buffered. 0 once `flush` has run.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Apply an input, producing zero or more actions.
    pub fn apply(&mut self, input: P2pJobInput) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match input {
            P2pJobInput::LocalOfferReady { sdp } => self.on_local_offer(sdp),
            P2pJobInput::RemoteAnswer { sdp } => self.on_remote_answer(sdp),
            P2pJobInput::RemoteCandidate(c) => self.on_remote_candidate(c),
            P2pJobInput::DriverConnected => self.on_driver_connected(),
            P2pJobInput::LocalBye { reason } => self.on_local_bye(reason),
            P2pJobInput::RemoteBye { reason } => self.on_remote_bye(reason),
            P2pJobInput::TransportError(reason) => self.on_transport_error(reason),
        }
    }

    fn header(&self) -> P2pMessageHeader {
        P2pMessageHeader {
            room_id: Some(self.config.peer_room_id.clone()),
            peer_id: Some(self.config.local_room_id.clone()),
            transport_id: Some(self.config.transport_id.clone()),
        }
    }

    fn on_local_offer(&mut self, sdp: String) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::Pending => {}
            _ => {
                return Err(P2pJobError::InvalidTransition {
                    what: "LocalOfferReady",
                    state: self.state,
                });
            }
        }
        self.state = P2pJobState::AwaitingAnswer;
        Ok(vec![P2pJobAction::SendCheckIn {
            message: P2pMessage::CheckIn {
                header: self.header(),
                direction: self.config.kind.into(),
                stream: self.config.stream.clone(),
                sdp: Some(sdp),
            },
        }])
    }

    fn on_remote_answer(&mut self, sdp: String) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::AwaitingAnswer => {}
            _ => {
                return Err(P2pJobError::InvalidTransition {
                    what: "RemoteAnswer",
                    state: self.state,
                });
            }
        }
        let mut actions = vec![P2pJobAction::ApplyRemoteAnswer { sdp }];
        // Flush whatever candidates accumulated before the answer.
        let buffered = self.pending.flush();
        actions.extend(buffered.into_iter().map(P2pJobAction::AddRemoteCandidate));
        // Stay in AwaitingAnswer until the driver reports
        // `DriverConnected`. The runner can still emit candidates in
        // the meantime — they go straight through because
        // `pending.state == Open`.
        Ok(actions)
    }

    fn on_remote_candidate(
        &mut self,
        candidate: PendingCandidate,
    ) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::Pending
            | P2pJobState::Offering
            | P2pJobState::AwaitingAnswer
            | P2pJobState::Connected => {}
            P2pJobState::Bye | P2pJobState::Failed => {
                return Err(P2pJobError::InvalidTransition {
                    what: "RemoteCandidate",
                    state: self.state,
                });
            }
        }
        let outcome = self.pending.push(candidate.clone());
        match outcome {
            PushOutcome::Buffered => {
                if matches!(self.pending.state(), BufferState::Open) {
                    Ok(vec![P2pJobAction::AddRemoteCandidate(candidate)])
                } else {
                    Ok(Vec::new())
                }
            }
            PushOutcome::Duplicate => Ok(vec![P2pJobAction::Diagnostic(
                "duplicate remote candidate ignored".into(),
            )]),
            PushOutcome::Evicted { dropped } => {
                let mut actions = vec![P2pJobAction::Diagnostic(format!(
                    "pending candidate buffer evicted oldest entry `{}`",
                    dropped.candidate
                ))];
                if matches!(self.pending.state(), BufferState::Open) {
                    actions.push(P2pJobAction::AddRemoteCandidate(candidate));
                }
                Ok(actions)
            }
            PushOutcome::Closed => Err(P2pJobError::InvalidTransition {
                what: "RemoteCandidate",
                state: self.state,
            }),
        }
    }

    fn on_driver_connected(&mut self) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::AwaitingAnswer | P2pJobState::Connected => {
                self.state = P2pJobState::Connected;
                Ok(Vec::new())
            }
            other => Err(P2pJobError::InvalidTransition {
                what: "DriverConnected",
                state: other,
            }),
        }
    }

    fn on_local_bye(&mut self, reason: Option<String>) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::Bye | P2pJobState::Failed => return Ok(Vec::new()),
            _ => {}
        }
        self.state = P2pJobState::Bye;
        self.pending.close();
        Ok(vec![P2pJobAction::SendBye {
            message: P2pMessage::Bye {
                header: self.header(),
                reason,
            },
        }])
    }

    fn on_remote_bye(&mut self, reason: Option<String>) -> Result<Vec<P2pJobAction>, P2pJobError> {
        match self.state {
            P2pJobState::Bye | P2pJobState::Failed => return Ok(Vec::new()),
            _ => {}
        }
        self.state = P2pJobState::Bye;
        self.pending.close();
        Ok(vec![P2pJobAction::Diagnostic(format!(
            "remote sent bye: {}",
            reason.unwrap_or_else(|| "<no reason>".into())
        ))])
    }

    fn on_transport_error(&mut self, reason: String) -> Result<Vec<P2pJobAction>, P2pJobError> {
        if matches!(self.state, P2pJobState::Failed | P2pJobState::Bye) {
            return Ok(Vec::new());
        }
        self.last_error = Some(reason.clone());
        self.state = P2pJobState::Failed;
        self.pending.close();
        Ok(vec![P2pJobAction::Fatal(reason)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: P2pJobKind) -> P2pJobConfig {
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

    #[test]
    fn pull_happy_path_state_progression() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        assert_eq!(job.state(), P2pJobState::Pending);
        let actions = job
            .apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            P2pJobAction::SendCheckIn { message } => match message {
                P2pMessage::CheckIn { direction, .. } => {
                    assert_eq!(*direction, P2pDirection::Pull);
                }
                other => panic!("expected CheckIn: {other:?}"),
            },
            other => panic!("expected SendCheckIn: {other:?}"),
        }
        assert_eq!(job.state(), P2pJobState::AwaitingAnswer);

        let actions = job
            .apply(P2pJobInput::RemoteAnswer {
                sdp: "v=0\nanswer".into(),
            })
            .unwrap();
        assert!(matches!(actions[0], P2pJobAction::ApplyRemoteAnswer { .. }));
        // No buffered candidates yet, so only one action.
        assert_eq!(actions.len(), 1);

        let actions = job.apply(P2pJobInput::DriverConnected).unwrap();
        assert!(actions.is_empty());
        assert_eq!(job.state(), P2pJobState::Connected);

        let actions = job.apply(P2pJobInput::LocalBye { reason: None }).unwrap();
        assert!(matches!(actions[0], P2pJobAction::SendBye { .. }));
        assert_eq!(job.state(), P2pJobState::Bye);
    }

    #[test]
    fn candidates_arriving_before_answer_are_flushed_after() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        job.apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        job.apply(P2pJobInput::RemoteCandidate(PendingCandidate {
            candidate: "c1".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        }))
        .unwrap();
        job.apply(P2pJobInput::RemoteCandidate(PendingCandidate {
            candidate: "c2".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        }))
        .unwrap();
        // Pending until answer arrives.
        assert_eq!(job.pending_count(), 2);

        let actions = job
            .apply(P2pJobInput::RemoteAnswer {
                sdp: "v=0\n".into(),
            })
            .unwrap();
        // ApplyRemoteAnswer + 2 candidates in arrival order.
        assert_eq!(actions.len(), 3);
        match &actions[0] {
            P2pJobAction::ApplyRemoteAnswer { .. } => {}
            other => panic!("first action should be ApplyRemoteAnswer: {other:?}"),
        }
        let candidates: Vec<&str> = actions[1..]
            .iter()
            .filter_map(|a| match a {
                P2pJobAction::AddRemoteCandidate(c) => Some(c.candidate.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(candidates, vec!["c1", "c2"]);
    }

    #[test]
    fn candidate_after_answer_passes_through() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Push));
        job.apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        job.apply(P2pJobInput::RemoteAnswer { sdp: "v=0".into() })
            .unwrap();
        let actions = job
            .apply(P2pJobInput::RemoteCandidate(PendingCandidate {
                candidate: "c1".into(),
                sdp_mid: None,
                sdp_mline_index: None,
            }))
            .unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], P2pJobAction::AddRemoteCandidate(_)));
    }

    #[test]
    fn duplicate_candidate_emits_diagnostic_only() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        let cand = PendingCandidate {
            candidate: "c1".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        };
        job.apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        job.apply(P2pJobInput::RemoteCandidate(cand.clone()))
            .unwrap();
        let actions = job.apply(P2pJobInput::RemoteCandidate(cand)).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], P2pJobAction::Diagnostic(_)));
    }

    #[test]
    fn transport_error_marks_job_failed() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        let actions = job
            .apply(P2pJobInput::TransportError("websocket peer reset".into()))
            .unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], P2pJobAction::Fatal(_)));
        assert_eq!(job.state(), P2pJobState::Failed);
        assert_eq!(job.last_error(), Some("websocket peer reset"));
    }

    #[test]
    fn invalid_offer_after_answer_returns_error() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        job.apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        let err = job
            .apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap_err();
        assert!(matches!(err, P2pJobError::InvalidTransition { .. }));
    }

    #[test]
    fn remote_bye_drains_pending_buffer() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        job.apply(P2pJobInput::LocalOfferReady { sdp: "v=0".into() })
            .unwrap();
        job.apply(P2pJobInput::RemoteCandidate(PendingCandidate {
            candidate: "c1".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        }))
        .unwrap();
        let actions = job
            .apply(P2pJobInput::RemoteBye {
                reason: Some("hangup".into()),
            })
            .unwrap();
        assert!(matches!(actions[0], P2pJobAction::Diagnostic(_)));
        assert_eq!(job.state(), P2pJobState::Bye);
        assert_eq!(job.pending_state(), BufferState::Closed);
    }

    #[test]
    fn double_bye_is_idempotent() {
        let mut job = P2pJob::new(cfg(P2pJobKind::Pull));
        job.apply(P2pJobInput::LocalBye { reason: None }).unwrap();
        let actions = job
            .apply(P2pJobInput::LocalBye {
                reason: Some("again".into()),
            })
            .unwrap();
        assert!(actions.is_empty(), "double bye should be a no-op");
    }
}
