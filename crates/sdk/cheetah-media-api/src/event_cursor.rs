//! Event-cursor types for resuming durable event streams.
//!
//! 用于恢复可重放事件流的事件游标类型。

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::controlled_event::EventSequence;
use crate::cursor::OpaqueCursor;
use crate::error::{MediaError, Result};
use crate::ids::{MediaNodeId, MediaNodeInstanceEpoch};

/// Decoded payload of an opaque event cursor.
///
/// 不透明事件游标的解码后载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCursorContents {
    /// Schema version of the event cursor encoding.
    pub schema_version: u32,
    /// Node that issued the cursor.
    pub media_node_id: MediaNodeId,
    /// Instance epoch the cursor was issued against.
    pub media_node_instance_epoch: MediaNodeInstanceEpoch,
    /// Last sequence delivered to the subscriber.
    pub last_delivered_sequence: EventSequence,
    /// Hex digest of the tenant and filter scope the cursor was issued for.
    pub tenant_filter_digest: String,
    /// Cursor issuance timestamp in milliseconds.
    pub issued_at_ms: i64,
    /// Cursor expiry timestamp in milliseconds.
    pub expires_at_ms: i64,
    /// Identifier of the signing key used to issue the cursor.
    pub key_id: String,
}

impl EventCursorContents {
    /// Current event cursor schema version.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SignedEventCursor {
    contents: EventCursorContents,
    hmac: String,
}

/// HMAC-based codec for event stream resume cursors.
///
/// 基于 HMAC 的事件流恢复游标 codec。
pub struct EventCursorCodec;

impl EventCursorCodec {
    /// Encode event cursor contents into a tamper-evident opaque cursor.
    ///
    /// `key` must be at least 32 bytes for HMAC-SHA256.
    pub fn encode(contents: &EventCursorContents, key: &[u8]) -> Result<OpaqueCursor> {
        if key.len() < 32 {
            return Err(MediaError::invalid_argument(
                "event cursor HMAC key must be at least 32 bytes",
            ));
        }

        let payload_bytes = serde_json::to_vec(contents).map_err(|e| {
            MediaError::invalid_argument(format!("failed to serialize event cursor contents: {e}"))
        })?;
        let hmac = Self::sign(&payload_bytes, key)?;
        let signed = SignedEventCursor {
            contents: contents.clone(),
            hmac: base64::engine::general_purpose::STANDARD.encode(&hmac),
        };
        let token_bytes = serde_json::to_vec(&signed).map_err(|e| {
            MediaError::invalid_argument(format!("failed to serialize signed event cursor: {e}"))
        })?;
        let token = base64::engine::general_purpose::STANDARD.encode(&token_bytes);
        OpaqueCursor::new(token)
    }

    /// Decode and verify an opaque event cursor.
    ///
    /// `now_ms` is the current cluster time; `current_epoch` is the node
    /// instance epoch the event stream is being served from.
    pub fn decode(
        cursor: &OpaqueCursor,
        key: &[u8],
        now_ms: i64,
        current_epoch: MediaNodeInstanceEpoch,
    ) -> Result<EventCursorContents> {
        if key.len() < 32 {
            return Err(MediaError::invalid_argument(
                "event cursor HMAC key must be at least 32 bytes",
            ));
        }

        let token_bytes = base64::engine::general_purpose::STANDARD
            .decode(cursor.as_str())
            .map_err(|_| MediaError::cursor_expired("event cursor is not valid base64"))?;
        let signed: SignedEventCursor = serde_json::from_slice(&token_bytes)
            .map_err(|_| MediaError::cursor_expired("event cursor is not valid JSON"))?;

        if signed.contents.schema_version != EventCursorContents::CURRENT_SCHEMA_VERSION {
            return Err(MediaError::cursor_expired(
                "event cursor schema version is not supported",
            ));
        }

        if signed.contents.expires_at_ms > 0 && now_ms > signed.contents.expires_at_ms {
            return Err(MediaError::cursor_expired("event cursor has expired"));
        }

        if signed.contents.media_node_instance_epoch != current_epoch {
            return Err(MediaError::cursor_expired(
                "event cursor instance epoch does not match the current node instance",
            ));
        }

        let payload_bytes = serde_json::to_vec(&signed.contents).map_err(|_| {
            MediaError::cursor_expired(
                "failed to re-serialize event cursor contents for verification",
            )
        })?;
        let expected = Self::sign(&payload_bytes, key)?;
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(&expected);
        if expected_b64 != signed.hmac {
            return Err(MediaError::cursor_expired(
                "event cursor HMAC verification failed",
            ));
        }

        Ok(signed.contents)
    }

    fn sign(payload: &[u8], key: &[u8]) -> Result<Vec<u8>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| {
            MediaError::invalid_argument(format!("failed to initialize event cursor HMAC: {e}"))
        })?;
        mac.update(payload);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn sample_contents(seq: u64) -> EventCursorContents {
        EventCursorContents {
            schema_version: EventCursorContents::CURRENT_SCHEMA_VERSION,
            media_node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            media_node_instance_epoch: MediaNodeInstanceEpoch(42),
            last_delivered_sequence: EventSequence(seq),
            tenant_filter_digest: "digest".to_string(),
            issued_at_ms: 1_000_000,
            expires_at_ms: 2_000_000,
            key_id: "key-1".to_string(),
        }
    }

    #[test]
    fn event_cursor_codec_round_trips() {
        let contents = sample_contents(7);
        let cursor = EventCursorCodec::encode(&contents, KEY).unwrap();
        let decoded =
            EventCursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(42)).unwrap();
        assert_eq!(decoded, contents);
    }

    #[test]
    fn event_cursor_rejects_expired_cursor() {
        let contents = sample_contents(7);
        let cursor = EventCursorCodec::encode(&contents, KEY).unwrap();
        assert!(
            EventCursorCodec::decode(&cursor, KEY, 2_500_000, MediaNodeInstanceEpoch(42)).is_err()
        );
    }

    #[test]
    fn event_cursor_rejects_wrong_epoch() {
        let contents = sample_contents(7);
        let cursor = EventCursorCodec::encode(&contents, KEY).unwrap();
        assert!(
            EventCursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(99)).is_err()
        );
    }

    #[test]
    fn event_cursor_rejects_tampered_cursor() {
        let contents = sample_contents(7);
        let mut cursor = EventCursorCodec::encode(&contents, KEY).unwrap();
        let raw = cursor.as_str().to_string();
        let mut chars: Vec<char> = raw.chars().collect();
        if let Some(c) = chars.get_mut(10) {
            *c = if *c == 'A' { 'B' } else { 'A' };
        }
        cursor = OpaqueCursor::new(chars.into_iter().collect::<String>()).unwrap();
        assert!(
            EventCursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(42)).is_err()
        );
    }

    #[test]
    fn event_cursor_rejects_short_key() {
        let contents = sample_contents(7);
        assert!(EventCursorCodec::encode(&contents, b"short").is_err());
    }
}
