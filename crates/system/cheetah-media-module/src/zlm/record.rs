//! ZLMediaKit-compatible record endpoint handlers.
//!
//! ZLMediaKit 兼容的录制端点处理函数。

use std::path::PathBuf;

use cheetah_media_api::command::{
    DeleteRecordRequest, OpenPlaybackRequest, RecordFileQuery, RecordPlaybackCommand,
    RecordTaskQuery, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::RecordFileId;
use cheetah_media_api::media_file_store::FileStoreEntry;
use cheetah_media_api::model::{RecordTaskState, StoragePolicy};
use cheetah_media_api::port::{MediaRequestContext, PlaybackApi};
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{
    page_from_params, page_size_from_params, zlm_record_format, zlm_response, Data,
    DeleteRecordDirectoryResult, Mp4FilesData, StartRecordResult, StatusResult,
    ZlmMediaHttpService, ZlmResponse, ZlmResult,
};

impl ZlmMediaHttpService {
    pub(crate) async fn record_start(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let request = StartRecordRequest {
            media_key: key,
            format: format.clone(),
            template: cheetah_media_api::model::RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: StoragePolicy::default(),
            idempotency_key: ctx
                .idempotency_key
                .clone()
                .map(cheetah_media_api::ids::IdempotencyKey),
        };
        let task = record_api.start_record(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(StartRecordResult {
            result: true,
            task_id: task.task_id.0,
        })))
    }

    pub(crate) async fn record_stop(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let mut query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordTaskQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_tasks(ctx, query).await?;
        let task = page
            .items
            .into_iter()
            .find(|t| {
                t.format == format
                    && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
            })
            .ok_or_else(|| {
                AdapterError::Media(cheetah_media_api::error::MediaError::not_found(
                    "record task",
                ))
            })?;
        record_api
            .stop_record(
                ctx,
                StopRecordRequest {
                    task_id: task.task_id,
                },
            )
            .await?;
        Ok(zlm_response(ZlmResponse::ok(ZlmResult { result: true })))
    }

    pub(crate) async fn is_recording(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let mut query = RecordTaskQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordTaskQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_tasks(ctx, query).await?;
        let recording = page.items.iter().any(|t| {
            t.format == format
                && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
        });
        Ok(zlm_response(ZlmResponse::ok(StatusResult {
            status: recording,
        })))
    }

    pub(crate) async fn get_mp4_files(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            format: Some("mp4".to_string()),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_files(ctx, query).await?;
        let paths: Vec<String> = page.items.iter().map(|f| f.path_handle.0.clone()).collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(Mp4FilesData {
            paths,
            root_path: String::new(),
        }))))
    }

    pub(crate) async fn delete_record_directory(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordFileQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let mut total_deleted = 0usize;
        let mut total_failed = 0usize;
        loop {
            let page = record_api.query_record_files(ctx, query.clone()).await?;
            if page.items.is_empty() {
                break;
            }
            let mut page_deleted = 0usize;
            for f in &page.items {
                match record_api
                    .delete_record_file(
                        ctx,
                        DeleteRecordRequest {
                            file_id: f.file_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => {
                        page_deleted += 1;
                        total_deleted += 1;
                    }
                    Err(_) => {
                        total_failed += 1;
                    }
                }
            }
            if (page.items.len() as u64) < query.page_size || page_deleted == 0 {
                break;
            }
        }
        let result = total_failed == 0;
        Ok(zlm_response(ZlmResponse::with_msg(
            0,
            if result { "success" } else { "partial success" },
            DeleteRecordDirectoryResult {
                result,
                deleted: total_deleted,
                failed: total_failed,
            },
        )))
    }

    pub(crate) async fn set_record_speed(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["speed", "scale", "value"])?;
        record_api
            .control_record_playback(
                ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Scale { value },
            )
            .await?;
        Ok(zlm_response(ZlmResponse::ok(ZlmResult { result: true })))
    }

    pub(crate) async fn seek_record_stamp(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let value = parse_zlm_playback_value(&params, &["stamp", "seek", "value"])?;
        record_api
            .control_record_playback(
                ctx,
                &RecordFileId(file_id),
                RecordPlaybackCommand::Seek {
                    value: value as i64,
                },
            )
            .await?;
        Ok(zlm_response(ZlmResponse::ok(ZlmResult { result: true })))
    }

    pub(crate) async fn control_record_play(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let params = self.extract_params(&req)?;
        let file_id = parse_zlm_file_id(&params)?;
        let command = parse_zlm_playback_command(&params)?;
        record_api
            .control_record_playback(ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(zlm_response(ZlmResponse::ok(ZlmResult { result: true })))
    }

    pub(crate) async fn load_mp4_file(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let playback = self.playback()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let file_path = params["file_path"]
            .as_str()
            .or_else(|| params["filePath"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("file_path is required".to_string()))?
            .to_string();
        let absolute = PathBuf::from(&file_path);
        if !absolute.is_absolute() {
            return Err(AdapterError::InvalidRequest(
                "file_path must be an absolute path".to_string(),
            ));
        }
        let size_bytes = std::fs::metadata(&absolute).map(|m| m.len()).unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let handle = self.ctx.media_file_store.register_file(
            ctx,
            FileStoreEntry {
                media_key: key.clone(),
                file_type: "vod".to_string(),
                content_type: "video/mp4".to_string(),
                size_bytes,
                created_at_ms: now,
                expires_at_ms: None,
                absolute_path: absolute.to_string_lossy().to_string(),
                owner_principal: None,
                allowed_principals: Vec::new(),
            },
        )?;
        let seek_ms = params["seek_ms"]
            .as_i64()
            .or_else(|| params["seekMS"].as_i64())
            .unwrap_or(0);
        let mut scale = params["speed"]
            .as_f64()
            .or_else(|| params["speed"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(1.0);
        if ![0.5, 1.0, 2.0, 4.0]
            .iter()
            .any(|s| (*s - scale).abs() < 1e-9)
        {
            scale = 1.0;
        }
        let session = playback
            .open_playback(
                ctx,
                OpenPlaybackRequest {
                    file_handle: handle,
                    media_key: key,
                    start_position_ms: seek_ms.max(0),
                    scale,
                },
            )
            .await?;
        Ok(zlm_response(ZlmResponse::ok(Data {
            data: serde_json::json!({
                "sessionId": session.session_id.0,
                "duration_ms": session.duration_ms,
            }),
        })))
    }

    fn playback(&self) -> Result<std::sync::Arc<dyn PlaybackApi>, AdapterError> {
        self.ctx.media_services.playback().ok_or_else(|| {
            AdapterError::Media(cheetah_media_api::error::MediaError::unavailable(
                "playback not available",
            ))
        })
    }
}

fn parse_json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

pub(crate) fn parse_zlm_file_id(params: &serde_json::Value) -> Result<String, AdapterError> {
    params["file_id"]
        .as_str()
        .or_else(|| params["fileId"].as_str())
        .or_else(|| params["file_path"].as_str())
        .map(String::from)
        .ok_or_else(|| AdapterError::InvalidRequest("file_id is required".to_string()))
}

pub(crate) fn parse_zlm_playback_value(
    params: &serde_json::Value,
    aliases: &[&str],
) -> Result<f64, AdapterError> {
    for alias in aliases {
        if let Some(v) = parse_json_f64(&params[*alias]) {
            return Ok(v);
        }
    }
    Err(AdapterError::InvalidRequest(
        "playback value is required".to_string(),
    ))
}

pub(crate) fn parse_zlm_playback_command(
    params: &serde_json::Value,
) -> Result<RecordPlaybackCommand, AdapterError> {
    let command = params["command"]
        .as_str()
        .ok_or_else(|| AdapterError::InvalidRequest("command is required".to_string()))?
        .to_lowercase();
    match command.as_str() {
        "pause" => Ok(RecordPlaybackCommand::Pause),
        "resume" => Ok(RecordPlaybackCommand::Resume),
        "scale" | "speed" => {
            let value = parse_zlm_playback_value(params, &["value", "speed", "scale"])?;
            Ok(RecordPlaybackCommand::Scale { value })
        }
        "seek" | "stamp" => {
            let value = parse_zlm_playback_value(params, &["value", "stamp", "seek"])?;
            Ok(RecordPlaybackCommand::Seek {
                value: value as i64,
            })
        }
        _ => Err(AdapterError::InvalidRequest(format!(
            "unsupported playback command {command}"
        ))),
    }
}
