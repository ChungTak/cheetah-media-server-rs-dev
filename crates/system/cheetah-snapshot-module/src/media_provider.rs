use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_codec::{CodecId, FrameFlags, MediaKind, MonoTime};
use cheetah_media_api::command::{
    DeleteSnapshotRequest, SnapshotQuery, SnapshotRequest, SubscribeRequest,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode, Result};
use cheetah_media_api::event::{EventHeader, MediaEvent, SnapshotCompleted};
use cheetah_media_api::ids::MediaSchema;
use cheetah_media_api::media_file_store::FileStoreEntry;
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::EngineContext;
use futures::FutureExt;
use tracing::debug;

use crate::config::SnapshotModuleConfig;
use crate::registry::SnapshotRegistry;

/// Production snapshot provider backed by the engine data plane and file store.
///
/// 由引擎数据面和文件存储支撑的生产快照 provider。
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
}

#[async_trait]
impl SnapshotApi for SnapshotMediaProvider {
    async fn take_snapshot(
        &self,
        ctx: &MediaRequestContext,
        request: SnapshotRequest,
    ) -> Result<SnapshotHandle> {
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
                    auth_context: Default::default(),
                },
            )
            .await
            .map_err(|e| MediaError::unavailable(format!("open subscriber: {e}")))?;

        let capture = capture_keyframe(&self.ctx.runtime_api, &mut *subscriber, timeout_ms).await;
        let _ = subscriber.close().await;

        let (payload, codec, content_type, format) = match capture {
            CaptureResult::Ok {
                payload,
                codec,
                content_type,
                format,
            } => (payload, codec, content_type, format),
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

        let ext = if format == "jpg" { "jpg" } else { "bin" };
        let snapshot_id = self.registry.generate_id();
        let file_name = format!("{}.{}", snapshot_id.0, ext);
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
        if let Err(e) = fs::write(&tmp_path, &payload) {
            return Err(MediaError::storage_failed(format!("write snapshot: {e}")));
        }
        if let Err(e) = fs::rename(&tmp_path, &abs_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(MediaError::storage_failed(format!(
                "finalize snapshot: {e}"
            )));
        }

        let size_bytes = payload.len() as u64;
        let created_at = now_ms();
        let entry = FileStoreEntry {
            media_key: request.media_key.clone(),
            file_type: "snapshot".to_string(),
            content_type,
            size_bytes,
            created_at_ms: created_at,
            expires_at_ms: None,
            absolute_path: abs_path.to_string_lossy().into_owned(),
            owner_principal: ctx.principal.as_ref().map(|p| p.identity.clone()),
            allowed_principals: Vec::new(),
        };
        let file_handle = self
            .ctx
            .media_file_store
            .register_file(ctx, entry)
            .map_err(|e| MediaError::internal(format!("register snapshot file: {e}")))?;

        let info = SnapshotInfo {
            snapshot_id: snapshot_id.clone(),
            media_key: request.media_key.clone(),
            state: SnapshotState::Completed,
            path_handle: file_handle.clone(),
            created_at,
            size_bytes: Some(size_bytes),
            format,
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
            }));

        debug!(
            snapshot_id = %snapshot_id.0,
            codec = ?codec,
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

    async fn delete_snapshot_directory(
        &self,
        _ctx: &MediaRequestContext,
        request: DeleteSnapshotRequest,
    ) -> Result<()> {
        let _ = self.registry.delete_matching(&request.media_key);
        Ok(())
    }
}

enum CaptureResult {
    Ok {
        payload: bytes::Bytes,
        codec: CodecId,
        content_type: String,
        format: String,
    },
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
                        let (content_type, format) = if frame.codec == CodecId::MJPEG {
                            ("image/jpeg".to_string(), "jpg".to_string())
                        } else {
                            ("application/octet-stream".to_string(), "bin".to_string())
                        };
                        return CaptureResult::Ok {
                            payload: frame.payload.clone(),
                            codec: frame.codec,
                            content_type,
                            format,
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
