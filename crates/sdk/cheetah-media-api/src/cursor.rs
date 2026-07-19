//! Cursor-based pagination types for control-plane queries.
//!
//! 控制面查询的游标分页类型。

use serde::{Deserialize, Serialize};

use crate::error::{MediaError, Result};

/// Opaque cursor token passed by clients to resume a paginated query.
///
/// 客户端传回的不透明游标令牌，用于继续分页查询。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
