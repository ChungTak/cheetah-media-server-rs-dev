//! Pull/push entrypoint helpers for ZLM `webrtc://` URLs.
//!
//! Phase 05 follow-up: when a client driver receives a
//! `webrtc://signaling-host:port/app/stream?signaling_protocols=1&peer_room_id=...`
//! URL, the module needs to:
//!
//! 1. Reject WHIP/WHEP-only URLs (`signaling_protocols=0`) so the
//!    operator gets a clear error instead of silently dropping the
//!    request.
//! 2. SSRF-check the signaling host using
//!    [`super::url::SignalingUrlPolicy`].
//! 3. Build the [`super::P2pBridgeConfig`] used by [`super::run_bridge`].
//!
//! This module is intentionally pure — no transport, no driver. It
//! turns a parsed ZLM URL into a ready-to-use plan that the runtime
//! glue layer can hand to `run_bridge`.

use std::time::Duration;

use cheetah_webrtc_core::WebRtcSessionId;
use thiserror::Error;

use super::bridge::P2pBridgeConfig;
use super::buffer::PENDING_CANDIDATE_DEFAULT_CAP;
use super::job::{P2pJobConfig, P2pJobKind};
use super::message::P2pStreamTuple;
use super::url::{
    parse as parse_signaling_url, SignalingUrl, SignalingUrlError, SignalingUrlPolicy,
};

use crate::compat::{ZlmRtcScheme, ZlmRtcUrl};

/// Default offer timeout. Mirrors the WHIP/WHEP pull job default.
pub const P2P_DEFAULT_OFFER_TIMEOUT: Duration = Duration::from_secs(10);

/// What the entrypoint produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pBridgePlan {
    pub bridge_config: P2pBridgeConfig,
    pub signaling_url: SignalingUrl,
    pub kind: P2pJobKind,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum P2pBridgePlanError {
    #[error("signaling_protocols={value}; expected 1 for P2P bridge plan")]
    NotP2p { value: u32 },
    #[error("missing peer_room_id query parameter")]
    MissingPeerRoom,
    #[error(transparent)]
    Url(#[from] SignalingUrlError),
}

/// Parameters threaded in from module configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pBridgePlanInput<'a> {
    pub url: &'a ZlmRtcUrl,
    pub kind: P2pJobKind,
    pub session_id: WebRtcSessionId,
    pub local_room_id: String,
    pub transport_id: String,
    pub policy: &'a SignalingUrlPolicy,
    /// Override `pending_candidate_cap`; falls back to
    /// [`PENDING_CANDIDATE_DEFAULT_CAP`] when zero.
    pub pending_candidate_cap: usize,
    pub offer_timeout: Option<Duration>,
}

/// Build a [`P2pBridgePlan`] from a ZLM URL. Pure function — does not
/// touch the network.
pub fn plan_from_zlm_url(
    input: P2pBridgePlanInput<'_>,
) -> Result<P2pBridgePlan, P2pBridgePlanError> {
    if input.url.signaling_protocols != 1 {
        return Err(P2pBridgePlanError::NotP2p {
            value: input.url.signaling_protocols,
        });
    }
    let peer_room_id = input
        .url
        .peer_room_id
        .clone()
        .ok_or(P2pBridgePlanError::MissingPeerRoom)?;
    let signaling_url = derive_signaling_url(input.url, input.policy)?;

    let pending_candidate_cap = if input.pending_candidate_cap == 0 {
        PENDING_CANDIDATE_DEFAULT_CAP
    } else {
        input.pending_candidate_cap
    };
    let job = P2pJobConfig {
        kind: input.kind,
        stream: P2pStreamTuple {
            // ZLM uses the URL host as the vhost for `rtc://` schemes;
            // for `webrtc://` the host is the signaling server, so we
            // fall back to the conventional default.
            vhost: match input.url.scheme {
                ZlmRtcScheme::Rtc | ZlmRtcScheme::Rtcs => input.url.host.clone(),
                ZlmRtcScheme::WebRtc | ZlmRtcScheme::WebRtcs => "__defaultVhost__".into(),
            },
            app: input.url.app.clone(),
            stream: input.url.stream.clone(),
        },
        local_room_id: input.local_room_id,
        peer_room_id,
        transport_id: input.transport_id,
        pending_candidate_cap,
    };
    Ok(P2pBridgePlan {
        bridge_config: P2pBridgeConfig {
            job,
            session_id: input.session_id,
            offer_timeout: input.offer_timeout.unwrap_or(P2P_DEFAULT_OFFER_TIMEOUT),
        },
        signaling_url,
        kind: input.kind,
    })
}

fn derive_signaling_url(
    url: &ZlmRtcUrl,
    policy: &SignalingUrlPolicy,
) -> Result<SignalingUrl, SignalingUrlError> {
    let secure = matches!(url.scheme, ZlmRtcScheme::Rtcs | ZlmRtcScheme::WebRtcs);
    let scheme = if secure { "wss" } else { "ws" };
    let port = match url.port {
        Some(p) => p,
        None => {
            if secure {
                443
            } else {
                80
            }
        }
    };
    let host = if url.host.contains(':') && !url.host.starts_with('[') {
        format!("[{}]", url.host)
    } else {
        url.host.clone()
    };
    let raw = format!("{scheme}://{host}:{port}/index/api/webrtc");
    parse_signaling_url(&raw, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::parse_zlm_rtc_url;

    fn allow_private_policy() -> SignalingUrlPolicy {
        SignalingUrlPolicy {
            allow_private_ips: true,
            ..Default::default()
        }
    }

    #[test]
    fn rejects_signaling_protocols_zero() {
        let url =
            parse_zlm_rtc_url("webrtc://example.com/live/demo?signaling_protocols=0").unwrap();
        let policy = SignalingUrlPolicy::default();
        let err = plan_from_zlm_url(P2pBridgePlanInput {
            url: &url,
            kind: P2pJobKind::Pull,
            session_id: WebRtcSessionId::new(1),
            local_room_id: "ringing".into(),
            transport_id: "tr".into(),
            policy: &policy,
            pending_candidate_cap: 0,
            offer_timeout: None,
        })
        .unwrap_err();
        assert!(matches!(err, P2pBridgePlanError::NotP2p { value: 0 }));
    }

    #[test]
    fn rejects_missing_peer_room_id() {
        let url =
            parse_zlm_rtc_url("webrtc://example.com/live/demo?signaling_protocols=1").unwrap();
        let policy = SignalingUrlPolicy::default();
        let err = plan_from_zlm_url(P2pBridgePlanInput {
            url: &url,
            kind: P2pJobKind::Pull,
            session_id: WebRtcSessionId::new(1),
            local_room_id: "ringing".into(),
            transport_id: "tr".into(),
            policy: &policy,
            pending_candidate_cap: 0,
            offer_timeout: None,
        })
        .unwrap_err();
        assert!(matches!(err, P2pBridgePlanError::MissingPeerRoom));
    }

    #[test]
    fn ssrf_blocks_loopback_signaling_host_by_default() {
        let url = parse_zlm_rtc_url(
            "webrtc://127.0.0.1:8443/live/demo?signaling_protocols=1&peer_room_id=room42",
        )
        .unwrap();
        let policy = SignalingUrlPolicy::default();
        let err = plan_from_zlm_url(P2pBridgePlanInput {
            url: &url,
            kind: P2pJobKind::Pull,
            session_id: WebRtcSessionId::new(1),
            local_room_id: "ringing".into(),
            transport_id: "tr".into(),
            policy: &policy,
            pending_candidate_cap: 0,
            offer_timeout: None,
        })
        .unwrap_err();
        assert!(matches!(
            err,
            P2pBridgePlanError::Url(SignalingUrlError::Blocked(_))
        ));
    }

    #[test]
    fn happy_path_pull_plan_round_trips_url_fields() {
        let url = parse_zlm_rtc_url(
            "webrtcs://signaling.example.com:9443/live/demo?signaling_protocols=1&peer_room_id=room42",
        )
        .unwrap();
        let policy = SignalingUrlPolicy::default();
        let plan = plan_from_zlm_url(P2pBridgePlanInput {
            url: &url,
            kind: P2pJobKind::Pull,
            session_id: WebRtcSessionId::new(7),
            local_room_id: "ringing".into(),
            transport_id: "tr".into(),
            policy: &policy,
            pending_candidate_cap: 16,
            offer_timeout: Some(Duration::from_secs(5)),
        })
        .expect("plan");
        assert_eq!(plan.kind, P2pJobKind::Pull);
        assert_eq!(plan.signaling_url.host, "signaling.example.com");
        assert!(plan.signaling_url.secure);
        assert_eq!(plan.signaling_url.port, 9443);
        assert_eq!(
            plan.bridge_config.job.peer_room_id, "room42",
            "peer_room_id should round-trip"
        );
        assert_eq!(plan.bridge_config.job.stream.app, "live");
        assert_eq!(plan.bridge_config.job.stream.stream, "demo");
        assert_eq!(plan.bridge_config.session_id, WebRtcSessionId::new(7));
        assert_eq!(plan.bridge_config.offer_timeout, Duration::from_secs(5));
        assert_eq!(plan.bridge_config.job.pending_candidate_cap, 16);
    }

    #[test]
    fn allow_private_policy_lets_loopback_signaling_url_through() {
        let url = parse_zlm_rtc_url(
            "webrtc://127.0.0.1:8443/live/demo?signaling_protocols=1&peer_room_id=room42",
        )
        .unwrap();
        let policy = allow_private_policy();
        let plan = plan_from_zlm_url(P2pBridgePlanInput {
            url: &url,
            kind: P2pJobKind::Push,
            session_id: WebRtcSessionId::new(8),
            local_room_id: "ringing".into(),
            transport_id: "tr".into(),
            policy: &policy,
            pending_candidate_cap: 0,
            offer_timeout: None,
        })
        .expect("private signaling host should be accepted under allow_private_ips");
        assert_eq!(plan.signaling_url.host, "127.0.0.1");
        assert_eq!(plan.signaling_url.port, 8443);
        assert_eq!(plan.kind, P2pJobKind::Push);
    }
}
