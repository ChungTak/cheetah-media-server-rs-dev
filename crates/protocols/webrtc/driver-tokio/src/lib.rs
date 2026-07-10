//! Tokio-backed driver for `cheetah-webrtc-core`.
//!
//! Phase 02 implements:
//! * UDP single-port listener with address-based session routing.
//! * STUN-driven session demultiplex via [`WebRtcCore::route_unbound_packet`].
//! * Per-session timer wheel built on `tokio::time::sleep_until`.
//! * Bounded command, event, and outbound-packet queues.
//! * Local candidate gathering from the configured listen address.
//! * Hooks for connection migration: when a known session sees a new
//!   remote address through its already-bound `Rtc::accepts`, the route
//!   table updates and a `RouteUpdated` event fires.
//! * RFC 4571 TCP framing helper ([`Tcp4571Decoder`]) and an optional
//!   TCP listener bound to `listen_tcp`. Inbound TCP frames are routed
//!   through the same `WebRtcCore::route_unbound_packet` path as UDP.
//!
//! Out of scope for Phase 02 (covered by later phases):
//! * Multi-shard work distribution. Today every session lives on one
//!   driver task. Sharding is a follow-up.
//! * Full STUN-binding migration with stale-route TTL trees; the basic
//!   migration path works because `WebRtcCore` updates routes whenever a
//!   network input is delivered through it, and the driver re-binds on
//!   the next observed packet.

#![allow(missing_docs)]

mod config;
mod directory;
/// `http_client` module.
/// `http_client` 模块.
pub mod http_client;
mod io_front;
mod migration;
mod route;
mod runner;
mod sdp;
mod shard;
mod stun;
mod tcp;
mod ws;

pub use config::{UdpPortRange, WebRtcDriverConfig};
pub use directory::{
    RouteDirectory, RouteDirectoryConfig, RouteDirectoryError, RouteDirectoryEvictionStats,
    RouteDirectoryStats, ShardCandidateTable, ShardId, WebRtcShardCandidateStats, WebRtcShardStats,
};
pub use http_client::{
    HttpClientError, HttpClientRequest, HttpClientResponse, HttpMethod, WhipWhepHttpClient,
};
pub use migration::{RouteCandidateDiff, WebRtcRouteUpdate};
pub use runner::{
    spawn_driver, WebRtcDriverCommand, WebRtcDriverDiagnostic, WebRtcDriverDiagnosticKind,
    WebRtcDriverEvent, WebRtcDriverHandle, WebRtcDriverStats, WebRtcSendError, WebRtcSessionSpec,
    WebRtcTcpCloseReason,
};
pub use sdp::{count_local_candidates, CandidateTransportPolicy, LocalCandidateCounts};
pub use shard::{
    BalancedStickyShardStrategy, HashShardStrategy, LeastLoadedShardStrategy,
    LoadAwareRebalanceStrategy, ShardLoad, ShardSelector, ShardSelectorStrategy,
    StickyHashShardStrategy, StickyOverRebalanceStrategy,
};
pub use tcp::{encode_frame as tcp_encode_frame, Tcp4571Decoder, Tcp4571Error};
pub use ws::{
    bind_ws_server, TokioWsConnection, TokioWsConnector, WsConnection, WsConnectionHandler,
    WsConnector, WsError, WsFrame, WsInbound, WsServerConfig, WsServerError, WsServerListener,
};

pub use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcCoreConfig, WebRtcCoreEvent, WebRtcSessionId, WebRtcSessionRole,
};
