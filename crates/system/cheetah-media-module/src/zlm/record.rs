//! Record route handlers for the ZLMediaKit-compatible adapter.
//!
//! 为 ZLMediaKit 兼容适配器实现的录制路由处理器。

use cheetah_media_api::command::{
    DeleteRecordRequest, RecordFileQuery, RecordTaskQuery, StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::model::{RecordTaskState, RecordTemplate, StoragePolicy};
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn record_start(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let format = zlm_record_format(&params["type"])?;
        let request = StartRecordRequest {
            media_key: key,
            format: format.clone(),
            template: RecordTemplate::Continuous,
            segment_duration_ms: None,
            max_segments: None,
            storage_policy: StoragePolicy::default(),
            idempotency_key: None,
        };
        let task = record_api.start_record(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true, "taskId": task.task_id.0}),
        ))
    }

    pub(crate) async fn record_stop(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
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
        let page = record_api.query_record_tasks(&ctx, query).await?;
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
                &ctx,
                StopRecordRequest {
                    task_id: task.task_id,
                },
            )
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn is_recording(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
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
        let page = record_api.query_record_tasks(&ctx, query).await?;
        let recording = page.items.iter().any(|t| {
            t.format == format
                && matches!(t.state, RecordTaskState::Running | RecordTaskState::Pending)
        });
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"status": recording}),
        ))
    }

    pub(crate) async fn get_mp4_files(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            format: Some("mp4".to_string()),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = record_api.query_record_files(&ctx, query).await?;
        let paths: Vec<String> = page.items.iter().map(|f| f.path_handle.0.clone()).collect();
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"paths": paths, "rootPath": ""}),
        ))
    }

    pub(crate) async fn delete_record_directory(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let record_api = self.record()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RecordFileQuery {
            vhost: Some(key.vhost.0.clone()),
            app: Some(key.app.0.clone()),
            stream: Some(key.stream.0.clone()),
            page_size: RecordFileQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let mut total_deleted = 0usize;
        let mut total_failed = 0usize;
        loop {
            let page = record_api.query_record_files(&ctx, query.clone()).await?;
            if page.items.is_empty() {
                break;
            }
            let mut page_deleted = 0usize;
            for f in &page.items {
                match record_api
                    .delete_record_file(
                        &ctx,
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
        let data = serde_json::json!({
            "result": result,
            "deleted": total_deleted,
            "failed": total_failed,
        });
        Ok(zlm_response(
            0,
            if result { "success" } else { "partial success" },
            data,
        ))
    }
}

/// Parse the ZLMediaKit record `type` parameter into a normalized format string.
///
/// Supports numeric values (0=mp4, 1=hls, 2=hls, 3=fmp4) and string values.
/// Missing or empty values default to "mp4".
fn zlm_record_format(value: &serde_json::Value) -> Result<String, AdapterError> {
    if value.is_null() {
        return Ok("mp4".to_string());
    }
    if let Some(num) = crate::util::parse_json_u64(value) {
        let format = match num {
            0 => "mp4",
            1 | 2 => "hls",
            3 => "fmp4",
            other => {
                return Err(AdapterError::InvalidRequest(format!(
                    "unsupported numeric record type {other}"
                )))
            }
        };
        return Ok(format.to_string());
    }
    if let Some(s) = value.as_str() {
        if s.trim().is_empty() {
            return Ok("mp4".to_string());
        }
        return Ok(s.to_lowercase());
    }
    Ok("mp4".to_string())
}
