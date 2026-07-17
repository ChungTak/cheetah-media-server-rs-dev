use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_codec::{CodecId, FrameFlags, MediaKind, MonoTime, TrackId, TrackInfo, TrackReadiness};
use cheetah_media_api::command::{
    DeleteSnapshotRequest, SnapshotQuery, SnapshotRequest, SubscribeRequest,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::event::{EventHeader, MediaEvent, SnapshotCompleted};
use cheetah_media_api::ids::MediaSchema;
use cheetah_media_api::image::{ImageArtifact, ImageFormat};
use cheetah_media_api::media_file_store::{DeleteBatchResult, DeleteFailure, FileStoreEntry};
use cheetah_media_api::model::{
    AdmissionAction, AdmissionRequest, Decision, Page, SnapshotHandle, SnapshotInfo, SnapshotState,
};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_media_api::processing::{ImageInput, ImageOperation, ImageProcessRequest};
use cheetah_sdk::{Deadline, EngineContext, RuntimeApi};
use futures::FutureExt;
use std::collections::HashMap;
use tracing::debug;

use crate::config::SnapshotModuleConfig;
use crate::registry::SnapshotRegistry;

/// Production snapshot provider backed by the engine data plane, image encoder,
/// and file store.
///
/// 由引擎数据面、图片编码器和文件存储支撑的生产快照 provider。
#[derive(Clone)]
pub struct SnapshotMediaProvider {
    ctx: EngineContext,
    registry: Arc<SnapshotRegistry>,
    config: SnapshotModuleConfig,
}

impl SnapshotMediaProvider {
    pub fn new(
        ctx: EngineContext,
        registry: Arc<SnapshotRegistry>,
        config: SnapshotModuleConfig,
    ) -> Self {
        Self {
            ctx,
            registry,
            config,
        }
    }

    /// Encode a captured video frame into the requested image format using the
    /// registered `ImageProcessApi`.
    ///
    /// 使用已注册的 `ImageProcessApi` 将捕获的视频帧编码为请求的图片格式。
    async fn encode_frame(
        &self,
        ctx: &MediaRequestContext,
        frame: &Arc<cheetah_codec::AVFrame>,
        request: &SnapshotRequest,
    ) -> Result<ImageArtifact> {
        let format = request
            .format
            .parse::<ImageFormat>()
            .map_err(MediaError::invalid_argument)?;
        let quality = request.quality.unwrap_or(90);

        let processor = self
            .ctx
            .media_services
            .image_process()
            .ok_or_else(|| MediaError::unavailable("image process backend is not available"))?;

        let mut operations = Vec::new();
        if request.max_width.is_some() || request.max_height.is_some() {
            operations.push(ImageOperation::Fit {
                width: request.max_width.unwrap_or(0),
                height: request.max_height.unwrap_or(0),
            });
        }

        let process_request = ImageProcessRequest::new(
            ImageInput::Frame {
                frame: Arc::clone(frame),
                track: track_info_for_frame(frame),
            },
            format,
        )
        .with_quality(quality)
        .with_operations(operations);

        processor.process(ctx, process_request).await
    }
}

#[async_trait]
impl SnapshotApi for SnapshotMediaProvider {
    async fn take_snapshot(
        &self,
        ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> Result<SnapshotHandle> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        if self.registry.is_full() {
            return Err(MediaError::unavailable("snapshot capacity exceeded"));
        }

        let timeout_ms = if request.timeout_ms == 0 {
            self.config.default_timeout_ms
        } else {
            request.timeout_ms
        };

        let control = self
            .ctx
            .media_services
            .control()
            .ok_or_else(|| MediaError::unavailable("media control"))?;
        let online_state = control.is_media_online(ctx, &request.media_key).await?;
        if !matches!(online_state, cheetah_media_api::model::OnlineState::Online) {
            return Err(MediaError::not_found(format!(
                "media not online: {}/{}/{}",
                request.media_key.vhost.0, request.media_key.app.0, request.media_key.stream.0
            )));
        }

        // Snapshot opens a play-side subscription; admit before the lease is created.
        if let Some(admission) = self.ctx.media_services.admission() {
            let decision = admission
                .authorize(
                    ctx,
                    AdmissionRequest {
                        action: AdmissionAction::Play,
                        principal: ctx.principal.clone(),
                        resource: request.media_key.clone(),
                        protocol: "snapshot".to_string(),
                        source_address: None,
                        params: HashMap::new(),
                    },
                )
                .await?;
            if let Decision::Deny { code, reason } = decision {
                return Err(MediaError::new(code, reason));
            }
        }

        let mut subscriber = self
            .ctx
            .media_data_plane
            .open_frame_subscriber(
                ctx,
                SubscribeRequest {
                    media_key: request.media_key.clone(),
                    output_schema: MediaSchema::Rtmp,
                    subscriber_kind: "snapshot".to_string(),
                    start_policy: "keyframe".to_string(),
                    protocol: "rtmp".to_string(),
                    remote_endpoint: None,
                    auth_context: Default::default(),
                },
            )
            .await
            .map_err(|e| MediaError::unavailable(format!("open subscriber: {e}")))?;

        let capture = capture_keyframe(&self.ctx.runtime_api, &mut *subscriber, timeout_ms).await;
        let _ = subscriber.close().await;

        let frame = match capture {
            CaptureResult::Ok { frame } => frame,
            CaptureResult::Timeout => {
                return Err(MediaError::new(
                    MediaErrorCode::Timeout,
                    "snapshot keyframe wait timed out",
                ));
            }
            CaptureResult::NoVideo => {
                return Err(MediaError::unsupported("snapshot requires a video track"));
            }
            CaptureResult::Error(e) => return Err(e),
        };

        let artifact = self.encode_frame(ctx, &frame, &request).await?;
        let format_ext = match artifact.format {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        };

        let snapshot_id = self.registry.generate_id();
        let file_name = format!("{}.{}", snapshot_id.0, format_ext);
        let dir = PathBuf::from(&self.config.root_path)
            .join(&request.media_key.app.0)
            .join(&request.media_key.stream.0);
        if let Err(e) = fs::create_dir_all(&dir) {
            return Err(MediaError::storage_failed(format!(
                "create snapshot directory: {e}"
            )));
        }
        let abs_path = dir.join(&file_name);
        let tmp_path = dir.join(format!("{file_name}.tmp"));

        if let Err(e) = write_atomic(&tmp_path, &abs_path, &artifact.payload) {
            let _ = fs::remove_file(&tmp_path);
            let _ = fs::remove_file(&abs_path);
            return Err(e);
        }

        let size_bytes = artifact.payload.len() as u64;
        let created_at = now_ms();
        let entry = FileStoreEntry {
            media_key: request.media_key.clone(),
            file_type: "snapshot".to_string(),
            content_type: artifact.content_type.clone(),
            size_bytes,
            created_at_ms: created_at,
            expires_at_ms: None,
            absolute_path: abs_path.to_string_lossy().into_owned(),
            owner_principal: ctx.principal.as_ref().map(|p| p.identity.clone()),
            allowed_principals: Vec::new(),
        };

        let file_handle = match self.ctx.media_file_store.register_file(ctx, entry) {
            Ok(h) => h,
            Err(e) => {
                let _ = fs::remove_file(&abs_path);
                return Err(MediaError::internal(format!("register snapshot file: {e}")));
            }
        };

        let info = SnapshotInfo {
            snapshot_id: snapshot_id.clone(),
            media_key: request.media_key.clone(),
            state: SnapshotState::Completed,
            path_handle: file_handle.clone(),
            created_at,
            size_bytes: Some(size_bytes),
            format: format_ext.to_string(),
            width: artifact.width,
            height: artifact.height,
        };
        self.registry.insert(info);

        let _ = self
            .ctx
            .media_event_bus
            .publish(MediaEvent::SnapshotCompleted(SnapshotCompleted {
                header: EventHeader {
                    event_id: format!("snapshot-{}", snapshot_id.0),
                    occurred_at: created_at,
                    sequence: None,
                    media_key: Some(request.media_key.clone()),
                    source: "snapshot".to_string(),
                    correlation_id: ctx.correlation_id.clone(),
                },
                snapshot_id: snapshot_id.clone(),
                path_handle: file_handle.clone(),
                url: None,
                format: format_ext.to_string(),
                width: artifact.width,
                height: artifact.height,
                size_bytes,
            }));

        debug!(
            snapshot_id = %snapshot_id.0,
            format = format_ext,
            width = artifact.width,
            height = artifact.height,
            bytes = size_bytes,
            "snapshot captured"
        );

        Ok(SnapshotHandle {
            snapshot_id,
            media_key: request.media_key,
            state: SnapshotState::Completed,
            path_handle: file_handle,
            download_url: None,
            created_at,
        })
    }

    async fn query_snapshots(
        &self,
        _ctx: &MediaRequestContext,
        mut query: SnapshotQuery,
    ) -> Result<Page<SnapshotInfo>> {
        query.clamp_page_size();
        let (items, total) = self.registry.query(&query);
        Ok(Page {
            items,
            page: query.page.max(1),
            page_size: query.page_size,
            total,
            next_cursor: None,
        })
    }

    async fn delete_snapshots(
        &self,
        ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> Result<DeleteBatchResult> {
        Deadline::from_context(ctx)
            .check()
            .map_err(|e| MediaError::unavailable(e.to_string()))?;
        let root = PathBuf::from(&self.config.root_path);

        let mut candidates = self.registry.find_by_media_key(&request.media_key);
        candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let matched = candidates.len() as u64;

        if let Some(retain) = request.retain_count {
            let retain = retain as usize;
            if candidates.len() > retain {
                candidates = candidates.split_off(retain);
            } else {
                candidates.clear();
            }
        }

        let mut deleted = 0u64;
        let mut failed = 0u64;
        let mut failures = Vec::new();

        for info in candidates {
            let handle = &info.path_handle;

            // Resolve the file store entry (auth check, ignore expiry for cleanup).
            let entry =
                match self
                    .ctx
                    .media_file_store
                    .resolve_for_read(ctx, handle, None, i64::MIN)
                {
                    Ok(e) => e,
                    Err(e) => {
                        failed += 1;
                        failures.push(DeleteFailure {
                            handle: handle.clone(),
                            reason: format!("failed to resolve file handle: {e}"),
                        });
                        continue;
                    }
                };

            // Ensure the file is located under the configured managed root and is
            // not a symlink or otherwise escaped path. Use the returned canonical
            // path for removal to avoid re-resolving the original path.
            let canonical_path = match is_safe_snapshot_path(Path::new(&entry.absolute_path), &root)
            {
                Ok(p) => p,
                Err(reason) => {
                    failed += 1;
                    failures.push(DeleteFailure {
                        handle: handle.clone(),
                        reason,
                    });
                    continue;
                }
            };

            // Remove the physical file first. If it cannot be removed we keep the
            // file-store and snapshot registry entries so the deletion can be retried.
            if let Err(e) = fs::remove_file(&canonical_path) {
                if e.kind() != io::ErrorKind::NotFound {
                    failed += 1;
                    failures.push(DeleteFailure {
                        handle: handle.clone(),
                        reason: format!("failed to delete physical file: {e}"),
                    });
                    continue;
                }
            }

            // Unregister the file and drop the snapshot entry once the physical file
            // is gone. If the file was already missing we still clean the metadata.
            if let Err(e) = self.ctx.media_file_store.delete(ctx, handle, now_ms()) {
                failed += 1;
                failures.push(DeleteFailure {
                    handle: handle.clone(),
                    reason: format!("failed to unregister file: {e}"),
                });
                continue;
            }

            self.registry.remove(&info.snapshot_id.0);
            deleted += 1;
        }

        Ok(DeleteBatchResult {
            matched,
            deleted,
            failed,
            failures,
        })
    }
}

enum CaptureResult {
    Ok { frame: Arc<cheetah_codec::AVFrame> },
    Timeout,
    NoVideo,
    Error(MediaError),
}

async fn capture_keyframe(
    runtime_api: &Arc<dyn RuntimeApi>,
    subscriber: &mut dyn cheetah_sdk::media_data_plane::MediaFrameSubscriber,
    timeout_ms: u64,
) -> CaptureResult {
    let deadline = MonoTime::from_micros(runtime_api.now().as_micros() + timeout_ms * 1_000);
    let mut timer = runtime_api.sleep_until(deadline);
    let mut saw_video = false;

    loop {
        let recv = subscriber.recv();
        futures::pin_mut!(recv);
        futures::select! {
            frame = recv.fuse() => {
                match frame {
                    Ok(Some(frame)) => {
                        if frame.media_kind != MediaKind::Video {
                            continue;
                        }
                        saw_video = true;
                        let is_key = frame.flags.contains(FrameFlags::KEY)
                            || frame.codec == CodecId::MJPEG
                            || frame.codec == CodecId::VP8;
                        if !is_key {
                            continue;
                        }
                        return CaptureResult::Ok {
                            frame: Arc::clone(&frame),
                        };
                    }
                    Ok(None) => {
                        return if saw_video {
                            CaptureResult::Error(MediaError::unavailable(
                                "subscriber closed before keyframe",
                            ))
                        } else {
                            CaptureResult::NoVideo
                        };
                    }
                    Err(e) => return CaptureResult::Error(e),
                }
            }
            _ = timer.wait().fuse() => {
                return if saw_video {
                    CaptureResult::Timeout
                } else {
                    CaptureResult::NoVideo
                };
            }
        }
    }
}

fn track_info_for_frame(frame: &cheetah_codec::AVFrame) -> TrackInfo {
    let mut info = TrackInfo::new(TrackId(0), frame.media_kind, frame.codec, 90_000);
    info.readiness = TrackReadiness::Ready;
    info
}

/// Write `data` to `tmp_path`, flush and fsync, then atomically rename to
/// `final_path`. On error the temporary file is removed.
///
/// 将 `data` 写入 `tmp_path`，flush 并 fsync，然后原子重命名为 `final_path`。
/// 出错时删除临时文件。
fn write_atomic(tmp_path: &Path, final_path: &Path, data: &[u8]) -> Result<()> {
    let mut file = File::create(tmp_path)
        .map_err(|e| MediaError::storage_failed(format!("create snapshot temp file: {e}")))?;
    if let Err(e) = (|| -> io::Result<()> {
        file.write_all(data)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })() {
        let _ = fs::remove_file(tmp_path);
        return Err(MediaError::storage_failed(format!("write snapshot: {e}")));
    }
    drop(file);

    fs::rename(tmp_path, final_path).map_err(|e| {
        let _ = fs::remove_file(tmp_path);
        MediaError::storage_failed(format!("finalize snapshot: {e}"))
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Verify that `path` is an absolute path located under `root`, contains no
/// parent-directory traversal, and has no symlink components below `root`.
///
/// Returns the canonical path that should be used for removal so the caller
/// does not re-resolve the original path after this check.
///
/// 验证 `path` 是绝对路径、位于 `root` 下、不含 `..` 且 root 以下无符号链接。
/// 返回规范路径供删除调用使用，避免检查后再重新解析原路径。
fn is_safe_snapshot_path(path: &Path, root: &Path) -> std::result::Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("path is not absolute".to_string());
    }

    let canonical_root =
        std::fs::canonicalize(root).map_err(|e| format!("failed to canonicalize root: {e}"))?;
    // Use the original root's component count so the symlink-check boundary
    // aligns with the components of the input path, even when the root itself
    // contains symlinks (e.g. /tmp on macOS).
    let root_component_count = root
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .count();
    let total_components = path.components().count();

    let mut current = PathBuf::new();
    let mut idx = 0usize;
    for component in path.components() {
        match component {
            std::path::Component::Prefix(p) => {
                current.push(p.as_os_str());
                idx += 1;
            }
            std::path::Component::RootDir => {
                current.push(component.as_os_str());
                idx += 1;
            }
            std::path::Component::CurDir => continue,
            std::path::Component::ParentDir => {
                return Err("path contains parent directory reference".to_string());
            }
            std::path::Component::Normal(name) => {
                current.push(name);
                idx += 1;
            }
        }

        // Only check components strictly below the configured root. Root itself
        // may legitimately contain symlinks (e.g. /tmp on macOS).
        if idx > root_component_count {
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err("path contains symlink".to_string());
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound && idx == total_components => {}
                Err(e) => return Err(format!("failed to stat path component: {e}")),
            }
        }
    }

    // Canonicalize the final path and compare with the canonical root. If the
    // final file no longer exists, canonicalize its parent and append the name
    // so missing files can still be cleaned up.
    let canonical_path = std::fs::canonicalize(&current).or_else(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            let file_name = current
                .file_name()
                .ok_or_else(|| "invalid path: no file name".to_string())?;
            let parent = current
                .parent()
                .ok_or_else(|| "invalid path: no parent directory".to_string())?;
            let canonical_parent = std::fs::canonicalize(parent)
                .map_err(|e| format!("failed to canonicalize parent: {e}"))?;
            Ok::<_, String>(canonical_parent.join(file_name))
        } else {
            Err(format!("failed to canonicalize path: {e}"))
        }
    })?;

    if !canonical_path.starts_with(&canonical_root) {
        return Err("path escapes managed root".to_string());
    }

    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_name(prefix: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{prefix}-{now}-{}", std::process::id())
    }

    #[cfg(unix)]
    #[test]
    fn safe_snapshot_path_accepts_symlinked_root() {
        use std::os::unix::fs::symlink;

        let real = std::env::temp_dir().join(unique_name("cheetah_real_root"));
        let link = std::env::temp_dir().join(unique_name("cheetah_link_root"));
        let _ = std::fs::remove_dir_all(&real);
        let _ = std::fs::remove_file(&link);
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        let file = link.join("live").join("stream").join("snap.jpg");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();

        let canonical = is_safe_snapshot_path(&file, &link).unwrap();
        assert!(canonical.starts_with(&real));

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&link);
        let _ = std::fs::remove_dir_all(&real);
    }

    #[cfg(unix)]
    #[test]
    fn safe_snapshot_path_rejects_symlink_below_root() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(unique_name("cheetah_safe_root"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let outside = std::env::temp_dir().join(unique_name("cheetah_outside"));
        std::fs::write(&outside, b"x").unwrap();

        let link = root.join("escape.jpg");
        symlink(&outside, &link).unwrap();

        let err = is_safe_snapshot_path(&link, &root).unwrap_err();
        assert!(err.contains("symlink"), "{err}");

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }
}
