//! Shard selection and per-shard load tracking.
//!
//! Phase 02 follow-up (`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`).
//!
//! The current driver still owns one [`crate::route::RouteTable`] and
//! one `WebRtcCore` instance, but the multi-shard front-end lands in
//! the next round. To unblock that work we already select a shard id
//! per session here and account for it in [`ShardLoadTable`]. The
//! existing event loop reads `ShardId(0)` for everything; the
//! upcoming `WebRtcIoFront + WebRtcShard[N]` topology will use the
//! same selector, so callers don't have to change when shards become
//! live.
//!
//! The selector wraps a [`ShardSelectorStrategy`]:
//!
//! * [`HashShardStrategy`] — splitmix64 fold of the session id; pure,
//!   deterministic, default. Production single- and multi-shard
//!   driver topologies use this.
//! * [`LeastLoadedShardStrategy`] — picks the shard with the smallest
//!   `session_count`, breaking ties by the deterministic hash. Useful
//!   when callers care about peak balance more than locality.
//!
//! The selector is intentionally tiny: it is a pure function of
//! `(session_id, shard_count [, load snapshot])` so the choice is
//! reproducible across crashes / cold starts when the strategy is
//! deterministic.
//!
//! shard 选择和每 shard 负载跟踪。
//!
//! 第 02 阶段后续行动 (`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`)。
//!
//! 当前的 driver 仍然拥有一个 [`crate::route::RouteTable`] 和一个 `WebRtcCore` 实例，但多 shard 前端将进入下一轮。
//! 为了解除对该工作的阻止，我们已经在此处为每个会话选择了一个 shard id，并在 [`ShardLoadTable`] 中对其进行了说明。
//! 现有的事件循环读取 `ShardId(0)` 的所有内容；
//! 即将推出的 `WebRtcIoFront + WebRtcShard[N]` 拓扑将使用相同的选择器，因此当 shards 生效时，调用者不必进行更改。
//!
//! 选择器包装了一个 [`ShardSelectorStrategy`]：
//!
//! * [`HashShardStrategy`] — 会话 id 的 splitmix64 折叠；
//!   纯粹的、确定性的、默认的。
//!   生产单和多 shard driver 拓扑使用此拓扑。
//! * [`LeastLoadedShardStrategy`] — 选择具有最小 `session_count` 的 shard，通过确定性哈希打破联系。
//!   当呼叫者更关心峰值平衡而不是位置时非常有用。
//!
//! 选择器故意很小：它是 `(session_id, shard_count [, load snapshot])` 的纯函数，因此当策略是确定性时，选择在崩溃/冷启动时是可重现的。

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_webrtc_core::WebRtcSessionId;
use parking_lot::Mutex;

use crate::directory::ShardId;

/// Strategy that maps a session to a shard.
///
/// Implementations should be cheap to call: the driver may invoke
/// `pick` on every accepted session and on every migration.
///
/// 将会话映射到 shard 的策略。
///
/// 实现的调用成本应该很低：driver 可以在每个接受的会话和每次迁移时调用 `pick`。
pub trait ShardSelectorStrategy: Send + Sync {
    /// Pick the shard that owns the given session.
    ///
    /// 选择拥有给定会话的 shard。
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId;
}

/// Hash-based strategy. Splitmix64 fold of the session id mod
/// `shard_count`. Stable across calls for a given id.
///
/// 基于哈希的策略。
/// 会话 id mod `shard_count` 的 Splitmix64 折叠。
/// 在给定 id 的调用中保持稳定。
#[derive(Debug, Clone, Default)]
pub struct HashShardStrategy;

impl ShardSelectorStrategy for HashShardStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        _loads: &ShardLoadTable,
    ) -> ShardId {
        if shard_count <= 1 {
            return ShardId::new(0);
        }
        let mut x = session_id.0;
        x ^= x.rotate_right(30);
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x.rotate_right(27);
        x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x.rotate_right(31);
        ShardId::new((x as usize) % shard_count)
    }
}

/// Least-loaded strategy. Picks the shard with the smallest
/// `session_count`. Ties resolve via [`HashShardStrategy`] so the
/// selection stays deterministic for callers that care.
///
/// This is **not** the default — most production deployments benefit
/// from hash locality (a session always lands on the same shard
/// regardless of cluster ordering). Operators who care about peak
/// balance can swap it in via [`ShardSelector::with_strategy`].
///
/// 最小负载策略。
/// 选择 `session_count` 最小的 shard。
/// 关系通过 [`HashShardStrategy`] 解决，因此对于关心的调用者来说，选择保持确定性。
///
/// 这不是默认设置 - 大多数生产部署都受益于哈希局部性（无论集群顺序如何，会话始终位于相同的 shard 上）。
/// 关心峰值平衡的操作员可以通过 [`ShardSelector::with_strategy`] 进行交换。
#[derive(Debug, Clone, Default)]
pub struct LeastLoadedShardStrategy;

impl ShardSelectorStrategy for LeastLoadedShardStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId {
        if shard_count <= 1 {
            return ShardId::new(0);
        }
        let snapshot = loads.snapshot();
        // Find min `session_count`. On tie, fall back to the hash.
        let min_load = snapshot
            .iter()
            .map(|(_, l)| l.session_count)
            .min()
            .unwrap_or(0);
        let candidates: Vec<ShardId> = snapshot
            .iter()
            .filter(|(_, l)| l.session_count == min_load)
            .map(|(id, _)| *id)
            .collect();
        if candidates.len() == 1 {
            return candidates[0];
        }
        // Tie-break with the hash applied to the candidate set.
        let hash = HashShardStrategy.pick(session_id, candidates.len(), loads);
        candidates[hash.as_usize()]
    }
}

/// Sticky strategy that preserves the owner shard across calls.
///
/// On the first call for a given session id the strategy delegates
/// to a configurable inner strategy (default: hash) and remembers
/// the result. Subsequent calls for the same id return the cached
/// shard regardless of inner-strategy state. This matters for ICE
/// restart, transient migration races, and any other path where the
/// driver picks a shard for an existing session — without sticky
/// affinity, a least-loaded inner strategy would land the restarted
/// session on a *different* shard while the previous `WebRtcCore`
/// state still lives on the original one.
///
/// Memory is bounded by `cache_capacity`; entries are evicted via a
/// simple FIFO ring once the cap is reached. The default cap (4 ×
/// `max_sessions` from the driver config) keeps a long history of
/// recent sessions without unbounded growth.
///
/// # Recipe: balanced-sticky
///
/// To combine "load-aware initial placement" with "stable owner",
/// wrap [`LeastLoadedShardStrategy`] in [`StickyHashShardStrategy`]:
///
/// ```ignore
/// use std::sync::Arc;
/// use cheetah_webrtc_driver_tokio::{
///     LeastLoadedShardStrategy, ShardSelector, StickyHashShardStrategy,
/// };
/// let strategy = Arc::new(StickyHashShardStrategy::new(
///     Arc::new(LeastLoadedShardStrategy),
///     8_192,
/// ));
/// let selector = ShardSelector::with_strategy(4, strategy);
/// ```
///
/// New sessions land on the emptiest shard; ICE restarts and other
/// re-pick paths still resolve to the originally chosen shard so
/// the `WebRtcCore` state is never orphaned.
///
/// 在调用之间保留所有者 shard 的粘性策略。
///
/// 在第一次调用给定会话 ID 时，策略会委托给可配置的内部策略（默认值：散列）并记住结果。
/// 无论内部策略状态如何，对同一 id 的后续调用都会返回缓存的 shard 。
/// 这对于 ICE 重启、短暂迁移竞赛以及 driver 为现有会话选择 shard 的任何其他路径都很重要 - 如果没有粘性亲和力，负载最小的内部策略会将重新启动的会话置于*不同的* shard 上
/// ，而之前的 `WebRtcCore` 状态仍保留在原始状态上。
///
/// 内存以 `cache_capacity` 为界；
/// 一旦达到上限，条目就会通过简单的 FIFO 环被驱逐。
/// 默认上限（来自 driver 配置的 4 × `max_sessions`）保留了最近会话的长期历史记录，而没有无限制的增长。
///
/// # 配方：平衡粘性
///
/// 要将“负载感知初始放置”与“稳定所有者”结合起来，请将 [`LeastLoadedShardStrategy`] 包装在 [`StickyHashShardStrategy`] 中：
///
/// ```ignore use std::sync::Arc; use cheetah_webrtc_driver_tokio::{ LeastLoadedShardStrategy, ShardSelector, StickyHashShardStrategy, }; let strategy = Arc::new(StickyHashShardStrategy::new( Arc::new(LeastLoadedShardStrategy), 8_192, )); let selector = ShardSelector::with_strategy(4, strategy); ```
///
/// 新会话降落在最空的 shard 上；
/// ICE 重新启动，其他重新选择路径仍解析为最初选择的 shard，因此 `WebRtcCore` 状态永远不会孤立。
pub struct StickyHashShardStrategy {
    inner: Arc<dyn ShardSelectorStrategy>,
    cache: Mutex<StickyCache>,
    cache_capacity: usize,
}

#[derive(Debug, Default)]
struct StickyCache {
    map: HashMap<WebRtcSessionId, ShardId>,
    order: std::collections::VecDeque<WebRtcSessionId>,
}

impl StickyHashShardStrategy {
    /// Build a sticky strategy wrapping the given inner strategy.
    /// `cache_capacity` bounds the affinity table so a long-running
    /// driver doesn't grow unboundedly.
    ///
    /// 构建一个包含给定内部策略的粘性策略。
    /// `cache_capacity` 限制了关联表，因此长时间运行的 driver 不会无限增长。
    pub fn new(inner: Arc<dyn ShardSelectorStrategy>, cache_capacity: usize) -> Self {
        Self {
            inner,
            cache: Mutex::new(StickyCache::default()),
            cache_capacity: cache_capacity.max(1),
        }
    }

    /// Convenience constructor: sticky over the default hash
    /// strategy, with a 16k cache.
    ///
    /// 方便的构造函数：粘在默认的哈希策略上，具有 16k 缓存。
    pub fn with_hash_default() -> Self {
        Self::new(Arc::new(HashShardStrategy), 16_384)
    }

    /// Drop the cached binding for a session, e.g. after the session
    /// is closed and its id may be reused for a new session that
    /// should be free to land on any shard. Bounded by the cache
    /// `cache_capacity`, so calling `forget` on every close is also
    /// safe but not strictly required.
    ///
    /// 删除会话的缓存绑定，例如会话关闭后，其 id 可以重新用于新会话，该新会话应该可以自由登陆到任何 shard 上。
    /// 由于受到缓存 `cache_capacity` 的限制，因此每次关闭时调用 `forget` 也是安全的，但不是严格要求的。
    pub fn forget(&self, session_id: WebRtcSessionId) {
        let mut cache = self.cache.lock();
        if cache.map.remove(&session_id).is_some() {
            cache.order.retain(|id| *id != session_id);
        }
    }
}

impl ShardSelectorStrategy for StickyHashShardStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId {
        if shard_count <= 1 {
            return ShardId::new(0);
        }
        // Fast path: cached.
        {
            let cache = self.cache.lock();
            if let Some(shard) = cache.map.get(&session_id).copied() {
                return shard;
            }
        }
        // Fall through: ask the inner strategy and remember.
        let chosen = self.inner.pick(session_id, shard_count, loads);
        let mut cache = self.cache.lock();
        // Avoid reinserting if another caller raced us to it.
        if cache.map.contains_key(&session_id) {
            return *cache
                .map
                .get(&session_id)
                .expect("checked contains_key above");
        }
        if cache.map.len() >= self.cache_capacity {
            if let Some(oldest) = cache.order.pop_front() {
                cache.map.remove(&oldest);
            }
        }
        cache.map.insert(session_id, chosen);
        cache.order.push_back(session_id);
        chosen
    }
}

impl std::fmt::Debug for StickyHashShardStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StickyHashShardStrategy")
            .field("cache_capacity", &self.cache_capacity)
            .finish_non_exhaustive()
    }
}

/// Pre-composed `LeastLoadedShardStrategy` wrapped in
/// [`StickyHashShardStrategy`]: new sessions land on the emptiest
/// shard while ICE-restart and migration paths still resolve to the
/// session's original owner.
///
/// This is the recommended production strategy for clusters that
/// care about both peak balance and state stability. See the
/// [`StickyHashShardStrategy`] docs for the manual recipe; this type
/// is just a convenience wrapper so callers can write:
///
/// ```ignore
/// use cheetah_webrtc_driver_tokio::{BalancedStickyShardStrategy, ShardSelector};
/// let selector = ShardSelector::with_strategy(
///     4,
///     std::sync::Arc::new(BalancedStickyShardStrategy::new(8_192)),
/// );
/// ```
///
/// instead of constructing the inner / sticky pair by hand.
///
/// 预组合的 `LeastLoadedShardStrategy` 包裹在 [`StickyHashShardStrategy`] 中：新会话降落在最空的 shard 上
/// ，而 ICE- 重新启动和迁移路径仍解析为会话的原始所有者。
///
/// 对于同时关心峰值平衡和状态稳定性的集群，这是推荐的生产策略。
/// 有关手动配方，请参阅 [`StickyHashShardStrategy`] 文档；
/// 这种类型只是一个方便的包装器，因此调用者可以编写：
///
/// ```ignore use cheetah_webrtc_driver_tokio::{BalancedStickyShardStrategy, ShardSelector}; let selector = ShardSelector::with_strategy( 4, std::sync::Arc::new(BalancedStickyShardStrategy::new(8_192)), ); ```
///
/// 而不是手动构建内部/粘性对。
pub struct BalancedStickyShardStrategy {
    inner: StickyHashShardStrategy,
}

impl BalancedStickyShardStrategy {
    /// Build a balanced-sticky strategy with the given affinity
    /// cache capacity. The capacity bounds how many recently
    /// observed session ids the strategy remembers; setting it to
    /// `~4 × max_sessions` from the driver config is usually right.
    ///
    /// 使用给定的亲和性缓存容量构建平衡粘性策略。
    /// 容量限制了策略记住的最近观察到的会话 ID 的数量；
    /// 从 driver 配置将其设置为 `~4 × max_sessions` 通常是正确的。
    pub fn new(cache_capacity: usize) -> Self {
        Self {
            inner: StickyHashShardStrategy::new(Arc::new(LeastLoadedShardStrategy), cache_capacity),
        }
    }

    /// Default-cap convenience: 16k session affinity entries.
    ///
    /// 默认上限方便：16k 会话亲和性条目。
    pub fn with_default_capacity() -> Self {
        Self::new(16_384)
    }

    /// Drop the cached binding for a session — passes through to
    /// the inner sticky strategy.
    ///
    /// 删除会话的缓存绑定 — 传递到内部粘性策略。
    pub fn forget(&self, session_id: WebRtcSessionId) {
        self.inner.forget(session_id);
    }
}

impl ShardSelectorStrategy for BalancedStickyShardStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId {
        self.inner.pick(session_id, shard_count, loads)
    }
}

impl std::fmt::Debug for BalancedStickyShardStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BalancedStickyShardStrategy")
            .field("inner", &self.inner)
            .finish()
    }
}

/// Load-aware rebalance strategy.
///
/// Periodically refreshes a cached "preferred shard" per session
/// based on the most recent [`ShardLoadTable`] snapshot. Between
/// refreshes the strategy returns the cached pick (sticky), so an
/// in-flight ICE restart never lands on a different shard. After
/// `refresh_interval_ticks` calls the strategy re-evaluates the
/// inner strategy (typically `LeastLoadedShardStrategy`) and
/// updates the cached entry.
///
/// This is a lightweight alternative to a fully periodic
/// rebalancer: it does not migrate existing sessions, only steers
/// _new_ sessions toward the currently emptiest shard. Combine it
/// with [`StickyHashShardStrategy`] when stronger affinity is
/// required across the entire session lifetime.
///
/// 负载感知的重新平衡策略。
///
/// 根据最新的 [`ShardLoadTable`] 快照定期刷新每个会话缓存的“首选 shard”。
/// 在刷新之间，策略会返回缓存的选择（粘性），因此正在进行的 ICE 重新启动永远不会落在不同的 shard 上。
/// 在 `refresh_interval_ticks` 调用后，策略重新评估内部策略（通常是 `LeastLoadedShardStrategy`）并更新缓存的条目。
///
/// 这是完全定期重新平衡器的轻量级替代方案：它不会迁移现有会话，仅将 _new_ 会话引导至当前最空的 shard。
/// 当整个会话生命周期需要更强的亲和力时，将其与 [`StickyHashShardStrategy`] 结合使用。
pub struct LoadAwareRebalanceStrategy {
    inner: Arc<dyn ShardSelectorStrategy>,
    cache: Mutex<RebalanceCache>,
    cache_capacity: usize,
    refresh_interval_ticks: u32,
}

#[derive(Debug, Default)]
struct RebalanceCache {
    map: HashMap<WebRtcSessionId, RebalanceEntry>,
    order: std::collections::VecDeque<WebRtcSessionId>,
    /// Counter of pick calls; used to decide when to refresh.
    ///
    /// 接听电话柜台；
    /// 用于决定何时刷新。
    tick: u32,
}

#[derive(Debug, Clone, Copy)]
struct RebalanceEntry {
    shard: ShardId,
    /// `tick` at which this entry was last refreshed.
    ///
    /// 上次刷新该条目的 `tick` 。
    refreshed_at: u32,
}

impl LoadAwareRebalanceStrategy {
    /// Build a load-aware rebalance strategy wrapping `inner`.
    /// `cache_capacity` bounds the affinity table; `refresh_interval_ticks`
    /// controls how often (per-session) the cached pick is
    /// re-evaluated. A typical setup is `LeastLoadedShardStrategy`
    /// inner + 256 ticks refresh interval.
    ///
    /// 构建一个包装 `inner` 的负载感知再平衡策略。
    /// `cache_capacity` 限制关联表；
    /// `refresh_interval_ticks` 控制重新评估缓存选择的频率（每个会话）。
    /// 典型的设置是 `LeastLoadedShardStrategy` 内部 + 256 个刻度刷新间隔。
    pub fn new(
        inner: Arc<dyn ShardSelectorStrategy>,
        cache_capacity: usize,
        refresh_interval_ticks: u32,
    ) -> Self {
        Self {
            inner,
            cache: Mutex::new(RebalanceCache::default()),
            cache_capacity: cache_capacity.max(1),
            refresh_interval_ticks: refresh_interval_ticks.max(1),
        }
    }

    /// Convenience constructor: load-aware rebalance over
    /// least-loaded inner with an 8k cache and 256-tick refresh
    /// interval.
    ///
    /// 便捷的构造函数：通过 8k 缓存和 256 滴答刷新间隔对负载最少的内部进行负载感知重新平衡。
    pub fn with_least_loaded_default() -> Self {
        Self::new(Arc::new(LeastLoadedShardStrategy), 8_192, 256)
    }

    /// Drop the cached binding for a session.
    ///
    /// 删除会话的缓存绑定。
    pub fn forget(&self, session_id: WebRtcSessionId) {
        let mut cache = self.cache.lock();
        if cache.map.remove(&session_id).is_some() {
            cache.order.retain(|id| *id != session_id);
        }
    }
}

impl ShardSelectorStrategy for LoadAwareRebalanceStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId {
        if shard_count <= 1 {
            return ShardId::new(0);
        }
        let mut cache = self.cache.lock();
        cache.tick = cache.tick.wrapping_add(1);
        let now_tick = cache.tick;
        if let Some(entry) = cache.map.get(&session_id).copied() {
            // Refresh when enough ticks have elapsed since the
            // entry was last evaluated. Wraparound is treated as
            // a refresh because the load distribution might have
            // moved meaningfully across `u32::MAX` ticks.
            let elapsed = now_tick.wrapping_sub(entry.refreshed_at);
            if elapsed < self.refresh_interval_ticks {
                return entry.shard;
            }
            // Re-evaluate the inner strategy and update.
            drop(cache);
            let new_shard = self.inner.pick(session_id, shard_count, loads);
            let mut cache = self.cache.lock();
            cache.map.insert(
                session_id,
                RebalanceEntry {
                    shard: new_shard,
                    refreshed_at: now_tick,
                },
            );
            return new_shard;
        }
        // First pick: insert with tick = now and respect capacity.
        if cache.map.len() >= self.cache_capacity {
            if let Some(oldest) = cache.order.pop_front() {
                cache.map.remove(&oldest);
            }
        }
        drop(cache);
        let chosen = self.inner.pick(session_id, shard_count, loads);
        let mut cache = self.cache.lock();
        if let std::collections::hash_map::Entry::Vacant(e) = cache.map.entry(session_id) {
            e.insert(RebalanceEntry {
                shard: chosen,
                refreshed_at: now_tick,
            });
            cache.order.push_back(session_id);
        }
        chosen
    }
}

impl std::fmt::Debug for LoadAwareRebalanceStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadAwareRebalanceStrategy")
            .field("cache_capacity", &self.cache_capacity)
            .field("refresh_interval_ticks", &self.refresh_interval_ticks)
            .finish_non_exhaustive()
    }
}

/// Pre-composed strategy: sticky outer cache wrapping a
/// load-aware rebalance inner. Suitable for deployments that want
/// strong session-affinity for the first window of a session
/// lifetime (sticky) but still want long-running sessions to be
/// considered for rebalance after the cache TTL elapses
/// (load-aware refresh).
///
/// The outer `StickyHashShardStrategy` caches the first decision
/// per session id; on its first miss it asks the inner
/// `LoadAwareRebalanceStrategy`, which itself caches the pick for
/// `refresh_interval_ticks` calls before re-evaluating against the
/// current load distribution. This means:
///
/// * First call for a session: load-aware pick (least-loaded shard).
/// * Subsequent calls within the sticky cache TTL: identical pick
///   regardless of load fluctuation.
/// * After the operator calls [`Self::forget`] on a session id (e.g.
///   on hard reset), both the outer sticky and inner rebalance
///   caches are cleared, so the next call re-runs the inner
///   strategy fresh against current load.
///
/// Use [`Self::with_default_capacity`] for the recommended 16k /
/// 8k cache + 256-tick refresh defaults, or [`Self::new`] when
/// you want to tune the parameters.
///
/// 预组合策略：粘性外部缓存包装负载感知重新平衡内部。
/// 适合于需要在会话生命周期的第一个窗口具有强会话亲和性（粘性）但仍希望在缓存 TTL 失效后考虑重新平衡长时间运行的会话（负载感知刷新）的部署。
///
/// 外部 `StickyHashShardStrategy` 缓存每个会话 ID 的第一个决策；
/// 在第一次错过时，它会询问内部 `LoadAwareRebalanceStrategy`，内部 `LoadAwareRebalanceStrategy` 本身会缓存 `refresh_interval_ticks` 调用的选择
/// ，然后再根据当前负载分布进行重新评估。
/// 这意味着：
///
/// * 第一次调用会话：负载感知选择（负载最少的 shard）。
/// * 粘性缓存 TTL 内的后续调用：无论负载波动如何，都相同的选择。
/// * 操作员在会话 ID 上调用 [`Self::forget`] 后（例如，在硬重置时），外部粘性缓存和内部重新平衡缓存都被清除，因此下一次调用将针对当前负载重新运行新鲜的内部策略。
///
/// 使用 [`Self::with_default_capacity`] 作为推荐的 16k / 8k 缓存 + 256 滴答刷新默认值，或者当您想要调整参数时使用 [`Self::new`]。
pub struct StickyOverRebalanceStrategy {
    inner_sticky: StickyHashShardStrategy,
    inner_rebalance: Arc<LoadAwareRebalanceStrategy>,
}

impl StickyOverRebalanceStrategy {
    /// Build a sticky-over-rebalance strategy with explicit
    /// capacity / refresh parameters.
    ///
    /// 使用明确的容量/刷新参数构建粘性重新平衡策略。
    pub fn new(
        sticky_capacity: usize,
        rebalance_capacity: usize,
        refresh_interval_ticks: u32,
    ) -> Self {
        let inner_rebalance = Arc::new(LoadAwareRebalanceStrategy::new(
            Arc::new(LeastLoadedShardStrategy),
            rebalance_capacity,
            refresh_interval_ticks,
        ));
        let inner_sticky = StickyHashShardStrategy::new(
            inner_rebalance.clone() as Arc<dyn ShardSelectorStrategy>,
            sticky_capacity,
        );
        Self {
            inner_sticky,
            inner_rebalance,
        }
    }

    /// Default-cap convenience: 16k sticky cache wrapping 8k
    /// rebalance cache with 256-tick refresh interval. The same
    /// defaults as [`StickyHashShardStrategy::with_hash_default`]
    /// + [`LoadAwareRebalanceStrategy::with_least_loaded_default`].
    ///
    /// 默认上限便利性：16k 粘性缓存包装 8k 重新平衡缓存，刷新间隔为 256 个刻度。
    /// 默认值与 [`StickyHashShardStrategy::with_hash_default`] + [`LoadAwareRebalanceStrategy::with_least_loaded_default`] 相同。
    pub fn with_default_capacity() -> Self {
        Self::new(16_384, 8_192, 256)
    }

    /// Drop both the outer sticky binding and the inner rebalance
    /// cache entry for a session. The next pick goes through the
    /// inner strategy fresh against the current load distribution.
    ///
    /// 删除会话的外部粘性绑定和内部重新平衡缓存条目。
    /// 下一个选择将根据当前负载分布重新执行内部策略。
    pub fn forget(&self, session_id: WebRtcSessionId) {
        self.inner_sticky.forget(session_id);
        self.inner_rebalance.forget(session_id);
    }
}

impl ShardSelectorStrategy for StickyOverRebalanceStrategy {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId {
        self.inner_sticky.pick(session_id, shard_count, loads)
    }
}

impl std::fmt::Debug for StickyOverRebalanceStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StickyOverRebalanceStrategy")
            .field("sticky", &self.inner_sticky)
            .field("rebalance", &self.inner_rebalance)
            .finish()
    }
}

/// Selects a shard for a new session.
///
/// Wraps a [`ShardSelectorStrategy`] so production callers can swap
/// strategies without churning the driver. Cheap to clone — just an
/// `Arc<dyn>` and a `usize`.
///
/// 为新会话选择 shard。
///
/// 包装 [`ShardSelectorStrategy`]，以便生产调用者可以交换策略，而无需搅动 driver。
/// 克隆成本低——只需一个 `Arc<dyn>` 和一个 `usize`。
#[derive(Clone)]
pub struct ShardSelector {
    shard_count: usize,
    strategy: Arc<dyn ShardSelectorStrategy>,
}

impl std::fmt::Debug for ShardSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShardSelector")
            .field("shard_count", &self.shard_count)
            .finish_non_exhaustive()
    }
}

impl ShardSelector {
    /// Build a selector with the default hash strategy. The count
    /// must be `>= 1`; pass 1 for the single-shard topology.
    ///
    /// 使用默认哈希策略构建选择器。
    /// 计数必须为 `>= 1`；
    /// 对于单 shard 拓扑，传递 1。
    pub fn new(shard_count: usize) -> Self {
        Self::with_strategy(shard_count, Arc::new(HashShardStrategy))
    }

    /// Build a selector with a custom strategy.
    ///
    /// 使用自定义策略构建选择器。
    pub fn with_strategy(shard_count: usize, strategy: Arc<dyn ShardSelectorStrategy>) -> Self {
        Self {
            shard_count: shard_count.max(1),
            strategy,
        }
    }

    /// Configured number of shards, always at least one.
    ///
    /// 配置的 shards 数量，始终至少为 1。
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// Map a session id to a shard. The selector caches no state of
    /// its own; callers thread the load table when relevant.
    ///
    /// For the default hash strategy the load table is unused — the
    /// driver passes a reference to the live table for forward
    /// compatibility with `LeastLoadedShardStrategy`.
    ///
    /// 将会话 ID 映射到 shard。
    /// 选择器不缓存自己的状态；
    /// 调用者在相关时对加载表进行线程化。
    ///
    /// 对于默认哈希策略，未使用加载表 - driver 传递对活动表的引用以与 `LeastLoadedShardStrategy` 向前兼容。
    pub fn pick(&self, session_id: WebRtcSessionId, loads: &ShardLoadTable) -> ShardId {
        self.strategy.pick(session_id, self.shard_count, loads)
    }

    /// Convenience wrapper for callers that don't have a live load
    /// table (e.g. tests). The selector passes an empty table to the
    /// underlying strategy.
    ///
    /// 为没有实时加载表（例如测试）的调用者提供便利的包装。
    /// 选择器将一个空表传递给底层策略。
    pub fn pick_no_loads(&self, session_id: WebRtcSessionId) -> ShardId {
        let empty = ShardLoadTable::new(self.shard_count);
        self.strategy.pick(session_id, self.shard_count, &empty)
    }
}

/// Per-shard load counter. Updated whenever a session is registered
/// or forgotten via a `WebRtcDriverHandle`-owned shard. Surfaced via
/// `WebRtcDriverHandle::shard_stats`.
///
/// 每 shard 加载计数器。
/// 每当通过 `WebRtcDriverHandle` 拥有的 shard 注册或忘记会话时更新。
/// 通过 `WebRtcDriverHandle::shard_stats` 浮出水面。
#[derive(Debug, Default)]
pub struct ShardLoadTable {
    inner: Mutex<HashMap<ShardId, ShardLoad>>,
    shard_count: usize,
}

/// Per-shard load reported by [`ShardLoadTable::snapshot`]. Used by
/// custom [`ShardSelectorStrategy`] implementations and by
/// `WebRtcDriverHandle::shard_stats` to attribute active / stale
/// route counts to the shard that actually owns them.
///
/// [`ShardLoadTable::snapshot`] 报告的每 shard 负载。
/// 由自定义 [`ShardSelectorStrategy`] 实现和 `WebRtcDriverHandle::shard_stats` 使用，将活动/陈旧路由计数归因于实际拥有它们的 shard。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShardLoad {
    /// Number of sessions assigned to the shard.
    ///
    /// 分配给 shard 的会话数。
    pub session_count: usize,
    /// Number of remote addresses currently bound to a session on
    /// this shard. Updated by the shard's event loop whenever it
    /// (re)binds a session route. `0` in single-shard mode where
    /// route counts live on the global directory.
    ///
    /// 当前绑定到此 shard 上的会话的远程地址数。
    /// 每当（重新）绑定会话路由时，都会由 shard 的事件循环进行更新。
    /// `0` 处于 single-shard 模式，其中路由计数位于全局目录中。
    pub active_routes: usize,
    /// Number of stale (post-migration) addresses still resolvable
    /// on this shard. Same single-shard caveat as `active_routes`.
    ///
    /// 在此 shard 上仍可解析的过时（迁移后）地址数量。
    /// 与 `active_routes` 相同的单 shard 警告。
    pub stale_routes: usize,
}

impl ShardLoadTable {
    /// Create a load table pre-populated with default entries for each shard.
    ///
    /// 创建一个加载表，预先填充每个 shard 的默认条目。
    pub fn new(shard_count: usize) -> Self {
        let shard_count = shard_count.max(1);
        let mut inner = HashMap::with_capacity(shard_count);
        for i in 0..shard_count {
            inner.insert(ShardId::new(i), ShardLoad::default());
        }
        Self {
            inner: Mutex::new(inner),
            shard_count,
        }
    }

    #[allow(dead_code)]
    /// Number of shard slots in the table.
    ///
    /// 表中 shard 槽的数量。
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// Increment the session counter for a shard.
    ///
    /// 增加 shard 的会话计数器。
    pub fn record_session_added(&self, shard: ShardId) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.session_count = entry.session_count.saturating_add(1);
    }

    /// Decrement the session counter for a shard.
    ///
    /// 减少 shard 的会话计数器。
    pub fn record_session_removed(&self, shard: ShardId) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.session_count = entry.session_count.saturating_sub(1);
    }

    /// Update the route counters for a shard. Called by the shard's
    /// event loop after every route table mutation. Writers race
    /// across shards but each shard only writes its own entry, so
    /// contention is limited to the dashboard reader.
    ///
    /// 更新 shard 的路由计数器。
    /// 在每次路由表变更后由 shard 的事件循环调用。
    /// 写入者在 shards 之间竞争，但每个 shard 只写入自己的条目，因此争用仅限于仪表板读取器。
    pub fn record_route_counts(&self, shard: ShardId, active: usize, stale: usize) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.active_routes = active;
        entry.stale_routes = stale;
    }

    /// Snapshot per-shard load. Returned in shard-id order so
    /// dashboards can assume index `i` corresponds to `ShardId(i)`.
    ///
    /// 每个 shard 负载的快照。
    /// 以 shard-id 顺序返回，因此仪表板可以假设索引 `i` 对应于 `ShardId(i)`。
    pub fn snapshot(&self) -> Vec<(ShardId, ShardLoad)> {
        let guard = self.inner.lock();
        let mut entries: Vec<(ShardId, ShardLoad)> = (0..self.shard_count)
            .map(|i| {
                let id = ShardId::new(i);
                let load = guard.get(&id).copied().unwrap_or_default();
                (id, load)
            })
            .collect();
        entries.sort_by_key(|(id, _)| id.as_usize());
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_shard_always_returns_zero() {
        let s = ShardSelector::new(1);
        for i in 0..32 {
            assert_eq!(s.pick_no_loads(WebRtcSessionId::new(i)).as_usize(), 0);
        }
    }

    #[test]
    fn multi_shard_distributes_sequential_ids() {
        let s = ShardSelector::new(8);
        // 1024 sequential ids should land in every bucket at least
        // once; this is a soft distribution check, not a strict
        // uniformity test.
        let mut hits = [0usize; 8];
        for i in 0..1024u64 {
            hits[s.pick_no_loads(WebRtcSessionId::new(i)).as_usize()] += 1;
        }
        for (i, h) in hits.iter().enumerate() {
            assert!(*h > 0, "shard {i} got zero sessions");
        }
    }

    #[test]
    fn pick_is_deterministic_across_calls() {
        let s = ShardSelector::new(4);
        let id = WebRtcSessionId::new(42);
        let first = s.pick_no_loads(id);
        for _ in 0..16 {
            assert_eq!(s.pick_no_loads(id), first);
        }
    }

    #[test]
    fn load_table_tracks_session_count_per_shard() {
        let table = ShardLoadTable::new(3);
        table.record_session_added(ShardId::new(0));
        table.record_session_added(ShardId::new(0));
        table.record_session_added(ShardId::new(2));
        let snap = table.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].1.session_count, 2);
        assert_eq!(snap[1].1.session_count, 0);
        assert_eq!(snap[2].1.session_count, 1);

        table.record_session_removed(ShardId::new(0));
        let snap = table.snapshot();
        assert_eq!(snap[0].1.session_count, 1);
    }

    #[test]
    fn load_table_remove_below_zero_saturates() {
        let table = ShardLoadTable::new(2);
        table.record_session_removed(ShardId::new(1));
        let snap = table.snapshot();
        assert_eq!(snap[1].1.session_count, 0, "must saturate at zero");
    }

    #[test]
    fn least_loaded_strategy_picks_emptiest_shard() {
        let table = ShardLoadTable::new(4);
        // Shard 0 already has two sessions, shard 2 has one, shards
        // 1 and 3 are empty. The strategy must pick one of the empty
        // ones.
        table.record_session_added(ShardId::new(0));
        table.record_session_added(ShardId::new(0));
        table.record_session_added(ShardId::new(2));
        let s = ShardSelector::with_strategy(4, Arc::new(LeastLoadedShardStrategy));
        let pick = s.pick(WebRtcSessionId::new(7), &table);
        assert!(
            pick == ShardId::new(1) || pick == ShardId::new(3),
            "expected emptiest shard, got {pick:?}"
        );
    }

    #[test]
    fn least_loaded_strategy_breaks_ties_deterministically() {
        let table = ShardLoadTable::new(4);
        // All shards empty — falls back to the hash. The strategy
        // must agree with itself for the same id across repeats.
        let s = ShardSelector::with_strategy(4, Arc::new(LeastLoadedShardStrategy));
        let id = WebRtcSessionId::new(99);
        let first = s.pick(id, &table);
        for _ in 0..8 {
            assert_eq!(s.pick(id, &table), first);
        }
    }

    #[test]
    fn least_loaded_strategy_collapses_to_zero_for_single_shard() {
        let table = ShardLoadTable::new(1);
        let s = ShardSelector::with_strategy(1, Arc::new(LeastLoadedShardStrategy));
        for i in 0..16 {
            assert_eq!(s.pick(WebRtcSessionId::new(i), &table).as_usize(), 0);
        }
    }

    #[test]
    fn sticky_strategy_remembers_first_decision() {
        // Wrap a least-loaded strategy with sticky affinity. The
        // first call decides; subsequent calls return the same shard
        // even when the underlying load distribution flips.
        let table = ShardLoadTable::new(4);
        let strat = Arc::new(StickyHashShardStrategy::new(
            Arc::new(LeastLoadedShardStrategy),
            16,
        ));
        let s = ShardSelector::with_strategy(4, strat);
        let id = WebRtcSessionId::new(42);
        let first = s.pick(id, &table);
        // Flip the load distribution by adding many sessions to the
        // first pick's shard. Without stickiness, least-loaded would
        // pick a different shard next time.
        for _ in 0..16 {
            table.record_session_added(first);
        }
        for _ in 0..8 {
            assert_eq!(
                s.pick(id, &table),
                first,
                "sticky strategy must keep id pinned to first shard"
            );
        }
    }

    #[test]
    fn sticky_strategy_forget_releases_binding() {
        // After `forget(id)` the strategy is free to re-pick. We
        // arrange the load so the new pick is provably different.
        let table = ShardLoadTable::new(2);
        let strat = StickyHashShardStrategy::new(Arc::new(LeastLoadedShardStrategy), 4);
        let id = WebRtcSessionId::new(7);
        let first = strat.pick(id, 2, &table);
        // Saturate the first shard so least-loaded would pick the
        // other one going forward.
        for _ in 0..4 {
            table.record_session_added(first);
        }
        strat.forget(id);
        let second = strat.pick(id, 2, &table);
        assert_ne!(
            first, second,
            "after forget, least-loaded must avoid the saturated shard"
        );
    }

    #[test]
    fn sticky_strategy_evicts_oldest_when_at_capacity() {
        // Cache cap = 2; insert 3 ids and verify the first id is
        // evicted (its next pick can land anywhere).
        let table = ShardLoadTable::new(4);
        let strat = StickyHashShardStrategy::new(Arc::new(HashShardStrategy), 2);
        let a = WebRtcSessionId::new(1);
        let b = WebRtcSessionId::new(2);
        let c = WebRtcSessionId::new(3);
        let _ = strat.pick(a, 4, &table);
        let _ = strat.pick(b, 4, &table);
        let _ = strat.pick(c, 4, &table); // evicts `a`
                                          // Re-picking `a` is allowed to land on any shard; we just
                                          // assert the cache is now bounded.
        let _ = strat.pick(a, 4, &table);
        let cache = strat.cache.lock();
        assert!(
            cache.map.len() <= 2,
            "cache must respect capacity, saw {}",
            cache.map.len()
        );
    }

    #[test]
    fn sticky_strategy_passes_through_single_shard() {
        let table = ShardLoadTable::new(1);
        let strat = StickyHashShardStrategy::new(Arc::new(LeastLoadedShardStrategy), 4);
        for i in 0..8 {
            assert_eq!(strat.pick(WebRtcSessionId::new(i), 1, &table).as_usize(), 0);
        }
    }

    #[test]
    fn route_counts_track_shard_locally() {
        let table = ShardLoadTable::new(3);
        table.record_route_counts(ShardId::new(1), 5, 2);
        let snap = table.snapshot();
        assert_eq!(snap[0].1.active_routes, 0);
        assert_eq!(snap[0].1.stale_routes, 0);
        assert_eq!(snap[1].1.active_routes, 5);
        assert_eq!(snap[1].1.stale_routes, 2);
        assert_eq!(snap[2].1.active_routes, 0);
        // Update an existing entry.
        table.record_route_counts(ShardId::new(1), 7, 0);
        let snap = table.snapshot();
        assert_eq!(snap[1].1.active_routes, 7);
        assert_eq!(snap[1].1.stale_routes, 0);
    }

    #[test]
    fn balanced_sticky_strategy_picks_emptiest_then_pins() {
        // BalancedStickyShardStrategy should:
        // 1. Initially pick the emptiest shard (least-loaded inner).
        // 2. Pin the session to that shard even after the load
        //    distribution flips.
        let table = ShardLoadTable::new(4);
        // Saturate shard 0 so least-loaded prefers 1, 2, or 3.
        for _ in 0..16 {
            table.record_session_added(ShardId::new(0));
        }
        let strat = BalancedStickyShardStrategy::with_default_capacity();
        let id = WebRtcSessionId::new(7);
        let first = strat.pick(id, 4, &table);
        assert_ne!(
            first,
            ShardId::new(0),
            "balanced-sticky must avoid the saturated shard initially"
        );
        // Now saturate the chosen shard and verify stickiness.
        for _ in 0..32 {
            table.record_session_added(first);
        }
        for _ in 0..8 {
            assert_eq!(
                strat.pick(id, 4, &table),
                first,
                "balanced-sticky must pin id to the originally chosen shard"
            );
        }
    }

    #[test]
    fn balanced_sticky_forget_re_picks_when_load_changes() {
        let table = ShardLoadTable::new(2);
        let strat = BalancedStickyShardStrategy::new(4);
        let id = WebRtcSessionId::new(11);
        let first = strat.pick(id, 2, &table);
        // Saturate the first pick so least-loaded would now prefer
        // the other shard.
        for _ in 0..8 {
            table.record_session_added(first);
        }
        strat.forget(id);
        let second = strat.pick(id, 2, &table);
        assert_ne!(
            first, second,
            "after forget, balanced-sticky must avoid the saturated shard"
        );
    }

    #[test]
    fn load_aware_rebalance_picks_emptiest_initially_and_caches() {
        let table = ShardLoadTable::new(4);
        for _ in 0..8 {
            table.record_session_added(ShardId::new(0));
        }
        let strat = LoadAwareRebalanceStrategy::new(
            Arc::new(LeastLoadedShardStrategy),
            32,
            8, // refresh every 8 ticks
        );
        let id = WebRtcSessionId::new(7);
        let first = strat.pick(id, 4, &table);
        assert_ne!(
            first,
            ShardId::new(0),
            "initial pick must avoid the saturated shard"
        );
        // Within the refresh window the cached pick is sticky,
        // even when load distribution flips.
        for _ in 0..32 {
            table.record_session_added(first);
        }
        let cached = strat.pick(id, 4, &table);
        assert_eq!(
            cached, first,
            "within refresh window, cached pick is reused"
        );
    }

    #[test]
    fn load_aware_rebalance_refreshes_after_interval() {
        let table = ShardLoadTable::new(2);
        let strat = LoadAwareRebalanceStrategy::new(
            Arc::new(LeastLoadedShardStrategy),
            16,
            4, // refresh every 4 ticks
        );
        let id = WebRtcSessionId::new(11);
        // Saturate shard 1 so the *first* pick lands on shard 0.
        for _ in 0..8 {
            table.record_session_added(ShardId::new(1));
        }
        let first = strat.pick(id, 2, &table);
        assert_eq!(first, ShardId::new(0));
        // Now flip the load: saturate shard 0 instead.
        // Reset the load table by removing what we added earlier
        // and re-adding to shard 0 so least-loaded would now
        // prefer shard 1.
        for _ in 0..8 {
            table.record_session_removed(ShardId::new(1));
        }
        for _ in 0..8 {
            table.record_session_added(ShardId::new(0));
        }
        // Burn ticks until refresh fires. Each call to pick
        // increments the tick counter.
        for _ in 0..4 {
            strat.pick(id, 2, &table);
        }
        let after = strat.pick(id, 2, &table);
        assert_eq!(
            after,
            ShardId::new(1),
            "after refresh interval, strategy must re-evaluate to new least-loaded shard"
        );
    }

    #[test]
    fn load_aware_rebalance_passes_through_single_shard() {
        let table = ShardLoadTable::new(1);
        let strat = LoadAwareRebalanceStrategy::with_least_loaded_default();
        for i in 0..8 {
            assert_eq!(strat.pick(WebRtcSessionId::new(i), 1, &table).as_usize(), 0);
        }
    }

    #[test]
    fn load_aware_rebalance_forget_releases_binding() {
        let table = ShardLoadTable::new(2);
        let strat = LoadAwareRebalanceStrategy::new(Arc::new(LeastLoadedShardStrategy), 4, 1024);
        let id = WebRtcSessionId::new(99);
        let first = strat.pick(id, 2, &table);
        for _ in 0..16 {
            table.record_session_added(first);
        }
        strat.forget(id);
        let second = strat.pick(id, 2, &table);
        assert_ne!(
            first, second,
            "after forget, fresh pick reflects current load"
        );
    }

    #[test]
    fn sticky_over_rebalance_pins_after_first_pick() {
        // Sticky outer cache pins the first decision; even though
        // the inner load-aware strategy would re-evaluate after
        // its refresh window, the outer cache holds the line.
        let table = ShardLoadTable::new(4);
        for _ in 0..8 {
            table.record_session_added(ShardId::new(0));
        }
        let strat = StickyOverRebalanceStrategy::with_default_capacity();
        let id = WebRtcSessionId::new(13);
        let first = strat.pick(id, 4, &table);
        assert_ne!(
            first,
            ShardId::new(0),
            "initial pick must avoid the saturated shard"
        );
        // Saturate the chosen shard; outer sticky should still pin.
        for _ in 0..32 {
            table.record_session_added(first);
        }
        for _ in 0..1024 {
            assert_eq!(
                strat.pick(id, 4, &table),
                first,
                "outer sticky cache must keep id pinned across many calls"
            );
        }
    }

    #[test]
    fn sticky_over_rebalance_forget_re_picks() {
        let table = ShardLoadTable::new(2);
        let strat = StickyOverRebalanceStrategy::new(8, 8, 128);
        let id = WebRtcSessionId::new(21);
        let first = strat.pick(id, 2, &table);
        for _ in 0..16 {
            table.record_session_added(first);
        }
        strat.forget(id);
        let second = strat.pick(id, 2, &table);
        assert_ne!(first, second, "forget releases sticky binding");
    }

    #[test]
    fn sticky_over_rebalance_passes_through_single_shard() {
        let table = ShardLoadTable::new(1);
        let strat = StickyOverRebalanceStrategy::with_default_capacity();
        for i in 0..8 {
            assert_eq!(strat.pick(WebRtcSessionId::new(i), 1, &table).as_usize(), 0);
        }
    }
}
