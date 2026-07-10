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
//!
//! Tokio 支持 driver 为 `cheetah-webrtc-core`。
//!
//! 第 02 阶段实施：
//! * UDP 单端口侦听器，具有基于地址的会话路由。
//! * 通过 [`WebRtcCore::route_unbound_packet`] STUN 驱动的会话多路分解。
//! * 基于 `tokio::time::sleep_until` 构建的每会话计时器轮。
//! * 有界命令、事件和出站数据包队列。
//! * 从配置的监听地址收集本地 candidate。
//! * 用于连接迁移的挂钩：当已知会话通过其已绑定的 `Rtc::accepts` 看到新的远程地址时，路由表会更新并触发 `RouteUpdated` 事件。
//! * RFC 4571 TCP 框架助手 ([`Tcp4571Decoder`]) 和绑定到 `listen_tcp` 的可选 TCP 侦听器。
//!   入站 TCP 帧通过与 UDP 相同的 `WebRtcCore::route_unbound_packet` 路径进行路由。
//!
//! 超出阶段 02 的范围（由后续阶段涵盖）：
//! * 多 shard 工作分配。
//!   如今，每个会话都围绕一项 driver 任务。
//!   shard 是后续。
//! * 使用陈旧路线 TTL 树进行完整的 STUN 绑定迁移；
//!   基本迁移路径之所以有效，是因为只要网络输入通过 `WebRtcCore` 传递，`WebRtcCore` 就会更新路由，并且 driver 会重新绑定下一个观察到的数据包。

#![allow(missing_docs)]

mod config;
mod directory;
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
