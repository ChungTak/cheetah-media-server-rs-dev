//! Snapshot capture executor.
//!
//! 截图执行器。

use std::path::PathBuf;

use bytes::Bytes;
use cheetah_codec::{AVFrame, CodecId, FrameFormat, MediaKind, MonoTime};
use cheetah_media_api::command::SnapshotRequest;
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::event::{EventHeader, MediaEvent, SnapshotCompleted};
use cheetah_media_api::ids::{FileHandle, MediaKey, SnapshotId, StreamKeyBridge};
use cheetah_sdk::{EngineContext, SubscriberOptions, SubscriberSource};
use futures::future::FutureExt;
use futures::pin_mut;
use tokio::fs;
use tracing::info;

use crate::config::SnapshotModuleConfig;

/// Run a snapshot capture and return the outcome.
///
/// 运行截图并返回结果。
pub async fn run_snapshot(
    ctx: &EngineContext,
    config: &SnapshotModuleConfig,
    request: &SnapshotRequest,
    snapshot_id: SnapshotId,
) -> Result<SnapshotOutcome, MediaError> {
    let media_key = request.media_key.clone();
    let (namespace, path) = StreamKeyBridge::to_namespace_path(&media_key);
    let stream_key = cheetah_sdk::StreamKey::new(&namespace, &path);

    let format = normalize_format(&request.format);
    if !is_format_supported(&format) {
        return Err(MediaError::unsupported(format!(
            "snapshot format `{format}` is not supported"
        )));
    }

    let timeout_ms = if request.timeout_ms > 0 {
        request.timeout_ms
    } else {
        config.default_timeout_ms
    };

    let mut options = SubscriberOptions::default();
    options.media_filter.enable_video = true;
    options.media_filter.enable_audio = false;
    options.queue_capacity = 16;

    let mut subscriber = ctx
        .subscriber_api
        .subscribe(stream_key, options)
        .await
        .map_err(|e| MediaError::unavailable(format!("failed to open subscriber: {e}")))?;

    let created_at = wall_clock_ms();
    let deadline = MonoTime::from_micros(
        ctx.runtime_api
            .now()
            .as_micros()
            .saturating_add(timeout_ms.saturating_mul(1_000)),
    );

    let frame = match wait_for_keyframe(&mut *subscriber, deadline, ctx.runtime_api.as_ref()).await
    {
        Ok(f) => f,
        Err(e) => {
            let _ = subscriber.close().await;
            return Err(e);
        }
    };

    // For now only MJPEG payloads can be captured directly as a still image.
    if frame.codec != CodecId::MJPEG || frame.format != FrameFormat::MjpegFrame {
        let _ = subscriber.close().await;
        return Err(MediaError::unsupported(
            "snapshot source is not a directly encodable MJPEG stream",
        ));
    }

    let payload = frame.payload.clone();
    let _ = subscriber.close().await;

    let outcome = write_and_register(
        ctx,
        config,
        &media_key,
        &snapshot_id,
        created_at,
        &format,
        payload,
    )
    .await?;

    publish_snapshot_completed(ctx, &outcome);
    Ok(outcome)
}

/// Result of a successful snapshot capture.
///
/// 成功截图的结果。
pub struct SnapshotOutcome {
    pub snapshot_id: SnapshotId,
    pub media_key: MediaKey,
    pub path_handle: FileHandle,
    pub created_at: i64,
    pub size_bytes: u64,
    pub format: String,
}

async fn wait_for_keyframe(
    subscriber: &mut dyn SubscriberSource,
    deadline: MonoTime,
    runtime: &dyn cheetah_runtime_api::RuntimeApi,
) -> Result<AVFrame, MediaError> {
    loop {
        let recv_fut = subscriber.recv().fuse();
        let mut timer = runtime.sleep_until(deadline);
        let timer_fut = timer.wait().fuse();
        pin_mut!(recv_fut, timer_fut);

        let next = futures::select_biased! {
            _ = timer_fut => {
                return Err(MediaError::new(
                    MediaErrorCode::Timeout,
                    "timed out waiting for video keyframe",
                ));
            }
            recv = recv_fut => recv,
        };

        match next {
            Ok(Some(frame)) => {
                if frame.media_kind == MediaKind::Video && frame.is_key_frame() {
                    return Ok((*frame).clone());
                }
            }
            Ok(None) => {
                return Err(MediaError::unavailable("subscriber closed before keyframe"));
            }
            Err(err) => {
                return Err(MediaError::unavailable(format!("subscriber error: {err}")));
            }
        }
    }
}

async fn write_and_register(
    ctx: &EngineContext,
    config: &SnapshotModuleConfig,
    media_key: &MediaKey,
    snapshot_id: &SnapshotId,
    created_at: i64,
    format: &str,
    payload: Bytes,
) -> Result<SnapshotOutcome, MediaError> {
    let mut dir = PathBuf::from(&config.root_path);
    dir.push(sanitize_segment(&media_key.vhost.0));
    dir.push(sanitize_segment(&media_key.app.0));
    dir.push(sanitize_segment(&media_key.stream.0));
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| MediaError::storage_failed(format!("failed to create snapshot dir: {e}")))?;

    let filename = format!("{}-{created_at}.{format}", snapshot_id.0);
    let mut temp_path = dir.clone();
    temp_path.push(format!(".tmp-{filename}"));
    let mut final_path = dir.clone();
    final_path.push(&filename);

    fs::write(&temp_path, payload)
        .await
        .map_err(|e| MediaError::storage_failed(format!("failed to write snapshot: {e}")))?;

    fs::rename(&temp_path, &final_path).await.map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        MediaError::storage_failed(format!("failed to finalize snapshot: {e}"))
    })?;

    let size_bytes = fs::metadata(&final_path)
        .await
        .map_err(|e| MediaError::storage_failed(format!("failed to stat snapshot: {e}")))?
        .len();

    let file_meta = cheetah_media_api::media_file_store::FileStoreEntry {
        media_key: media_key.clone(),
        file_type: "snapshot".to_string(),
        content_type: content_type_for_format(format),
        size_bytes,
        created_at_ms: created_at,
        expires_at_ms: None,
        absolute_path: final_path.to_string_lossy().to_string(),
        owner_principal: None,
        allowed_principals: Vec::new(),
    };

    let handle = ctx
        .media_file_store
        .register_file(
            &cheetah_media_api::port::MediaRequestContext::default(),
            file_meta,
        )
        .map_err(|e| MediaError::storage_failed(format!("failed to register snapshot: {e}")))?;

    info!(
        %snapshot_id,
        ?final_path,
        "snapshot captured and registered"
    );

    Ok(SnapshotOutcome {
        snapshot_id: snapshot_id.clone(),
        media_key: media_key.clone(),
        path_handle: handle,
        created_at,
        size_bytes,
        format: format.to_string(),
    })
}

fn publish_snapshot_completed(ctx: &EngineContext, outcome: &SnapshotOutcome) {
    let header = EventHeader {
        event_id: format!(
            "snapshot-{}-{}-completed",
            outcome.snapshot_id.0, outcome.created_at
        ),
        occurred_at: outcome.created_at,
        sequence: None,
        media_key: Some(outcome.media_key.clone()),
        source: "snapshot".to_string(),
        correlation_id: None,
    };
    let event = MediaEvent::SnapshotCompleted(SnapshotCompleted {
        header,
        snapshot_id: outcome.snapshot_id.clone(),
        path_handle: outcome.path_handle.clone(),
        url: None,
    });
    let _ = ctx.media_event_sender.send(event);
}

fn normalize_format(format: &str) -> String {
    format.to_lowercase()
}

fn is_format_supported(format: &str) -> bool {
    matches!(format, "jpg" | "jpeg")
}

/// Replace path separators and dots with underscores so a user-controlled
/// string cannot traverse out of the configured root directory.
fn sanitize_segment(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '/' | '\\' | '.' => '_',
            _ => c,
        })
        .collect()
}

fn content_type_for_format(format: &str) -> String {
    match format {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_format_lowercases() {
        assert_eq!(normalize_format("JPG"), "jpg");
        assert_eq!(normalize_format("JPEG"), "jpeg");
    }

    #[test]
    fn only_jpg_jpeg_supported() {
        assert!(is_format_supported("jpg"));
        assert!(is_format_supported("jpeg"));
        assert!(!is_format_supported("png"));
        assert!(!is_format_supported("gif"));
    }

    #[test]
    fn jpeg_content_type_mapped() {
        assert_eq!(content_type_for_format("jpg"), "image/jpeg");
        assert_eq!(content_type_for_format("jpeg"), "image/jpeg");
    }

    #[test]
    fn unknown_content_type_is_octet_stream() {
        assert_eq!(content_type_for_format("png"), "application/octet-stream");
    }

    #[test]
    fn sanitize_segment_replaces_separators_and_dots() {
        assert_eq!(sanitize_segment("a/b"), "a_b");
        assert_eq!(sanitize_segment(r"a\b"), "a_b");
        assert_eq!(sanitize_segment(".."), "__");
        assert_eq!(sanitize_segment("a.b"), "a_b");
    }
}

fn wall_clock_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
