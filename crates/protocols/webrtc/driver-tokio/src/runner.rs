//! Driver runtime: owns the UDP listener, the [`WebRtcCore`] instance,
//! and dispatches commands and events.
//!
//! driver 运行时：拥有 UDP 侦听器、[`WebRtcCore`] 实例，并调度命令和事件。

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcCore, WebRtcCoreCommand, WebRtcCoreEvent, WebRtcCoreInput,
    WebRtcCoreOutput, WebRtcNetworkInput, WebRtcRequestKeyframeKind, WebRtcSessionId,
    WebRtcSessionRole,
};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::sleep_until;
use tracing::{debug, info, warn};

use crate::config::WebRtcDriverConfig;
use crate::directory::{
    RouteDirectory, RouteDirectoryConfig, RouteDirectoryStats, ShardCandidateTable, ShardId,
    WebRtcShardCandidateStats, WebRtcShardStats,
};
use crate::migration::{RouteCandidateDiff, WebRtcRouteUpdate};
use crate::route::RouteTable;
use crate::sdp::{
    count_local_candidates, ensure_end_of_candidates, filter_local_candidates,
    CandidateTransportPolicy, LocalCandidateCounts,
};
use crate::shard::{ShardLoadTable, ShardSelector};
use crate::tcp::{encode_frame, Tcp4571Decoder};

/// Spec for creating a new session inside the driver.
///
/// 在 driver 中创建新会话的规范。
#[derive(Debug, Clone)]
pub struct WebRtcSessionSpec {
    /// Session identifier assigned by the caller.
    ///
    /// 由调用者分配的会话标识符。
    pub session_id: WebRtcSessionId,
    /// Whether this endpoint offers or answers.
    ///
    /// 此端点是 offers 还是 answers。
    pub role: WebRtcSessionRole,
    /// Remote SDP offer to accept.
    ///
    /// 远程 SDP offer 接受。
    pub remote_sdp_offer: String,
    /// Policy controlling which local candidates are generated.
    ///
    /// 控制生成哪些本地 candidates 的策略。
    pub candidate_transport_policy: CandidateTransportPolicy,
}

/// Per-shard command envelope used in the multi-shard topology.
///
/// The I/O front-end forwards driver commands to the shard that owns
/// the target session. We wrap the public `WebRtcDriverCommand` enum
/// rather than expose a separate type so the multi-shard plumbing
/// stays an internal concern of the driver crate.
///
/// 多 shard 拓扑中使用的每 shard 命令包络。
///
/// I/O 前端将 driver 命令转发到拥有目标会话的 shard。
/// 我们包装公共 `WebRtcDriverCommand` 枚举，而不是公开单独的类型，因此多 shard 管道仍然是 driver crate 的内部关注点。
#[derive(Debug, Clone)]
pub(crate) enum ShardCommand {
    /// A driver command targeted at this shard.
    ///
    /// 针对此 shard 的 driver 命令。
    Driver(WebRtcDriverCommand),
    /// Test-only: force the shard loop to panic so the supervisor's
    /// auto-eviction path can be exercised from integration tests.
    ///
    /// 仅测试：强制 shard 循环发生恐慌，以便可以从集成测试中执行主管的自动驱逐路径。
    Panic,
}

/// Boundary commands accepted by the driver.
///
/// driver 接受的边界命令。
#[derive(Debug, Clone)]
pub enum WebRtcDriverCommand {
    /// Create a session and accept a remote offer.
    ///
    /// 创建一个会话并接受远程 offer。
    AcceptOffer(WebRtcSessionSpec),
    /// Create a session in offering mode and produce a local SDP offer.
    /// The resulting offer is delivered as a `LocalDescription` core
    /// output and surfaced via [`WebRtcDriverEvent::OfferReady`].
    ///
    /// 在提供模式下创建会话并生成本地 SDP offer。
    /// 生成的 offer 作为 `LocalDescription` 核心输出提供并通过 [`WebRtcDriverEvent::OfferReady`] 呈现。
    CreateOffer {
        session_id: WebRtcSessionId,
        role: WebRtcSessionRole,
        spec: cheetah_webrtc_core::WebRtcOfferSpec,
        candidate_transport_policy: CandidateTransportPolicy,
    },
    /// Trickle a remote ICE candidate into an existing session.
    ///
    /// trickle 将远程 ICE candidate 插入现有会话。
    AddRemoteCandidate {
        session_id: WebRtcSessionId,
        candidate: String,
    },
    /// Trigger an ICE restart on an existing session, producing a
    /// fresh local SDP offer with rotated ICE credentials. The new
    /// offer is delivered via [`WebRtcDriverEvent::OfferReady`] just
    /// like a `CreateOffer` result.
    ///
    /// 在现有会话上触发 ICE 重新启动，生成具有轮换 ICE 凭据的新本地 SDP offer。
    /// 新的 offer 通过 [`WebRtcDriverEvent::OfferReady`] 传递，就像 `CreateOffer` 结果一样。
    IceRestart {
        session_id: WebRtcSessionId,
        keep_local_candidates: bool,
    },
    /// Apply an SDP answer to a previously sent local offer.
    ///
    /// 将 SDP answer 应用到先前发送的本地 offer。
    ApplyRemoteAnswer {
        session_id: WebRtcSessionId,
        remote_sdp: String,
    },
    /// Send a media frame to a player session.
    ///
    /// 将媒体帧发送到播放器会话。
    SendFrame(Box<cheetah_webrtc_core::WebRtcSendFrame>),
    /// Send DataChannel data on a previously opened channel.
    ///
    /// 在先前打开的通道上发送 DataChannel 数据。
    SendDataChannel(cheetah_webrtc_core::WebRtcDataChannelOut),
    /// Ask the remote sender to emit a keyframe for an existing track.
    ///
    /// 要求远程发送者为现有轨道发出关键帧。
    RequestKeyframe {
        session_id: WebRtcSessionId,
        mid: cheetah_webrtc_core::MidLabel,
        kind: WebRtcRequestKeyframeKind,
    },
    /// Close the session and release its resources.
    ///
    /// 关闭会话并释放其资源。
    StopSession {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
    /// Test-only: inject a panic on the target shard so the
    /// supervisor's auto-eviction path can be exercised from
    /// integration tests. Not intended for production use.
    ///
    /// 仅测试：在目标 shard 上注入恐慌，以便可以从集成测试中执行主管的自动驱逐路径。
    /// 不适合生产用途。
    #[doc(hidden)]
    PanicShard { shard_id: ShardId },
}

/// Events surfaced by the driver to the module layer.
///
/// 事件由 driver 呈现到模块层。
#[derive(Debug, Clone)]
pub enum WebRtcDriverEvent {
    /// Session was created and an SDP answer is ready.
    ///
    /// 会话已创建，并且 SDP answer 已准备就绪。
    AnswerReady {
        session_id: WebRtcSessionId,
        sdp: String,
    },
    /// Session was created and a local SDP offer is ready (`CreateOffer`).
    ///
    /// 会话已创建，本地 SDP offer 已准备就绪 (`CreateOffer`)。
    OfferReady {
        session_id: WebRtcSessionId,
        sdp: String,
    },
    /// Plain core event surfaced to the module.
    ///
    /// 普通核心事件出现在模块中。
    Core(WebRtcCoreEvent),
    /// Session has been closed by the driver.
    ///
    /// 会议已于 driver 结束。
    SessionClosed {
        session_id: WebRtcSessionId,
        reason: WebRtcCloseReason,
    },
    /// Connection migration detected.
    ///
    /// 检测到连接迁移。
    RouteUpdated(WebRtcRouteUpdate),
    /// A remote peer opened a TCP connection. Surfaced before any
    /// frames are delivered so observers can correlate STUN binding
    /// requests with the source TCP peer.
    ///
    /// 远程对等点打开了 TCP 连接。
    /// 在传递任何帧之前浮出水面，以便观察者可以将 STUN 绑定请求与源 TCP 对等方关联起来。
    TcpAccepted { remote_addr: SocketAddr },
    /// A previously-accepted TCP connection has been closed (graceful
    /// EOF, peer reset, framing error, or driver shutdown).
    ///
    /// 先前接受的 TCP 连接已关闭（正常 EOF、对等重置、帧错误或 driver 关闭）。
    TcpClosed {
        remote_addr: SocketAddr,
        reason: WebRtcTcpCloseReason,
    },
    /// Diagnostic record from the driver layer.
    ///
    /// 来自 driver 层的诊断记录。
    Diagnostic(WebRtcDriverDiagnostic),
    /// Outbound packet/event queue is approaching capacity. The
    /// `queue` field identifies which queue is congested:
    ///
    /// - `"events"`: the driver→module event channel is full and
    ///   diagnostic events are being dropped.
    /// - `"packets"`: the driver→socket UDP send queue is full and
    ///   media packets are being delayed or dropped.
    ///
    /// `pending` is the current observed depth at the time the
    /// backpressure was detected.
    ///
    /// 出站数据包/事件队列已接近容量。
    /// `queue` 字段标识哪个队列拥塞：
    ///
    /// - `"events"`：driver→模块事件通道已满，诊断事件正在被丢弃。
    /// - `"packets"`：driver→套接字 UDP 发送队列已满，媒体数据包被延迟或丢弃。
    ///
    /// `pending` 是检测到背压时当前观测到的深度。
    Backpressure { queue: String, pending: usize },
    /// A shard task exited. `reason` describes why (`"cancelled"`,
    /// `"panic"`, or a free-form message). Surfaced by the supervisor
    /// in multi-shard mode so operators can tell whether a shard
    /// died unexpectedly. After this event the driver will not
    /// auto-restart the shard; sessions on that shard are
    /// effectively orphaned and the operator should restart the
    /// driver.
    ///
    /// shard 任务已退出。
    /// `reason` 描述了原因（`"cancelled"`、`"panic"` 或自由格式消息）。
    /// 由主管以多 shard 模式显示，以便操作员可以判断 shard 是否意外死亡。
    /// 在此事件之后，driver 将不会自动重新启动 shard；
    /// shard 上的会话实际上是孤立的，操作员应重新启动 driver。
    ShardStopped { shard_id: ShardId, reason: String },
    /// Snapshot of the local ICE candidate counts for a session,
    /// emitted alongside [`WebRtcDriverEvent::AnswerReady`] /
    /// [`WebRtcDriverEvent::OfferReady`] each time the driver
    /// produces a new local description. `shard_id` identifies the
    /// shard that owns the session so multi-shard observers can
    /// attribute candidate gathering results to a specific shard
    /// (single-shard mode reports `ShardId(0)`).
    ///
    /// 本地 ICE candidate 的快照计入一个会话，每次 driver 生成新的本地描述时，与 [`WebRtcDriverEvent::AnswerReady`] / [`WebRtcDriverEvent::OfferReady`] 一起发出。
    /// `shard_id` 标识拥有会话的 shard，因此多 shard 观察者可以将 candidate 收集结果归因于特定的 shard（单 shard 模式报告 `ShardId(0)`）。
    LocalCandidateSnapshot {
        shard_id: ShardId,
        session_id: WebRtcSessionId,
        counts: LocalCandidateCounts,
    },
}

/// Reason a TCP connection ended.
///
/// TCP 连接结束的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcTcpCloseReason {
    /// Remote peer closed the connection cleanly.
    ///
    /// 远程对等点干净地关闭了连接。
    PeerEof,
    /// Read or write returned an error.
    ///
    /// 读取或写入返回错误。
    Io { message: String },
    /// Decoder rejected an oversize / malformed frame.
    ///
    /// 解码器拒绝了过大/格式错误的帧。
    FramingError { message: String },
    /// Connection received no bytes for `tcp_idle_timeout_ms`.
    ///
    /// 连接未收到 `tcp_idle_timeout_ms` 的字节。
    IdleTimeout,
    /// Driver was cancelled or shutting down.
    ///
    /// driver 被取消或关闭。
    Shutdown,
}

/// Driver-level diagnostic.
///
/// driver 级诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcDriverDiagnostic {
    pub session_id: Option<WebRtcSessionId>,
    pub kind: WebRtcDriverDiagnosticKind,
    pub message: String,
}
/// Categories of diagnostic records emitted by the driver.
///
/// driver 发出的诊断记录的类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRtcDriverDiagnosticKind {
    /// A packet arrived with no matching session or ufrag.
    ///
    /// 数据包到达时没有匹配的会话或 ufrag。
    UnroutedPacket,
    /// A UDP send/recv or TCP accept/write failed.
    ///
    /// UDP 发送/recv 或 TCP 接受/写入失败。
    SocketError,
    /// A bounded queue dropped a command or datagram.
    ///
    /// 有界队列丢弃了命令或数据报。
    QueueFull,
    /// A command variant is not yet implemented for the current topology.
    ///
    /// 当前拓扑尚未实现命令变体。
    UnsupportedCommand,
    /// A session or shard lifecycle event worth logging.
    ///
    /// 值得记录的会话或 shard 生命周期事件。
    Lifecycle,
    /// A stale route entry expired after the migration TTL elapsed.
    /// The session is still active on its new address; this is purely
    /// informational for operators monitoring migration behaviour.
    ///
    /// 过时的路由条目在迁移 TTL 结束后过期。
    /// 该会话在新地址上仍然活跃；
    /// 这纯粹是为监测迁移行为的运营商提供信息。
    RouteExpired,
    /// A session migration attempt was rejected because the route
    /// table is at hard capacity. The session continues on its
    /// previous address; the new packet is dropped.
    ///
    /// 由于路由表已达到硬容量，会话迁移尝试被拒绝。
    /// 会议继续之前的地址；
    /// 新数据包被丢弃。
    MigrationRejected,
    /// The global route directory rejected a new binding because it
    /// hit `route_directory_capacity`. Surfaced by the front-end so
    /// operators can grow the cap before sessions start failing.
    ///
    /// 全局路由目录拒绝新绑定，因为它命中了 `route_directory_capacity`。
    /// 由前端显示，以便操作员可以在会话开始失败之前增加上限。
    RouteDirectoryFull,
}

/// Handle to a running driver task.
///
/// 正在运行的 driver 任务的句柄。
pub struct WebRtcDriverHandle {
    cmd_tx: mpsc::Sender<WebRtcDriverCommand>,
    event_rx: tokio::sync::Mutex<mpsc::Receiver<WebRtcDriverEvent>>,
    local_udp_addr: SocketAddr,
    local_tcp_addr: Option<SocketAddr>,
    session_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Cumulative count of commands accepted by the driver task.
    /// Incremented on every successful pop from the command channel.
    /// Used by `stats_snapshot()` for operator-facing observability.
    ///
    /// driver 任务接受的命令的累积计数。
    /// 每次从命令通道成功弹出时都会增加。
    /// 由 `stats_snapshot()` 用于面向操作员的可观察性。
    commands_accepted: Arc<std::sync::atomic::AtomicU64>,
    /// Cumulative count of events surfaced via `recv_event`.
    ///
    /// 通过 `recv_event` 出现的事件累积计数。
    events_emitted: Arc<std::sync::atomic::AtomicU64>,
    /// Cumulative count of unrouted UDP packets dropped at the
    /// driver boundary (no session matched).
    ///
    /// 在 driver 边界丢弃的未路由 UDP 数据包的累积计数（没有匹配的会话）。
    unrouted_packets: Arc<std::sync::atomic::AtomicU64>,
    /// Effective shard count. The current driver task owns one shard
    /// internally; this value is exposed so downstream code can plan
    /// for the multi-shard front-end without churning when it lands.
    ///
    /// 有效 shard 计数。
    /// 当前 driver 任务内部拥有一个 shard；
    /// 该值被公开，以便下游代码可以规划多 shard 前端，而不会在落地时发生混乱。
    shard_count: usize,
    /// Global route directory. Currently populated by a single shard
    /// but cloneable so the upcoming multi-shard front-end can share
    /// it across shards.
    ///
    /// 全局路由目录。
    /// 目前由单个 shard 填充，但可克隆，因此即将推出的多 shard 前端可以在 shards 之间共享它。
    route_directory: Arc<RouteDirectory>,
    /// Per-shard load counts. Selected via [`ShardSelector`] on
    /// session creation; the multi-shard front-end will consume this
    /// table directly to decide where to deliver inbound packets.
    ///
    /// 每个 shard 负载计数。
    /// 在会话创建时通过 [`ShardSelector`] 选择；
    /// multi-shard 前端将直接使用该表来决定将入站数据包传递到何处。
    shard_loads: Arc<ShardLoadTable>,
    /// Per-shard candidate gathering snapshots. Each shard updates
    /// its slot in this table on every `LocalCandidateSnapshot` event
    /// emission so dashboards can read the latest gathering result
    /// per shard without accumulating events themselves.
    ///
    /// 每 shard candidate 收集快照。
    /// 每个 shard 在每次 `LocalCandidateSnapshot` 事件发射时都会更新此表中的槽位，因此仪表板可以读取每个 shard 的最新收集结果，
    /// 而无需累积事件本身。
    shard_candidates: Arc<ShardCandidateTable>,
    /// Shard selector — exposed so tests and integration code can
    /// pre-compute the owner shard for a session id.
    ///
    /// shard 选择器 — 公开，以便测试和集成代码可以预先计算会话 ID 的所有者 shard。
    shard_selector: ShardSelector,
    /// Driver-wide TCP writer registry. Cloned from the same
    /// `Arc<TcpWriterRegistry>` the front-end / shard tasks
    /// consult on the hot path. Held here so [`Self::evict_shard`]
    /// can cascade an operator-driven shard eviction into the
    /// registry, freeing every TCP connection the dead shard
    /// owned without waiting for per-connection idle timeouts.
    ///
    /// driver 范围 TCP 写入器注册表。
    /// 从相同的 `Arc<TcpWriterRegistry>` 克隆前端/shard 任务在热路径上进行咨询。
    /// 保存在这里，以便 [`Self::evict_shard`] 可以将操作员驱动的 shard 驱逐级联到注册表中，释放死 shard 拥有的每个 TCP 连接，
    /// 而无需等待每个连接空闲超时。
    tcp_writers: Arc<TcpWriterRegistry>,
}

/// Snapshot of driver-level counters and bound addresses.
///
/// Returned by [`WebRtcDriverHandle::stats_snapshot`]. Counters are
/// monotonically increasing since the driver started; consumers
/// compute deltas between snapshots if they need rate-of-change
/// metrics. The snapshot is intentionally cheap to take (a few
/// atomic loads) so dashboards can poll it without contention.
///
/// driver 级计数器和绑定地址的快照。
///
/// 由 [`WebRtcDriverHandle::stats_snapshot`] 返回。
/// 自 driver 开始以来，计数器单调递增；
/// 如果消费者需要变化率指标，则可以计算快照之间的增量。
/// 快照的获取成本故意较低（一些原子负载），因此仪表板可以轮询它而不会发生争用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcDriverStats {
    /// UDP listen address.
    ///
    /// UDP 监听地址。
    pub local_udp_addr: SocketAddr,
    /// TCP listen address, if configured and bound.
    ///
    /// TCP 监听地址（如果已配置并绑定）。
    pub local_tcp_addr: Option<SocketAddr>,
    /// Current number of active sessions.
    ///
    /// 当前活动会话数。
    pub session_count: usize,
    /// Cumulative commands accepted since start.
    ///
    /// 自启动以来接受的累积命令。
    pub commands_accepted_total: u64,
    /// Cumulative events emitted since start.
    ///
    /// 自启动以来发出的累积事件。
    pub events_emitted_total: u64,
    /// Cumulative packets that could not be routed to a session.
    ///
    /// 无法路由到会话的累积数据包。
    pub unrouted_packets_total: u64,
    /// Number of session-owner shards. `1` for the current default
    /// driver topology; will grow once the multi-shard front-end is
    /// wired in.
    ///
    /// 会话所有者的数量 shards。
    /// `1` 表示当前默认的 driver 拓扑；
    /// 一旦连接了多 shard 前端，就会增长。
    pub shard_count: usize,
    /// Snapshot of the global route directory.
    ///
    /// 全局路由目录的快照。
    pub route_directory: RouteDirectoryStats,
}

impl WebRtcDriverHandle {
    /// Send a command to the driver, waiting for channel capacity.
    ///
    /// 发送命令到 driver，等待通道容量。
    pub async fn send_command(&self, cmd: WebRtcDriverCommand) {
        if let Err(err) = self.cmd_tx.send(cmd).await {
            warn!("WebRTC driver command channel closed: {err}");
        }
    }
    /// Try to send a command without waiting; fails if the channel is full or closed.
    ///
    /// 尝试不等待地发送命令；
    /// 如果通道已满或关闭，则失败。
    pub async fn try_send_command(&self, cmd: WebRtcDriverCommand) -> Result<(), WebRtcSendError> {
        match self.cmd_tx.try_send(cmd) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(WebRtcSendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(WebRtcSendError::Closed),
        }
    }
    /// Wait for the next driver event and update the emitted counter.
    ///
    /// 等待下一个 driver 事件并更新发出的计数器。
    pub async fn recv_event(&self) -> Option<WebRtcDriverEvent> {
        let evt = self.event_rx.lock().await.recv().await;
        if evt.is_some() {
            self.events_emitted
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        evt
    }
    /// Address the UDP listener is bound to.
    ///
    /// UDP 侦听器绑定的地址。
    pub fn local_udp_addr(&self) -> SocketAddr {
        self.local_udp_addr
    }

    /// Bound TCP address, if [`WebRtcDriverConfig::listen_tcp`] was set
    /// and the listener bound successfully.
    ///
    /// 绑定 TCP 地址，如果设置了 [`WebRtcDriverConfig::listen_tcp`] 并且侦听器绑定成功。
    pub fn local_tcp_addr(&self) -> Option<SocketAddr> {
        self.local_tcp_addr
    }
    /// Current number of registered sessions.
    ///
    /// 当前注册会话数。
    pub fn session_count(&self) -> usize {
        self.session_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of currently registered TCP writer entries. This
    /// counts every accepted RFC-4571 TCP connection that still
    /// has a live writer half in the driver. The value drops to
    /// zero after [`WebRtcDriverHandle::evict_shard`] runs against
    /// every shard, after `tcp_connection_loop` notices the peer
    /// closed, or when the supervisor's auto-evict path cascades
    /// into the registry. Exposed for integration tests and
    /// operator dashboards that want a black-box view of the TCP
    /// writer registry without having to drain shard stats.
    ///
    /// 当前注册的 TCP 作者条目数。
    /// 这计算了每个已接受的 RFC-4571 TCP 连接，该连接在 driver 中仍然有一半的实时写入器。
    /// 在 [`WebRtcDriverHandle::evict_shard`] 针对每个 shard 运行之后、在 `tcp_connection_loop` 注意到对等点关闭之后、或者当主管的自动逐出路径级联到注册表中时
    /// ，该值会降至零。
    /// 暴露于集成测试和操作员仪表板，这些测试和操作员仪表板需要 TCP 编写器注册表的黑盒视图，而不必耗尽 shard 统计信息。
    pub fn tcp_writer_count(&self) -> usize {
        self.tcp_writers.len()
    }

    /// Snapshot driver-level counters and bound addresses.
    ///
    /// All counters are atomic, monotonically increasing since the
    /// driver started. Dashboard-style consumers compute deltas
    /// across snapshots to derive rates. The snapshot is cheap
    /// (a few `Ordering::Relaxed` loads) and does not lock the
    /// command or event channel.
    ///
    /// 快照 driver 级计数器和绑定地址。
    ///
    /// 所有计数器都是原子的，自 driver 开始以来单调递增。
    /// 仪表板式消费者计算快照之间的增量来得出费率。
    /// 快照很便宜（一些 `Ordering::Relaxed` 加载）并且不会锁定命令或事件通道。
    pub fn stats_snapshot(&self) -> WebRtcDriverStats {
        use std::sync::atomic::Ordering;
        WebRtcDriverStats {
            local_udp_addr: self.local_udp_addr,
            local_tcp_addr: self.local_tcp_addr,
            session_count: self.session_count.load(Ordering::Relaxed),
            commands_accepted_total: self.commands_accepted.load(Ordering::Relaxed),
            events_emitted_total: self.events_emitted.load(Ordering::Relaxed),
            unrouted_packets_total: self.unrouted_packets.load(Ordering::Relaxed),
            shard_count: self.shard_count,
            route_directory: self.route_directory.stats_snapshot(),
        }
    }

    /// Effective shard count (matches `WebRtcDriverConfig::driver_shards`
    /// when non-zero, otherwise the runtime auto-detected value).
    ///
    /// 有效 shard 计数（非零时匹配 `WebRtcDriverConfig::driver_shards`，否则为运行时自动检测到的值）。
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// Per-shard observability snapshot. In multi-shard mode each
    /// entry reports the actual route counts the owning shard has
    /// committed via `ShardLoadTable::record_route_counts`. In
    /// single-shard mode (`shard_count == 1`) the global directory's
    /// counts are reported on shard 0 because the legacy
    /// `run_driver_core` path doesn't publish per-shard metrics.
    ///
    /// 每个 shard 可观察性快照。
    /// 在多 shard 模式下，每个条目报告拥有 shard 已通过 `ShardLoadTable::record_route_counts` 提交的实际路由计数。
    /// 在单 shard 模式 (`shard_count == 1`) 中，全局目录的计数在 shard 0 上报告，因为旧版 `run_driver_core` 路径不发布 per-shard 指标。
    pub fn shard_stats(&self) -> Vec<WebRtcShardStats> {
        let dir = self.route_directory.stats_snapshot();
        let loads = self.shard_loads.snapshot();
        let single_shard = self.shard_count == 1;
        loads
            .into_iter()
            .map(|(shard_id, load)| WebRtcShardStats {
                shard_id,
                session_count: load.session_count,
                active_routes: if single_shard && shard_id.as_usize() == 0 {
                    dir.addresses
                } else {
                    load.active_routes
                },
                stale_routes: if single_shard && shard_id.as_usize() == 0 {
                    dir.stale_addresses
                } else {
                    load.stale_routes
                },
            })
            .collect()
    }

    /// Shard selector for the running driver. Useful for upstream
    /// code that wants to pre-compute the owning shard of a session
    /// id (e.g. when dispatching commands to a future per-shard
    /// command channel).
    ///
    /// 正在运行的 driver 的 shard 选择器。
    /// 对于想要预先计算会话 ID 的所属 shard 的上游代码很有用（例如，当将命令分派到未来的每 shard 命令通道时）。
    pub fn shard_selector(&self) -> ShardSelector {
        self.shard_selector.clone()
    }

    /// Per-shard candidate gathering snapshot. Each entry exposes
    /// the latest [`LocalCandidateCounts`] reported by the shard's
    /// event loop, in shard-id order. Entries default to all-zero
    /// for shards that have not yet emitted a
    /// [`WebRtcDriverEvent::LocalCandidateSnapshot`]; the supervisor
    /// auto-evict path resets a shard's slot to zero when the shard
    /// panics. This is a cheap read — one `RwLock::read` plus a
    /// `Vec` copy proportional to `shard_count`.
    ///
    /// 每 shard candidate 收集快照。
    /// 每个条目都按 shard-id 顺序公开 shard 的事件循环报告的最新 [`LocalCandidateCounts`]。
    /// 对于尚未发出 [`WebRtcDriverEvent::LocalCandidateSnapshot`] 的 shards ，条目默认为全零；
    /// 当 shard 发生恐慌时，主管自动驱逐路径将 shard 的插槽重置为零。
    /// 这是一本廉价的读物——一个 `RwLock::read` 加上一个 `Vec` 副本，与 `shard_count` 成比例。
    pub fn shard_candidate_stats(&self) -> Vec<WebRtcShardCandidateStats> {
        self.shard_candidates.snapshot()
    }

    /// Best-effort graceful drain. Returns once
    /// [`Self::session_count`] reaches zero or `timeout` elapses,
    /// whichever happens first. Callers should use this before
    /// cancelling the driver token so in-flight sessions get a chance
    /// to issue their final RTCP and DTLS shutdown packets.
    ///
    /// Implementation note: the driver does not yet expose a list of
    /// active session ids, so we poll `session_count()` instead of
    /// per-session promises. The poll cadence is 25 ms which is fast
    /// enough for operator-driven drains and slow enough to avoid
    /// hot-spinning a CPU.
    ///
    /// Returns `true` when all sessions drained cleanly, `false` when
    /// the timeout fired with sessions still active.
    ///
    /// 尽力优雅地排水。
    /// 一旦 [`Self::session_count`] 达到零或 `timeout` 过去（以先发生者为准），则返回。
    /// 调用者应在取消 driver 令牌之前使用此命令，以便正在进行的会话有机会发出最终的 RTCP 和 DTLS 关闭数据包。
    ///
    /// 实现说明：driver 尚未公开活动会话 ID 列表，因此我们轮询 `session_count()` 而不是每个会话的承诺。
    /// 轮询节奏为 25 毫秒，对于操作员驱动的排水来说足够快，并且足够慢以避免热旋转 CPU。
    ///
    /// 当所有会话都完全耗尽时，返回 `true`；
    /// 当超时触发但会话仍处于活动状态时，返回 `false`。
    pub async fn drain_within(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let poll = std::time::Duration::from_millis(25);
        loop {
            if self.session_count() == 0 {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// Borrow the global route directory. Useful for tests and
    /// for any front-end code that needs to observe directory state
    /// without going through `stats_snapshot`.
    ///
    /// 借用全局路由目录。
    /// 对于测试和任何需要观察目录状态而不通过 `stats_snapshot` 的前端代码很有用。
    pub fn route_directory(&self) -> Arc<RouteDirectory> {
        self.route_directory.clone()
    }

    /// Evict every directory entry owned by `shard` and reset the
    /// shard's load counters. Operators call this **after**
    /// observing a non-graceful
    /// [`WebRtcDriverEvent::ShardStopped`] (panic / unexpected
    /// exit) to release sessions, addresses, ufrags, and stale
    /// routes the dead shard owned. Returns the eviction stats so
    /// dashboards can record the cleanup.
    ///
    /// This does **not** restart the shard task — that is a future
    /// follow-up. Until then, after evicting, operators bring up a
    /// fresh driver instance via `spawn_driver` to recover full
    /// shard count.
    ///
    /// 逐出 `shard` 拥有的每个目录条目并重置 shard 的加载计数器。
    /// 操作员在观察到非优雅的 [`WebRtcDriverEvent::ShardStopped`]（恐慌/意外退出）释放会话、地址、ufrag 和失效的 shard 拥有的过时路由之后
    /// ，将此称为**。
    /// 返回驱逐统计信息，以便仪表板可以记录清理情况。
    ///
    /// 这**不会**重新启动 shard 任务——这是未来的后续任务。
    /// 在此之前，在驱逐后，操作员将通过 `spawn_driver` 调出一个新的 driver 实例，以恢复完整的 shard 计数。
    pub fn evict_shard(&self, shard: ShardId) -> crate::directory::RouteDirectoryEvictionStats {
        let mut evicted = self.route_directory.forget_shard(shard);
        // Cascade into the TCP writer registry so connections owned
        // by the dead shard release their sockets immediately
        // instead of waiting for per-connection idle timeouts.
        let tcp_evicted = self.tcp_writers.forget_shard(shard);
        evicted.tcp_writers = tcp_evicted;
        // Reset the load counters so `shard_stats()` does not
        // report ghost sessions on an evicted shard.
        self.shard_loads.record_route_counts(shard, 0, 0);
        for _ in 0..evicted.sessions {
            self.shard_loads.record_session_removed(shard);
        }
        // session_count is the global aggregate; subtract evicted
        // sessions so the handle's view of liveness reflects
        // reality. Saturating_sub via fetch_sub is racy under load
        // but the operator-driven path is bounded.
        let prev = self
            .session_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let new = prev.saturating_sub(evicted.sessions);
        self.session_count
            .store(new, std::sync::atomic::Ordering::Relaxed);
        evicted
    }
}
/// Reasons a command could not be submitted to the driver.
///
/// 命令无法提交到 driver 的原因。
#[derive(Debug, thiserror::Error)]
pub enum WebRtcSendError {
    /// The driver command channel is at capacity.
    ///
    /// driver 命令通道已满。
    #[error("driver command queue is full")]
    QueueFull,
    /// The driver task has exited and the channel is closed.
    ///
    /// driver 任务已退出并且通道已关闭。
    #[error("driver command channel closed")]
    Closed,
}

/// Spawn the driver and return a handle.
///
/// Returns `Err` if the UDP socket cannot be bound. Other failures during
/// the lifetime of the driver are surfaced as
/// [`WebRtcDriverEvent::Diagnostic`] records on the event channel.
///
/// 生成 driver 并返回一个句柄。
///
/// 如果无法绑定 UDP 套接字，则返回 `Err`。
/// driver 生命周期内的其他故障在事件通道上显示为 [`WebRtcDriverEvent::Diagnostic`] 记录。
pub async fn spawn_driver(
    config: WebRtcDriverConfig,
    cancel: CancellationToken,
) -> std::io::Result<Arc<WebRtcDriverHandle>> {
    // Validate config before attempting any I/O.
    config
        .validate()
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))?;

    let socket = bind_udp_socket(&config).await?;
    let local_udp_addr = socket.local_addr()?;
    info!("WebRTC UDP driver listening on {local_udp_addr}");
    let socket = Arc::new(socket);

    let (cmd_tx, cmd_rx) = mpsc::channel(config.command_queue_capacity);
    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity);
    let (packet_tx, packet_rx) = mpsc::channel::<NetDatagram>(config.write_queue_capacity);

    let session_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // TCP listener is optional. We bind eagerly in spawn_driver so the
    // caller learns immediately if the configured port is unavailable
    // — the same contract as for UDP.
    let (tcp_listener, local_tcp_addr) = match config.listen_tcp {
        Some(addr) => {
            let listener = TcpListener::bind(addr).await?;
            let bound = listener.local_addr()?;
            info!("WebRTC TCP driver listening on {bound}");
            (Some(listener), Some(bound))
        }
        None => (None, None),
    };

    let tcp_writers: Arc<TcpWriterRegistry> = Arc::new(TcpWriterRegistry::default());

    let commands_accepted = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let events_emitted = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let unrouted_packets = Arc::new(std::sync::atomic::AtomicU64::new(0));

    let shard_count = config.effective_shard_count();
    let route_directory = Arc::new(RouteDirectory::new(RouteDirectoryConfig {
        address_capacity: config.route_directory_capacity,
        stale_capacity: config.route_directory_stale_capacity,
        stale_ttl: Duration::from_millis(config.migration_route_ttl_ms),
    }));
    let shard_selector = ShardSelector::new(shard_count);
    let shard_loads = Arc::new(ShardLoadTable::new(shard_count));
    let shard_candidates = Arc::new(ShardCandidateTable::new(shard_count));

    let handle = Arc::new(WebRtcDriverHandle {
        cmd_tx,
        event_rx: tokio::sync::Mutex::new(event_rx),
        local_udp_addr,
        local_tcp_addr,
        session_count: session_count.clone(),
        commands_accepted: commands_accepted.clone(),
        events_emitted: events_emitted.clone(),
        unrouted_packets: unrouted_packets.clone(),
        shard_count,
        route_directory: route_directory.clone(),
        shard_loads: shard_loads.clone(),
        shard_candidates: shard_candidates.clone(),
        shard_selector: shard_selector.clone(),
        tcp_writers: tcp_writers.clone(),
    });

    {
        let socket = socket.clone();
        let cancel = cancel.clone();
        let read_buf = config.read_buffer_size;
        let packet_tx = packet_tx.clone();
        tokio::spawn(async move {
            udp_recv_loop(socket, packet_tx, cancel, read_buf).await;
        });
    }

    if let Some(listener) = tcp_listener {
        let cancel = cancel.clone();
        let event_tx = event_tx.clone();
        let packet_tx = packet_tx.clone();
        let tcp_writers = tcp_writers.clone();
        let chunk = config.tcp_read_chunk_size;
        let max_frame = config.tcp_frame_max_bytes;
        let idle_timeout = if config.tcp_idle_timeout_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(config.tcp_idle_timeout_ms))
        };
        tokio::spawn(async move {
            tcp_accept_loop(
                listener,
                packet_tx,
                event_tx,
                tcp_writers,
                cancel,
                chunk,
                max_frame,
                idle_timeout,
                shard_count,
            )
            .await;
        });
    }

    {
        let cancel = cancel.clone();
        let event_tx = event_tx.clone();
        let session_count = session_count.clone();
        let socket = socket.clone();
        let tcp_writers = tcp_writers.clone();
        let commands_accepted = commands_accepted.clone();
        let unrouted_packets = unrouted_packets.clone();
        let route_directory = route_directory.clone();
        let shard_loads = shard_loads.clone();
        let shard_candidates = shard_candidates.clone();
        let shard_selector = shard_selector.clone();
        if shard_count > 1 {
            // Multi-shard topology: spawn N shard loops + a front-end
            // task that routes commands and packets.
            let shards = crate::io_front::spawn_shards(
                &config,
                shard_count,
                socket.clone(),
                tcp_writers.clone(),
                event_tx.clone(),
                cancel.clone(),
                session_count.clone(),
                unrouted_packets.clone(),
                route_directory.clone(),
                shard_loads.clone(),
                shard_candidates.clone(),
            );
            let cfg = crate::io_front::IoFrontConfig {
                shards,
                directory: route_directory,
                selector: shard_selector,
                shard_loads,
                commands_accepted,
                unrouted_packets,
                event_tx,
                cmd_rx,
                packet_rx,
            };
            tokio::spawn(async move {
                crate::io_front::run_io_front(cfg, cancel).await;
            });
        } else {
            tokio::spawn(async move {
                run_driver_core(
                    config,
                    socket,
                    tcp_writers,
                    cmd_rx,
                    packet_rx,
                    event_tx,
                    cancel,
                    session_count,
                    commands_accepted,
                    unrouted_packets,
                    route_directory,
                    shard_loads,
                    shard_candidates,
                    shard_selector,
                )
                .await;
            });
        }
    }

    Ok(handle)
}
/// UDP payload received from the listener.
///
/// 从侦听器接收的 UDP 有效负载。
#[derive(Debug, Clone)]
pub(crate) struct UdpDatagram {
    pub(crate) source: SocketAddr,
    pub(crate) data: Bytes,
    pub(crate) received_at: Instant,
}

/// Payload arriving from either the UDP listener or a TCP connection.
///
/// 来自 UDP 侦听器或 TCP 连接的有效负载。
#[derive(Debug, Clone)]
pub(crate) enum NetDatagram {
    Udp(UdpDatagram),
    Tcp(TcpDatagram),
}
/// TCP payload received after RFC 4571 deframing.
///
/// RFC 4571 解帧后收到 TCP 有效负载。
#[derive(Debug, Clone)]
pub(crate) struct TcpDatagram {
    pub(crate) source: SocketAddr,
    pub(crate) data: Bytes,
    pub(crate) received_at: Instant,
}

impl NetDatagram {
    pub(crate) fn source(&self) -> SocketAddr {
        match self {
            Self::Udp(d) => d.source,
            Self::Tcp(d) => d.source,
        }
    }
    pub(crate) fn received_at(&self) -> Instant {
        match self {
            Self::Udp(d) => d.received_at,
            Self::Tcp(d) => d.received_at,
        }
    }
    pub(crate) fn into_data(self) -> Bytes {
        match self {
            Self::Udp(d) => d.data,
            Self::Tcp(d) => d.data,
        }
    }
    pub(crate) fn data(&self) -> &Bytes {
        match self {
            Self::Udp(d) => &d.data,
            Self::Tcp(d) => &d.data,
        }
    }
    /// `true` when this datagram was received over a TCP connection
    /// (RFC 4571 framed). Used by inbound datagram handlers to
    /// decide whether the source `SocketAddr` corresponds to an
    /// entry in [`TcpWriterRegistry`] and therefore needs an
    /// owner-shard reassignment after STUN binding-request parsing.
    ///
    /// `true` 当通过 TCP 连接接收到此数据报时（RFC 4571 帧）。
    /// 由入站数据报处理程序用来确定源 `SocketAddr` 是否对应于 [`TcpWriterRegistry`] 中的条目，因此在 STUN 绑定请求解析后需要重新分配所有者 shard 。
    pub(crate) fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }
}

/// Registry of active TCP connections keyed by remote SocketAddr.
///
/// Each connection task owns the read half. The write half is wrapped
/// in a `tokio::sync::Mutex` so the driver core loop can frame and
/// send outbound packets back to the same peer without contending the
/// per-connection task. Closing the entry signals the connection has
/// gone away; subsequent send attempts by the driver core fall back
/// to UDP if the session has migrated.
///
/// The registry is owned by the I/O front-end so multiple shards can
/// share a single TCP listener / writer pool without each shard
/// duplicating bookkeeping.
///
/// A parallel `SocketAddr → ShardId` index records which shard
/// currently "owns" each connection. The `get` signature is kept
/// writer-only because shard owners are bookkeeping for
/// `evict_shard` / supervisor auto-evict paths and not consulted on
/// the per-packet send path. Both maps are mutated under the same
/// lock so they cannot drift.
///
/// 由远程 SocketAddr 键入的活动 TCP 连接的注册表。
///
/// 每个连接任务都拥有读取的一半。
/// 写入部分被包装在 `tokio::sync::Mutex` 中，因此 driver 核心循环可以构建出站数据包并将其发送回同一对等点，而无需竞争每个连接的任务。
/// 关闭入口表示连接已经消失；
/// 如果会话已迁移，driver 核心的后续发送尝试将回退到 UDP。
///
/// 注册表由 I/O 前端拥有，因此多个 shards 可以共享一个 TCP 侦听器/编写器池，而无需每个 shard 重复簿记。
///
/// 并行 `SocketAddr → ShardId` 索引记录哪个 shard 当前“拥有”每个连接。
/// `get` 签名仅保留为写入者，因为 shard 所有者正在为 `evict_shard` / 主管自动逐出路径进行簿记，并且不会在每个数据包发送路径上进行咨询。
/// 两张地图都在同一个锁下发生变异，因此它们无法漂移。
#[derive(Default)]
pub(crate) struct TcpWriterRegistry {
    inner: parking_lot::Mutex<TcpWriterRegistryInner>,
}

#[derive(Default)]
struct TcpWriterRegistryInner {
    writers: std::collections::HashMap<
        SocketAddr,
        Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    >,
    owners: std::collections::HashMap<SocketAddr, ShardId>,
}

impl TcpWriterRegistry {
    fn insert(
        &self,
        addr: SocketAddr,
        shard: ShardId,
        writer: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    ) {
        let mut guard = self.inner.lock();
        guard.writers.insert(addr, writer);
        guard.owners.insert(addr, shard);
    }

    pub(crate) fn remove(&self, addr: &SocketAddr) {
        let mut guard = self.inner.lock();
        guard.writers.remove(addr);
        guard.owners.remove(addr);
    }

    pub(crate) fn get(
        &self,
        addr: &SocketAddr,
    ) -> Option<Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>> {
        self.inner.lock().writers.get(addr).cloned()
    }

    /// Drop every writer owned by `shard`. Returns the number of
    /// writer entries removed; the parallel owner index is cleaned
    /// in the same critical section so observers cannot see a half
    /// state.
    ///
    /// 删除 `shard` 拥有的所有 writer。
    /// 返回删除的写入器条目数；
    /// 并行所有者索引在同一关键部分中被清理，因此观察者看不到半状态。
    pub(crate) fn forget_shard(&self, shard: ShardId) -> usize {
        let mut guard = self.inner.lock();
        let to_remove: Vec<SocketAddr> = guard
            .owners
            .iter()
            .filter_map(|(addr, owner)| (*owner == shard).then_some(*addr))
            .collect();
        for addr in &to_remove {
            guard.writers.remove(addr);
            guard.owners.remove(addr);
        }
        to_remove.len()
    }

    /// Number of currently active writers. Useful for diagnostics
    /// and for asserting that `forget_shard` / `evict_shard` paths
    /// fully drained their owned connections.
    ///
    /// 当前活跃作家的数量。
    /// 对于诊断和断言 `forget_shard` / `evict_shard` 路径完全耗尽其拥有的连接很有用。
    pub(crate) fn len(&self) -> usize {
        self.inner.lock().writers.len()
    }

    /// Reassign the owner of an existing writer entry. Used when a
    /// freshly accepted TCP connection's hash-based provisional
    /// owner is known to differ from the actual session owner shard
    /// (e.g. after STUN binding-request parsing reveals the ufrag).
    ///
    /// Returns `true` when the entry was found and updated. Returns
    /// `false` when there is no writer for `addr`, in which case the
    /// caller does not need to take action — the writer was likely
    /// already removed by `remove` / `forget_shard`.
    ///
    /// 重新分配现有写入器条目的所有者。
    /// 当已知新接受的 TCP 连接的基于哈希的临时所有者与实际会话所有者 shard 不同时使用（例如，在 STUN 绑定请求解析显示 ufrag 之后）。
    ///
    /// 找到并更新条目后返回 `true`。
    /// 当 `addr` 没有写入器时，返回 `false`，在这种情况下，调用者不需要采取操作 - 写入器可能已被 `remove` / `forget_shard` 删除。
    pub(crate) fn reassign_shard(&self, addr: &SocketAddr, shard: ShardId) -> bool {
        let mut guard = self.inner.lock();
        if guard.writers.contains_key(addr) {
            guard.owners.insert(*addr, shard);
            true
        } else {
            false
        }
    }
}

/// Pick the owner shard for a freshly accepted TCP connection.
///
/// Uses a splitmix64-style fold over the `(IpAddr, u16 port)` byte
/// representation of `addr`. Independent from the session-id-based
/// [`ShardSelector`] on purpose: at the point a TCP connection is
/// accepted we don't yet know which session it belongs to (STUN
/// hasn't been parsed). Subtask 1.3 will reassign the owner once
/// the ufrag is known.
///
/// `shard_count <= 1` collapses to [`ShardId::new(0)`] so the
/// single-shard topology stays a no-op.
///
/// 为新接受的 TCP 连接选择所有者 shard。
///
/// 对 `addr` 的 `(IpAddr, u16 port)` 字节表示形式使用 splitmix64 样式折叠。
/// 故意独立于基于会话 ID 的 [`ShardSelector`]：在接受 TCP 连接时，我们还不知道它属于哪个会话（STUN 尚未解析）。
/// 一旦 ufrag 已知，子任务 1.3 将重新分配所有者。
///
/// `shard_count <= 1` 折叠为 [`ShardId::new(0)`]，因此单 shard 拓扑保持无操作状态。
pub(crate) fn shard_for_remote_addr(addr: SocketAddr, shard_count: usize) -> ShardId {
    if shard_count <= 1 {
        return ShardId::new(0);
    }
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    let port = addr.port() as u64;
    h = h.wrapping_add(port).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    match addr.ip() {
        std::net::IpAddr::V4(v4) => {
            for &b in v4.octets().iter() {
                h ^= b as u64;
                h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
            }
        }
        std::net::IpAddr::V6(v6) => {
            for &b in v6.octets().iter() {
                h ^= b as u64;
                h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
            }
        }
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    let idx = (h % shard_count as u64) as usize;
    ShardId::new(idx)
}

/// Bind the UDP listener socket. When a port range is configured, the
/// driver tries each port in `[min, max]` sequentially until one
/// succeeds. On exhaustion, returns the last bind error. When no range
/// is configured, falls back to the address in `config.listen_udp`
/// (which may use port 0 for OS-assigned ephemeral ports).
///
/// 绑定 UDP 侦听器套接字。
/// 配置端口范围后，driver 会按顺序尝试 `[min, max]` 中的每个端口，直到成功为止。
/// 耗尽时，返回最后一个绑定错误。
/// 如果未配置范围，则回退到 `config.listen_udp` 中的地址（可能使用端口 0 作为操作系统分配的临时端口）。
async fn bind_udp_socket(config: &WebRtcDriverConfig) -> std::io::Result<UdpSocket> {
    match &config.udp_port_range {
        Some(range) => {
            let ip = config.listen_udp.ip();
            let mut last_err = None;
            for port in range.min..=range.max {
                let addr = SocketAddr::new(ip, port);
                match UdpSocket::bind(addr).await {
                    Ok(socket) => return Ok(socket),
                    Err(err) => {
                        debug!("WebRTC UDP bind {addr} failed: {err}, trying next port");
                        last_err = Some(err);
                    }
                }
            }
            Err(last_err.unwrap_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!(
                        "no available UDP port in range {}..={}",
                        range.min, range.max
                    ),
                )
            }))
        }
        None => UdpSocket::bind(config.listen_udp).await,
    }
}
/// Generate host candidate lines for the bound UDP address, public IPs, and hostname.
///
/// 为绑定的 UDP 地址、公共 IP 和主机名生成主机 candidate 行。
fn build_local_candidate_sdps(
    local_udp_addr: Option<SocketAddr>,
    public_ips: &[IpAddr],
    candidate_hostname: Option<&str>,
) -> Vec<String> {
    let Some(bound_addr) = local_udp_addr else {
        return Vec::new();
    };
    let port = bound_addr.port();
    if port == 0 {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for ip in public_ips {
        push_host_candidate(&mut candidates, &mut seen, &ip.to_string(), port);
    }

    if public_ips.is_empty() && !bound_addr.ip().is_unspecified() {
        push_host_candidate(
            &mut candidates,
            &mut seen,
            &bound_addr.ip().to_string(),
            port,
        );
    }

    if let Some(hostname) = candidate_hostname {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            push_host_candidate(&mut candidates, &mut seen, hostname, port);
        }
    }

    candidates
}
/// Append a unique host candidate string, deduplicated by address:port.
///
/// 附加一个唯一的主机 candidate 字符串，按地址：端口进行重复数据删除。
fn push_host_candidate(
    candidates: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
    address: &str,
    port: u16,
) {
    let key = format!("{address}:{port}");
    if !seen.insert(key) {
        return;
    }
    let foundation = candidates.len() + 1;
    candidates.push(format!(
        "candidate:{foundation} 1 UDP 2130706431 {address} {port} typ host"
    ));
}
/// Loop reading UDP datagrams and forwarding them to the packet channel.
///
/// 循环读取 UDP 数据报并将其转发到数据包通道。
async fn udp_recv_loop(
    socket: Arc<UdpSocket>,
    packet_tx: mpsc::Sender<NetDatagram>,
    cancel: CancellationToken,
    read_buf: usize,
) {
    let mut buf = vec![0u8; read_buf];
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("WebRTC UDP recv loop cancelled");
                break;
            }
            res = socket.recv_from(&mut buf) => match res {
                Ok((len, source)) => {
                    let datagram = UdpDatagram {
                        source,
                        data: Bytes::copy_from_slice(&buf[..len]),
                        received_at: Instant::now(),
                    };
                    if packet_tx.send(NetDatagram::Udp(datagram)).await.is_err() {
                        debug!("WebRTC packet receiver dropped, exiting recv loop");
                        break;
                    }
                }
                Err(err) => {
                    warn!("WebRTC UDP recv error: {err}");
                    // Brief sleep so we do not spin on persistent errors.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}
/// Accept incoming TCP connections and spawn per-connection framing loops.
///
/// 接受传入的 TCP 连接并生成每个连接的帧循环。
#[allow(clippy::too_many_arguments)]
async fn tcp_accept_loop(
    listener: TcpListener,
    packet_tx: mpsc::Sender<NetDatagram>,
    event_tx: mpsc::Sender<WebRtcDriverEvent>,
    writers: Arc<TcpWriterRegistry>,
    cancel: CancellationToken,
    chunk_size: usize,
    max_frame: usize,
    idle_timeout: Option<Duration>,
    shard_count: usize,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("WebRTC TCP accept loop cancelled");
                break;
            }
            res = listener.accept() => match res {
                Ok((stream, remote_addr)) => {
                    let _ = stream.set_nodelay(true);
                    // Enable OS-level TCP keepalive to detect dead
                    // connections behind NAT. This complements the
                    // application-level idle timeout by sending probes
                    // at the TCP layer.
                    #[cfg(unix)]
                    {
                        use std::os::unix::io::AsRawFd;
                        let fd = stream.as_raw_fd();
                        unsafe {
                            let enable: libc::c_int = 1;
                            libc::setsockopt(
                                fd,
                                libc::SOL_SOCKET,
                                libc::SO_KEEPALIVE,
                                &enable as *const _ as *const libc::c_void,
                                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                            );
                            // Start probes after 30s idle
                            let idle: libc::c_int = 30;
                            libc::setsockopt(
                                fd,
                                libc::IPPROTO_TCP,
                                libc::TCP_KEEPIDLE,
                                &idle as *const _ as *const libc::c_void,
                                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                            );
                            // Probe interval 10s
                            let interval: libc::c_int = 10;
                            libc::setsockopt(
                                fd,
                                libc::IPPROTO_TCP,
                                libc::TCP_KEEPINTVL,
                                &interval as *const _ as *const libc::c_void,
                                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                            );
                            // Give up after 3 failed probes
                            let count: libc::c_int = 3;
                            libc::setsockopt(
                                fd,
                                libc::IPPROTO_TCP,
                                libc::TCP_KEEPCNT,
                                &count as *const _ as *const libc::c_void,
                                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                            );
                        }
                    }
                    let _ = event_tx
                        .send(WebRtcDriverEvent::TcpAccepted { remote_addr })
                        .await;
                    let (read_half, write_half) = stream.into_split();
                    let writer = Arc::new(tokio::sync::Mutex::new(write_half));
                    // Pick an owner shard locally by hashing the
                    // remote addr; this stays independent of the
                    // session-id-based `ShardSelector` so we don't
                    // pollute that path. Once STUN parsing reveals
                    // the actual ufrag in subtask 1.3 the owner
                    // gets reassigned to the session's shard.
                    let owner_shard = shard_for_remote_addr(remote_addr, shard_count);
                    writers.insert(remote_addr, owner_shard, writer);
                    let packet_tx = packet_tx.clone();
                    let event_tx = event_tx.clone();
                    let writers = writers.clone();
                    let cancel = cancel.clone();
                    tokio::spawn(async move {
                        tcp_connection_loop(
                            read_half,
                            remote_addr,
                            packet_tx,
                            event_tx,
                            writers,
                            cancel,
                            chunk_size,
                            max_frame,
                            idle_timeout,
                            owner_shard,
                        )
                        .await;
                    });
                }
                Err(err) => {
                    warn!("WebRTC TCP accept error: {err}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}
/// Read RFC 4571 frames from one TCP peer and forward complete packets.
///
/// 从一个 TCP 对等方读取 RFC 4571 帧并转发完整的数据包。
#[allow(clippy::too_many_arguments)]
async fn tcp_connection_loop(
    mut read_half: tokio::net::tcp::OwnedReadHalf,
    remote_addr: SocketAddr,
    packet_tx: mpsc::Sender<NetDatagram>,
    event_tx: mpsc::Sender<WebRtcDriverEvent>,
    writers: Arc<TcpWriterRegistry>,
    cancel: CancellationToken,
    chunk_size: usize,
    max_frame: usize,
    idle_timeout: Option<Duration>,
    owner_shard: ShardId,
) {
    tracing::debug!(
        remote_addr = %remote_addr,
        owner_shard = owner_shard.as_usize(),
        "tcp connection accepted"
    );
    let mut decoder = Tcp4571Decoder::with_max_frame(max_frame);
    let mut buf = vec![0u8; chunk_size];
    let close_reason = loop {
        // The idle-timeout future drives close-on-silence semantics.
        // We rebuild it on every read; `tokio::time::sleep` is a
        // small allocation and recreating it on each iteration is
        // simpler than tracking a deadline by hand. When the timeout
        // is `None` we fall through to a future that never fires.
        let idle_sleep = async {
            match idle_timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(idle_sleep);

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                break WebRtcTcpCloseReason::Shutdown;
            }
            _ = &mut idle_sleep => {
                break WebRtcTcpCloseReason::IdleTimeout;
            }
            res = read_half.read(&mut buf) => match res {
                Ok(0) => break WebRtcTcpCloseReason::PeerEof,
                Ok(n) => {
                    decoder.extend(&buf[..n]);
                    loop {
                        match decoder.next_frame() {
                            Ok(Some(frame)) => {
                                let datagram = TcpDatagram {
                                    source: remote_addr,
                                    data: frame,
                                    received_at: Instant::now(),
                                };
                                if packet_tx.send(NetDatagram::Tcp(datagram)).await.is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(err) => {
                                let reason = WebRtcTcpCloseReason::FramingError {
                                    message: err.to_string(),
                                };
                                writers.remove(&remote_addr);
                                let _ = event_tx
                                    .send(WebRtcDriverEvent::TcpClosed {
                                        remote_addr,
                                        reason: reason.clone(),
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
                }
                Err(err) => {
                    break WebRtcTcpCloseReason::Io {
                        message: err.to_string(),
                    };
                }
            }
        }
    };
    writers.remove(&remote_addr);
    let _ = event_tx
        .send(WebRtcDriverEvent::TcpClosed {
            remote_addr,
            reason: close_reason,
        })
        .await;
}
/// Single-shard driver core: owns the WebRtcCore, routes commands, datagrams, and timers.
///
/// Single-shard driver 核心：拥有 WebRtcCore、路由命令、数据报和计时器。
#[allow(clippy::too_many_arguments)]
async fn run_driver_core(
    config: WebRtcDriverConfig,
    socket: Arc<UdpSocket>,
    tcp_writers: Arc<TcpWriterRegistry>,
    mut cmd_rx: mpsc::Receiver<WebRtcDriverCommand>,
    mut packet_rx: mpsc::Receiver<NetDatagram>,
    event_tx: mpsc::Sender<WebRtcDriverEvent>,
    cancel: CancellationToken,
    session_count: Arc<std::sync::atomic::AtomicUsize>,
    commands_accepted: Arc<std::sync::atomic::AtomicU64>,
    unrouted_packets: Arc<std::sync::atomic::AtomicU64>,
    route_directory: Arc<RouteDirectory>,
    shard_loads: Arc<ShardLoadTable>,
    shard_candidates: Arc<ShardCandidateTable>,
    shard_selector: ShardSelector,
) {
    let start_instant = Instant::now();
    let core = Arc::new(Mutex::new(WebRtcCore::new(
        config.core.clone(),
        start_instant,
    )));
    let local_candidate_sdps = build_local_candidate_sdps(
        socket.local_addr().ok(),
        &config.public_ips,
        config.candidate_hostname.as_deref(),
    );
    let mut routes = RouteTable::new(
        config.max_sessions,
        Duration::from_millis(config.migration_route_ttl_ms),
    );
    let mut session_remote: std::collections::HashMap<WebRtcSessionId, SocketAddr> =
        std::collections::HashMap::new();

    // Per-session handshake watchdog: every AcceptOffer / CreateOffer
    // adds an entry whose deadline equals "now + handshake_timeout_ms".
    // The entry is cleared as soon as the session reports
    // `Lifecycle::Connected` (ICE+DTLS+SRTP up). The periodic tick
    // walks the map and force-closes any session that misses the
    // deadline, surfacing `WebRtcCloseReason::HandshakeTimeout`. A
    // `handshake_timeout_ms == 0` config disables the watchdog.
    let handshake_timeout = if config.handshake_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(config.handshake_timeout_ms))
    };
    let mut pending_handshakes: std::collections::HashMap<WebRtcSessionId, Instant> =
        std::collections::HashMap::new();
    let mut session_candidate_policies: std::collections::HashMap<
        WebRtcSessionId,
        CandidateTransportPolicy,
    > = std::collections::HashMap::new();
    // We sweep the watchdog at most once per second. The tick wheel
    // is already running for `str0m` timeouts so we piggy-back on it
    // when the next timer happens to fire near our sweep cadence.
    let watchdog_interval = Duration::from_secs(1);
    let mut next_watchdog_sweep = start_instant + watchdog_interval;
    // Backpressure monitoring runs on the same cadence as the
    // watchdog so we don't spam the event channel when it's full.
    let backpressure_interval = Duration::from_secs(1);
    let mut next_backpressure_check = start_instant + backpressure_interval;

    // Per-session deadline. When `None` we use a far-future sleep so the
    // tokio select never wakes spuriously.
    let mut next_deadline: Option<Instant> = None;

    let mut output_buf = Vec::with_capacity(64);

    loop {
        let sleep = match next_deadline {
            Some(deadline) => {
                // Wake up at whichever comes first: a core timer or
                // the next watchdog sweep. Skipping the watchdog
                // sweep because of a long core deadline would let
                // expired handshakes linger.
                let effective = if handshake_timeout.is_some() && next_watchdog_sweep < deadline {
                    next_watchdog_sweep
                } else {
                    deadline
                };
                sleep_until(tokio::time::Instant::from_std(effective))
            }
            None => {
                // No core timer pending: park until the next
                // watchdog sweep when the watchdog is enabled, or
                // for a long time when it is not.
                let target = if handshake_timeout.is_some() {
                    next_watchdog_sweep
                } else {
                    Instant::now() + Duration::from_secs(60 * 60)
                };
                sleep_until(tokio::time::Instant::from_std(target))
            }
        };
        tokio::pin!(sleep);

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("WebRTC driver core loop cancelled");
                break;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        commands_accepted
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let new_session = handle_command(
                            &core,
                            cmd,
                            now_micros(start_instant),
                            &local_candidate_sdps,
                            &event_tx,
                            &session_count,
                        ).await;
                        if let Some((session_id, candidate_policy)) = new_session {
                            // Pick the owner shard. Today the
                            // protocol state still lives on a single
                            // core; the directory + load table track
                            // the selection so the future
                            // multi-shard front-end can drop in
                            // without changing the public API.
                            let shard = shard_selector.pick(session_id, &shard_loads);
                            route_directory.register_session(session_id, shard);
                            shard_loads.record_session_added(shard);
                            session_candidate_policies.insert(session_id, candidate_policy);
                            if let Some(timeout) = handshake_timeout {
                                // Mark the deadline relative to the
                                // moment the offer/answer command was
                                // accepted. We prefer the receive-arm
                                // clock here over a per-iteration
                                // `Instant::now()` so the watchdog's
                                // notion of "deadline" stays close to
                                // when the SDP exchange actually
                                // started in the driver.
                                pending_handshakes
                                    .insert(session_id, Instant::now() + timeout);
                            }
                        }
                    }
                    None => {
                        info!("WebRTC driver command channel closed; exiting");
                        break;
                    }
                }
            }
            datagram = packet_rx.recv() => {
                match datagram {
                    Some(datagram) => {
                        let is_tcp = datagram.is_tcp();
                        handle_datagram(
                            &core,
                            &mut routes,
                            &mut session_remote,
                            &socket,
                            &tcp_writers,
                            datagram,
                            is_tcp,
                            start_instant,
                            &event_tx,
                            &unrouted_packets,
                            &route_directory,
                        ).await;
                    }
                    None => {
                        info!("WebRTC driver packet channel closed; exiting");
                        break;
                    }
                }
            }
            _ = &mut sleep => {
                let now = Instant::now();
                let now_us = (now.saturating_duration_since(start_instant)).as_micros() as u64;
                // `WebRtcCoreInput::Tick` already iterates every session
                // inside the core; we only need to invoke it once per
                // wake-up. Dispatching it per session would do O(n^2)
                // work as the session count grows.
                let mut core_guard = core.lock();
                if let Err(err) =
                    core_guard.handle_input(WebRtcCoreInput::Tick { now_micros: now_us })
                {
                    drop(core_guard);
                    let _ = event_tx
                        .try_send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                            session_id: None,
                            kind: WebRtcDriverDiagnosticKind::Lifecycle,
                            message: format!("tick failed: {err}"),
                        }));
                }
            }
        }

        // Drain pending outputs after every iteration.
        next_deadline = drain_core_outputs(
            &core,
            &mut routes,
            &mut session_remote,
            &socket,
            &tcp_writers,
            &mut output_buf,
            start_instant,
            &event_tx,
            &session_count,
            &mut pending_handshakes,
            &mut session_candidate_policies,
            &route_directory,
            &shard_loads,
            &shard_candidates,
        )
        .await;

        // Watchdog sweep: close any session whose handshake deadline
        // has passed. Runs at most once per `watchdog_interval` to
        // avoid scanning the map on every wake-up. We use the local
        // `Instant::now()` rather than a `Tick` time because the
        // watchdog's contract is "wall-clock elapsed since command
        // accepted" not "core ticks observed".
        if handshake_timeout.is_some() {
            let now = Instant::now();
            if now >= next_watchdog_sweep && !pending_handshakes.is_empty() {
                let expired: Vec<WebRtcSessionId> = pending_handshakes
                    .iter()
                    .filter(|(_, deadline)| now >= **deadline)
                    .map(|(id, _)| *id)
                    .collect();
                for session_id in expired {
                    pending_handshakes.remove(&session_id);
                    {
                        let mut core_guard = core.lock();
                        let _ = core_guard.handle_input(WebRtcCoreInput::Command(
                            WebRtcCoreCommand::Close {
                                session_id,
                                reason: WebRtcCloseReason::HandshakeTimeout,
                            },
                        ));
                    }
                    let _ = event_tx
                        .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                            session_id: Some(session_id),
                            kind: WebRtcDriverDiagnosticKind::Lifecycle,
                            message: format!("session {session_id} handshake timed out, closing"),
                        }))
                        .await;
                }
                next_watchdog_sweep = now + watchdog_interval;
            }
        }

        // Route table compaction: expire stale migration routes and
        // emit RouteExpired diagnostics so operators can observe
        // migration lifecycle completion.
        {
            let now = Instant::now();
            let expired_routes = routes.compact_expired(now);
            for (addr, session_id) in expired_routes {
                let _ = event_tx.try_send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcDriverDiagnosticKind::RouteExpired,
                    message: format!(
                        "stale route {addr} for session {session_id} expired after migration TTL"
                    ),
                }));
            }
            // Mirror compaction on the global route directory so the
            // multi-shard front-end's stats stay consistent. The
            // directory can keep its own stale entries when a future
            // shard owner relinquishes a peer; we drop them here so
            // dashboards don't see ghost addresses.
            let _ = route_directory.compact_expired(now);
        }

        // Backpressure monitoring: emit a Backpressure event when the
        // event channel capacity drops below 25% of total. This lets
        // module / operations see when the driver is filling up before
        // events start being dropped. Throttled to once per second so
        // we don't spam the event channel that we're already worried
        // about being full.
        {
            let now = Instant::now();
            if now >= next_backpressure_check {
                let max_cap = event_tx.max_capacity();
                let remaining = event_tx.capacity();
                if remaining < max_cap.saturating_div(4) {
                    let pending = max_cap.saturating_sub(remaining);
                    let _ = event_tx.try_send(WebRtcDriverEvent::Backpressure {
                        queue: "events".to_string(),
                        pending,
                    });
                }
                next_backpressure_check = now + backpressure_interval;
            }
        }
    }
    info!("WebRTC driver core loop terminated");
}

/// Extract the `a=ice-ufrag:` value from a local SDP offer/answer so
/// the multi-shard front-end can route initial STUN binding requests
/// to the shard that generated this SDP. Returns `None` if the SDP
/// does not contain the attribute (which would indicate a malformed
/// SDP — the caller treats it as a non-fatal warning).
///
/// 从本地 SDP offer/answer 中提取 `a=ice-ufrag:` 值，以便 multi-shard 前端可以将初始 STUN 绑定请求路由到生成此 SDP 的 shard。
/// 如果 SDP 不包含该属性，则返回 `None` （这将指示格式错误的 SDP — 调用者将其视为非致命警告）。
fn extract_local_ufrag_from_sdp(sdp: &str) -> Option<String> {
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=ice-ufrag:") {
            let value = rest.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Per-shard event loop used in the multi-shard topology.
///
/// Same protocol-state plumbing as [`run_driver_core`], but:
/// * commands and packets arrive on per-shard mpsc channels (the I/O
///   front-end has already routed them),
/// * sessions are pinned to `shard_id` (no recomputation via the
///   selector — under the least-loaded strategy that would drift),
/// * once a session produces its `LocalDescription`, the shard
///   registers the local ufrag with the directory so the front-end
///   can route initial STUN binding requests by ufrag.
///
/// 多 shard 拓扑中使用的每 shard 事件循环。
///
/// 与 [`run_driver_core`] 相同的协议状态管道，但是：
/// * 命令和数据包到达 per-shard mpsc 通道（I/O 前端已经路由它们），
/// * 会话被固定到 `shard_id` （没有通过选择器重新计算 - 在会漂移的最小加载策略下），
/// * 一旦会话生成其 `LocalDescription`，shard 就会向目录注册本地 ufrag，以便前端可以通过 ufrag 路由初始 STUN 绑定请求。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_shard_loop(
    shard_id: ShardId,
    config: WebRtcDriverConfig,
    socket: Arc<UdpSocket>,
    tcp_writers: Arc<TcpWriterRegistry>,
    mut cmd_rx: mpsc::Receiver<ShardCommand>,
    mut packet_rx: mpsc::Receiver<NetDatagram>,
    event_tx: mpsc::Sender<WebRtcDriverEvent>,
    cancel: CancellationToken,
    session_count: Arc<std::sync::atomic::AtomicUsize>,
    unrouted_packets: Arc<std::sync::atomic::AtomicU64>,
    route_directory: Arc<RouteDirectory>,
    shard_loads: Arc<ShardLoadTable>,
    shard_candidates: Arc<ShardCandidateTable>,
) {
    let start_instant = Instant::now();
    let core = Arc::new(Mutex::new(WebRtcCore::new(
        config.core.clone(),
        start_instant,
    )));
    let local_candidate_sdps = build_local_candidate_sdps(
        socket.local_addr().ok(),
        &config.public_ips,
        config.candidate_hostname.as_deref(),
    );
    let mut routes = RouteTable::new(
        config.max_sessions,
        Duration::from_millis(config.migration_route_ttl_ms),
    );
    let mut session_remote: std::collections::HashMap<WebRtcSessionId, SocketAddr> =
        std::collections::HashMap::new();
    // Track which sessions belong to this shard so we can unregister
    // them from the directory and load table on close.
    let mut owned_sessions: std::collections::HashSet<WebRtcSessionId> =
        std::collections::HashSet::new();
    // Track ufrag registrations so we can `forget_ufrag` on close.
    let mut session_ufrags: std::collections::HashMap<WebRtcSessionId, String> =
        std::collections::HashMap::new();

    let handshake_timeout = if config.handshake_timeout_ms == 0 {
        None
    } else {
        Some(Duration::from_millis(config.handshake_timeout_ms))
    };
    let mut pending_handshakes: std::collections::HashMap<WebRtcSessionId, Instant> =
        std::collections::HashMap::new();
    let mut session_candidate_policies: std::collections::HashMap<
        WebRtcSessionId,
        CandidateTransportPolicy,
    > = std::collections::HashMap::new();
    let watchdog_interval = Duration::from_secs(1);
    let mut next_watchdog_sweep = start_instant + watchdog_interval;
    let backpressure_interval = Duration::from_secs(1);
    let mut next_backpressure_check = start_instant + backpressure_interval;

    let mut next_deadline: Option<Instant> = None;
    let mut output_buf = Vec::with_capacity(64);

    info!("WebRTC shard {shard_id} loop running");

    loop {
        let sleep = match next_deadline {
            Some(deadline) => {
                let effective = if handshake_timeout.is_some() && next_watchdog_sweep < deadline {
                    next_watchdog_sweep
                } else {
                    deadline
                };
                sleep_until(tokio::time::Instant::from_std(effective))
            }
            None => {
                let target = if handshake_timeout.is_some() {
                    next_watchdog_sweep
                } else {
                    Instant::now() + Duration::from_secs(60 * 60)
                };
                sleep_until(tokio::time::Instant::from_std(target))
            }
        };
        tokio::pin!(sleep);

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("WebRTC shard {shard_id} loop cancelled");
                break;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ShardCommand::Driver(cmd)) => {
                        let new_session = handle_command(
                            &core,
                            cmd,
                            now_micros(start_instant),
                            &local_candidate_sdps,
                            &event_tx,
                            &session_count,
                        ).await;
                        if let Some((session_id, candidate_policy)) = new_session {
                            // Pin the session to this shard regardless
                            // of what the global selector would say
                            // now: the front-end already chose us
                            // before the command landed.
                            route_directory.register_session(session_id, shard_id);
                            shard_loads.record_session_added(shard_id);
                            owned_sessions.insert(session_id);
                            session_candidate_policies.insert(session_id, candidate_policy);
                            if let Some(timeout) = handshake_timeout {
                                pending_handshakes
                                    .insert(session_id, Instant::now() + timeout);
                            }
                        }
                    }
                    Some(ShardCommand::Panic) => {
                        panic!("shard {shard_id} panic injected by test");
                    }
                    None => {
                        info!("WebRTC shard {shard_id} command channel closed; exiting");
                        break;
                    }
                }
            }
            datagram = packet_rx.recv() => {
                match datagram {
                    Some(datagram) => {
                        let is_tcp = datagram.is_tcp();
                        handle_datagram_for_shard(
                            shard_id,
                            &core,
                            &mut routes,
                            &mut session_remote,
                            &owned_sessions,
                            &socket,
                            &tcp_writers,
                            datagram,
                            is_tcp,
                            start_instant,
                            &event_tx,
                            &unrouted_packets,
                            &route_directory,
                        ).await;
                    }
                    None => {
                        info!("WebRTC shard {shard_id} packet channel closed; exiting");
                        break;
                    }
                }
            }
            _ = &mut sleep => {
                let now = Instant::now();
                let now_us = (now.saturating_duration_since(start_instant)).as_micros() as u64;
                let mut core_guard = core.lock();
                if let Err(err) =
                    core_guard.handle_input(WebRtcCoreInput::Tick { now_micros: now_us })
                {
                    drop(core_guard);
                    let _ = event_tx
                        .try_send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                            session_id: None,
                            kind: WebRtcDriverDiagnosticKind::Lifecycle,
                            message: format!("shard {shard_id} tick failed: {err}"),
                        }));
                }
            }
        }

        next_deadline = drain_shard_outputs(
            shard_id,
            &core,
            &mut routes,
            &mut session_remote,
            &mut owned_sessions,
            &mut session_ufrags,
            &socket,
            &tcp_writers,
            &mut output_buf,
            start_instant,
            &event_tx,
            &session_count,
            &mut pending_handshakes,
            &mut session_candidate_policies,
            &route_directory,
            &shard_loads,
            &shard_candidates,
        )
        .await;

        if handshake_timeout.is_some() {
            let now = Instant::now();
            if now >= next_watchdog_sweep && !pending_handshakes.is_empty() {
                let expired: Vec<WebRtcSessionId> = pending_handshakes
                    .iter()
                    .filter(|(_, deadline)| now >= **deadline)
                    .map(|(id, _)| *id)
                    .collect();
                for session_id in expired {
                    pending_handshakes.remove(&session_id);
                    {
                        let mut core_guard = core.lock();
                        let _ = core_guard.handle_input(WebRtcCoreInput::Command(
                            WebRtcCoreCommand::Close {
                                session_id,
                                reason: WebRtcCloseReason::HandshakeTimeout,
                            },
                        ));
                    }
                    let _ = event_tx
                        .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                            session_id: Some(session_id),
                            kind: WebRtcDriverDiagnosticKind::Lifecycle,
                            message: format!(
                                "shard {shard_id} session {session_id} handshake timed out, closing"
                            ),
                        }))
                        .await;
                }
                next_watchdog_sweep = now + watchdog_interval;
            }
        }

        {
            let now = Instant::now();
            let expired_routes = routes.compact_expired(now);
            for (addr, session_id) in expired_routes {
                let _ = event_tx.try_send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcDriverDiagnosticKind::RouteExpired,
                    message: format!(
                        "shard {shard_id} stale route {addr} for session {session_id} expired after migration TTL"
                    ),
                }));
            }
            // Directory compaction is owned by shard 0 to avoid all
            // shards racing for the same lock; shards > 0 only touch
            // their own active bindings.
            if shard_id.as_usize() == 0 {
                let _ = route_directory.compact_expired(now);
            }
            // Publish shard-local route counts so the public
            // `shard_stats()` snapshot reports per-shard rather than
            // aggregate values. Cheap: a `parking_lot::Mutex` write
            // bounded by `shard_count` writers.
            let (active, stale) = routes.route_counts();
            shard_loads.record_route_counts(shard_id, active, stale);
        }

        {
            let now = Instant::now();
            if now >= next_backpressure_check {
                let max_cap = event_tx.max_capacity();
                let remaining = event_tx.capacity();
                if remaining < max_cap.saturating_div(4) {
                    let pending = max_cap.saturating_sub(remaining);
                    let _ = event_tx.try_send(WebRtcDriverEvent::Backpressure {
                        queue: format!("shard-{shard_id}-events"),
                        pending,
                    });
                }
                next_backpressure_check = now + backpressure_interval;
            }
        }
    }
    info!("WebRTC shard {shard_id} loop terminated");
}
/// Route one inbound datagram to a session and update per-shard state.
///
/// 将一个入站数据报路由到会话并更新每个 shard 状态。
#[allow(clippy::too_many_arguments)]
async fn handle_datagram_for_shard(
    shard_id: ShardId,
    core: &Arc<Mutex<WebRtcCore>>,
    routes: &mut RouteTable,
    session_remote: &mut std::collections::HashMap<WebRtcSessionId, SocketAddr>,
    owned_sessions: &std::collections::HashSet<WebRtcSessionId>,
    socket: &Arc<UdpSocket>,
    tcp_writers: &Arc<TcpWriterRegistry>,
    datagram: NetDatagram,
    is_tcp: bool,
    start_instant: Instant,
    event_tx: &mpsc::Sender<WebRtcDriverEvent>,
    unrouted_packets: &Arc<std::sync::atomic::AtomicU64>,
    route_directory: &Arc<RouteDirectory>,
) {
    let received_at = datagram.received_at();
    let source = datagram.source();
    let now_us = received_at
        .saturating_duration_since(start_instant)
        .as_micros() as u64;
    let data = datagram.into_data();

    let dest_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            let _ = event_tx
                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: None,
                    kind: WebRtcDriverDiagnosticKind::SocketError,
                    message: format!("shard {shard_id} local_addr lookup failed: {err}"),
                }))
                .await;
            return;
        }
    };

    if let Some(session_id) = routes.lookup(&source) {
        // Only feed the packet into the core if this shard owns the
        // session. A broadcast packet that lands on the wrong shard
        // is dropped silently here (the owner shard will accept it).
        if !owned_sessions.contains(&session_id) {
            return;
        }
        let mut core_guard = core.lock();
        let _ = core_guard.handle_input(WebRtcCoreInput::Network(WebRtcNetworkInput {
            session_id,
            source,
            destination: dest_addr,
            data,
            now_micros: now_us,
        }));
        drop(core_guard);
        return;
    }

    // Unbound packet: ask the core if any session on this shard
    // accepts it (matches by ICE ufrag/credentials). If we are part
    // of a broadcast and the packet does not match any of our
    // sessions, route_unbound_packet returns None and we silently
    // drop. Only the front-end's diagnostic counter would matter for
    // truly unbound packets, but in the multi-shard topology the
    // front-end has already broadcast — so the right counter behaviour
    // is to *not* increment here unless we see it as a session owner.
    let routed = {
        let mut core_guard = core.lock();
        core_guard
            .route_unbound_packet(source, dest_addr, data, now_us)
            .ok()
            .flatten()
    };
    let Some(session_id) = routed else {
        // Silently drop on a broadcast miss.
        return;
    };
    if !owned_sessions.contains(&session_id) {
        // Defensive: a session id we don't own should never come back
        // from `route_unbound_packet` (it iterates this shard's
        // `WebRtcCore` only), but guard anyway so a future
        // refactor doesn't accidentally cross-talk.
        return;
    }

    let previous_addr = session_remote.get(&session_id).copied();
    let is_migration = previous_addr.is_some() && previous_addr != Some(source);
    let mut unbind_diff = RouteCandidateDiff::default();
    if let Some(prev) = previous_addr {
        if prev != source {
            unbind_diff = routes.unbind_address(&prev, received_at);
        }
    }
    let bind_result = if is_migration {
        routes.try_bind_migration(source, session_id, received_at)
    } else {
        Ok(routes.bind(source, session_id, received_at))
    };
    match bind_result {
        Ok((_, bind_diff)) => {
            session_remote.insert(session_id, source);
            let owner_shard = route_directory
                .lookup_session(session_id)
                .unwrap_or(shard_id);
            let directory_result = if is_migration {
                route_directory
                    .migrate_remote(previous_addr, source, session_id, owner_shard, received_at)
                    .map(|_| ())
            } else {
                route_directory.bind_remote(source, session_id, owner_shard)
            };
            if let Err(err) = directory_result {
                match err {
                    crate::directory::RouteDirectoryError::AddressCapacityExceeded(cap) => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                session_id: Some(session_id),
                                kind: WebRtcDriverDiagnosticKind::RouteDirectoryFull,
                                message: format!(
                                    "shard {shard_id} route directory at capacity {cap}; \
                                     session {session_id} bound on shard but not in directory"
                                ),
                            }))
                            .await;
                    }
                    crate::directory::RouteDirectoryError::AddressAlreadyBound { .. } => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                session_id: Some(session_id),
                                kind: WebRtcDriverDiagnosticKind::MigrationRejected,
                                message: format!(
                                    "shard {shard_id} route directory rejected {source} \
                                     for session {session_id}: {err}"
                                ),
                            }))
                            .await;
                    }
                }
            }
            // Pin the TCP writer's owner shard to the session's
            // actual owner. The writer was registered with a
            // hash-based provisional owner at accept time
            // (`shard_for_remote_addr`); now that STUN parsing has
            // resolved the session id we know the real owner. We
            // only retag when the directory explicitly reports
            // this shard as the session owner; otherwise the
            // session lives elsewhere and a different shard's
            // packet path will retag the writer when its own bind
            // lands. If the writer entry is gone (peer closed,
            // idle timeout, etc.) `reassign_shard` is a no-op.
            if is_tcp && route_directory.lookup_session(session_id) == Some(shard_id) {
                tcp_writers.reassign_shard(&source, shard_id);
            }
            if is_migration {
                let _ = event_tx
                    .send(WebRtcDriverEvent::RouteUpdated(WebRtcRouteUpdate {
                        session_id,
                        previous_addr,
                        new_addr: source,
                        diff: merge_route_diffs(unbind_diff, bind_diff),
                    }))
                    .await;
            }
        }
        Err(()) => {
            unrouted_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = event_tx
                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: Some(session_id),
                    kind: WebRtcDriverDiagnosticKind::MigrationRejected,
                    message: format!(
                        "shard {shard_id} session {session_id} migration to {source} rejected: route table at hard capacity"
                    ),
                }))
                .await;
        }
    }
}
/// Empty the core output queue and emit events, timers, and network sends.
///
/// 清空核心输出队列并发出事件、计时器和网络发送。
#[allow(clippy::too_many_arguments)]
async fn drain_shard_outputs(
    shard_id: ShardId,
    core: &Arc<Mutex<WebRtcCore>>,
    routes: &mut RouteTable,
    session_remote: &mut std::collections::HashMap<WebRtcSessionId, SocketAddr>,
    owned_sessions: &mut std::collections::HashSet<WebRtcSessionId>,
    session_ufrags: &mut std::collections::HashMap<WebRtcSessionId, String>,
    socket: &Arc<UdpSocket>,
    tcp_writers: &Arc<TcpWriterRegistry>,
    output_buf: &mut Vec<WebRtcCoreOutput>,
    start_instant: Instant,
    event_tx: &mpsc::Sender<WebRtcDriverEvent>,
    session_count: &Arc<std::sync::atomic::AtomicUsize>,
    pending_handshakes: &mut std::collections::HashMap<WebRtcSessionId, Instant>,
    session_candidate_policies: &mut std::collections::HashMap<
        WebRtcSessionId,
        CandidateTransportPolicy,
    >,
    route_directory: &Arc<RouteDirectory>,
    shard_loads: &Arc<ShardLoadTable>,
    shard_candidates: &Arc<ShardCandidateTable>,
) -> Option<Instant> {
    output_buf.clear();
    {
        let mut core_guard = core.lock();
        core_guard.pump_outputs(output_buf);
    }

    let mut next_deadline: Option<Instant> = None;
    for output in output_buf.drain(..) {
        match output {
            WebRtcCoreOutput::SendPacket(packet) => {
                let sent_via_tcp = if let Some(writer) = tcp_writers.get(&packet.destination) {
                    match encode_frame(&packet.data) {
                        Ok(framed) => {
                            let mut guard = writer.lock().await;
                            match guard.write_all(&framed).await {
                                Ok(()) => true,
                                Err(err) => {
                                    let _ = event_tx
                                        .send(WebRtcDriverEvent::Diagnostic(
                                            WebRtcDriverDiagnostic {
                                                session_id: Some(packet.session_id),
                                                kind: WebRtcDriverDiagnosticKind::SocketError,
                                                message: format!(
                                                    "shard {shard_id} tcp write to {} failed: {err}",
                                                    packet.destination
                                                ),
                                            },
                                        ))
                                        .await;
                                    drop(guard);
                                    tcp_writers.remove(&packet.destination);
                                    false
                                }
                            }
                        }
                        Err(err) => {
                            let _ = event_tx
                                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                    session_id: Some(packet.session_id),
                                    kind: WebRtcDriverDiagnosticKind::SocketError,
                                    message: format!(
                                        "shard {shard_id} tcp encode for {} failed: {err}",
                                        packet.destination
                                    ),
                                }))
                                .await;
                            true
                        }
                    }
                } else {
                    false
                };
                if !sent_via_tcp {
                    if let Err(err) = socket.send_to(&packet.data, packet.destination).await {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                session_id: Some(packet.session_id),
                                kind: WebRtcDriverDiagnosticKind::SocketError,
                                message: format!(
                                    "shard {shard_id} send_to({}) failed: {err}",
                                    packet.destination
                                ),
                            }))
                            .await;
                    }
                }
                session_remote.insert(packet.session_id, packet.destination);
            }
            WebRtcCoreOutput::SetTimer(timer) => {
                let deadline = start_instant + Duration::from_micros(timer.deadline_micros);
                next_deadline = Some(match next_deadline {
                    Some(prev) if prev < deadline => prev,
                    _ => deadline,
                });
            }
            WebRtcCoreOutput::LocalDescription {
                session_id,
                mut sdp,
                kind,
            } => {
                use cheetah_webrtc_core::WebRtcLocalDescriptionKind;
                let policy = session_candidate_policies
                    .get(&session_id)
                    .copied()
                    .unwrap_or(CandidateTransportPolicy::All);
                sdp = filter_local_candidates(&sdp, policy);
                sdp = ensure_end_of_candidates(&sdp);
                // Best-effort ufrag registration so the front-end can
                // route initial STUN binding requests to the right
                // shard. We register every time we see a fresh SDP
                // (e.g. on ICE restart) and replace the stored value.
                if let Some(ufrag) = extract_local_ufrag_from_sdp(&sdp) {
                    if let Some(prev) = session_ufrags.insert(session_id, ufrag.clone()) {
                        if prev != ufrag {
                            route_directory.forget_ufrag(&prev);
                        }
                    }
                    route_directory.register_ufrag(ufrag, shard_id);
                }
                // Snapshot the local ICE candidate counts from the SDP
                // before we move `sdp` into the ready event. Best
                // effort and non-blocking: drop on backpressure so we
                // never delay the corresponding `AnswerReady` /
                // `OfferReady` emission.
                let counts = count_local_candidates(&sdp);
                // Persist into the per-shard candidate table BEFORE
                // emitting the event so handle observers that read
                // `shard_candidate_stats()` after seeing the event
                // never observe stale gauge state.
                shard_candidates.record_snapshot(shard_id, counts);
                let _ = event_tx.try_send(WebRtcDriverEvent::LocalCandidateSnapshot {
                    shard_id,
                    session_id,
                    counts,
                });
                match kind {
                    WebRtcLocalDescriptionKind::Answer => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::AnswerReady { session_id, sdp })
                            .await;
                    }
                    WebRtcLocalDescriptionKind::Offer => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::OfferReady { session_id, sdp })
                            .await;
                    }
                }
            }
            WebRtcCoreOutput::Event(event) => {
                if let WebRtcCoreEvent::Lifecycle {
                    session_id,
                    state: cheetah_webrtc_core::WebRtcSessionLifecycle::Connected,
                } = &event
                {
                    pending_handshakes.remove(session_id);
                }
                let _ = event_tx.send(WebRtcDriverEvent::Core(event)).await;
            }
            WebRtcCoreOutput::Diagnostic(diag) => {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: diag.session_id,
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: diag.message,
                    }))
                    .await;
            }
            WebRtcCoreOutput::CloseSession { session_id, reason } => {
                // Close path stays quiet: we do not emit
                // `RouteUpdated` here, only `SessionClosed`. The
                // `RouteCandidateDiff` returned by `forget_session`
                // is intentionally discarded this round; a future
                // round may surface it as part of an aggregated
                // `RouteClosed` event.
                let _ = routes.forget_session(session_id);
                session_remote.remove(&session_id);
                pending_handshakes.remove(&session_id);
                session_candidate_policies.remove(&session_id);
                if let Some(ufrag) = session_ufrags.remove(&session_id) {
                    route_directory.forget_ufrag(&ufrag);
                }
                let owner = route_directory
                    .lookup_session(session_id)
                    .unwrap_or(shard_id);
                route_directory.forget_session(session_id);
                shard_loads.record_session_removed(owner);
                if owned_sessions.remove(&session_id) {
                    session_count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = event_tx
                    .send(WebRtcDriverEvent::SessionClosed { session_id, reason })
                    .await;
            }
        }
    }
    next_deadline
}
/// Apply a driver command to the core, creating or mutating sessions.
///
/// 将 driver 命令应用于核心，创建或改变会话。
async fn handle_command(
    core: &Arc<Mutex<WebRtcCore>>,
    cmd: WebRtcDriverCommand,
    now_us: u64,
    local_candidates: &[String],
    event_tx: &mpsc::Sender<WebRtcDriverEvent>,
    session_count: &Arc<std::sync::atomic::AtomicUsize>,
) -> Option<(WebRtcSessionId, CandidateTransportPolicy)> {
    match cmd {
        WebRtcDriverCommand::AcceptOffer(spec) => {
            let session_id = spec.session_id;
            let candidate_policy = spec.candidate_transport_policy;
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::AcceptOffer {
                    session_id,
                    role: spec.role,
                    remote_sdp: spec.remote_sdp_offer,
                    local_candidates: local_candidates.to_vec(),
                    now_micros: now_us,
                }))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("AcceptOffer failed: {err}"),
                    }))
                    .await;
                None
            } else {
                session_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some((session_id, candidate_policy))
            }
        }
        WebRtcDriverCommand::CreateOffer {
            session_id,
            role,
            spec,
            candidate_transport_policy,
        } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::CreateOffer {
                    session_id,
                    role,
                    spec,
                    local_candidates: local_candidates.to_vec(),
                    now_micros: now_us,
                }))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("CreateOffer failed: {err}"),
                    }))
                    .await;
                None
            } else {
                session_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some((session_id, candidate_transport_policy))
            }
        }
        WebRtcDriverCommand::AddRemoteCandidate {
            session_id,
            candidate,
        } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(
                    WebRtcCoreCommand::AddRemoteCandidate {
                        session_id,
                        candidate,
                        now_micros: now_us,
                    },
                ))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("AddRemoteCandidate failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::IceRestart {
            session_id,
            keep_local_candidates,
        } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::IceRestart {
                    session_id,
                    keep_local_candidates,
                    now_micros: now_us,
                }))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("IceRestart failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::ApplyRemoteAnswer {
            session_id,
            remote_sdp,
        } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::ApplyAnswer {
                    session_id,
                    remote_sdp,
                    now_micros: now_us,
                }))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("ApplyRemoteAnswer failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::SendFrame(frame) => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::SendFrame(
                    frame,
                )))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: None,
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("SendFrame failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::SendDataChannel(out) => {
            let session_id = out.session_id;
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(
                    WebRtcCoreCommand::SendDataChannel(out),
                ))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("SendDataChannel failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::RequestKeyframe {
            session_id,
            mid,
            kind,
        } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(
                    WebRtcCoreCommand::RequestKeyframe {
                        session_id,
                        mid,
                        kind,
                        now_micros: now_us,
                    },
                ))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("RequestKeyframe failed: {err}"),
                    }))
                    .await;
            }
            None
        }
        WebRtcDriverCommand::StopSession { session_id, reason } => {
            let result = {
                let mut core_guard = core.lock();
                core_guard.handle_input(WebRtcCoreInput::Command(WebRtcCoreCommand::Close {
                    session_id,
                    reason: reason.clone(),
                }))
            };
            if let Err(err) = result {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: Some(session_id),
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: format!("StopSession failed: {err}"),
                    }))
                    .await;
            }
            // Route table / session_count cleanup is owned by the
            // `WebRtcCoreOutput::CloseSession` drain handler so that
            // both explicit close and core-initiated close go through
            // the same path. Doing it here as well caused the
            // session_count to wrap on `usize` underflow.
            None
        }
        WebRtcDriverCommand::PanicShard { shard_id } => {
            panic!("shard {shard_id} panic injected by test");
        }
    }
}
/// Process one routed datagram through the core, binding or migrating addresses.
///
/// 通过核心处理一个路由数据报，绑定或迁移地址。
#[allow(clippy::too_many_arguments)]
async fn handle_datagram(
    core: &Arc<Mutex<WebRtcCore>>,
    routes: &mut RouteTable,
    session_remote: &mut std::collections::HashMap<WebRtcSessionId, SocketAddr>,
    socket: &Arc<UdpSocket>,
    tcp_writers: &Arc<TcpWriterRegistry>,
    datagram: NetDatagram,
    is_tcp: bool,
    start_instant: Instant,
    event_tx: &mpsc::Sender<WebRtcDriverEvent>,
    unrouted_packets: &Arc<std::sync::atomic::AtomicU64>,
    route_directory: &Arc<RouteDirectory>,
) {
    let received_at = datagram.received_at();
    let source = datagram.source();
    let now_us = received_at
        .saturating_duration_since(start_instant)
        .as_micros() as u64;
    let data = datagram.into_data();

    let dest_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            let _ = event_tx
                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: None,
                    kind: WebRtcDriverDiagnosticKind::SocketError,
                    message: format!("local_addr lookup failed: {err}"),
                }))
                .await;
            return;
        }
    };

    if let Some(session_id) = routes.lookup(&source) {
        let mut core_guard = core.lock();
        let _ = core_guard.handle_input(WebRtcCoreInput::Network(WebRtcNetworkInput {
            session_id,
            source,
            destination: dest_addr,
            data,
            now_micros: now_us,
        }));
        // The bound address might have changed if the session migrated;
        // refresh the route below in `drain_core_outputs` based on
        // `session_remote`.
        drop(core_guard);
        return;
    }

    // Unbound packet: ask the core if any session accepts it (matches by
    // ICE ufrag/credentials inside the STUN binding request).
    let packet_len = data.len();
    let routed = {
        let mut core_guard = core.lock();
        core_guard
            .route_unbound_packet(source, dest_addr, data, now_us)
            .ok()
            .flatten()
    };
    match routed {
        Some(session_id) => {
            // Detect migration: if we previously knew the session at a
            // different address, surface a RouteUpdated event. We
            // consult `session_remote` BEFORE rebinding so the
            // pre-migration address survives the update.
            let previous_addr = session_remote.get(&session_id).copied();
            let is_migration = previous_addr.is_some() && previous_addr != Some(source);
            // When a session migrates, the OLD address mapping in the
            // route table would still point at this session, so packets
            // that race the migration on the old path would get
            // accepted incorrectly. Move the old binding into the
            // stale set so it expires after `stale_ttl` instead.
            let mut unbind_diff = RouteCandidateDiff::default();
            if let Some(prev) = previous_addr {
                if prev != source {
                    unbind_diff = routes.unbind_address(&prev, received_at);
                }
            }
            // For migration, use try_bind_migration to enforce the hard
            // capacity cap. For non-migration (first bind), use the
            // standard bind which only enforces the soft cap.
            let bind_result = if is_migration {
                routes.try_bind_migration(source, session_id, received_at)
            } else {
                Ok(routes.bind(source, session_id, received_at))
            };
            match bind_result {
                Ok((_, bind_diff)) => {
                    session_remote.insert(session_id, source);
                    // Mirror the binding to the global route
                    // directory. We only register the new active
                    // address; the previous one (if any) is moved to
                    // the directory's stale set automatically by
                    // `migrate_remote`. Capacity overflow surfaces a
                    // RouteDirectoryFull diagnostic so the operator
                    // can grow the cap before sessions start failing.
                    //
                    // The owning shard is whatever was assigned at
                    // session creation time. Recomputing via the
                    // selector here would be wrong under the
                    // least-loaded strategy because the load table
                    // changes as sessions come and go.
                    let owner_shard = route_directory
                        .lookup_session(session_id)
                        .unwrap_or_else(|| ShardId::new(0));
                    let directory_result = if is_migration {
                        route_directory
                            .migrate_remote(
                                previous_addr,
                                source,
                                session_id,
                                owner_shard,
                                received_at,
                            )
                            .map(|_| ())
                    } else {
                        route_directory.bind_remote(source, session_id, owner_shard)
                    };
                    if let Err(err) = directory_result {
                        match err {
                            crate::directory::RouteDirectoryError::AddressCapacityExceeded(cap) => {
                                let _ = event_tx
                                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                        session_id: Some(session_id),
                                        kind: WebRtcDriverDiagnosticKind::RouteDirectoryFull,
                                        message: format!(
                                            "route directory at capacity {cap}; session {session_id} bound on shard 0 but not in directory"
                                        ),
                                    }))
                                    .await;
                            }
                            crate::directory::RouteDirectoryError::AddressAlreadyBound {
                                ..
                            } => {
                                // Same-address-different-session is a
                                // concurrent migration race. Surface
                                // it as a RouteExpired diagnostic so
                                // operators see the conflict.
                                let _ = event_tx
                                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                        session_id: Some(session_id),
                                        kind: WebRtcDriverDiagnosticKind::MigrationRejected,
                                        message: format!(
                                            "route directory rejected {source} for session {session_id}: {err}"
                                        ),
                                    }))
                                    .await;
                            }
                        }
                    }
                    // Pin the TCP writer's owner shard to the
                    // session's actual owner. In single-shard mode
                    // (`shard_count <= 1`) the provisional owner
                    // chosen by `shard_for_remote_addr` is already
                    // `ShardId(0)`, so this reassign is a no-op —
                    // we keep the call site for symmetry so a
                    // future multi-shard refactor in this fast path
                    // does not silently leave writers on stale
                    // hash-based owners.
                    if is_tcp {
                        tcp_writers.reassign_shard(&source, owner_shard);
                    }
                    if is_migration {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::RouteUpdated(WebRtcRouteUpdate {
                                session_id,
                                previous_addr,
                                new_addr: source,
                                diff: merge_route_diffs(unbind_diff, bind_diff),
                            }))
                            .await;
                    }
                }
                Err(()) => {
                    // Migration rejected — route table at hard capacity.
                    // The session continues on its previous address;
                    // this packet is effectively dropped.
                    unrouted_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let _ = event_tx
                        .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                            session_id: Some(session_id),
                            kind: WebRtcDriverDiagnosticKind::MigrationRejected,
                            message: format!(
                                "session {session_id} migration to {source} rejected: route table at hard capacity"
                            ),
                        }))
                        .await;
                }
            }
        }
        None => {
            unrouted_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = event_tx
                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                    session_id: None,
                    kind: WebRtcDriverDiagnosticKind::UnroutedPacket,
                    message: format!("dropped {packet_len} byte packet from {source}"),
                }))
                .await;
        }
    }
}
/// Drain all pending core outputs in the single-shard core loop.
///
/// 耗尽单 shard 核心循环中的所有待处理核心输出。
#[allow(clippy::too_many_arguments)]
async fn drain_core_outputs(
    core: &Arc<Mutex<WebRtcCore>>,
    routes: &mut RouteTable,
    session_remote: &mut std::collections::HashMap<WebRtcSessionId, SocketAddr>,
    socket: &Arc<UdpSocket>,
    tcp_writers: &Arc<TcpWriterRegistry>,
    output_buf: &mut Vec<WebRtcCoreOutput>,
    start_instant: Instant,
    event_tx: &mpsc::Sender<WebRtcDriverEvent>,
    session_count: &Arc<std::sync::atomic::AtomicUsize>,
    pending_handshakes: &mut std::collections::HashMap<WebRtcSessionId, Instant>,
    session_candidate_policies: &mut std::collections::HashMap<
        WebRtcSessionId,
        CandidateTransportPolicy,
    >,
    route_directory: &Arc<RouteDirectory>,
    shard_loads: &Arc<ShardLoadTable>,
    shard_candidates: &Arc<ShardCandidateTable>,
) -> Option<Instant> {
    output_buf.clear();
    {
        let mut core_guard = core.lock();
        core_guard.pump_outputs(output_buf);
    }

    let mut next_deadline: Option<Instant> = None;
    for output in output_buf.drain(..) {
        match output {
            WebRtcCoreOutput::SendPacket(packet) => {
                // If this destination has an active TCP connection
                // registered, prefer TCP framing. Otherwise fall back
                // to UDP. This matches RFC 4571 single-port behaviour:
                // outbound packets follow the same transport the
                // remote peer chose.
                let sent_via_tcp = if let Some(writer) = tcp_writers.get(&packet.destination) {
                    match encode_frame(&packet.data) {
                        Ok(framed) => {
                            let mut guard = writer.lock().await;
                            match guard.write_all(&framed).await {
                                Ok(()) => true,
                                Err(err) => {
                                    let _ = event_tx
                                        .send(WebRtcDriverEvent::Diagnostic(
                                            WebRtcDriverDiagnostic {
                                                session_id: Some(packet.session_id),
                                                kind: WebRtcDriverDiagnosticKind::SocketError,
                                                message: format!(
                                                    "tcp write to {} failed: {err}",
                                                    packet.destination
                                                ),
                                            },
                                        ))
                                        .await;
                                    drop(guard);
                                    tcp_writers.remove(&packet.destination);
                                    false
                                }
                            }
                        }
                        Err(err) => {
                            let _ = event_tx
                                .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                    session_id: Some(packet.session_id),
                                    kind: WebRtcDriverDiagnosticKind::SocketError,
                                    message: format!(
                                        "tcp encode for {} failed: {err}",
                                        packet.destination
                                    ),
                                }))
                                .await;
                            // Encode failure is a packet loss, not a
                            // transport demotion. Don't fall back to UDP
                            // because the same payload would still be
                            // oversize for either RTP or DTLS.
                            true
                        }
                    }
                } else {
                    false
                };

                if !sent_via_tcp {
                    if let Err(err) = socket.send_to(&packet.data, packet.destination).await {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                                session_id: Some(packet.session_id),
                                kind: WebRtcDriverDiagnosticKind::SocketError,
                                message: format!("send_to({}) failed: {err}", packet.destination),
                            }))
                            .await;
                    }
                }
                // Track the most recent send destination per session.
                // Migration detection itself is driven from inbound
                // packets in `handle_datagram` which emits the
                // `RouteUpdated` event; this map is just used as a
                // hint when we need to start a new outbound stream
                // for the same session.
                session_remote.insert(packet.session_id, packet.destination);
            }
            WebRtcCoreOutput::SetTimer(timer) => {
                let deadline = start_instant + Duration::from_micros(timer.deadline_micros);
                next_deadline = Some(match next_deadline {
                    Some(prev) if prev < deadline => prev,
                    _ => deadline,
                });
            }
            WebRtcCoreOutput::LocalDescription {
                session_id,
                mut sdp,
                kind,
            } => {
                use cheetah_webrtc_core::WebRtcLocalDescriptionKind;
                let policy = session_candidate_policies
                    .get(&session_id)
                    .copied()
                    .unwrap_or(CandidateTransportPolicy::All);
                sdp = filter_local_candidates(&sdp, policy);
                sdp = ensure_end_of_candidates(&sdp);
                // Snapshot the local ICE candidate counts before
                // moving `sdp` into the ready event. The single-shard
                // fast path canonically reports `ShardId(0)` so
                // observers can treat single- and multi-shard event
                // streams uniformly. Best effort and non-blocking:
                // drop on backpressure so we never delay the
                // corresponding `AnswerReady` / `OfferReady`.
                let counts = count_local_candidates(&sdp);
                // Persist into the per-shard candidate table BEFORE
                // emitting the event so handle observers that read
                // `shard_candidate_stats()` after seeing the event
                // never observe stale gauge state.
                shard_candidates.record_snapshot(ShardId::new(0), counts);
                let _ = event_tx.try_send(WebRtcDriverEvent::LocalCandidateSnapshot {
                    shard_id: ShardId::new(0),
                    session_id,
                    counts,
                });
                match kind {
                    WebRtcLocalDescriptionKind::Answer => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::AnswerReady { session_id, sdp })
                            .await;
                    }
                    WebRtcLocalDescriptionKind::Offer => {
                        let _ = event_tx
                            .send(WebRtcDriverEvent::OfferReady { session_id, sdp })
                            .await;
                    }
                }
            }
            WebRtcCoreOutput::Event(event) => {
                // The handshake watchdog clears its pending entry as
                // soon as the session reports `Lifecycle::Connected`.
                // We do this before forwarding so a slow downstream
                // module can't hold the watchdog map open after the
                // session is actually up.
                if let WebRtcCoreEvent::Lifecycle {
                    session_id,
                    state: cheetah_webrtc_core::WebRtcSessionLifecycle::Connected,
                } = &event
                {
                    pending_handshakes.remove(session_id);
                }
                // All core events are forwarded to the module which is
                // the canonical decision-maker for whether (and how)
                // they translate into engine action. Stray events for
                // sessions that have already been torn down are no-ops
                // because the module's session registry rejects them.
                let _ = event_tx.send(WebRtcDriverEvent::Core(event)).await;
            }
            WebRtcCoreOutput::Diagnostic(diag) => {
                let _ = event_tx
                    .send(WebRtcDriverEvent::Diagnostic(WebRtcDriverDiagnostic {
                        session_id: diag.session_id,
                        kind: WebRtcDriverDiagnosticKind::Lifecycle,
                        message: diag.message,
                    }))
                    .await;
            }
            WebRtcCoreOutput::CloseSession { session_id, reason } => {
                // Close path stays quiet: we do not emit
                // `RouteUpdated` here, only `SessionClosed`. The
                // `RouteCandidateDiff` returned by `forget_session`
                // is intentionally discarded this round; a future
                // round may surface it as part of an aggregated
                // `RouteClosed` event.
                let _ = routes.forget_session(session_id);
                session_remote.remove(&session_id);
                pending_handshakes.remove(&session_id);
                session_candidate_policies.remove(&session_id);
                // Look up the owning shard *before* `forget_session`
                // wipes the directory entry. Recomputing via
                // `shard_selector.pick(...)` is unsafe with the
                // least-loaded strategy because the live load table
                // has changed since the session was added, which
                // would yield a different shard and corrupt the
                // counter.
                let owner = route_directory
                    .lookup_session(session_id)
                    .unwrap_or_else(|| ShardId::new(0));
                route_directory.forget_session(session_id);
                shard_loads.record_session_removed(owner);
                session_count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                let _ = event_tx
                    .send(WebRtcDriverEvent::SessionClosed { session_id, reason })
                    .await;
            }
        }
    }
    next_deadline
}
/// Convert an Instant to monotonic microseconds since driver start.
///
/// 将 Instant 转换为自 driver 开始以来的单调微秒。
fn now_micros(start: Instant) -> u64 {
    Instant::now().saturating_duration_since(start).as_micros() as u64
}

/// Merge two [`RouteCandidateDiff`] values into one.
///
/// Used by the migration code paths that emit
/// [`WebRtcDriverEvent::RouteUpdated`] so the event carries the union
/// of the diff produced by `RouteTable::unbind_address` (for the old
/// active address) and the diff produced by `RouteTable::bind` /
/// `RouteTable::try_bind_migration` (for the new active address).
///
/// The vecs are tiny in practice (typically a single addr each) so we
/// just sort and dedup after extending; this keeps the merge stable
/// and idempotent without pulling in a dedicated set type.
///
/// 将两个 [`RouteCandidateDiff`] 值合并为一个。
///
/// 由发出 [`WebRtcDriverEvent::RouteUpdated`] 的迁移代码路径使用，因此该事件携带 `RouteTable::unbind_address` （对于旧的活动地址）生成的 diff 和 `RouteTable::bind` / `RouteTable::try_bind_migration` （对于新的活动地址）生成的 diff 的并集。
///
/// 在实践中，vec 很小（通常每个地址只有一个地址），因此我们只需在扩展后进行排序和去重即可；
/// 这可以保持合并的稳定性和幂等性，而无需引入专用的集合类型。
fn merge_route_diffs(
    mut left: RouteCandidateDiff,
    right: RouteCandidateDiff,
) -> RouteCandidateDiff {
    left.added.extend(right.added);
    left.removed.extend(right.removed);
    left.stale.extend(right.stale);
    left.added.sort();
    left.added.dedup();
    left.removed.sort();
    left.removed.dedup();
    left.stale.sort();
    left.stale.dedup();
    left
}

#[cfg(test)]
mod tcp_writer_registry_tests {
    //! Unit tests for [`TcpWriterRegistry`] (subtask 1.4).
    //!
    //! Covers the (writers, owners) index invariants that subtasks 1.1
    //! through 1.3 introduced:
    //! * `insert` populates both maps; `remove` clears them in the same
    //!   critical section.
    //! * `forget_shard` only drops entries owned by the target shard.
    //! * `reassign_shard` rewrites the owner of an existing entry but
    //!   is a no-op (and does not leak owner state) for unknown
    //!   addresses.
    //!
    //! Tests run on a real `tokio::net::TcpStream` pair (`bind +
    //! connect + accept`) so the writer half we register is the same
    //! shape the driver sees in production. We never write any bytes
    //! through the writer; the connection is held open for the
    //! lifetime of each test only to keep the `OwnedWriteHalf` valid.

    use super::*;
    use std::sync::Arc;
    use tokio::net::{TcpListener, TcpStream};

    /// Establishes one loopback TCP connection and returns
    /// `(client_local_addr, accepted_writer)`. The client end is kept
    /// alive by returning it so the connection stays open for the
    /// duration of the test.
    async fn make_pair() -> (
        SocketAddr,
        Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
        TcpStream,
    ) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let local = listener.local_addr().unwrap();
        let (accepted, client) = tokio::join!(listener.accept(), TcpStream::connect(local));
        let (server_stream, _peer_addr) = accepted.unwrap();
        let client = client.unwrap();
        let client_addr = client.local_addr().unwrap();
        let (_read, write) = server_stream.into_split();
        (
            client_addr,
            Arc::new(tokio::sync::Mutex::new(write)),
            client,
        )
    }

    #[tokio::test]
    async fn tcp_writer_registry_insert_and_remove_sync_writer_and_owner_index() {
        let registry = TcpWriterRegistry::default();
        let (addr, writer, _client) = make_pair().await;

        registry.insert(addr, ShardId::new(2), writer.clone());
        assert_eq!(registry.len(), 1);
        assert!(registry.get(&addr).is_some());

        registry.remove(&addr);
        assert_eq!(registry.len(), 0);
        assert!(registry.get(&addr).is_none());
        // The owner index must be cleared in the same critical
        // section: forget_shard on the shard we just removed must
        // not find any leftover entry.
        assert_eq!(registry.forget_shard(ShardId::new(2)), 0);
    }

    #[tokio::test]
    async fn tcp_writer_registry_forget_shard_clears_only_target_shard() {
        let registry = TcpWriterRegistry::default();
        let (addr0, w0, _c0) = make_pair().await;
        let (addr1, w1, _c1) = make_pair().await;
        let (addr2, w2, _c2) = make_pair().await;

        registry.insert(addr0, ShardId::new(0), w0);
        registry.insert(addr1, ShardId::new(1), w1);
        registry.insert(addr2, ShardId::new(0), w2);
        assert_eq!(registry.len(), 3);

        // Drop the two entries owned by shard 0; shard 1's entry must
        // remain.
        assert_eq!(registry.forget_shard(ShardId::new(0)), 2);
        assert_eq!(registry.len(), 1);
        assert!(registry.get(&addr0).is_none());
        assert!(registry.get(&addr2).is_none());
        assert!(registry.get(&addr1).is_some());

        // Drop the last shard's entry too.
        assert_eq!(registry.forget_shard(ShardId::new(1)), 1);
        assert_eq!(registry.len(), 0);
        assert!(registry.get(&addr1).is_none());
    }

    #[tokio::test]
    async fn tcp_writer_registry_reassign_shard_updates_owner_for_existing_entry() {
        let registry = TcpWriterRegistry::default();
        let (addr, writer, _client) = make_pair().await;

        registry.insert(addr, ShardId::new(0), writer);
        assert!(registry.reassign_shard(&addr, ShardId::new(7)));

        // After reassignment the entry no longer belongs to shard 0.
        assert_eq!(registry.forget_shard(ShardId::new(0)), 0);
        // It now belongs to shard 7 and forget_shard cleans it up.
        assert_eq!(registry.forget_shard(ShardId::new(7)), 1);
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn tcp_writer_registry_reassign_shard_for_unknown_addr_is_noop() {
        let registry = TcpWriterRegistry::default();
        // We only need a SocketAddr; the writer is never inserted so
        // the connection is dropped immediately after we capture it.
        let (addr, _writer, _client) = make_pair().await;

        assert!(!registry.reassign_shard(&addr, ShardId::new(3)));
        // Reassigning an unknown addr must not leave a dangling
        // owner entry — otherwise `forget_shard` would report a
        // ghost writer.
        assert_eq!(registry.forget_shard(ShardId::new(3)), 0);
        assert_eq!(registry.len(), 0);
    }
}
