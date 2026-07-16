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
use cheetah_media_api::image::{ImageArtifact, ImageEncodeRequest, ImageFormat};
use cheetah_media_api::media_file_store::FileStoreEntry;
use cheetah_media_api::model::{Page, SnapshotHandle, SnapshotInfo, SnapshotState};
use cheetah_media_api::port::{MediaRequestContext, SnapshotApi};
use cheetah_runtime_api::RuntimeApi;
use cheetah_sdk::EngineContext;
use futures::FutureExt;
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
    /// registered `ImageEncodeApi`. Falls back to MJPEG passthrough only when no
    /// backend is available and the source frame is already a valid JPEG.
    ///
    /// 使用已注册的 `ImageEncodeApi` 将捕获的视频帧编码为请求的图片格式。
    /// 仅当没有后端且源帧本身是有效 JPEG 时才透传 MJPEG。
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

        if let Some(encoder) = self.ctx.media_services.image_encode() {
            let track_info = track_info_for_frame(frame);
            return encoder
                .encode(
                    ctx,
                    ImageEncodeRequest {
                        frame: Arc::clone(frame),
                        track_info,
                        format,
                        quality,
                        max_width: request.max_width,
                        max_height: request.max_height,
                    },
                )
                .await;
        }

        // No image encode backend is registered. Allow MJPEG -> JPEG passthrough
        // after validating the payload is a complete JPEG.
        if frame.codec == CodecId::MJPEG && format == ImageFormat::Jpeg {
            let decoded = image::load_from_memory(&frame.payload)
                .map_err(|e| MediaError::invalid_argument(format!("invalid mjpeg payload: {e}")))?;
            let (width, height) = (decoded.width(), decoded.height());
            return Ok(ImageArtifact {
                payload: frame.payload.clone(),
                content_type: "image/jpeg".to_string(),
                format,
                width,
                height,
            });
        }

        Err(MediaError::unavailable(
            "image encode backend is not available",
        ))
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

        // Re-validate the produced image with an independent decode. This catches
        // corrupt encoder output before it is persisted.
        let decoded = image::load_from_memory(&artifact.payload).map_err(|e| {
            MediaError::storage_failed(format!("encoded image failed validation: {e}"))
        })?;
        if decoded.width() != artifact.width || decoded.height() != artifact.height {
            return Err(MediaError::storage_failed(
                "encoded image dimensions do not match artifact metadata".to_string(),
            ));
        }

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
