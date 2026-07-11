//! Connection migration helpers.
//!
//! 连接迁移助手。

use std::net::SocketAddr;

use cheetah_webrtc_core::WebRtcSessionId;

/// Emitted by the driver when a session's remote address changes.
///
/// `diff` carries the candidate-level delta produced by `RouteTable`
/// during the migration. Existing call sites that do not yet compute
/// a diff fall back to [`RouteCandidateDiff::default`], which is an
/// empty diff and therefore preserves prior observable behaviour.
///
/// 当会话的远程地址更改时由 driver 发出。
///
/// `diff` 携带 `RouteTable` 在迁移过程中产生的 candidate 级别增量。
/// 尚未计算 diff 的现有调用站点会回退到 [`RouteCandidateDiff::default`]，这是一个空 diff，因此保留了先前可观察的行为。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcRouteUpdate {
    /// Session whose remote address changed.
    ///
    /// 远程地址已更改的会话。
    pub session_id: WebRtcSessionId,
    /// Previous remote address, if any.
    ///
    /// 以前的远程地址（如果有）。
    pub previous_addr: Option<SocketAddr>,
    /// New remote address observed by the driver.
    ///
    /// driver 观察到的新远程地址。
    pub new_addr: SocketAddr,
    /// Candidate-level delta produced by the route table.
    ///
    /// 由路由表产生的 candidate 级增量。
    pub diff: RouteCandidateDiff,
}

/// Candidate-level diff produced by `RouteTable` mutations on session migration.
///
/// `added` lists remote addresses newly bound as the active route for a session.
/// `removed` lists addresses that were previously active and are no longer bound
/// to any session via the active mapping. `stale` lists addresses that were
/// downgraded from active to a tombstone / stale state (for example when a
/// session migrates away from a previously active address but the address is
/// still tracked for grace-period reasoning).
///
/// `Default` returns an all-empty diff so existing callers that do not yet
/// compute a diff can fall back to it without behavioural change.
///
/// 会话迁移时由 `RouteTable` 变更产生的 candidate 级别差异。
///
/// `added` 列出新绑定为会话活动路由的远程地址。
/// `removed` 列出以前处于活动状态且不再通过活动映射绑定到任何会话的地址。
/// `stale` 列出从活动状态降级为逻辑删除/过时状态的地址（例如，当会话从以前的活动地址迁移但仍会跟踪该地址以进行宽限期推理时）。
///
/// `Default` 返回一个全空的 diff，因此尚未计算 diff 的现有调用者可以回退到它，而无需行为改变。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCandidateDiff {
    /// Addresses newly bound as active routes.
    ///
    /// 新绑定为活动路由的地址。
    pub added: Vec<SocketAddr>,
    /// Addresses no longer bound to any session.
    ///
    /// 地址不再绑定到任何会话。
    pub removed: Vec<SocketAddr>,
    /// Addresses downgraded to stale/tombstone state.
    ///
    /// 地址降级为陈旧/墓碑状态。
    pub stale: Vec<SocketAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn route_candidate_diff_default_is_all_empty() {
        let diff = RouteCandidateDiff::default();
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.stale.is_empty());
    }

    #[test]
    fn webrtc_route_update_with_default_diff_round_trips_via_clone() {
        let original = WebRtcRouteUpdate {
            session_id: WebRtcSessionId::new(42),
            previous_addr: Some(SocketAddr::from(([127, 0, 0, 1], 4000))),
            new_addr: SocketAddr::from(([127, 0, 0, 1], 5000)),
            diff: RouteCandidateDiff::default(),
        };

        let cloned = original.clone();
        assert_eq!(cloned, original);

        assert_eq!(cloned.session_id, WebRtcSessionId::new(42));
        assert_eq!(
            cloned.previous_addr,
            Some(SocketAddr::from(([127, 0, 0, 1], 4000)))
        );
        assert_eq!(cloned.new_addr, SocketAddr::from(([127, 0, 0, 1], 5000)));
        assert_eq!(cloned.diff, RouteCandidateDiff::default());
        assert!(cloned.diff.added.is_empty());
        assert!(cloned.diff.removed.is_empty());
        assert!(cloned.diff.stale.is_empty());
    }
}
