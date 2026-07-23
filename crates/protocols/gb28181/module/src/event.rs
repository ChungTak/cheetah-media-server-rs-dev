//! Structured media events emitted by the GB28181 module.
//!
//! These events are published through the runtime-neutral `MediaEventBusApi` and only carry
//! normalized media metadata (protocol, session id, endpoint, reason). They never carry raw
//! SIP/SDP/XML payloads, secrets, or URL userinfo.

use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_sdk::media_api::event::{EventHeader, MediaEvent, SessionClosed, SessionOpened};
use cheetah_sdk::media_api::ids::SessionId;
use cheetah_sdk::media_api::model::{CloseReason, SessionKind};

const GB_PROTOCOL: &str = "gb28181";
const GB_SOURCE: &str = "cheetah-gb28181-module";

fn now_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn gb_event_header(session_id: &str) -> EventHeader {
    let occurred_at = now_ms_i64();
    EventHeader {
        event_id: format!("{GB_PROTOCOL}/{session_id}/{occurred_at}"),
        occurred_at,
        sequence: None,
        media_key: None,
        source: GB_SOURCE.to_string(),
        correlation_id: None,
    }
}

/// Build a `MediaEvent` for a GB28181 media session that has opened.
pub fn session_opened(
    session_id: &str,
    kind: SessionKind,
    remote_endpoint: Option<&str>,
) -> MediaEvent {
    MediaEvent::SessionOpened(SessionOpened {
        header: gb_event_header(session_id),
        kind,
        session_id: SessionId(session_id.to_string()),
        remote_endpoint: remote_endpoint.map(|s| s.to_string()),
        protocol: GB_PROTOCOL.to_string(),
    })
}

/// Build a `MediaEvent` for a GB28181 media session that has closed.
pub fn session_closed(session_id: &str, kind: SessionKind, reason: CloseReason) -> MediaEvent {
    MediaEvent::SessionClosed(SessionClosed {
        header: gb_event_header(session_id),
        kind,
        session_id: SessionId(session_id.to_string()),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_opened_has_gb_protocol_and_endpoint() {
        let event = session_opened("s-1", SessionKind::RtpReceiver, Some("10.0.0.1:10000"));
        match event {
            MediaEvent::SessionOpened(ref e) => {
                assert_eq!(e.protocol, "gb28181");
                assert_eq!(e.session_id.0, "s-1");
                assert_eq!(e.kind, SessionKind::RtpReceiver);
                assert_eq!(e.remote_endpoint.as_deref(), Some("10.0.0.1:10000"));
                assert_eq!(e.header.source, "cheetah-gb28181-module");
                assert!(!e.header.event_id.is_empty());
                assert!(e.header.occurred_at > 0);
            }
            _ => panic!("expected SessionOpened"),
        }
    }

    #[test]
    fn session_closed_has_reason() {
        let event = session_closed("s-2", SessionKind::RtpSender, CloseReason::Normal);
        match event {
            MediaEvent::SessionClosed(ref e) => {
                assert_eq!(e.session_id.0, "s-2");
                assert_eq!(e.kind, SessionKind::RtpSender);
                assert_eq!(e.reason, CloseReason::Normal);
            }
            _ => panic!("expected SessionClosed"),
        }
    }
}
