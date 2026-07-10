//! WebRTC play disconnect event and emission logic.
//!
//! ABL 2026-02-02 added a `on_play_disconnect` notification carrying
//! app, stream, networkType, key, ip, port, playDuration. This module
//! implements the equivalent for cheetah:
//!
//! - [`WebRtcPlayDisconnectEvent`] is the protocol-specific event
//!   payload published through the SDK event bus.
//! - [`PlayDisconnectReason`] enumerates the close triggers.
//! - [`evaluate_play_disconnect`] decides whether to emit the
//!   business event (duration >= threshold) or only record a metric
//!   (short connection).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use cheetah_sdk::{EventBus, ProtocolEvent, StreamKey, SystemEvent};
use cheetah_webrtc_core::WebRtcSessionRole;

/// Reason the play session was closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayDisconnectReason {
    /// Client sent DELETE.
    ClientDelete,
    /// ICE/DTLS timeout or transport failure.
    TransportTimeout,
    /// Source stream closed (publisher gone).
    StreamClosed,
    /// Module/server shutdown.
    ServerShutdown,
    /// Session idle timeout.
    IdleTimeout,
    /// Driver reported session failure.
    DriverClose,
    /// Unknown / other.
    Other(String),
}

impl std::fmt::Display for PlayDisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientDelete => write!(f, "client_delete"),
            Self::TransportTimeout => write!(f, "transport_timeout"),
            Self::StreamClosed => write!(f, "stream_closed"),
            Self::ServerShutdown => write!(f, "server_shutdown"),
            Self::IdleTimeout => write!(f, "idle_timeout"),
            Self::DriverClose => write!(f, "driver_close"),
            Self::Other(s) => write!(f, "other:{s}"),
        }
    }
}

/// Network type hint for the play session. Derived from the selected
/// ICE candidate pair transport when available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    Udp,
    Tcp,
    Unknown,
}

impl std::fmt::Display for NetworkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Udp => write!(f, "udp"),
            Self::Tcp => write!(f, "tcp"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Protocol-specific play disconnect event published on the SDK event
/// bus when a WebRTC play session ends after exceeding the minimum
/// duration threshold.
///
/// Fields align with ABL's `on_play_disconnect` notification:
/// app, stream, networkType, key, ip, port, playDuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRtcPlayDisconnectEvent {
    pub stream_key: String,
    pub session_id: u64,
    pub network_type: NetworkType,
    pub remote_addr: String,
    pub duration_ms: u64,
    pub close_reason: String,
}

/// Result of evaluating a play disconnect against the minimum
/// duration threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayDisconnectOutcome {
    /// Duration met the threshold — business event should be emitted.
    EmitEvent(WebRtcPlayDisconnectEvent),
    /// Short connection — only record metrics, no business event.
    ShortConnection {
        stream_key: String,
        session_id: u64,
        duration_ms: u64,
    },
}

/// Evaluate whether a play disconnect should emit a business event
/// or only record a metric.
///
/// - If `duration >= min_duration_threshold`, returns
///   `PlayDisconnectOutcome::EmitEvent`.
/// - Otherwise returns `PlayDisconnectOutcome::ShortConnection`.
pub fn evaluate_play_disconnect(
    stream_key: &StreamKey,
    session_id: u64,
    network_type: NetworkType,
    remote_addr: SocketAddr,
    duration: Duration,
    close_reason: PlayDisconnectReason,
    min_duration_threshold: Duration,
) -> PlayDisconnectOutcome {
    let duration_ms = duration.as_millis() as u64;
    let stream_key_str = format!("{}/{}", stream_key.namespace, stream_key.path);

    if duration >= min_duration_threshold {
        PlayDisconnectOutcome::EmitEvent(WebRtcPlayDisconnectEvent {
            stream_key: stream_key_str,
            session_id,
            network_type,
            remote_addr: remote_addr.to_string(),
            duration_ms,
            close_reason: close_reason.to_string(),
        })
    } else {
        PlayDisconnectOutcome::ShortConnection {
            stream_key: stream_key_str,
            session_id,
            duration_ms,
        }
    }
}

/// Publish a [`WebRtcPlayDisconnectEvent`] on the SDK event bus as a
/// `SystemEvent::Protocol` envelope.
pub fn publish_play_disconnect_event(event_bus: &dyn EventBus, event: &WebRtcPlayDisconnectEvent) {
    let payload = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    event_bus.publish(SystemEvent::Protocol(ProtocolEvent {
        protocol: "webrtc".to_string(),
        event_type: "play_disconnect".to_string(),
        payload,
    }));
}

/// Closes the `reason to play disconnect reason`.
/// 关闭 `reason to play disconnect reason`。
pub fn close_reason_to_play_disconnect_reason(
    reason: &cheetah_webrtc_core::WebRtcCloseReason,
) -> PlayDisconnectReason {
    match reason {
        cheetah_webrtc_core::WebRtcCloseReason::Normal
        | cheetah_webrtc_core::WebRtcCloseReason::PeerClosed => PlayDisconnectReason::DriverClose,
        cheetah_webrtc_core::WebRtcCloseReason::HandshakeTimeout => {
            PlayDisconnectReason::TransportTimeout
        }
        cheetah_webrtc_core::WebRtcCloseReason::Idle => PlayDisconnectReason::IdleTimeout,
        cheetah_webrtc_core::WebRtcCloseReason::Internal(reason) => {
            PlayDisconnectReason::Other(reason.clone())
        }
    }
}

/// Evaluate and record cleanup for a removed play session.
pub fn observe_play_session_cleanup(
    event_bus: &dyn EventBus,
    metrics: &crate::metrics::WebRtcModuleMetrics,
    session: &crate::session::WebRtcModuleSession,
    close_reason: PlayDisconnectReason,
    min_duration_threshold: Duration,
    now: Instant,
) {
    if session.role != WebRtcSessionRole::Player {
        return;
    }

    let remote_addr = session
        .remote_addr
        .unwrap_or_else(|| SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0)));
    match evaluate_play_disconnect(
        &session.stream_key,
        session.id.value(),
        NetworkType::Unknown,
        remote_addr,
        now.saturating_duration_since(session.created_at),
        close_reason,
        min_duration_threshold,
    ) {
        PlayDisconnectOutcome::EmitEvent(event) => {
            metrics.inc_play_disconnect_event();
            publish_play_disconnect_event(event_bus, &event);
        }
        PlayDisconnectOutcome::ShortConnection { .. } => {
            metrics.inc_play_disconnect_short();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::{Arc, Mutex};

    fn test_stream_key() -> StreamKey {
        StreamKey::new("live", "camera01")
    }

    fn test_remote_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 54321)
    }

    #[test]
    fn play_disconnect_event_respects_min_duration() {
        // Duration above threshold → emit event
        let outcome = evaluate_play_disconnect(
            &test_stream_key(),
            42,
            NetworkType::Udp,
            test_remote_addr(),
            Duration::from_secs(10),
            PlayDisconnectReason::ClientDelete,
            Duration::from_secs(8),
        );
        match outcome {
            PlayDisconnectOutcome::EmitEvent(evt) => {
                assert_eq!(evt.stream_key, "live/camera01");
                assert_eq!(evt.session_id, 42);
                assert_eq!(evt.network_type, NetworkType::Udp);
                assert_eq!(evt.remote_addr, "192.168.1.100:54321");
                assert_eq!(evt.duration_ms, 10_000);
                assert_eq!(evt.close_reason, "client_delete");
            }
            other => panic!("expected EmitEvent, got {other:?}"),
        }

        // Duration below threshold → short connection (metrics only)
        let outcome = evaluate_play_disconnect(
            &test_stream_key(),
            43,
            NetworkType::Tcp,
            test_remote_addr(),
            Duration::from_secs(3),
            PlayDisconnectReason::TransportTimeout,
            Duration::from_secs(8),
        );
        match outcome {
            PlayDisconnectOutcome::ShortConnection {
                stream_key,
                session_id,
                duration_ms,
            } => {
                assert_eq!(stream_key, "live/camera01");
                assert_eq!(session_id, 43);
                assert_eq!(duration_ms, 3_000);
            }
            other => panic!("expected ShortConnection, got {other:?}"),
        }
    }

    #[test]
    fn play_disconnect_exact_threshold_emits_event() {
        // Duration exactly at threshold → emit event (>= semantics)
        let outcome = evaluate_play_disconnect(
            &test_stream_key(),
            44,
            NetworkType::Unknown,
            test_remote_addr(),
            Duration::from_secs(8),
            PlayDisconnectReason::StreamClosed,
            Duration::from_secs(8),
        );
        assert!(matches!(outcome, PlayDisconnectOutcome::EmitEvent(_)));
    }

    #[test]
    fn play_disconnect_zero_threshold_always_emits() {
        // Zero threshold means all disconnects emit events
        let outcome = evaluate_play_disconnect(
            &test_stream_key(),
            45,
            NetworkType::Udp,
            test_remote_addr(),
            Duration::from_millis(1),
            PlayDisconnectReason::IdleTimeout,
            Duration::ZERO,
        );
        assert!(matches!(outcome, PlayDisconnectOutcome::EmitEvent(_)));
    }

    #[test]
    fn play_disconnect_event_contains_all_required_fields() {
        let outcome = evaluate_play_disconnect(
            &test_stream_key(),
            99,
            NetworkType::Tcp,
            test_remote_addr(),
            Duration::from_secs(15),
            PlayDisconnectReason::ServerShutdown,
            Duration::from_secs(8),
        );
        match outcome {
            PlayDisconnectOutcome::EmitEvent(evt) => {
                // Verify all ABL-equivalent fields are present
                assert!(!evt.stream_key.is_empty());
                assert!(evt.session_id > 0);
                assert_eq!(evt.network_type, NetworkType::Tcp);
                assert!(!evt.remote_addr.is_empty());
                assert!(evt.duration_ms > 0);
                assert!(!evt.close_reason.is_empty());
            }
            other => panic!("expected EmitEvent, got {other:?}"),
        }
    }

    #[test]
    fn play_disconnect_reason_display() {
        assert_eq!(
            PlayDisconnectReason::ClientDelete.to_string(),
            "client_delete"
        );
        assert_eq!(
            PlayDisconnectReason::TransportTimeout.to_string(),
            "transport_timeout"
        );
        assert_eq!(
            PlayDisconnectReason::StreamClosed.to_string(),
            "stream_closed"
        );
        assert_eq!(
            PlayDisconnectReason::ServerShutdown.to_string(),
            "server_shutdown"
        );
        assert_eq!(
            PlayDisconnectReason::IdleTimeout.to_string(),
            "idle_timeout"
        );
        assert_eq!(
            PlayDisconnectReason::DriverClose.to_string(),
            "driver_close"
        );
        assert_eq!(
            PlayDisconnectReason::Other("custom".into()).to_string(),
            "other:custom"
        );
    }

    #[test]
    fn play_disconnect_event_serializes_to_json() {
        let evt = WebRtcPlayDisconnectEvent {
            stream_key: "live/camera01".to_string(),
            session_id: 42,
            network_type: NetworkType::Udp,
            remote_addr: "192.168.1.100:54321".to_string(),
            duration_ms: 10_000,
            close_reason: "client_delete".to_string(),
        };
        let json = serde_json::to_value(&evt).expect("serializes");
        assert_eq!(json["stream_key"], "live/camera01");
        assert_eq!(json["session_id"], 42);
        assert_eq!(json["network_type"], "udp");
        assert_eq!(json["remote_addr"], "192.168.1.100:54321");
        assert_eq!(json["duration_ms"], 10_000);
        assert_eq!(json["close_reason"], "client_delete");
    }

    struct CapturingEventBus {
        events: Arc<Mutex<Vec<SystemEvent>>>,
    }

    impl EventBus for CapturingEventBus {
        fn publish(&self, event: SystemEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn subscribe(&self, _capacity: usize) -> Box<dyn cheetah_sdk::EventSubscriber> {
            panic!("not used")
        }
    }

    #[test]
    fn observe_play_session_cleanup_emits_event_and_metric() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let event_bus = CapturingEventBus {
            events: events.clone(),
        };
        let metrics = crate::metrics::WebRtcModuleMetrics::new();
        let mut session = crate::session::WebRtcModuleSession::new(
            cheetah_webrtc_core::WebRtcSessionId::new(42),
            test_stream_key(),
            cheetah_webrtc_core::WebRtcSessionRole::Player,
            crate::session::WebRtcApiKind::Whep,
        );
        session.remote_addr = Some(test_remote_addr());
        session.created_at = std::time::Instant::now() - Duration::from_secs(10);

        observe_play_session_cleanup(
            &event_bus,
            &metrics,
            &session,
            PlayDisconnectReason::ClientDelete,
            Duration::from_secs(8),
            std::time::Instant::now(),
        );

        let snap = metrics.snapshot_counters();
        assert_eq!(snap.play_disconnect_events, 1);
        assert_eq!(snap.play_disconnect_short, 0);
        assert_eq!(events.lock().unwrap().len(), 1);
    }

    #[test]
    fn observe_short_play_session_records_short_metric_only() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let event_bus = CapturingEventBus {
            events: events.clone(),
        };
        let metrics = crate::metrics::WebRtcModuleMetrics::new();
        let mut session = crate::session::WebRtcModuleSession::new(
            cheetah_webrtc_core::WebRtcSessionId::new(43),
            test_stream_key(),
            cheetah_webrtc_core::WebRtcSessionRole::Player,
            crate::session::WebRtcApiKind::Whep,
        );
        session.created_at = std::time::Instant::now() - Duration::from_secs(1);

        observe_play_session_cleanup(
            &event_bus,
            &metrics,
            &session,
            PlayDisconnectReason::ClientDelete,
            Duration::from_secs(8),
            std::time::Instant::now(),
        );

        let snap = metrics.snapshot_counters();
        assert_eq!(snap.play_disconnect_events, 0);
        assert_eq!(snap.play_disconnect_short, 1);
        assert!(events.lock().unwrap().is_empty());
    }
}
