//! Output items emitted by the core.

use std::net::SocketAddr;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::WebRtcCoreDiagnostic;
use crate::event::WebRtcCoreEvent;
use crate::input::WebRtcCloseReason;
use crate::types::WebRtcSessionId;

/// `WebRtcCoreOutput` enumeration.
/// `WebRtcCoreOutput` 枚举。
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

/// Kind of `Web Rtc Local Description`.
/// `Web Rtc Local Description` 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebRtcLocalDescriptionKind {
    Offer,
    Answer,
}

/// `WebRtcPacketOut` data structure.
/// `WebRtcPacketOut` 数据结构。
#[derive(Debug, Clone)]
pub struct WebRtcPacketOut {
    pub session_id: WebRtcSessionId,
    pub source: Option<SocketAddr>,
    pub destination: SocketAddr,
    pub data: Bytes,
}

/// `WebRtcTimer` data structure.
/// `WebRtcTimer` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcTimer {
    pub session_id: WebRtcSessionId,
    /// Deadline expressed in microseconds anchored at the core start time.
    pub deadline_micros: u64,
}
