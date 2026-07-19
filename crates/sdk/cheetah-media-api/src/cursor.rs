//! Cursor-based pagination types for control-plane queries.
//!
//! 控制面查询的游标分页类型。

use std::cmp::Ordering;
use std::fmt;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{MediaError, Result};
use crate::ids::MediaNodeInstanceEpoch;

/// Opaque cursor token passed by clients to resume a paginated query.
///
/// 客户端传回的不透明游标令牌，用于继续分页查询。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueCursor(String);

impl OpaqueCursor {
    /// Maximum length of an encoded cursor token.
    pub const MAX_LEN: usize = 4096;

    /// Create a new opaque cursor, rejecting empty or overly long values.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(MediaError::invalid_argument("cursor must be non-empty"));
        }
        if value.len() > Self::MAX_LEN {
            return Err(MediaError::invalid_argument(
                "cursor exceeds maximum length",
            ));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(MediaError::invalid_argument(
                "cursor contains control characters",
            ));
        }
        Ok(Self(value))
    }

    /// Return the raw cursor string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OpaqueCursor {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OpaqueCursorVisitor;

        impl<'de> Visitor<'de> for OpaqueCursorVisitor {
            type Value = OpaqueCursor;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a non-empty opaque cursor string")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                OpaqueCursor::new(value).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(OpaqueCursorVisitor)
    }
}

impl std::fmt::Display for OpaqueCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Request for a single page of results using an opaque cursor.
///
/// 使用不透明游标请求单页结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPageRequest {
    pub cursor: Option<OpaqueCursor>,
    pub page_size: u32,
}

impl CursorPageRequest {
    /// Default page size when the caller does not specify one.
    pub const DEFAULT_PAGE_SIZE: u32 = 50;
    /// Maximum allowed page size for cluster cursor queries.
    pub const MAX_PAGE_SIZE: u32 = 1_000;

    /// Clamp the page size to the allowed range and provide a default if zero.
    pub fn clamp_page_size(&mut self) {
        if self.page_size == 0 {
            self.page_size = Self::DEFAULT_PAGE_SIZE;
        }
        self.page_size = self.page_size.min(Self::MAX_PAGE_SIZE);
    }
}

impl Default for CursorPageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            page_size: Self::DEFAULT_PAGE_SIZE,
        }
    }
}

/// A single page of cursor-paginated results.
///
/// 一页游标分页结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<OpaqueCursor>,
}

impl<T> Default for CursorPage<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_cursor: None,
        }
    }
}

/// Timestamp used as the primary sort key for a resource.
///
/// 用作资源主排序键的时间戳。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortTimestamp {
    /// `updated_at` when available.
    UpdatedAt(i64),
    /// `created_at` fallback when `updated_at` is not stable.
    CreatedAt(i64),
}

impl SortTimestamp {
    /// Return the millisecond timestamp value regardless of variant.
    pub fn millis(&self) -> i64 {
        match self {
            SortTimestamp::UpdatedAt(v) | SortTimestamp::CreatedAt(v) => *v,
        }
    }
}

/// Stable sort key for a single resource.
///
/// Ordering is `(timestamp, resource_handle)` with `resource_handle` as the
/// final tie-breaker. The `updated_at`/`created_at` timestamp source is not
/// part of the ordering; only the millisecond value matters. A unique handle is
/// required for every resource in the queried scope so pagination is stable.
///
/// 单个资源的稳定排序键。排序规则为 `(timestamp, resource_handle)`，
/// `resource_handle` 作为最终 tie-breaker。`updated_at`/`created_at` 来源
/// 不影响排序位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortKey {
    pub timestamp: SortTimestamp,
    pub resource_handle: String,
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp.millis() == other.timestamp.millis()
            && self.resource_handle == other.resource_handle
    }
}

impl Eq for SortKey {}

impl SortKey {
    /// Create a new sort key using the preferred `updated_at` timestamp.
    pub fn updated_at(updated_at_ms: i64, resource_handle: impl Into<String>) -> Self {
        Self {
            timestamp: SortTimestamp::UpdatedAt(updated_at_ms),
            resource_handle: resource_handle.into(),
        }
    }

    /// Create a new sort key using the `created_at` fallback.
    pub fn created_at(created_at_ms: i64, resource_handle: impl Into<String>) -> Self {
        Self {
            timestamp: SortTimestamp::CreatedAt(created_at_ms),
            resource_handle: resource_handle.into(),
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.timestamp
            .millis()
            .cmp(&other.timestamp.millis())
            .then_with(|| self.resource_handle.cmp(&other.resource_handle))
    }
}

/// High-water boundary captured at the start of a paginated query.
///
/// 分页查询开始时捕获的 high-water 边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorBoundary {
    /// Last sort key of the returned page.
    pub sort_key: SortKey,
    /// Node instance epoch the snapshot was taken from.
    pub snapshot_epoch: MediaNodeInstanceEpoch,
    /// Expiry timestamp after which the cursor must not be silently reused.
    pub snapshot_expiry_ms: i64,
}

/// Decoded payload of an opaque cursor.
///
/// 不透明游标的解码后载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorContents {
    /// Schema version of the cursor encoding.
    pub schema_version: u32,
    /// Resource kind being paginated (e.g. `session`, `rtp`, `proxy`).
    pub resource_kind: String,
    /// Sort key of the last returned item.
    pub sort_key: SortKey,
    /// Hex digest of the tenant and filter scope the cursor was issued for.
    pub tenant_filter_digest: String,
    /// Target node instance epoch from the originating request.
    pub node_instance_epoch: MediaNodeInstanceEpoch,
    /// Boundary captured at query start.
    pub boundary: CursorBoundary,
}

impl CursorContents {
    /// Current cursor schema version.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

/// Signed cursor envelope used for wire encoding.
///
/// 用于线传输编码的已签名游标信封。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SignedCursor {
    pub contents: CursorContents,
    pub hmac: String,
}

/// HMAC-based cursor codec.
///
/// Encodes a `CursorContents` into an `OpaqueCursor` and verifies it on decode.
/// The codec rejects tampered cursors, expired snapshots, and cursors whose
/// snapshot epoch no longer matches the current node instance.
///
/// 基于 HMAC 的游标 codec。编码 `CursorContents` 为 `OpaqueCursor`，
/// 解码时校验 HMAC、过期时间和节点实例 epoch。
pub struct CursorCodec;

impl CursorCodec {
    /// Encode the given contents into a tamper-evident opaque cursor.
    ///
    /// `key` must be at least 32 bytes for HMAC-SHA256.
    pub fn encode(contents: &CursorContents, key: &[u8]) -> Result<OpaqueCursor> {
        if key.len() < 32 {
            return Err(MediaError::invalid_argument(
                "cursor HMAC key must be at least 32 bytes",
            ));
        }

        let payload_bytes = serde_json::to_vec(contents).map_err(|e| {
            MediaError::invalid_argument(format!("failed to serialize cursor contents: {e}"))
        })?;
        let hmac = Self::sign(&payload_bytes, key)?;
        let signed = SignedCursor {
            contents: contents.clone(),
            hmac: base64::engine::general_purpose::STANDARD.encode(&hmac),
        };
        let token_bytes = serde_json::to_vec(&signed).map_err(|e| {
            MediaError::invalid_argument(format!("failed to serialize signed cursor: {e}"))
        })?;
        let token = base64::engine::general_purpose::STANDARD.encode(&token_bytes);
        OpaqueCursor::new(token)
    }

    /// Decode and verify an opaque cursor.
    ///
    /// `now_ms` is the current cluster time; `current_epoch` is the node
    /// instance epoch the query is running against.
    pub fn decode(
        cursor: &OpaqueCursor,
        key: &[u8],
        now_ms: i64,
        current_epoch: MediaNodeInstanceEpoch,
    ) -> Result<CursorContents> {
        if key.len() < 32 {
            return Err(MediaError::invalid_argument(
                "cursor HMAC key must be at least 32 bytes",
            ));
        }

        let token_bytes = base64::engine::general_purpose::STANDARD
            .decode(cursor.as_str())
            .map_err(|_| MediaError::cursor_expired("cursor is not valid base64"))?;
        let signed: SignedCursor = serde_json::from_slice(&token_bytes)
            .map_err(|_| MediaError::cursor_expired("cursor is not valid JSON"))?;

        if signed.contents.schema_version != CursorContents::CURRENT_SCHEMA_VERSION {
            return Err(MediaError::cursor_expired(
                "cursor schema version is not supported",
            ));
        }

        if signed.contents.boundary.snapshot_expiry_ms > 0
            && now_ms > signed.contents.boundary.snapshot_expiry_ms
        {
            return Err(MediaError::cursor_expired("cursor snapshot has expired"));
        }

        if signed.contents.boundary.snapshot_epoch != current_epoch {
            return Err(MediaError::cursor_expired(
                "cursor snapshot epoch does not match the current node instance",
            ));
        }

        let payload_bytes = serde_json::to_vec(&signed.contents).map_err(|_| {
            MediaError::cursor_expired("failed to re-serialize cursor contents for verification")
        })?;
        let expected = Self::sign(&payload_bytes, key)?;
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(&expected);
        if expected_b64 != signed.hmac {
            return Err(MediaError::cursor_expired(
                "cursor HMAC verification failed",
            ));
        }

        Ok(signed.contents)
    }

    fn sign(payload: &[u8], key: &[u8]) -> Result<Vec<u8>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| {
            MediaError::invalid_argument(format!("failed to initialize cursor HMAC: {e}"))
        })?;
        mac.update(payload);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn sample_contents(handle: &str) -> CursorContents {
        CursorContents {
            schema_version: CursorContents::CURRENT_SCHEMA_VERSION,
            resource_kind: "session".to_string(),
            sort_key: SortKey::updated_at(1_000_000, handle),
            tenant_filter_digest: "digest".to_string(),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            boundary: CursorBoundary {
                sort_key: SortKey::updated_at(1_000_000, handle),
                snapshot_epoch: MediaNodeInstanceEpoch(42),
                snapshot_expiry_ms: 2_000_000,
            },
        }
    }

    #[test]
    fn cursor_rejects_empty_and_control_chars() {
        assert!(OpaqueCursor::new("").is_err());
        assert!(OpaqueCursor::new("bad\ncursor").is_err());
        assert!(OpaqueCursor::new("valid-cursor").is_ok());
    }

    #[test]
    fn cursor_enforces_max_length() {
        let long = "a".repeat(OpaqueCursor::MAX_LEN + 1);
        assert!(OpaqueCursor::new(long).is_err());
    }

    #[test]
    fn cursor_deserialize_validates() {
        let valid = "\"valid-cursor\"";
        let decoded: OpaqueCursor = serde_json::from_str(valid).unwrap();
        assert_eq!(decoded.as_str(), "valid-cursor");

        let empty = "\"\"";
        assert!(serde_json::from_str::<OpaqueCursor>(empty).is_err());

        let long = format!("\"{}\"", "a".repeat(OpaqueCursor::MAX_LEN + 1));
        assert!(serde_json::from_str::<OpaqueCursor>(&long).is_err());
    }

    #[test]
    fn page_request_defaults_and_clamps() {
        let mut req = CursorPageRequest::default();
        assert_eq!(req.page_size, CursorPageRequest::DEFAULT_PAGE_SIZE);
        req.page_size = 0;
        req.clamp_page_size();
        assert_eq!(req.page_size, CursorPageRequest::DEFAULT_PAGE_SIZE);

        let mut req = CursorPageRequest {
            cursor: None,
            page_size: 10_000,
        };
        req.clamp_page_size();
        assert_eq!(req.page_size, CursorPageRequest::MAX_PAGE_SIZE);
    }

    #[test]
    fn cursor_page_default_is_empty() {
        let page: CursorPage<String> = CursorPage::default();
        assert!(page.items.is_empty());
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn sort_key_orders_by_timestamp_then_handle() {
        let a = SortKey::updated_at(1, "a");
        let b = SortKey::updated_at(2, "a");
        let c = SortKey::created_at(2, "b");
        let d = SortKey::updated_at(2, "c");

        assert!(a < b);
        assert!(b < c);
        assert!(c < d);
    }

    #[test]
    fn sort_key_treats_timestamp_source_as_equal_for_ordering() {
        let updated = SortKey::updated_at(2, "a");
        let created = SortKey::created_at(2, "a");
        assert_eq!(updated, created);
        assert_eq!(updated.cmp(&created), Ordering::Equal);
    }

    #[test]
    fn codec_round_trips_valid_cursor() {
        let contents = sample_contents("handle-1");
        let cursor = CursorCodec::encode(&contents, KEY).unwrap();
        let decoded =
            CursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(42)).unwrap();
        assert_eq!(decoded, contents);
    }

    #[test]
    fn codec_rejects_expired_cursor() {
        let contents = sample_contents("handle-1");
        let cursor = CursorCodec::encode(&contents, KEY).unwrap();
        assert!(CursorCodec::decode(&cursor, KEY, 2_500_000, MediaNodeInstanceEpoch(42)).is_err());
    }

    #[test]
    fn codec_rejects_wrong_epoch() {
        let contents = sample_contents("handle-1");
        let cursor = CursorCodec::encode(&contents, KEY).unwrap();
        assert!(CursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(99)).is_err());
    }

    #[test]
    fn codec_rejects_tampered_cursor() {
        let contents = sample_contents("handle-1");
        let mut cursor = CursorCodec::encode(&contents, KEY).unwrap();
        let raw = cursor.as_str().to_string();
        let mut chars: Vec<char> = raw.chars().collect();
        if let Some(c) = chars.get_mut(10) {
            // flip a base64 character to another valid base64 alphabet char
            *c = if *c == 'A' { 'B' } else { 'A' };
        }
        cursor = OpaqueCursor::new(chars.into_iter().collect::<String>()).unwrap();
        assert!(CursorCodec::decode(&cursor, KEY, 1_500_000, MediaNodeInstanceEpoch(42)).is_err());
    }

    #[test]
    fn codec_rejects_short_key() {
        let contents = sample_contents("handle-1");
        assert!(CursorCodec::encode(&contents, b"short").is_err());
    }
}
