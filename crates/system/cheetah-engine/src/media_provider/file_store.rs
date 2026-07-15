//! In-memory managed file store backed by `MediaFileStoreApi`.
//!
//! This implementation keeps a registry mapping `FileHandle` tokens to
//! `FileStoreEntry` metadata. Physical file paths are stored internally but
//! are never exposed in public payloads. Download reads are performed with
//! `std::fs` so the trait stays runtime-neutral; the caller (HTTP adapter) is
//! responsible for not blocking the async executor if this becomes a concern.
//!
//! 受管文件存储的内存实现。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use bytes::Bytes;
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{FileHandle, MediaKey};
use cheetah_media_api::media_file_store::{
    sanitize_filename, DeleteBatchResult, FileDownload, FileRange, FileStoreEntry, FileStoreQuery,
    MediaFileStoreApi,
};
use cheetah_media_api::port::MediaRequestContext;
use parking_lot::RwLock;

/// Maximum number of bytes a single `resolve_download` call will read into
/// memory. Requests for larger ranges must be split by the caller.
///
/// 单次 resolve_download 调用最多读取的字节数。
const MAX_DOWNLOAD_RANGE_BYTES: usize = 8 * 1024 * 1024;

/// Default engine file store.
///
/// 默认引擎文件存储。
#[derive(Default)]
pub struct EngineMediaFileStore {
    files: RwLock<HashMap<String, FileStoreEntry>>,
}

impl EngineMediaFileStore {
    /// Create a new empty file store.
    pub fn new() -> Self {
        Self::default()
    }

    fn generate_handle() -> Result<String> {
        let mut buf = [0u8; 16];
        getrandom::getrandom(&mut buf).map_err(|e| {
            MediaError::unavailable(format!("failed to generate secure file handle: {e}"))
        })?;
        Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
    }

    fn is_authorized(&self, ctx: &MediaRequestContext, entry: &FileStoreEntry) -> bool {
        // Files with no owner and no allowed list are public.
        if entry.owner_principal.is_none() && entry.allowed_principals.is_empty() {
            return true;
        }
        let identity = ctx.principal.as_ref().map(|p| p.identity.as_str());
        match identity {
            None => false,
            Some(identity) => {
                if let Some(owner) = &entry.owner_principal {
                    if owner == identity {
                        return true;
                    }
                }
                entry.allowed_principals.iter().any(|p| p == identity)
            }
        }
    }

    fn matches_query(entry: &FileStoreEntry, query: &FileStoreQuery) -> bool {
        if let Some(key) = &query.media_key {
            if &entry.media_key != key {
                return false;
            }
        }
        if let Some(file_type) = &query.file_type {
            if &entry.file_type != file_type {
                return false;
            }
        }
        if let Some(before) = query.created_before_ms {
            if entry.created_at_ms >= before {
                return false;
            }
        }
        if let Some(after) = query.created_after_ms {
            if entry.created_at_ms <= after {
                return false;
            }
        }
        if let Some(owner) = &query.owner_principal {
            if entry.owner_principal.as_ref() != Some(owner) {
                return false;
            }
        }
        true
    }

    fn read_range(path: &str, range: Option<FileRange>, total: u64) -> Result<(u64, u64, Bytes)> {
        let mut file = File::open(path)
            .map_err(|e| MediaError::storage_failed(format!("failed to open file: {e}")))?;
        if total == 0 {
            return Ok((0, 0, Bytes::new()));
        }
        let max_end = total.saturating_sub(1);
        let (start, end) = match range {
            Some(r) if r.is_suffix => {
                if r.start == 0 {
                    return Err(MediaError::invalid_argument("invalid suffix range"));
                }
                let start = total.saturating_sub(r.start).min(total);
                (start, max_end)
            }
            Some(r) => {
                let start = r.start.min(total);
                let end = r.end.map(|e| e.min(max_end)).unwrap_or_else(|| max_end);
                if start > end {
                    return Err(MediaError::invalid_argument("invalid byte range"));
                }
                (start, end)
            }
            None => (0, max_end),
        };
        if start > total {
            return Ok((start, end, Bytes::new()));
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|e| MediaError::storage_failed(format!("failed to seek file: {e}")))?;
        let size = (end.saturating_sub(start).saturating_add(1)) as usize;
        if size > MAX_DOWNLOAD_RANGE_BYTES {
            return Err(MediaError::invalid_argument(
                "requested download range exceeds maximum allowed size",
            ));
        }
        let mut buf = vec![0u8; size];
        file.read_exact(&mut buf)
            .map_err(|e| MediaError::storage_failed(format!("failed to read file: {e}")))?;
        Ok((start, end, Bytes::from(buf)))
    }
}

impl MediaFileStoreApi for EngineMediaFileStore {
    fn register_file(
        &self,
        ctx: &MediaRequestContext,
        entry: FileStoreEntry,
    ) -> Result<FileHandle> {
        if entry.absolute_path.is_empty() {
            return Err(MediaError::invalid_argument("absolute_path is required"));
        }
        if entry.size_bytes == 0 {
            return Err(MediaError::invalid_argument(
                "file size must be greater than 0",
            ));
        }
        let handle = Self::generate_handle()?;
        let file_entry = FileStoreEntry {
            owner_principal: entry
                .owner_principal
                .or_else(|| ctx.principal.as_ref().map(|p| p.identity.clone())),
            ..entry
        };
        self.files.write().insert(handle.clone(), file_entry);
        Ok(FileHandle(handle))
    }

    fn resolve_for_read(
        &self,
        ctx: &MediaRequestContext,
        handle: &FileHandle,
        resource_scope: Option<&MediaKey>,
        now_ms: i64,
    ) -> Result<FileStoreEntry> {
        let entry = self
            .files
            .read()
            .get(&handle.0)
            .cloned()
            .ok_or_else(|| MediaError::not_found(format!("file not found: {}", handle.0)))?;

        if let Some(expiry) = entry.expires_at_ms {
            if now_ms > expiry {
                return Err(MediaError::not_found(format!(
                    "file handle expired: {}",
                    handle.0
                )));
            }
        }

        if !self.is_authorized(ctx, &entry) {
            return Err(MediaError::new(
                cheetah_media_api::error::MediaErrorCode::PermissionDenied,
                format!("principal denied for file: {}", handle.0),
            ));
        }

        if let Some(scope) = resource_scope {
            if &entry.media_key != scope {
                return Err(MediaError::new(
                    cheetah_media_api::error::MediaErrorCode::PermissionDenied,
                    "file does not belong to requested resource scope",
                ));
            }
        }

        Ok(entry)
    }

    fn delete(&self, ctx: &MediaRequestContext, handle: &FileHandle, _now_ms: i64) -> Result<()> {
        let entry = self
            .files
            .read()
            .get(&handle.0)
            .cloned()
            .ok_or_else(|| MediaError::not_found(format!("file not found: {}", handle.0)))?;
        if !self.is_authorized(ctx, &entry) {
            return Err(MediaError::new(
                cheetah_media_api::error::MediaErrorCode::PermissionDenied,
                format!("principal denied for file: {}", handle.0),
            ));
        }
        self.files.write().remove(&handle.0);
        Ok(())
    }

    fn delete_batch(
        &self,
        ctx: &MediaRequestContext,
        query: FileStoreQuery,
        batch_limit: u32,
        now_ms: i64,
    ) -> Result<DeleteBatchResult> {
        let mut deleted = 0u64;
        let mut failed = 0u64;
        let limit = batch_limit.max(1) as usize;
        let mut to_remove = Vec::with_capacity(limit);

        {
            let files = self.files.read();
            for (handle, entry) in files.iter() {
                if to_remove.len() >= limit {
                    break;
                }
                if !Self::matches_query(entry, &query) {
                    continue;
                }
                if let Some(expiry) = entry.expires_at_ms {
                    if now_ms > expiry {
                        to_remove.push(handle.clone());
                        continue;
                    }
                }
                if !self.is_authorized(ctx, entry) {
                    failed += 1;
                    continue;
                }
                to_remove.push(handle.clone());
            }
        }

        {
            let mut files = self.files.write();
            for handle in to_remove {
                if files.remove(&handle).is_some() {
                    deleted += 1;
                } else {
                    failed += 1;
                }
            }
        }

        Ok(DeleteBatchResult { deleted, failed })
    }

    fn resolve_download(
        &self,
        ctx: &MediaRequestContext,
        handle: &FileHandle,
        range: Option<FileRange>,
        filename: Option<String>,
        now_ms: i64,
    ) -> Result<FileDownload> {
        let entry = self.resolve_for_read(ctx, handle, None, now_ms)?;
        let total = std::fs::metadata(&entry.absolute_path)
            .map(|m| m.len())
            .map_err(|e| MediaError::storage_failed(format!("failed to stat file: {e}")))?;

        let safe_name = sanitize_filename(
            filename.as_deref().unwrap_or(&entry.media_key.stream.0),
            &handle.0,
        );

        let (start, end, body) = Self::read_range(&entry.absolute_path, range, total)?;

        let content_type = if entry.content_type.is_empty() {
            guess_content_type(&safe_name)
        } else {
            entry.content_type.clone()
        };

        let effective_range = range.map(|_| FileRange::bounded(start, end));

        Ok(FileDownload {
            content_type,
            total_size: total,
            body,
            filename: safe_name,
            range: effective_range,
        })
    }
}

fn guess_content_type(name: &str) -> String {
    if name.ends_with(".mp4") {
        "video/mp4".to_string()
    } else if name.ends_with(".flv") {
        "video/x-flv".to_string()
    } else if name.ends_with(".m3u8") || name.ends_with(".hls") {
        "application/vnd.apple.mpegurl".to_string()
    } else if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if name.ends_with(".png") {
        "image/png".to_string()
    } else if name.ends_with(".ts") {
        "video/mp2t".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::ids::MediaKey;

    fn make_entry(path: &str) -> FileStoreEntry {
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        FileStoreEntry {
            media_key: MediaKey::new("__defaultVhost__", "live", "test", None).unwrap(),
            file_type: "record".to_string(),
            content_type: "video/mp4".to_string(),
            size_bytes,
            created_at_ms: 1000,
            expires_at_ms: None,
            absolute_path: path.to_string(),
            owner_principal: None,
            allowed_principals: Vec::new(),
        }
    }

    fn write_temp_file(content: &[u8]) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::SystemTime;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!(
            "cheetah-file-store-test-{}-{}-{}",
            std::process::id(),
            now,
            id
        ));
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn register_and_resolve_file() {
        let path = write_temp_file(b"test");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let entry = make_entry(&path);
        let handle = store.register_file(&ctx, entry.clone()).unwrap();
        assert!(!handle.0.is_empty());

        let resolved = store.resolve_for_read(&ctx, &handle, None, 2000).unwrap();
        assert_eq!(resolved.absolute_path, path);
    }

    #[test]
    fn expired_file_is_not_found() {
        let path = write_temp_file(b"test");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let mut entry = make_entry(&path);
        entry.expires_at_ms = Some(500);
        let handle = store.register_file(&ctx, entry).unwrap();
        let err = store
            .resolve_for_read(&ctx, &handle, None, 1000)
            .unwrap_err();
        assert_eq!(err.code, cheetah_media_api::error::MediaErrorCode::NotFound);
    }

    #[test]
    fn download_returns_full_body_and_total_size() {
        let path = write_temp_file(b"0123456789");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let handle = store.register_file(&ctx, make_entry(&path)).unwrap();
        let dl = store
            .resolve_download(&ctx, &handle, None, Some("my/file.mp4".to_string()), 2000)
            .unwrap();
        assert_eq!(dl.body, bytes::Bytes::from_static(b"0123456789"));
        assert_eq!(dl.total_size, 10);
        assert_eq!(dl.filename, "my_file.mp4");
    }

    #[test]
    fn download_range_returns_subset() {
        let path = write_temp_file(b"0123456789");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let handle = store.register_file(&ctx, make_entry(&path)).unwrap();
        let dl = store
            .resolve_download(&ctx, &handle, Some(FileRange::bounded(2, 5)), None, 2000)
            .unwrap();
        assert_eq!(dl.body, bytes::Bytes::from_static(b"2345"));
        assert_eq!(dl.total_size, 10);
    }

    #[test]
    fn download_zero_length_file_returns_empty_body() {
        let path = write_temp_file(b"0123456789");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let handle = store.register_file(&ctx, make_entry(&path)).unwrap();
        // Simulate external truncation after registration.
        std::fs::write(&path, b"").unwrap();
        let dl = store
            .resolve_download(&ctx, &handle, None, None, 2000)
            .unwrap();
        assert_eq!(dl.body, bytes::Bytes::new());
        assert_eq!(dl.total_size, 0);
    }

    #[test]
    fn authenticated_user_can_access_public_file() {
        let path = write_temp_file(b"public");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext {
            principal: Some(cheetah_media_api::Principal {
                identity: "alice".to_string(),
                scopes: vec![cheetah_media_api::MediaScope::FileRead],
                resource_grants: Vec::new(),
            }),
            ..Default::default()
        };
        let handle = store.register_file(&ctx, make_entry(&path)).unwrap();
        let dl = store
            .resolve_download(&ctx, &handle, None, None, 2000)
            .unwrap();
        assert_eq!(dl.body, bytes::Bytes::from_static(b"public"));
    }

    #[test]
    fn download_range_capped_to_max_size() {
        let oversized = vec![0u8; MAX_DOWNLOAD_RANGE_BYTES + 1];
        let path = write_temp_file(&oversized);
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let handle = store.register_file(&ctx, make_entry(&path)).unwrap();

        // Full download of a file larger than the cap is rejected.
        let err = store
            .resolve_download(&ctx, &handle, None, None, 2000)
            .unwrap_err();
        assert_eq!(
            err.code,
            cheetah_media_api::error::MediaErrorCode::InvalidArgument
        );

        // A range at the cap is accepted.
        let dl = store
            .resolve_download(
                &ctx,
                &handle,
                Some(FileRange::bounded(0, MAX_DOWNLOAD_RANGE_BYTES as u64 - 1)),
                None,
                2000,
            )
            .unwrap();
        assert_eq!(dl.body.len(), MAX_DOWNLOAD_RANGE_BYTES);
    }

    #[test]
    fn batch_delete_respects_limit() {
        let path = write_temp_file(b"x");
        let store = EngineMediaFileStore::new();
        let ctx = MediaRequestContext::default();
        let h1 = store.register_file(&ctx, make_entry(&path)).unwrap();
        let h2 = store
            .register_file(
                &ctx,
                FileStoreEntry {
                    media_key: MediaKey::new("__defaultVhost__", "live", "other", None).unwrap(),
                    ..make_entry(&path)
                },
            )
            .unwrap();
        let h3 = store.register_file(&ctx, make_entry(&path)).unwrap();

        let result = store
            .delete_batch(
                &ctx,
                FileStoreQuery {
                    media_key: Some(
                        MediaKey::new("__defaultVhost__", "live", "test", None).unwrap(),
                    ),
                    ..Default::default()
                },
                2,
                2000,
            )
            .unwrap();
        assert_eq!(result.deleted, 2);

        store.resolve_for_read(&ctx, &h1, None, 2000).unwrap_err();
        store.resolve_for_read(&ctx, &h3, None, 2000).unwrap_err();
        store.resolve_for_read(&ctx, &h2, None, 2000).unwrap();
    }
}
