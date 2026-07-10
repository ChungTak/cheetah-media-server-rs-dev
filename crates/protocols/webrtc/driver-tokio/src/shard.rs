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

use std::collections::HashMap;
use std::sync::Arc;

use cheetah_webrtc_core::WebRtcSessionId;
use parking_lot::Mutex;

use crate::directory::ShardId;

/// Strategy that maps a session to a shard.
///
/// Implementations should be cheap to call: the driver may invoke
/// `pick` on every accepted session and on every migration.
pub trait ShardSelectorStrategy: Send + Sync {
    fn pick(
        &self,
        session_id: WebRtcSessionId,
        shard_count: usize,
        loads: &ShardLoadTable,
    ) -> ShardId;
}

/// Hash-based strategy. Splitmix64 fold of the session id mod
/// `shard_count`. Stable across calls for a given id.
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
pub struct StickyHashShardStrategy {
    /// `inner` field.
    /// `inner` 字段.
    inner: Arc<dyn ShardSelectorStrategy>,
    /// `cache` field.
    /// `cache` 字段.
    cache: Mutex<StickyCache>,
    /// `cache_capacity` field of type `usize`.
    /// `cache_capacity` 字段，类型为 `usize`.
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
    pub fn new(inner: Arc<dyn ShardSelectorStrategy>, cache_capacity: usize) -> Self {
        Self {
            inner,
            cache: Mutex::new(StickyCache::default()),
            cache_capacity: cache_capacity.max(1),
        }
    }

    /// Convenience constructor: sticky over the default hash
    /// strategy, with a 16k cache.
    pub fn with_hash_default() -> Self {
        Self::new(Arc::new(HashShardStrategy), 16_384)
    }

    /// Drop the cached binding for a session, e.g. after the session
    /// is closed and its id may be reused for a new session that
    /// should be free to land on any shard. Bounded by the cache
    /// `cache_capacity`, so calling `forget` on every close is also
    /// safe but not strictly required.
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
pub struct BalancedStickyShardStrategy {
    /// `inner` field of type `StickyHashShardStrategy`.
    /// `inner` 字段，类型为 `StickyHashShardStrategy`.
    inner: StickyHashShardStrategy,
}

impl BalancedStickyShardStrategy {
    /// Build a balanced-sticky strategy with the given affinity
    /// cache capacity. The capacity bounds how many recently
    /// observed session ids the strategy remembers; setting it to
    /// `~4 × max_sessions` from the driver config is usually right.
    pub fn new(cache_capacity: usize) -> Self {
        Self {
            inner: StickyHashShardStrategy::new(Arc::new(LeastLoadedShardStrategy), cache_capacity),
        }
    }

    /// Default-cap convenience: 16k session affinity entries.
    pub fn with_default_capacity() -> Self {
        Self::new(16_384)
    }

    /// Drop the cached binding for a session — passes through to
    /// the inner sticky strategy.
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
pub struct LoadAwareRebalanceStrategy {
    /// `inner` field.
    /// `inner` 字段.
    inner: Arc<dyn ShardSelectorStrategy>,
    /// `cache` field.
    /// `cache` 字段.
    cache: Mutex<RebalanceCache>,
    /// `cache_capacity` field of type `usize`.
    /// `cache_capacity` 字段，类型为 `usize`.
    cache_capacity: usize,
    /// `refresh_interval_ticks` field of type `u32`.
    /// `refresh_interval_ticks` 字段，类型为 `u32`.
    refresh_interval_ticks: u32,
}

#[derive(Debug, Default)]
struct RebalanceCache {
    map: HashMap<WebRtcSessionId, RebalanceEntry>,
    order: std::collections::VecDeque<WebRtcSessionId>,
    /// Counter of pick calls; used to decide when to refresh.
    tick: u32,
}

#[derive(Debug, Clone, Copy)]
struct RebalanceEntry {
    shard: ShardId,
    /// `tick` at which this entry was last refreshed.
    refreshed_at: u32,
}

impl LoadAwareRebalanceStrategy {
    /// Build a load-aware rebalance strategy wrapping `inner`.
    /// `cache_capacity` bounds the affinity table; `refresh_interval_ticks`
    /// controls how often (per-session) the cached pick is
    /// re-evaluated. A typical setup is `LeastLoadedShardStrategy`
    /// inner + 256 ticks refresh interval.
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
    pub fn with_least_loaded_default() -> Self {
        Self::new(Arc::new(LeastLoadedShardStrategy), 8_192, 256)
    }

    /// Drop the cached binding for a session.
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
pub struct StickyOverRebalanceStrategy {
    /// `inner_sticky` field of type `StickyHashShardStrategy`.
    /// `inner_sticky` 字段，类型为 `StickyHashShardStrategy`.
    inner_sticky: StickyHashShardStrategy,
    /// `inner_rebalance` field.
    /// `inner_rebalance` 字段.
    inner_rebalance: Arc<LoadAwareRebalanceStrategy>,
}

impl StickyOverRebalanceStrategy {
    /// Build a sticky-over-rebalance strategy with explicit
    /// capacity / refresh parameters.
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
    pub fn with_default_capacity() -> Self {
        Self::new(16_384, 8_192, 256)
    }

    /// Drop both the outer sticky binding and the inner rebalance
    /// cache entry for a session. The next pick goes through the
    /// inner strategy fresh against the current load distribution.
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
#[derive(Clone)]
pub struct ShardSelector {
    /// `shard_count` field of type `usize`.
    /// `shard_count` 字段，类型为 `usize`.
    shard_count: usize,
    /// `strategy` field.
    /// `strategy` 字段.
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
    pub fn new(shard_count: usize) -> Self {
        Self::with_strategy(shard_count, Arc::new(HashShardStrategy))
    }

    /// Build a selector with a custom strategy.
    pub fn with_strategy(shard_count: usize, strategy: Arc<dyn ShardSelectorStrategy>) -> Self {
        Self {
            shard_count: shard_count.max(1),
            strategy,
        }
    }

    /// `shard_count` function.
    /// `shard_count` 函数.
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// Map a session id to a shard. The selector caches no state of
    /// its own; callers thread the load table when relevant.
    ///
    /// For the default hash strategy the load table is unused — the
    /// driver passes a reference to the live table for forward
    /// compatibility with `LeastLoadedShardStrategy`.
    pub fn pick(&self, session_id: WebRtcSessionId, loads: &ShardLoadTable) -> ShardId {
        self.strategy.pick(session_id, self.shard_count, loads)
    }

    /// Convenience wrapper for callers that don't have a live load
    /// table (e.g. tests). The selector passes an empty table to the
    /// underlying strategy.
    pub fn pick_no_loads(&self, session_id: WebRtcSessionId) -> ShardId {
        let empty = ShardLoadTable::new(self.shard_count);
        self.strategy.pick(session_id, self.shard_count, &empty)
    }
}

/// Per-shard load counter. Updated whenever a session is registered
/// or forgotten via a `WebRtcDriverHandle`-owned shard. Surfaced via
/// `WebRtcDriverHandle::shard_stats`.
#[derive(Debug, Default)]
pub struct ShardLoadTable {
    /// `inner` field.
    /// `inner` 字段.
    inner: Mutex<HashMap<ShardId, ShardLoad>>,
    /// `shard_count` field of type `usize`.
    /// `shard_count` 字段，类型为 `usize`.
    shard_count: usize,
}

/// Per-shard load reported by [`ShardLoadTable::snapshot`]. Used by
/// custom [`ShardSelectorStrategy`] implementations and by
/// `WebRtcDriverHandle::shard_stats` to attribute active / stale
/// route counts to the shard that actually owns them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShardLoad {
    /// `session_count` field of type `usize`.
    /// `session_count` 字段，类型为 `usize`.
    pub session_count: usize,
    /// Number of remote addresses currently bound to a session on
    /// this shard. Updated by the shard's event loop whenever it
    /// (re)binds a session route. `0` in single-shard mode where
    /// route counts live on the global directory.
    pub active_routes: usize,
    /// Number of stale (post-migration) addresses still resolvable
    /// on this shard. Same single-shard caveat as `active_routes`.
    pub stale_routes: usize,
}

impl ShardLoadTable {
    /// Creates a new instance.
    /// 创建 新的 实例.
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

    /// `shard_count` function.
    /// `shard_count` 函数.
    #[allow(dead_code)]
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// `record_session_added` function.
    /// `record_session_added` 函数.
    pub fn record_session_added(&self, shard: ShardId) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.session_count = entry.session_count.saturating_add(1);
    }

    /// `record_session_removed` function.
    /// `record_session_removed` 函数.
    pub fn record_session_removed(&self, shard: ShardId) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.session_count = entry.session_count.saturating_sub(1);
    }

    /// Update the route counters for a shard. Called by the shard's
    /// event loop after every route table mutation. Writers race
    /// across shards but each shard only writes its own entry, so
    /// contention is limited to the dashboard reader.
    pub fn record_route_counts(&self, shard: ShardId, active: usize, stale: usize) {
        let mut guard = self.inner.lock();
        let entry = guard.entry(shard).or_default();
        entry.active_routes = active;
        entry.stale_routes = stale;
    }

    /// Snapshot per-shard load. Returned in shard-id order so
    /// dashboards can assume index `i` corresponds to `ShardId(i)`.
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
