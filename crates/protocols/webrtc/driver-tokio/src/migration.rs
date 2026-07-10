//! Connection migration helpers.

use std::net::SocketAddr;

use cheetah_webrtc_core::WebRtcSessionId;

/// Emitted by the driver when a session's remote address changes.
///
/// `diff` carries the candidate-level delta produced by `RouteTable`
/// during the migration. Existing call sites that do not yet compute
/// a diff fall back to [`RouteCandidateDiff::default`], which is an
/// empty diff and therefore preserves prior observable behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcRouteUpdate {
    /// `session_id` field of type `WebRtcSessionId`.
    /// `session_id` 字段，类型为 `WebRtcSessionId`.
    pub session_id: WebRtcSessionId,
    /// `previous_addr` field.
    /// `previous_addr` 字段.
    pub previous_addr: Option<SocketAddr>,
    /// `new_addr` field of type `SocketAddr`.
    /// `new_addr` 字段，类型为 `SocketAddr`.
    pub new_addr: SocketAddr,
    /// `diff` field of type `RouteCandidateDiff`.
    /// `diff` 字段，类型为 `RouteCandidateDiff`.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCandidateDiff {
    /// `added` field.
    /// `added` 字段.
    pub added: Vec<SocketAddr>,
    /// `removed` field.
    /// `removed` 字段.
    pub removed: Vec<SocketAddr>,
    /// `stale` field.
    /// `stale` 字段.
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
