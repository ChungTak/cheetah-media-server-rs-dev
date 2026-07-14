//! Snapshot API provider.
//!
//! 快照 API provider。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::command::{DeleteSnapshotRequest, SnapshotQuery, SnapshotRequest};
use cheetah_media_api::error::{MediaErrorCode, Result as MediaResult};
use cheetah_media_api::ids::{MediaKey, SnapshotId};
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_sdk::EngineContext;
use tracing::{info, warn};

use crate::config::SnapshotModuleConfig;
use crate::executor::run_snapshot;
use crate::registry::SnapshotRegistry;

/// Provider that implements `SnapshotApi` for the engine.
///
/// 实现 `SnapshotApi` 的引擎 provider。
pub struct SnapshotMediaProvider {
    ctx: EngineContext,
    config: SnapshotModuleConfig,
    registry: Arc<SnapshotRegistry>,
}

impl SnapshotMediaProvider {
    /// Create a new snapshot provider.
    pub fn new(
        ctx: EngineContext,
        config: SnapshotModuleConfig,
        registry: Arc<SnapshotRegistry>,
    ) -> Self {
        Self {
            ctx,
            config,
            registry,
        }
    }
}

#[async_trait]
impl SnapshotApi for SnapshotMediaProvider {
    async fn take_snapshot(
        &self,
        _ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> MediaResult<SnapshotHandle> {
        let snapshot_id = SnapshotId(format!("snap-{}", generate_id()));
        let media_key = request.media_key.clone();
        let created_at = wall_clock_ms();

        self.registry.upsert(SnapshotInfo {
            snapshot_id: snapshot_id.clone(),
            media_key: media_key.clone(),
            state: SnapshotState::Capturing,
            path_handle: cheetah_media_api::ids::FileHandle(String::new()),
            created_at,
            size_bytes: None,
            format: normalize_format(&request.format),
        });

        match run_snapshot(&self.ctx, &self.config, &request, snapshot_id.clone()).await {
            Ok(outcome) => {
                self.registry.upsert(SnapshotInfo {
                    snapshot_id: snapshot_id.clone(),
                    media_key: media_key.clone(),
                    state: SnapshotState::Completed,
                    path_handle: outcome.path_handle.clone(),
                    created_at: outcome.created_at,
                    size_bytes: Some(outcome.size_bytes),
                    format: outcome.format.clone(),
                });

                info!(%snapshot_id, path_handle = %outcome.path_handle, "snapshot completed");
                Ok(SnapshotHandle {
                    snapshot_id,
                    media_key,
                    state: SnapshotState::Completed,
                    path_handle: outcome.path_handle,
                    download_url: None,
                    created_at: outcome.created_at,
                })
            }
            Err(err) => {
                let state = if err.code == MediaErrorCode::Timeout {
                    SnapshotState::Timeout
                } else {
                    SnapshotState::Failed
                };
                self.registry.upsert(SnapshotInfo {
                    snapshot_id: snapshot_id.clone(),
                    media_key: media_key.clone(),
                    state,
                    path_handle: cheetah_media_api::ids::FileHandle(String::new()),
                    created_at,
                    size_bytes: None,
                    format: normalize_format(&request.format),
                });
                warn!(%snapshot_id, %err, "snapshot failed");
                Err(err)
            }
        }
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        mut query: SnapshotQuery,
    ) -> MediaResult<Page<SnapshotInfo>> {
        query.clamp_page_size();

        let media_key = build_media_key(&query);
        let mut all: Vec<_> = self
            .registry
            .list(media_key.as_ref())
            .into_iter()
            .filter(|info| {
                query
                    .start_time_ms
                    .is_none_or(|start| info.created_at >= start)
                    && query.end_time_ms.is_none_or(|end| info.created_at <= end)
            })
            .collect();
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let total = all.len() as u64;
        let start = (query.page * query.page_size) as usize;
        let items = if start >= all.len() {
            Vec::new()
        } else {
            all.into_iter()
                .skip(start)
                .take(query.page_size as usize)
                .collect()
        };

        Ok(Page {
            items,
            total,
            page: query.page,
            page_size: query.page_size,
            next_cursor: None,
        })
    }

    async fn delete_snapshot_directory(
        &self,
        _ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> MediaResult<()> {
        // The file store handles physical deletion; the registry only drops metadata.
        // A real implementation would batch-delete files through MediaFileStoreApi.
        let removed = self.registry.delete_by_media_key(&request.media_key);
        info!(
            media_key = %request.media_key,
            removed,
            "snapshot directory deleted from registry"
        );
        Ok(())
    }
}

fn build_media_key(query: &SnapshotQuery) -> Option<MediaKey> {
    let vhost = query.vhost.as_deref().unwrap_or("__defaultVhost__");
    let app = query.app.as_deref()?;
    let stream = query.stream.as_deref()?;
    MediaKey::new(vhost, app, stream, None)
        .or_else(|_| MediaKey::with_default_vhost(app, stream, None))
        .ok()
}

fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{n:016x}")
}

fn wall_clock_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_media_key_from_query_uses_default_vhost() {
        let query = SnapshotQuery {
            app: Some("live".to_string()),
            stream: Some("test".to_string()),
            ..Default::default()
        };
        let key = build_media_key(&query).unwrap();
        assert_eq!(key.vhost.0, "__defaultVhost__");
        assert_eq!(key.app.0, "live");
        assert_eq!(key.stream.0, "test");
    }

    #[test]
    fn build_media_key_from_query_honors_custom_vhost() {
        let query = SnapshotQuery {
            vhost: Some("custom".to_string()),
            app: Some("live".to_string()),
            stream: Some("test".to_string()),
            ..Default::default()
        };
        let key = build_media_key(&query).unwrap();
        assert_eq!(key.vhost.0, "custom");
        assert_eq!(key.app.0, "live");
        assert_eq!(key.stream.0, "test");
    }

    #[test]
    fn build_media_key_missing_app_or_stream_returns_none() {
        assert!(build_media_key(&SnapshotQuery::default()).is_none());
        assert!(build_media_key(&SnapshotQuery {
            app: Some("live".to_string()),
            ..Default::default()
        })
        .is_none());
    }

    #[test]
    fn normalize_format_lowercases() {
        assert_eq!(normalize_format("JPG"), "jpg");
    }
}

fn normalize_format(format: &str) -> String {
    format.to_lowercase()
}
