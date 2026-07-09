//! Output items emitted by the core.

use std::net::SocketAddr;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::WebRtcCoreDiagnostic;
use crate::event::WebRtcCoreEvent;
use crate::input::WebRtcCloseReason;
use crate::types::WebRtcSessionId;

#[derive(Debug, Clone)]
pub enum WebRtcCoreOutput {
    /// Network packet to deliver via the driver socket.
    SendPacket(WebRtcPacketOut),
    /// Driver should arm a timer for the given deadline.
    ///
    /// Subsequent `SetTimer` for the same session supersede earlier ones.
    SetTimer(WebRtcTimer),
    /// Session emitted a state-machine event.
    Event(WebRtcCoreEvent),
    /// Diagnostic record (non-fatal observation).
    Diagnostic(WebRtcCoreDiagnostic),
    /// Local SDP description ready (offer or answer).
    LocalDescription {
        session_id: WebRtcSessionId,
        sdp: String,
        kind: WebRtcLocalDescriptionKind,
    },
    /// The driver should release this session and any associated state.
    CloseSession {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcLocalDescriptionKind {
    Offer,
    Answer,
}

#[derive(Debug, Clone)]
pub struct WebRtcPacketOut {
    pub session_id: WebRtcSessionId,
    pub source: Option<SocketAddr>,
    pub destination: SocketAddr,
    pub data: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcTimer {
    pub session_id: WebRtcSessionId,
    /// Deadline expressed in microseconds anchored at the core start time.
    pub deadline_micros: u64,
}
