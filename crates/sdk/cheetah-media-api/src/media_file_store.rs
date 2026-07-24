//! Runtime-neutral managed file storage API.
//!
//! `MediaFileStoreApi` maps server-side file metadata to unguessable `FileHandle`
//! tokens so that payloads never expose absolute server paths. The concrete
//! implementation lives in the engine and is injected through `EngineContext`.
//!
//! 运行时无关的受管文件存储 API。

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::ids::{FileHandle, MediaKey};
use crate::port::MediaRequestContext;

/// Entry describing a managed file.
///
/// 受管文件描述项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStoreEntry {
    pub media_key: MediaKey,
    pub file_type: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    /// Absolute server path. Never returned in public payloads.
    pub absolute_path: String,
    pub owner_principal: Option<String>,
    pub allowed_principals: Vec<String>,
}

impl Default for FileStoreEntry {
    fn default() -> Self {
        Self {
            media_key: MediaKey::with_default_vhost("live", "default", None)
                .expect("default media key is valid"),
            file_type: String::new(),
            content_type: String::new(),
            size_bytes: 0,
            created_at_ms: 0,
            expires_at_ms: None,
            absolute_path: String::new(),
            owner_principal: None,
            allowed_principals: Vec::new(),
        }
    }
}

/// Query used for batch file deletion.
///
/// 用于批量文件删除的查询条件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileStoreQuery {
    pub media_key: Option<MediaKey>,
    pub file_type: Option<String>,
    pub created_before_ms: Option<i64>,
    pub created_after_ms: Option<i64>,
    pub owner_principal: Option<String>,
}

/// Per-handle deletion failure.
///
/// 单个句柄删除失败详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteFailure {
    pub handle: FileHandle,
    pub reason: String,
}

/// Result of a batch deletion.
///
/// 批量删除结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteBatchResult {
    pub matched: u64,
    pub deleted: u64,
    pub failed: u64,
    pub failures: Vec<DeleteFailure>,
}

/// A byte range within a file.
///
/// When `is_suffix` is `true`, `start` is the number of bytes to read from
/// the end of the file and `end` is ignored.
///
/// 文件内的字节范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRange {
    pub start: u64,
    pub end: Option<u64>,
    #[serde(default)]
    pub is_suffix: bool,
}

impl FileRange {
    /// Read from `start` through the end of the file.
    pub fn from(start: u64) -> Self {
        Self {
            start,
            end: None,
            is_suffix: false,
        }
    }

    /// Read the explicit inclusive byte range `[start, end]`.
    pub fn bounded(start: u64, end: u64) -> Self {
        Self {
            start,
            end: Some(end),
            is_suffix: false,
        }
    }

    /// Read the last `n` bytes of the file.
    pub fn suffix(n: u64) -> Self {
        Self {
            start: n,
            end: None,
            is_suffix: true,
        }
    }
}

/// Prepared response for a file download.
///
/// 文件下载的已准备响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDownload {
    pub content_type: String,
    pub total_size: u64,
    pub body: Bytes,
    pub filename: String,
    pub range: Option<FileRange>,
}

/// Runtime-neutral file store. Implementations must be `Send + Sync` so they
/// can be shared across modules and the engine.
///
/// 运行时无关的文件存储。
pub trait MediaFileStoreApi: Send + Sync {
    /// Register a file and return an unguessable handle.
    ///
    /// 注册文件并返回不可猜测的句柄。
    fn register_file(&self, ctx: &MediaRequestContext, entry: FileStoreEntry)
        -> Result<FileHandle>;

    /// Resolve a file for reading after checking principal, scope, and expiry.
    ///
    /// 校验 principal、作用域和过期时间后解析文件。
    fn resolve_for_read(
        &self,
        ctx: &MediaRequestContext,
        handle: &FileHandle,
        resource_scope: Option<&MediaKey>,
        now_ms: i64,
    ) -> Result<FileStoreEntry>;

    /// Delete the registry entry for a handle. The underlying file on disk is
    /// intentionally not removed here; the caller or an external janitor handles
    /// physical cleanup.
    ///
    /// 删除句柄对应的注册表项，不删除磁盘文件。
    fn delete(&self, ctx: &MediaRequestContext, handle: &FileHandle, now_ms: i64) -> Result<()>;

    /// Delete all matching files in bounded batches.
    ///
    /// 按条件分批删除文件。
    fn delete_batch(
        &self,
        ctx: &MediaRequestContext,
        query: FileStoreQuery,
        batch_limit: u32,
        now_ms: i64,
    ) -> Result<DeleteBatchResult>;

    /// Resolve a file for download, optionally returning a byte range. The
    /// returned `FileDownload` contains a safe filename and the full file size
    /// so the HTTP layer can set `Content-Range` correctly.
    ///
    /// 解析文件以下载，支持可选字节范围。
    fn resolve_download(
        &self,
        ctx: &MediaRequestContext,
        handle: &FileHandle,
        range: Option<FileRange>,
        filename: Option<String>,
        now_ms: i64,
    ) -> Result<FileDownload>;
}

impl FileDownload {
    /// Return the size of the returned body. For a full download this equals
    /// `total_size`; for a range it is the range length.
    pub fn body_size(&self) -> u64 {
        self.body.len() as u64
    }
}

/// Validate and sanitize a client-supplied filename.
///
/// 校验并清理客户端提供的文件名。
pub fn sanitize_filename(name: &str, fallback: &str) -> String {
    if name.is_empty() {
        return fallback.to_string();
    }
    let mut sanitized = name
        .replace("..", "_")
        .replace(['/', '\\', '\0', '\r', '\n', '"'], "_")
        .replace(|c: char| c.is_ascii_control(), "_");
    sanitized.truncate(255);
    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_path_separators_and_traversal() {
        assert_eq!(sanitize_filename("../etc/passwd", "file"), "__etc_passwd");
        assert_eq!(sanitize_filename("foo/bar\\baz", "file"), "foo_bar_baz");
        assert_eq!(sanitize_filename("", "file"), "file");
    }
}
