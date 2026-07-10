//! ZLMediaKit-style WebRTC P2P signaling (`signaling_protocols=1`).
//!
//! Phase 05 follow-up (`plans-27-webrtc-zlm2/phase-05-p2p-signaling.md`):
//! provides the ZLM-compatible WebSocket P2P signaling stack used when
//! a `webrtc://...?signaling_protocols=1&peer_room_id=...` URL drives
//! `webrtc-module` pull/push. Only signaling lives here — ICE/DTLS/SRTP
//! and SCTP stay in `cheetah-webrtc-core` (`str0m`).
//!
//! ## Module layout
//!
//! * [`message`] — wire schema with strict per-field bounds. Unknown
//!   message types decode into [`message::P2pMessage::Unknown`] and
//!   are rejected on the receive path with an explicit `error` reply.
//! * [`room`] — local room keeper registry with bounded membership.
//!
//! ## Boundary rules
//!
//! * Wire schema is parse-then-validate: every `String` field has a
//!   length cap and the candidate / SDP fields are bounded
//!   independently so a misbehaving peer can't flood memory.
//! * The registry never mutates `WebRtcCore` state; it only tracks
//!   keeper configuration and bookkeeping.
//! * Both layers are runtime-neutral: no `tokio::*`, no socket types,
//!   no I/O.
//!
//! Subsequent rounds of work (still draft as of this commit):
//!
//! * `client.rs` — outbound WebSocket signaling client built on the
//!   message + room modules.
//! * `job.rs` — pull/push job state machine bridging the client to
//!   `WebRtcDriverHandle`.

/// `bridge` module.
/// `bridge` 模块.
pub mod bridge;
/// `buffer` module.
/// `buffer` 模块.
pub mod buffer;
/// `entrypoint` module.
/// `entrypoint` 模块.
pub mod entrypoint;
/// `hub` module.
/// `hub` 模块.
pub mod hub;
/// `job` module.
/// `job` 模块.
pub mod job;
/// `lifecycle_dispatcher` module.
/// `lifecycle_dispatcher` 模块.
pub mod lifecycle_dispatcher;
/// `message` module.
/// `message` 模块.
pub mod message;
/// `room` module.
/// `room` 模块.
pub mod room;
/// `server` module.
/// `server` 模块.
pub mod server;
/// `supervisor` module.
/// `supervisor` 模块.
pub mod supervisor;
/// `transport` module.
/// `transport` 模块.
pub mod transport;
/// `url` module.
/// `url` 模块.
pub mod url;
/// `ws` module.
/// `ws` 模块.
pub mod ws;

pub use bridge::{
    run_bridge, run_bridge_with_lifecycle, BridgeLifecycleEvent, BridgeLifecycleSource,
    DispatcherOfferOutcome, DispatcherOfferWaiter, NoopLifecycleSource, P2pBridgeConfig,
    P2pBridgeOutcome, P2pDriverSink, P2pOfferWaiter,
};
pub use entrypoint::{
    plan_from_zlm_url, P2pBridgePlan, P2pBridgePlanError, P2pBridgePlanInput,
    P2P_DEFAULT_OFFER_TIMEOUT,
};
pub use hub::{HubBackedTransport, KeeperHub, KeeperHubConfig, KeeperHubError, PeerKey};
pub use lifecycle_dispatcher::LifecycleDispatcher;
pub use server::{
    run_server as run_signaling_server, ConnectionHandler as SignalingServerHandler,
    InboundConnection as SignalingServerInbound, SignalingServerConfig, SignalingServerError,
};
pub use supervisor::{
    run_supervisor, run_supervisor_with_hub, KeeperHubObserver, KeeperSupervisorConfig,
    KeeperSupervisorOutcome, KeeperTransportFactory,
};
pub use ws::{
    snapshot_counters as snapshot_websocket_counters, WebSocketCounterSnapshot, WebSocketCounters,
    WebSocketP2pTransport, WebSocketTransportConfig, WebSocketTransportError,
    WebSocketTransportFactory,
};

pub use buffer::{
    BufferState, PendingBufferError, PendingCandidate, PendingCandidateBuffer, PushOutcome,
    PENDING_CANDIDATE_DEFAULT_CAP,
};
pub use job::{
    P2pJob, P2pJobAction, P2pJobConfig, P2pJobError, P2pJobInput, P2pJobKind, P2pJobState,
};
pub use message::{
    P2pDirection, P2pMessage, P2pMessageError, P2pMessageHeader, P2pStreamTuple,
    P2P_DEFAULT_MAX_CANDIDATE_BYTES, P2P_DEFAULT_MAX_MESSAGE_BYTES, P2P_DEFAULT_MAX_SDP_BYTES,
    P2P_MAX_FIELD_BYTES,
};
pub use room::{
    P2pKeeperState, P2pKeeperStatus, P2pRoomKeeperConfig, P2pRoomKeeperError, P2pRoomKeeperKey,
    P2pRoomKeeperRegistry, P2pRoomKeeperSnapshot,
};
pub use transport::{InMemoryTransport, P2pTransport, P2pTransportError, P2pTransportEvent};
pub use url::{
    is_private_ip, parse as parse_signaling_url, SignalingUrl, SignalingUrlError,
    SignalingUrlPolicy, SIGNALING_URL_MAX_BYTES,
};
