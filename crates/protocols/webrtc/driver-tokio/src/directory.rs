//! Global route directory used by the multi-shard driver topology.
//!
//! Phase 02 follow-up (`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`):
//! a multi-shard WebRTC driver splits the existing single-task event loop
//! into a UDP/TCP front-end and `N` session owner shards. The front-end
//! never owns `WebRtcCore` state — it only routes inbound packets and
//! commands to the shard that owns the session.
//!
//! The directory is intentionally small and lock-protected: it stores
//! routing metadata only (session id ⇒ shard id, remote address ⇒ shard
//! id, ICE ufrag ⇒ shard id). It never holds protocol state. All
//! mutations are O(1) `HashMap` updates and serialised under a single
//! `parking_lot::Mutex`. Lookups are cheap enough to do on the hot path:
//! every UDP datagram pays one mutex acquisition and one `HashMap::get`.
//!
//! ## Bounding
//!
//! The directory has a hard cap on the number of address bindings
//! (`address_capacity`). Above the cap, [`RouteDirectory::bind_remote`]
//! returns [`RouteDirectoryError::AddressCapacityExceeded`] and the
//! caller is expected to surface a `Diagnostic`. There is also a hard
//! cap on the number of stale entries kept around for migration
//! straggler routing (`stale_capacity`); above that cap the oldest
//! stale entry is evicted.
//!
//! ## Why a separate module
//!
//! The existing per-shard [`crate::route::RouteTable`] is sufficient
//! when there is exactly one shard. With `driver_shards >= 2` the
//! front-end needs a globally consistent view of "which shard owns
//! this address". Doing that inside `RouteTable` would mix concerns
//! (per-session route data vs. cross-shard routing); keeping the
//! directory small and focused is closer to the architecture document.
//!
//! 多 shard driver 拓扑使用的全局路由目录。
//!
//! 第 02 阶段后续（`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`）：多 shard WebRTC driver 将现有的单任务事件循环拆分为 UDP/TCP 前端和 `N` 会话所有者 shards。
//! 前端从不拥有 `WebRtcCore` 状态 - 它仅将入站数据包和命令路由到拥有会话的 shard 。
//!
//! 该目录故意很小并且受锁定保护：它仅存储路由元数据（会话 id ⇒ shard id、远程地址 ⇒ shard id、ICE ufrag ⇒ shard id）。
//! 它从不保存协议状态。
//! 所有变更都是 O(1) `HashMap` 更新并在单个 `parking_lot::Mutex` 下序列化。
//! 在热路径上进行查找的成本足够低：每个 UDP 数据报都需要支付一次互斥锁获取和一次 `HashMap::get` 费用。
//!
//! ## 边界
//!
//! 该目录对地址绑定的数量有硬性上限 (`address_capacity`)。
//! 在上限之上，[`RouteDirectory::bind_remote`] 返回 [`RouteDirectoryError::AddressCapacityExceeded`]
//! ，调用者预计会显示 `Diagnostic`。
//! 对于迁移落后者路由保留的过时条目数量也有硬性上限（`stale_capacity`）；
//! 超过该上限，最旧的陈旧条目将被驱逐。
//!
//! ## 为什么需要一个单独的模块
//!
//! 当恰好有一个 shard 时，现有的 per-shard [`crate::route::RouteTable`] 就足够了。
//! 对于 `driver_shards >= 2`，前端需要“哪个 shard 拥有这个地址”的全局一致视图。
//! 在 `RouteTable` 内部执行此操作会混合问题（每个会话路由数据与跨 shard 路由）；
//! 保持目录小而集中，更接近架构文档。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cheetah_webrtc_core::WebRtcSessionId;
use parking_lot::{Mutex, RwLock};
use thiserror::Error;

use crate::sdp::LocalCandidateCounts;

/// Identifies a session-owner shard.
///
/// Currently a `usize` — the front-end picks shards via
/// `session_id % shard_count`. Wrapping it in a newtype lets us swap the
/// strategy later (e.g. least-loaded) without churning the rest of the
/// code base.
///
/// 标识会话所有者 shard。
///
/// 目前是 `usize` — 前端通过 `session_id % shard_count` 选择 shards。
/// 将其包装在新类型中可以让我们稍后交换策略（例如加载最少），而无需搅动其余代码库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ShardId(pub usize);

impl ShardId {
    /// Create a new shard id from a raw index.
    ///
    /// 从原始索引创建一个新的 shard id。
    pub const fn new(value: usize) -> Self {
        Self(value)
    }
    /// Return the underlying shard index.
    ///
    /// 返回底层 shard 索引。
    pub fn as_usize(self) -> usize {
        self.0
    }
}

impl std::fmt::Display for ShardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for [`RouteDirectory`]. Defaults are chosen so existing
/// single-shard deployments do not change behaviour.
///
/// [`RouteDirectory`] 的配置。
/// 选择默认值是为了确保现有的 single-shard 部署不会改变行为。
#[derive(Debug, Clone)]
pub struct RouteDirectoryConfig {
    /// Hard cap on the number of (remote address, shard) bindings.
    /// Reaching this cap causes [`RouteDirectory::bind_remote`] to
    /// return [`RouteDirectoryError::AddressCapacityExceeded`].
    ///
    /// （远程地址，shard）绑定数量的硬性上限。
    /// 达到此上限会导致 [`RouteDirectory::bind_remote`] 返回 [`RouteDirectoryError::AddressCapacityExceeded`]。
    pub address_capacity: usize,
    /// Hard cap on the number of stale (migrated) address bindings.
    /// When exceeded, the oldest stale entry is evicted to make room.
    ///
    /// 过时（已迁移）地址绑定数量的硬性上限。
    /// 当超过时，最旧的陈旧条目将被驱逐以腾出空间。
    pub stale_capacity: usize,
    /// TTL for stale entries. After this duration the entry is
    /// removed by [`RouteDirectory::compact_expired`].
    ///
    /// TTL 表示过时的条目。
    /// 在此持续时间之后，该条目将被 [`RouteDirectory::compact_expired`] 删除。
    pub stale_ttl: Duration,
}

impl Default for RouteDirectoryConfig {
    fn default() -> Self {
        Self {
            address_capacity: 16_384,
            stale_capacity: 4_096,
            stale_ttl: Duration::from_secs(30),
        }
    }
}

/// Failures the directory can return on a mutation.
///
/// 目录可能会因变更而返回失败。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RouteDirectoryError {
    /// The directory is at hard capacity for active address bindings.
    /// New sessions must wait or be rejected upstream.
    ///
    /// 该目录具有用于活动地址绑定的硬容量。
    /// 新会话必须等待或被上游拒绝。
    #[error("route directory at address capacity ({0})")]
    AddressCapacityExceeded(usize),
    /// Attempted to bind a remote address that is already actively
    /// bound to a *different* session. Migration paths should call
    /// [`RouteDirectory::migrate_remote`] instead.
    ///
    /// 尝试绑定已主动绑定到*不同*会话的远程地址。
    /// 迁移路径应改为调用 [`RouteDirectory::migrate_remote`]。
    #[error("address {addr} already bound to session {existing} on shard {shard}")]
    AddressAlreadyBound {
        addr: SocketAddr,
        existing: WebRtcSessionId,
        shard: ShardId,
    },
}

#[derive(Debug, Clone, Copy)]
struct AddressEntry {
    session: WebRtcSessionId,
    shard: ShardId,
}

#[derive(Debug, Clone)]
struct StaleEntry {
    session: WebRtcSessionId,
    shard: ShardId,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct DirectoryInner {
    session_to_shard: HashMap<WebRtcSessionId, ShardId>,
    /// Active, primary remote address binding per session. The
    /// front-end resolves UDP datagrams and TCP frames through this
    /// map.
    ///
    /// 每个会话的活动主要远程地址绑定。
    /// 前端通过这个映射解析 UDP 数据报和 TCP 帧。
    remote_to_entry: HashMap<SocketAddr, AddressEntry>,
    /// ICE ufrag-to-shard. Used when a STUN binding request lands on
    /// the listener but the source IP is not yet bound (initial ICE
    /// arrival, or a NAT-rebound peer).
    ///
    /// ICE ufrag-to-shard。
    /// 当 STUN 绑定请求到达侦听器但源 IP 尚未绑定时使用（初始 ICE 到达，或 NAT 反弹对等点）。
    ufrag_to_shard: HashMap<String, ShardId>,
    /// Stale routes during connection migration. Packets arriving at
    /// these addresses still resolve to the same session and shard
    /// for `stale_ttl`, then expire.
    ///
    /// 连接迁移期间的陈旧路由。
    /// 到达这些地址的数据包仍解析为同一会话和 `stale_ttl` 的 shard，然后过期。
    stale: HashMap<SocketAddr, StaleEntry>,
}

/// Global, lock-protected directory mapping sessions and addresses to
/// shards. Cheap to clone: uses an [`Arc`]-style internal layout but
/// stays in the driver crate so we can swap implementations later
/// without churning callers.
///
/// 全局、受锁保护的目录将会话和地址映射到 shards。
/// 克隆成本低：使用 [`Arc`] 风格的内部布局，但保留在 driver crate 中，因此我们可以稍后交换实现，而不会影响调用者。
#[derive(Debug)]
pub struct RouteDirectory {
    inner: Mutex<DirectoryInner>,
    config: RouteDirectoryConfig,
}

impl Default for RouteDirectory {
    fn default() -> Self {
        Self::new(RouteDirectoryConfig::default())
    }
}

impl RouteDirectory {
    pub fn new(config: RouteDirectoryConfig) -> Self {
        Self {
            inner: Mutex::new(DirectoryInner::default()),
            config,
        }
    }

    /// Register a new session and assign it to a shard.
    ///
    /// Idempotent: re-registering the same session with the same shard
    /// is a no-op. Re-registering with a *different* shard panics in
    /// debug builds (a session must never migrate across shards) and
    /// is a soft no-op in release builds — the caller's earlier
    /// assignment is preserved.
    ///
    /// 注册一个新会话并将其分配给 shard。
    ///
    /// 幂等：使用相同的 shard 重新注册相同的会话是无操作的。
    /// 在调试版本中重新注册*不同的* shard 会出现恐慌（会话绝不能跨 shards 迁移），并且在发布版本中是软无操作 - 调用者之前的分配被保留。
    pub fn register_session(&self, session: WebRtcSessionId, shard: ShardId) {
        let mut guard = self.inner.lock();
        match guard.session_to_shard.get(&session) {
            Some(existing) if *existing != shard => {
                debug_assert_eq!(
                    *existing, shard,
                    "session {session:?} cannot migrate from shard {existing:?} to {shard:?}"
                );
                // Release builds keep the original assignment.
            }
            _ => {
                guard.session_to_shard.insert(session, shard);
            }
        }
    }

    /// Look up the shard that owns the session.
    ///
    /// 查找拥有该会话的 shard。
    pub fn lookup_session(&self, session: WebRtcSessionId) -> Option<ShardId> {
        self.inner.lock().session_to_shard.get(&session).copied()
    }

    /// Bind an ICE ufrag to a shard so STUN binding requests can be
    /// routed to the right shard before the remote address is known.
    ///
    /// 将 ICE ufrag 绑定到 shard，以便在知道远程地址之前将 STUN 绑定请求路由到正确的 shard。
    pub fn register_ufrag(&self, ufrag: String, shard: ShardId) {
        if ufrag.is_empty() {
            return;
        }
        self.inner.lock().ufrag_to_shard.insert(ufrag, shard);
    }

    /// Resolve a STUN ufrag to a shard.
    ///
    /// 将 STUN ufrag 解析为 shard。
    pub fn lookup_ufrag(&self, ufrag: &str) -> Option<ShardId> {
        if ufrag.is_empty() {
            return None;
        }
        self.inner.lock().ufrag_to_shard.get(ufrag).copied()
    }

    /// Bind a remote address to a session/shard. Used the first time a
    /// peer's address is observed.
    ///
    /// 将远程地址绑定到会话/shard。
    /// 第一次观察到对等方地址时使用。
    pub fn bind_remote(
        &self,
        addr: SocketAddr,
        session: WebRtcSessionId,
        shard: ShardId,
    ) -> Result<(), RouteDirectoryError> {
        let mut guard = self.inner.lock();
        if let Some(existing) = guard.remote_to_entry.get(&addr) {
            if existing.session == session && existing.shard == shard {
                return Ok(());
            }
            return Err(RouteDirectoryError::AddressAlreadyBound {
                addr,
                existing: existing.session,
                shard: existing.shard,
            });
        }
        if guard.remote_to_entry.len() >= self.config.address_capacity {
            return Err(RouteDirectoryError::AddressCapacityExceeded(
                self.config.address_capacity,
            ));
        }
        guard
            .remote_to_entry
            .insert(addr, AddressEntry { session, shard });
        Ok(())
    }

    /// Migrate a remote address from `previous` to `new`, recording the
    /// previous binding in the stale set.
    ///
    /// Returns the previous shard that owned the address (`None` if it
    /// was unbound). Same-shard migrations are the only kind currently
    /// supported — sessions never migrate across shards because their
    /// `WebRtcCore` state is shard-local.
    ///
    /// 将远程地址从 `previous` 迁移到 `new`，记录陈旧集中的先前绑定。
    ///
    /// 返回拥有该地址的前一个 shard （如果未绑定，则返回 `None` ）。
    /// Same-shard 迁移是当前支持的唯一类型 - 会话永远不会跨 shards 迁移，因为它们的 `WebRtcCore` 状态是 shard-local。
    pub fn migrate_remote(
        &self,
        previous: Option<SocketAddr>,
        new_addr: SocketAddr,
        session: WebRtcSessionId,
        shard: ShardId,
        now: Instant,
    ) -> Result<Option<ShardId>, RouteDirectoryError> {
        let mut guard = self.inner.lock();
        let prev_shard = match previous {
            Some(prev) if prev != new_addr => guard
                .remote_to_entry
                .remove(&prev)
                .map(|entry| entry.shard)
                .inspect(|_| {
                    if guard.stale.len() >= self.config.stale_capacity {
                        // Evict the oldest stale entry to make room.
                        if let Some(oldest_key) = guard
                            .stale
                            .iter()
                            .min_by_key(|(_, e)| e.expires_at)
                            .map(|(k, _)| *k)
                        {
                            guard.stale.remove(&oldest_key);
                        }
                    }
                    let expires_at = now + self.config.stale_ttl;
                    guard.stale.insert(
                        prev,
                        StaleEntry {
                            session,
                            shard,
                            expires_at,
                        },
                    );
                }),
            _ => None,
        };

        match guard.remote_to_entry.get(&new_addr) {
            Some(existing) if existing.session != session => {
                return Err(RouteDirectoryError::AddressAlreadyBound {
                    addr: new_addr,
                    existing: existing.session,
                    shard: existing.shard,
                });
            }
            _ => {}
        }
        if guard.remote_to_entry.len() >= self.config.address_capacity
            && !guard.remote_to_entry.contains_key(&new_addr)
        {
            return Err(RouteDirectoryError::AddressCapacityExceeded(
                self.config.address_capacity,
            ));
        }
        guard
            .remote_to_entry
            .insert(new_addr, AddressEntry { session, shard });
        Ok(prev_shard)
    }

    /// Resolve a remote address to its owning shard. Falls back to the
    /// stale set so packets racing a migration on the old path still
    /// reach their session.
    ///
    /// 将远程地址解析为其所属的 shard。
    /// 回落到陈旧的设置，因此在旧路径上进行迁移的数据包仍然可以到达其会话。
    pub fn lookup_remote(&self, addr: &SocketAddr) -> Option<(WebRtcSessionId, ShardId)> {
        let guard = self.inner.lock();
        if let Some(entry) = guard.remote_to_entry.get(addr) {
            return Some((entry.session, entry.shard));
        }
        guard
            .stale
            .get(addr)
            .map(|stale| (stale.session, stale.shard))
    }

    /// Drop all bindings for the given session. Called on session
    /// teardown by the owning shard so the directory does not leak
    /// entries.
    ///
    /// 删除给定会话的所有绑定。
    /// 由拥有者 shard 在会话拆卸时调用，以便目录不会泄漏条目。
    pub fn forget_session(&self, session: WebRtcSessionId) {
        let mut guard = self.inner.lock();
        guard.session_to_shard.remove(&session);
        guard
            .remote_to_entry
            .retain(|_, entry| entry.session != session);
        guard.ufrag_to_shard.retain(|_, _| true); // ufrag isn't keyed by session, leave it
        guard.stale.retain(|_, entry| entry.session != session);
    }

    /// Drop a single ufrag binding.
    ///
    /// 删除单个 ufrag 绑定。
    pub fn forget_ufrag(&self, ufrag: &str) {
        if ufrag.is_empty() {
            return;
        }
        self.inner.lock().ufrag_to_shard.remove(ufrag);
    }

    /// Drop **all** bindings owned by `shard`. Used by operators
    /// after observing [`crate::WebRtcDriverEvent::ShardStopped`]
    /// with a non-graceful reason (panic / unexpected exit) — the
    /// shard's `WebRtcCore` state is gone, but the directory still
    /// thinks every session and ufrag the shard owned is reachable.
    /// Calling `forget_shard` clears those orphaned mappings so new
    /// sessions can take over the addresses.
    ///
    /// Returns the number of `(session, address, ufrag, stale)`
    /// entries removed, in that order, for observability.
    ///
    /// 删除 `shard` 拥有的**所有**绑定。
    /// 由操作员在以非优雅原因（恐慌/意外退出）观察 [`crate::WebRtcDriverEvent::ShardStopped`] 后使用 - shard 的 `WebRtcCore` 状态消失
    /// ，但目录仍然认为 shard 拥有的每个会话和 ufrag 是可访问的。
    /// 调用 `forget_shard` 会清除这些孤立的映射，以便新会话可以接管这些地址。
    ///
    /// 返回按顺序删除的 `(session, address, ufrag, stale)` 条目数，以便于观察。
    pub fn forget_shard(&self, shard: ShardId) -> RouteDirectoryEvictionStats {
        let mut guard = self.inner.lock();
        let mut stats = RouteDirectoryEvictionStats::default();
        let sessions: Vec<WebRtcSessionId> = guard
            .session_to_shard
            .iter()
            .filter_map(|(session, owner)| {
                if *owner == shard {
                    Some(*session)
                } else {
                    None
                }
            })
            .collect();
        for session in &sessions {
            guard.session_to_shard.remove(session);
            stats.sessions += 1;
        }
        let session_set: std::collections::HashSet<WebRtcSessionId> =
            sessions.into_iter().collect();

        let addrs: Vec<SocketAddr> = guard
            .remote_to_entry
            .iter()
            .filter_map(|(addr, entry)| {
                if entry.shard == shard || session_set.contains(&entry.session) {
                    Some(*addr)
                } else {
                    None
                }
            })
            .collect();
        for addr in &addrs {
            guard.remote_to_entry.remove(addr);
            stats.addresses += 1;
        }

        let ufrags: Vec<String> = guard
            .ufrag_to_shard
            .iter()
            .filter_map(|(uf, owner)| {
                if *owner == shard {
                    Some(uf.clone())
                } else {
                    None
                }
            })
            .collect();
        for uf in &ufrags {
            guard.ufrag_to_shard.remove(uf);
            stats.ufrags += 1;
        }

        let stale: Vec<SocketAddr> = guard
            .stale
            .iter()
            .filter_map(|(addr, entry)| {
                if entry.shard == shard || session_set.contains(&entry.session) {
                    Some(*addr)
                } else {
                    None
                }
            })
            .collect();
        for addr in &stale {
            guard.stale.remove(addr);
            stats.stale += 1;
        }

        stats
    }

    /// Compact expired stale entries and return the list of removed
    /// `(addr, session, shard)` tuples. Callers translate this into
    /// observability events.
    ///
    /// 压缩过期的陈旧条目并返回已删除的 `(addr, session, shard)` 元组的列表。
    /// 调用者将其转化为可观察事件。
    pub fn compact_expired(&self, now: Instant) -> Vec<(SocketAddr, WebRtcSessionId, ShardId)> {
        let mut expired = Vec::new();
        let mut guard = self.inner.lock();
        guard.stale.retain(|addr, entry| {
            if now >= entry.expires_at {
                expired.push((*addr, entry.session, entry.shard));
                false
            } else {
                true
            }
        });
        expired
    }

    /// Snapshot directory sizes for stats / dashboards. Cheap: takes
    /// the lock once and reads three lengths.
    ///
    /// 统计数据/仪表板的快照目录大小。
    /// 便宜：获取一次锁并读取三个长度。
    pub fn stats_snapshot(&self) -> RouteDirectoryStats {
        let guard = self.inner.lock();
        RouteDirectoryStats {
            sessions: guard.session_to_shard.len(),
            addresses: guard.remote_to_entry.len(),
            ufrags: guard.ufrag_to_shard.len(),
            stale_addresses: guard.stale.len(),
        }
    }
}

/// Per-shard eviction counters returned by
/// [`RouteDirectory::forget_shard`]. Useful for observability when
/// an operator triggers a recovery flow after a shard panic.
///
/// [`RouteDirectory::forget_shard`] 返回的每 shard 逐出计数器。
/// 当操作员在 shard 恐慌后触发恢复流程时，对于可观察性很有用。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RouteDirectoryEvictionStats {
    /// Number of session entries removed by the eviction.
    ///
    /// 因驱逐而删除的会话条目数。
    pub sessions: usize,
    /// Number of remote-address entries removed by the eviction.
    ///
    /// 通过驱逐删除的远程地址条目数。
    pub addresses: usize,
    /// Number of ICE ufrag entries removed by the eviction.
    ///
    /// 通过驱逐删除的 ICE ufrag 条目数。
    pub ufrags: usize,
    /// Number of stale address entries removed by the eviction.
    ///
    /// 通过驱逐删除的过时地址条目数。
    pub stale: usize,
    /// Number of TCP writer entries removed when an operator-driven
    /// `evict_shard` (or supervisor auto-evict) cascades into the
    /// driver's TCP writer registry. The directory itself does not
    /// touch TCP writers, so [`RouteDirectory::forget_shard`]
    /// always reports `0` here; the field is populated by
    /// [`WebRtcDriverHandle::evict_shard`] and by the supervisor's
    /// auto-evict diagnostic path before the value is surfaced
    /// outside the driver crate.
    ///
    /// [`WebRtcDriverHandle::evict_shard`]: crate::WebRtcDriverHandle::evict_shard
    ///
    /// 当操作员驱动的 `evict_shard` （或主管自动逐出）级联到 driver 的 TCP 写入器注册表时，删除的 TCP 写入器条目数。
    /// 目录本身不涉及 TCP 编写者，因此 [`RouteDirectory::forget_shard`] 始终在此处报告 `0` ；
    /// 在该值出现在 driver crate 之外之前，该字段由 [`WebRtcDriverHandle::evict_shard`] 和主管的自动逐出诊断路径填充。
    ///
    /// [`WebRtcDriverHandle::evict_shard`]: crate::WebRtcDriverHandle::evict_shard
    pub tcp_writers: usize,
}

/// Snapshot of the directory's current size, returned by
/// [`RouteDirectory::stats_snapshot`].
///
/// 目录当前大小的快照，由 [`RouteDirectory::stats_snapshot`] 返回。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteDirectoryStats {
    /// Number of sessions currently registered in the directory.
    ///
    /// 当前在目录中注册的会话数。
    pub sessions: usize,
    /// Number of active remote-address bindings.
    ///
    /// 活动远程地址绑定的数量。
    pub addresses: usize,
    /// Number of ICE ufrag-to-session bindings.
    ///
    /// ICE ufrag 到会话的绑定数量。
    pub ufrags: usize,
    /// Number of stale addresses awaiting eviction.
    ///
    /// 等待驱逐的过时地址数量。
    pub stale_addresses: usize,
}

/// Per-shard observability snapshot. Surfaced via
/// `WebRtcDriverEvent::ShardStats` so operators can see per-shard load.
///
/// The driver's single-shard mode reports one entry with `shard_id = 0`.
///
/// 每个 shard 可观察性快照。
/// 通过 `WebRtcDriverEvent::ShardStats` 浮出水面，以便操作员可以查看每个 shard 负载。
///
/// driver 的单 shard 模式报告一个带有`shard_id = 0` 的条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcShardStats {
    /// Shard identifier.
    ///
    /// shard 标识符。
    pub shard_id: ShardId,
    /// Number of sessions owned by this shard.
    ///
    /// 此 shard 拥有的会话数。
    pub session_count: usize,
    /// Number of addresses currently bound to a session on this shard.
    ///
    /// 当前绑定到此 shard 上的会话的地址数。
    pub active_routes: usize,
    /// Number of stale routes still resolvable on this shard.
    ///
    /// 此 shard 上仍可解析的过时路由数量。
    pub stale_routes: usize,
}

/// Per-shard candidate statistics snapshot. Returned by
/// [`ShardCandidateTable::snapshot`] so operators can observe the
/// local candidate gathering result per shard without accumulating
/// events themselves.
///
/// 每个 shard candidate 统计快照。
/// 由 [`ShardCandidateTable::snapshot`] 返回，因此操作员可以观察每个 shard 的本地 candidate 收集结果，而无需自己累积事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcShardCandidateStats {
    /// Shard identifier.
    ///
    /// shard 标识符。
    pub shard_id: ShardId,
    /// Latest candidate counts for this shard.
    ///
    /// 最新的 candidate 计入此 shard。
    pub counts: LocalCandidateCounts,
}

/// Per-shard table of the latest [`LocalCandidateCounts`] reported by
/// each shard's event loop. Uses last-writer-wins (gauge) semantics:
/// each `record_snapshot` overwrites the previous value for that shard.
///
/// Follows the same pattern as [`crate::shard::ShardLoadTable`] but
/// stores candidate counts instead of session/route load.
///
/// 每个 shard 的事件循环报告的最新 [`LocalCandidateCounts`] 的 Per-shard 表。
/// 使用最后写入者获胜（计量器）语义：每个 `record_snapshot` 都会覆盖该 shard 的先前值。
///
/// 遵循与 [`crate::shard::ShardLoadTable`] 相同的模式，但存储 candidate 计数而不是会话/路由负载。
#[derive(Debug)]
pub struct ShardCandidateTable {
    inner: RwLock<Vec<LocalCandidateCounts>>,
}

impl ShardCandidateTable {
    /// Create a new table pre-allocated for `shard_count` shards, each
    /// initialized to [`LocalCandidateCounts::default()`] (all zeros).
    ///
    /// 创建一个为 `shard_count` shards 预分配的新表，每个表都初始化为 [`LocalCandidateCounts::default()`]（全零）。
    pub fn new(shard_count: usize) -> Self {
        let shard_count = shard_count.max(1);
        Self {
            inner: RwLock::new(vec![LocalCandidateCounts::default(); shard_count]),
        }
    }

    /// Record the latest candidate counts for a shard. Last-writer-wins
    /// semantics — each call overwrites the previous snapshot for the
    /// given shard slot (gauge, not accumulator).
    ///
    /// 记录 shard 的最新 candidate 计数。
    /// 最后写入者获胜语义 - 每个调用都会覆盖给定 shard 槽（计量器，而不是累加器）的先前快照。
    pub fn record_snapshot(&self, shard: ShardId, counts: LocalCandidateCounts) {
        let mut guard = self.inner.write();
        if let Some(slot) = guard.get_mut(shard.as_usize()) {
            *slot = counts;
        }
    }

    /// Return a snapshot of all shards' candidate counts in shard-id
    /// order. Each entry pairs the shard id with its latest counts.
    ///
    /// 以 shard-id 顺序返回所有 shards' candidate 计数的快照。
    /// 每个条目将 shard id 与其最新计数配对。
    pub fn snapshot(&self) -> Vec<WebRtcShardCandidateStats> {
        let guard = self.inner.read();
        guard
            .iter()
            .enumerate()
            .map(|(i, counts)| WebRtcShardCandidateStats {
                shard_id: ShardId::new(i),
                counts: *counts,
            })
            .collect()
    }

    /// Reset the candidate counts for a single shard back to all zeros.
    /// Called by the supervisor's auto-evict path when a shard panics
    /// and its state is discarded.
    ///
    /// 将单个 shard 的 candidate 计数重置回全零。
    /// 当 shard 发生恐慌并且其状态被丢弃时，由主管的自动逐出路径调用。
    pub fn clear_shard(&self, shard: ShardId) {
        let mut guard = self.inner.write();
        if let Some(slot) = guard.get_mut(shard.as_usize()) {
            *slot = LocalCandidateCounts::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn register_and_lookup_session_round_trip() {
        let dir = RouteDirectory::default();
        dir.register_session(WebRtcSessionId::new(1), ShardId(2));
        assert_eq!(
            dir.lookup_session(WebRtcSessionId::new(1)),
            Some(ShardId(2))
        );
        assert_eq!(dir.lookup_session(WebRtcSessionId::new(99)), None);
    }

    #[test]
    fn register_session_is_idempotent_for_same_shard() {
        let dir = RouteDirectory::default();
        dir.register_session(WebRtcSessionId::new(1), ShardId(0));
        dir.register_session(WebRtcSessionId::new(1), ShardId(0));
        assert_eq!(
            dir.lookup_session(WebRtcSessionId::new(1)),
            Some(ShardId(0))
        );
    }

    #[test]
    fn ufrag_lookup_round_trip() {
        let dir = RouteDirectory::default();
        dir.register_ufrag("UFRAG1".into(), ShardId(3));
        assert_eq!(dir.lookup_ufrag("UFRAG1"), Some(ShardId(3)));
        assert_eq!(dir.lookup_ufrag("missing"), None);
        // Empty ufrag never resolves.
        dir.register_ufrag(String::new(), ShardId(0));
        assert_eq!(dir.lookup_ufrag(""), None);
    }

    #[test]
    fn bind_remote_and_lookup_remote() {
        let dir = RouteDirectory::default();
        let addr1 = addr(5000);
        dir.bind_remote(addr1, WebRtcSessionId::new(7), ShardId(1))
            .expect("bind ok");
        assert_eq!(
            dir.lookup_remote(&addr1),
            Some((WebRtcSessionId::new(7), ShardId(1)))
        );
    }

    #[test]
    fn bind_remote_rejects_existing_session_collision() {
        let dir = RouteDirectory::default();
        let a = addr(6000);
        dir.bind_remote(a, WebRtcSessionId::new(1), ShardId(0))
            .unwrap();
        let err = dir
            .bind_remote(a, WebRtcSessionId::new(2), ShardId(0))
            .expect_err("collision must be rejected");
        assert!(matches!(
            err,
            RouteDirectoryError::AddressAlreadyBound { .. }
        ));
    }

    #[test]
    fn bind_remote_rejects_capacity_overflow() {
        let dir = RouteDirectory::new(RouteDirectoryConfig {
            address_capacity: 2,
            ..Default::default()
        });
        dir.bind_remote(addr(1), WebRtcSessionId::new(1), ShardId(0))
            .unwrap();
        dir.bind_remote(addr(2), WebRtcSessionId::new(2), ShardId(0))
            .unwrap();
        let err = dir
            .bind_remote(addr(3), WebRtcSessionId::new(3), ShardId(0))
            .expect_err("third bind must hit cap");
        assert!(matches!(
            err,
            RouteDirectoryError::AddressCapacityExceeded(2)
        ));
    }

    #[test]
    fn migrate_remote_moves_old_to_stale_and_resolves_via_stale() {
        let dir = RouteDirectory::default();
        let session = WebRtcSessionId::new(11);
        let prev = addr(1100);
        let new = addr(1200);
        let now = Instant::now();
        dir.bind_remote(prev, session, ShardId(0)).unwrap();
        let prev_shard = dir
            .migrate_remote(Some(prev), new, session, ShardId(0), now)
            .expect("migrate ok");
        assert_eq!(prev_shard, Some(ShardId(0)));
        // New address resolves directly.
        assert_eq!(dir.lookup_remote(&new), Some((session, ShardId(0))));
        // Old address still resolves via the stale set during the TTL.
        assert_eq!(dir.lookup_remote(&prev), Some((session, ShardId(0))));
        // After TTL expires the old binding is gone.
        let later = now + Duration::from_secs(60);
        let expired = dir.compact_expired(later);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], (prev, session, ShardId(0)));
        assert_eq!(dir.lookup_remote(&prev), None);
    }

    #[test]
    fn migrate_remote_rejects_cross_session_collision() {
        let dir = RouteDirectory::default();
        let now = Instant::now();
        dir.bind_remote(addr(1), WebRtcSessionId::new(1), ShardId(0))
            .unwrap();
        // Attempt to migrate session 2 onto address 1, which already
        // belongs to session 1. This must be rejected so the driver
        // can surface a `MigrationRejected` diagnostic.
        let err = dir
            .migrate_remote(None, addr(1), WebRtcSessionId::new(2), ShardId(0), now)
            .expect_err("cross-session migration should fail");
        assert!(matches!(
            err,
            RouteDirectoryError::AddressAlreadyBound { .. }
        ));
    }

    #[test]
    fn forget_session_drops_all_bindings() {
        let dir = RouteDirectory::default();
        let session = WebRtcSessionId::new(5);
        dir.register_session(session, ShardId(0));
        dir.bind_remote(addr(1), session, ShardId(0)).unwrap();
        dir.bind_remote(addr(2), session, ShardId(0)).unwrap();
        dir.forget_session(session);
        assert_eq!(dir.lookup_session(session), None);
        assert_eq!(dir.lookup_remote(&addr(1)), None);
        assert_eq!(dir.lookup_remote(&addr(2)), None);
    }

    #[test]
    fn stats_snapshot_counts_entries() {
        let dir = RouteDirectory::default();
        dir.register_session(WebRtcSessionId::new(1), ShardId(0));
        dir.bind_remote(addr(100), WebRtcSessionId::new(1), ShardId(0))
            .unwrap();
        dir.register_ufrag("UF".into(), ShardId(0));
        let stats = dir.stats_snapshot();
        assert_eq!(stats.sessions, 1);
        assert_eq!(stats.addresses, 1);
        assert_eq!(stats.ufrags, 1);
        assert_eq!(stats.stale_addresses, 0);
    }

    #[test]
    fn stale_capacity_evicts_oldest_when_full() {
        let dir = RouteDirectory::new(RouteDirectoryConfig {
            stale_capacity: 2,
            stale_ttl: Duration::from_secs(60),
            ..Default::default()
        });
        let session = WebRtcSessionId::new(1);
        let now = Instant::now();
        dir.bind_remote(addr(1), session, ShardId(0)).unwrap();
        dir.bind_remote(addr(2), session, ShardId(0)).unwrap();
        dir.bind_remote(addr(3), session, ShardId(0)).unwrap();
        // Migrate old → newer in three steps. The oldest stale entry
        // must be evicted on the third migration so the stale set
        // never grows past `stale_capacity`.
        dir.migrate_remote(Some(addr(1)), addr(10), session, ShardId(0), now)
            .unwrap();
        dir.migrate_remote(
            Some(addr(2)),
            addr(20),
            session,
            ShardId(0),
            now + Duration::from_millis(5),
        )
        .unwrap();
        dir.migrate_remote(
            Some(addr(3)),
            addr(30),
            session,
            ShardId(0),
            now + Duration::from_millis(10),
        )
        .unwrap();
        let stats = dir.stats_snapshot();
        assert!(
            stats.stale_addresses <= 2,
            "stale set must not exceed cap, saw {} (snapshot={:?})",
            stats.stale_addresses,
            stats
        );
    }

    #[test]
    fn forget_shard_drops_all_bindings_owned_by_shard() {
        // Set up two shards with sessions, addresses, ufrags, stale.
        let dir = RouteDirectory::default();
        let session_a = WebRtcSessionId::new(101);
        let session_b = WebRtcSessionId::new(102);
        let now = Instant::now();
        dir.register_session(session_a, ShardId(0));
        dir.register_session(session_b, ShardId(1));
        dir.bind_remote(addr(1000), session_a, ShardId(0)).unwrap();
        dir.bind_remote(addr(1001), session_a, ShardId(0)).unwrap();
        dir.bind_remote(addr(2000), session_b, ShardId(1)).unwrap();
        dir.register_ufrag("UFRAG_A".into(), ShardId(0));
        dir.register_ufrag("UFRAG_B".into(), ShardId(1));
        // Migrate session A so it has a stale entry.
        dir.migrate_remote(Some(addr(1000)), addr(1500), session_a, ShardId(0), now)
            .unwrap();

        // Evict shard 0.
        let evicted = dir.forget_shard(ShardId(0));
        assert!(
            evicted.sessions >= 1 && evicted.addresses >= 1 && evicted.ufrags >= 1,
            "expected non-zero evictions, saw {evicted:?}"
        );

        // Shard 0 entries are gone.
        assert_eq!(dir.lookup_session(session_a), None);
        assert_eq!(dir.lookup_remote(&addr(1500)), None);
        assert_eq!(dir.lookup_ufrag("UFRAG_A"), None);
        // Shard 1 entries survive.
        assert_eq!(dir.lookup_session(session_b), Some(ShardId(1)));
        assert_eq!(
            dir.lookup_remote(&addr(2000)),
            Some((session_b, ShardId(1)))
        );
        assert_eq!(dir.lookup_ufrag("UFRAG_B"), Some(ShardId(1)));
    }

    #[test]
    fn forget_shard_returns_zero_for_unknown_shard() {
        let dir = RouteDirectory::default();
        dir.register_session(WebRtcSessionId::new(1), ShardId(0));
        let evicted = dir.forget_shard(ShardId(99));
        assert_eq!(evicted, RouteDirectoryEvictionStats::default());
        // Unrelated shards are untouched.
        assert_eq!(
            dir.lookup_session(WebRtcSessionId::new(1)),
            Some(ShardId(0))
        );
    }

    // --- ShardCandidateTable tests ---

    #[test]
    fn shard_candidate_table_default_is_zero() {
        let table = ShardCandidateTable::new(4);
        let snap = table.snapshot();
        assert_eq!(snap.len(), 4);
        for entry in &snap {
            assert_eq!(entry.counts, LocalCandidateCounts::default());
        }
    }

    #[test]
    fn record_snapshot_updates_only_target_shard() {
        let table = ShardCandidateTable::new(3);
        let counts = LocalCandidateCounts {
            host: 2,
            srflx: 1,
            udp: 3,
            ..Default::default()
        };
        table.record_snapshot(ShardId(1), counts);
        let snap = table.snapshot();
        assert_eq!(snap[0].counts, LocalCandidateCounts::default());
        assert_eq!(snap[1].counts, counts);
        assert_eq!(snap[2].counts, LocalCandidateCounts::default());
    }

    #[test]
    fn record_snapshot_is_last_writer_wins() {
        let table = ShardCandidateTable::new(2);
        let first = LocalCandidateCounts {
            host: 1,
            ..Default::default()
        };
        let second = LocalCandidateCounts {
            host: 5,
            relay: 2,
            ..Default::default()
        };
        table.record_snapshot(ShardId(0), first);
        table.record_snapshot(ShardId(0), second);
        let snap = table.snapshot();
        assert_eq!(snap[0].counts, second, "last write must win");
    }

    #[test]
    fn clear_shard_resets_only_target() {
        let table = ShardCandidateTable::new(3);
        let counts = LocalCandidateCounts {
            host: 4,
            tcp: 2,
            ipv6: 1,
            ..Default::default()
        };
        table.record_snapshot(ShardId(0), counts);
        table.record_snapshot(ShardId(1), counts);
        table.record_snapshot(ShardId(2), counts);
        table.clear_shard(ShardId(1));
        let snap = table.snapshot();
        assert_eq!(snap[0].counts, counts);
        assert_eq!(snap[1].counts, LocalCandidateCounts::default());
        assert_eq!(snap[2].counts, counts);
    }

    #[test]
    fn snapshot_returns_entries_in_shard_id_order() {
        let table = ShardCandidateTable::new(4);
        // Write in reverse order to verify ordering is by shard id.
        for i in (0..4).rev() {
            let counts = LocalCandidateCounts {
                host: i + 1,
                ..Default::default()
            };
            table.record_snapshot(ShardId(i), counts);
        }
        let snap = table.snapshot();
        for (i, entry) in snap.iter().enumerate() {
            assert_eq!(entry.shard_id, ShardId(i));
            assert_eq!(entry.counts.host, i + 1);
        }
    }
}
