//! Single-port UDP routing table.
//!
//! Maps remote `SocketAddr` to session id with bounded eviction. The table
//! is rebuilt as `WebRtcCore` accepts packets, so we keep it small and
//! transparent — no fancy LRU, just a `HashMap` with a soft cap and the
//! same address replacing entries when a session migrates.
//!
//! 单端口 UDP 路由表。
//!
//! 通过有界驱逐将远程 `SocketAddr` 映射到会话 id。
//! 该表是在 `WebRtcCore` 接受数据包时重建的，因此我们保持它小而透明 - 没有花哨的 LRU，只是一个具有软上限的 `HashMap` 以及会话迁移时替换条目的相同地址。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use cheetah_webrtc_core::WebRtcSessionId;

use crate::migration::RouteCandidateDiff;
/// Per-shard active and stale address-to-session routing table.
///
/// Per-shard 活动和过时的地址到会话路由表。
#[derive(Default, Debug)]
pub(crate) struct RouteTable {
    by_addr: HashMap<SocketAddr, WebRtcSessionId>,
    /// Stale routes kept around for a short TTL during connection migration.
    /// Whenever a session re-binds to a new address we move the old entry
    /// here so reorderd packets from the old path do not panic the
    /// `WebRtcCore` (it just rejects them).
    ///
    /// 在连接迁移期间，陈旧路由会短暂保留 TTL。
    /// 每当会话重新绑定到新地址时，我们都会将旧条目移至此处，以便来自旧路径的重新排序的数据包不会引起 `WebRtcCore` 恐慌（它只是拒绝它们）。
    stale: HashMap<SocketAddr, (WebRtcSessionId, std::time::Instant)>,
    soft_cap: usize,
    stale_ttl: Duration,
}

impl RouteTable {
    /// Create an empty route table with soft/hard capacity and stale TTL.
    ///
    /// 创建一个具有软/硬容量和陈旧 TTL 的空路由表。
    pub(crate) fn new(soft_cap: usize, stale_ttl: Duration) -> Self {
        Self {
            by_addr: HashMap::new(),
            stale: HashMap::new(),
            soft_cap,
            stale_ttl,
        }
    }

    /// Resolve an address to a session, consulting the active table then stale.
    ///
    /// 解析会话的地址，查询活动表然后陈旧。
    pub(crate) fn lookup(&self, addr: &SocketAddr) -> Option<WebRtcSessionId> {
        self.by_addr
            .get(addr)
            .copied()
            .or_else(|| self.stale.get(addr).map(|(id, _)| *id))
    }

    /// Bind a remote address to a session and return the prior binding diff.
    ///
    /// 将远程地址绑定到会话并返回先前的绑定差异。
    pub(crate) fn bind(
        &mut self,
        addr: SocketAddr,
        session: WebRtcSessionId,
        now: std::time::Instant,
    ) -> (Option<WebRtcSessionId>, RouteCandidateDiff) {
        // Evict over-capacity entries if we are about to grow past the
        // soft cap. The cap is a soft bound; we only ever drop a single
        // entry per insert to avoid pathological cleanup latency.
        if self.by_addr.len() >= self.soft_cap.saturating_mul(2) {
            self.compact(now);
        }
        let prior = self.by_addr.insert(addr, session);
        let diff = match prior {
            None => RouteCandidateDiff {
                added: vec![addr],
                removed: Vec::new(),
                stale: Vec::new(),
            },
            Some(prev) if prev == session => {
                // Idempotent rebind: no observable change.
                RouteCandidateDiff::default()
            }
            Some(prev) => {
                // Active address moved from a different session to
                // this one. Track the old binding as stale so
                // straggler packets can still resolve briefly.
                self.stale.insert(addr, (prev, now));
                RouteCandidateDiff {
                    added: vec![addr],
                    removed: vec![addr],
                    stale: vec![addr],
                }
            }
        };
        (prior, diff)
    }

    /// Attempt a migration bind, but reject if the active route table
    /// is at hard capacity (`hard_cap = soft_cap * 4`). Used for
    /// migration-detected packets where rebinding could push the table
    /// past safe limits. Returns `Ok((prior, diff))` on success or
    /// `Err(())` if migration was rejected.
    ///
    /// 尝试迁移绑定，但如果活动路由表处于硬容量 (`hard_cap = soft_cap * 4`)，则拒绝。
    /// 用于迁移检测到的数据包，其中重新绑定可能会使表超出安全限制。
    /// 如果成功，则返回 `Ok((prior, diff))`；
    /// 如果迁移被拒绝，则返回 `Err(())`。
    pub(crate) fn try_bind_migration(
        &mut self,
        addr: SocketAddr,
        session: WebRtcSessionId,
        now: std::time::Instant,
    ) -> Result<(Option<WebRtcSessionId>, RouteCandidateDiff), ()> {
        let hard_cap = self.soft_cap.saturating_mul(4);
        // If the address is already bound to this session, migration
        // is a no-op — always allow.
        if self.by_addr.get(&addr) == Some(&session) {
            let prior = self.by_addr.insert(addr, session);
            return Ok((prior, RouteCandidateDiff::default()));
        }
        // Try to free space by compacting first.
        if self.by_addr.len() >= hard_cap {
            self.compact(now);
        }
        if self.by_addr.len() >= hard_cap {
            return Err(());
        }
        Ok(self.bind(addr, session, now))
    }

    /// Remove all active and stale bindings for a session.
    ///
    /// 删除会话的所有活动和陈旧绑定。
    pub(crate) fn forget_session(&mut self, session: WebRtcSessionId) -> RouteCandidateDiff {
        let mut removed = Vec::new();
        let mut stale_addrs = Vec::new();
        self.by_addr.retain(|addr, sid| {
            if *sid == session {
                removed.push(*addr);
                false
            } else {
                true
            }
        });
        self.stale.retain(|addr, (sid, _)| {
            if *sid == session {
                stale_addrs.push(*addr);
                false
            } else {
                true
            }
        });
        RouteCandidateDiff {
            added: Vec::new(),
            removed,
            stale: stale_addrs,
        }
    }

    /// Move the binding at `addr` (if any) into the stale set so
    /// straggler packets from that address still resolve to the same
    /// session for `stale_ttl` and then expire. Used when a session
    /// migrates to a new remote address: the old address binding must
    /// not stay active in the primary table because new sessions might
    /// reuse it.
    ///
    /// 将 `addr` 处的绑定（如果有）移至过时集中，以便来自该地址的落后数据包仍解析为 `stale_ttl` 的同一会话，然后过期。
    /// 当会话迁移到新的远程地址时使用：旧地址绑定不得在主表中保持活动状态，因为新会话可能会重用它。
    pub(crate) fn unbind_address(
        &mut self,
        addr: &SocketAddr,
        now: std::time::Instant,
    ) -> RouteCandidateDiff {
        if let Some(prev) = self.by_addr.remove(addr) {
            self.stale.insert(*addr, (prev, now));
            RouteCandidateDiff {
                added: Vec::new(),
                removed: vec![*addr],
                stale: vec![*addr],
            }
        } else {
            RouteCandidateDiff::default()
        }
    }

    /// Drop stale entries whose TTL has elapsed.
    ///
    /// 删除 TTL 已过期的过时条目。
    pub(crate) fn compact(&mut self, now: std::time::Instant) {
        self.stale
            .retain(|_, (_, recorded)| now.duration_since(*recorded) < self.stale_ttl);
    }

    /// Compact stale routes and return the list of expired entries.
    /// Used by the driver to emit `RouteExpired` diagnostics.
    ///
    /// 压缩陈旧路由并返回过期条目列表。
    /// 由 driver 用于发出 `RouteExpired` 诊断信息。
    pub(crate) fn compact_expired(
        &mut self,
        now: std::time::Instant,
    ) -> Vec<(SocketAddr, WebRtcSessionId)> {
        let mut expired = Vec::new();
        self.stale.retain(|addr, (sid, recorded)| {
            if now.duration_since(*recorded) >= self.stale_ttl {
                expired.push((*addr, *sid));
                false
            } else {
                true
            }
        });
        expired
    }

    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.by_addr.len()
    }

    /// Snapshot the route table's active and stale counts. Used by
    /// the shard event loop to publish per-shard metrics through
    /// `ShardLoadTable::record_route_counts`.
    ///
    /// 快照路由表的活动和过时计数。
    /// 由 shard 事件循环使用，通过 `ShardLoadTable::record_route_counts` 发布每个 shard 指标。
    pub(crate) fn route_counts(&self) -> (usize, usize) {
        (self.by_addr.len(), self.stale.len())
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
    fn bind_and_lookup() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        assert_eq!(table.lookup(&addr(1000)), Some(WebRtcSessionId::new(1)));
    }

    #[test]
    fn rebind_moves_old_to_stale() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        let (prev, _diff) = table.bind(addr(1000), WebRtcSessionId::new(2), now);
        assert_eq!(prev, Some(WebRtcSessionId::new(1)));
        assert_eq!(table.lookup(&addr(1000)), Some(WebRtcSessionId::new(2)));
    }

    #[test]
    fn forget_session_removes_all_routes() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        table.bind(addr(2000), WebRtcSessionId::new(1), now);
        table.forget_session(WebRtcSessionId::new(1));
        assert!(table.lookup(&addr(1000)).is_none());
        assert!(table.lookup(&addr(2000)).is_none());
    }

    #[test]
    fn stale_routes_expire_after_ttl() {
        let mut table = RouteTable::new(8, Duration::from_millis(1));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        table.bind(addr(1000), WebRtcSessionId::new(2), now);
        std::thread::sleep(Duration::from_millis(5));
        table.compact(std::time::Instant::now());
        assert_eq!(table.lookup(&addr(1000)), Some(WebRtcSessionId::new(2)));
        assert!(table.stale.is_empty());
    }

    #[test]
    fn compact_expired_returns_expired_entries() {
        let mut table = RouteTable::new(8, Duration::from_millis(1));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        // Rebind to move session 1 to stale
        table.bind(addr(1000), WebRtcSessionId::new(2), now);
        std::thread::sleep(Duration::from_millis(5));
        let expired = table.compact_expired(std::time::Instant::now());
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], (addr(1000), WebRtcSessionId::new(1)));
        assert!(table.stale.is_empty());
    }

    #[test]
    fn compact_expired_does_not_return_fresh_entries() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        table.bind(addr(1000), WebRtcSessionId::new(2), now);
        let expired = table.compact_expired(now);
        assert!(expired.is_empty());
        // Stale entry should still be there
        assert_eq!(table.lookup(&addr(1000)), Some(WebRtcSessionId::new(2)));
    }

    #[test]
    fn try_bind_migration_succeeds_when_below_hard_cap() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(1), now);
        let result = table.try_bind_migration(addr(2000), WebRtcSessionId::new(1), now);
        assert!(result.is_ok());
        assert_eq!(table.lookup(&addr(2000)), Some(WebRtcSessionId::new(1)));
    }

    #[test]
    fn try_bind_migration_rejects_when_at_hard_cap() {
        // soft_cap = 2, hard_cap = soft_cap * 4 = 8
        let mut table = RouteTable::new(2, Duration::from_secs(30));
        let now = std::time::Instant::now();
        // Fill the table to hard_cap
        for i in 0..8u16 {
            table.bind(addr(1000 + i), WebRtcSessionId::new(i as u64 + 1), now);
        }
        // Attempt to migrate session 99 to a new address
        let result = table.try_bind_migration(addr(9000), WebRtcSessionId::new(99), now);
        assert!(result.is_err(), "migration should be rejected at hard cap");
    }

    #[test]
    fn try_bind_migration_allows_reaffirming_existing_binding() {
        // soft_cap = 2, hard_cap = 8
        let mut table = RouteTable::new(2, Duration::from_secs(30));
        let now = std::time::Instant::now();
        for i in 0..8u16 {
            table.bind(addr(1000 + i), WebRtcSessionId::new(i as u64 + 1), now);
        }
        // Re-binding the same session at its current address must
        // succeed even at hard cap (idempotent).
        let result = table.try_bind_migration(addr(1000), WebRtcSessionId::new(1), now);
        assert!(result.is_ok());
    }

    #[test]
    fn unbind_address_moves_route_to_stale() {
        // Simulates connection migration: a session previously at
        // addr(1000) moves to addr(2000). Packets that race the
        // migration on the old path still resolve for `stale_ttl`.
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        table.bind(addr(1000), WebRtcSessionId::new(7), now);
        table.unbind_address(&addr(1000), now);
        // No active route, but a stale route still resolves.
        assert!(!table.by_addr.contains_key(&addr(1000)));
        assert_eq!(table.lookup(&addr(1000)), Some(WebRtcSessionId::new(7)));
        // After TTL the stale entry expires.
        let later = now + Duration::from_secs(60);
        table.compact(later);
        assert!(table.lookup(&addr(1000)).is_none());
    }

    /// netem-style reorder simulation: packets from the migrating
    /// peer arrive *out of order* — the old-path tail packet lands
    /// after the new-path head packet. The route table must not
    /// resurrect the old binding when the stale tail arrives.
    #[test]
    fn reordered_old_path_packet_does_not_resurrect_active_binding() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let session = WebRtcSessionId::new(11);
        let t0 = std::time::Instant::now();

        // Initial bind on the old path.
        table.bind(addr(5000), session, t0);
        // Migration: new path becomes active, old path goes stale.
        let t1 = t0 + Duration::from_millis(50);
        table.unbind_address(&addr(5000), t1);
        table.bind(addr(6000), session, t1);

        // Reordered packet from old path arrives now: it must
        // resolve via the stale set (so we don't drop a legitimate
        // straggler) but the active binding must remain on the new
        // path.
        assert_eq!(table.lookup(&addr(5000)), Some(session));
        assert_eq!(table.lookup(&addr(6000)), Some(session));
        assert!(!table.by_addr.contains_key(&addr(5000)));
        assert_eq!(table.by_addr.get(&addr(6000)).copied(), Some(session));
    }

    /// netem-style packet-loss simulation: a session migration is
    /// followed by a long burst of "no packets" before the stale TTL
    /// expires. After expiry the route table must drop the stale
    /// entry so a *new* session that happens to come from the same
    /// remote address (post-NAT churn) does not get cross-routed
    /// onto the migrated session.
    #[test]
    fn stale_route_drops_after_loss_burst_then_new_session_binds_cleanly() {
        let stale_ttl = Duration::from_millis(100);
        let mut table = RouteTable::new(8, stale_ttl);
        let session_a = WebRtcSessionId::new(20);
        let session_b = WebRtcSessionId::new(21);
        let t0 = std::time::Instant::now();

        // Session A originally at addr(7000), then migrates.
        table.bind(addr(7000), session_a, t0);
        table.unbind_address(&addr(7000), t0);
        table.bind(addr(8000), session_a, t0);
        assert_eq!(table.lookup(&addr(7000)), Some(session_a));

        // Loss burst: nothing arrives at all for > stale_ttl.
        let t_after_loss = t0 + stale_ttl + Duration::from_millis(50);
        table.compact(t_after_loss);
        assert!(
            table.lookup(&addr(7000)).is_none(),
            "stale entry must expire after stale_ttl"
        );

        // A brand new session B happens to land on addr(7000) (NAT
        // re-binding, common after a long idle period). It must not
        // collide with the old session A's stale ghost.
        table.bind(addr(7000), session_b, t_after_loss);
        assert_eq!(table.lookup(&addr(7000)), Some(session_b));
        assert_eq!(table.lookup(&addr(8000)), Some(session_a));
    }

    #[test]
    fn bind_first_time_returns_diff_with_only_added() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        let a = addr(1000);
        let (prior, diff) = table.bind(a, WebRtcSessionId::new(1), now);
        assert_eq!(prior, None);
        assert_eq!(
            diff,
            RouteCandidateDiff {
                added: vec![a],
                removed: Vec::new(),
                stale: Vec::new(),
            }
        );
    }

    #[test]
    fn bind_overwrite_same_addr_different_session_returns_added_removed_stale() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        let a = addr(1000);
        let session1 = WebRtcSessionId::new(1);
        let session2 = WebRtcSessionId::new(2);
        let (_, _) = table.bind(a, session1, now);
        let (prior, diff) = table.bind(a, session2, now);
        assert_eq!(prior, Some(session1));
        assert_eq!(
            diff,
            RouteCandidateDiff {
                added: vec![a],
                removed: vec![a],
                stale: vec![a],
            }
        );
    }

    #[test]
    fn unbind_address_returns_removed_and_stale_for_target_addr() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        let a = addr(1000);
        table.bind(a, WebRtcSessionId::new(1), now);
        let diff = table.unbind_address(&a, now);
        assert_eq!(
            diff,
            RouteCandidateDiff {
                added: Vec::new(),
                removed: vec![a],
                stale: vec![a],
            }
        );

        // Calling unbind_address on an addr that is not bound returns
        // the default empty diff.
        let unbound = addr(2000);
        let diff_empty = table.unbind_address(&unbound, now);
        assert_eq!(diff_empty, RouteCandidateDiff::default());
    }

    #[test]
    fn try_bind_migration_success_returns_diff_added_for_new_addr() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        let session = WebRtcSessionId::new(1);
        let addr_a = addr(1000);
        let addr_b = addr(2000);
        table.bind(addr_a, session, now);
        let result = table.try_bind_migration(addr_b, session, now);
        assert_eq!(
            result,
            Ok((
                None,
                RouteCandidateDiff {
                    added: vec![addr_b],
                    removed: Vec::new(),
                    stale: Vec::new(),
                }
            ))
        );
    }

    #[test]
    fn forget_session_returns_diff_listing_active_and_stale_addresses() {
        let mut table = RouteTable::new(8, Duration::from_secs(30));
        let now = std::time::Instant::now();
        let session1 = WebRtcSessionId::new(1);
        let session2 = WebRtcSessionId::new(2);
        let addr1 = addr(1000);
        let addr2 = addr(2000);
        let addr3 = addr(3000);

        // session1 originally bound to addr1 and addr2.
        table.bind(addr1, session1, now);
        table.bind(addr2, session1, now);
        // session2 takes over addr1, pushing session1 at addr1 into stale.
        table.bind(addr1, session2, now);
        // session1 picks up a third active address.
        table.bind(addr3, session1, now);

        let diff = table.forget_session(session1);

        // added must be empty for forget_session.
        assert!(diff.added.is_empty());

        // removed should contain addr2 and addr3 (the active addresses
        // session1 still owned). Sort to avoid HashMap iteration order
        // dependencies.
        let mut removed_sorted = diff.removed.clone();
        removed_sorted.sort_by_key(|a| a.port());
        assert_eq!(removed_sorted, vec![addr2, addr3]);

        // stale should contain addr1 (the address whose session1
        // binding was already in the stale set).
        let mut stale_sorted = diff.stale.clone();
        stale_sorted.sort_by_key(|a| a.port());
        assert_eq!(stale_sorted, vec![addr1]);
    }
}
