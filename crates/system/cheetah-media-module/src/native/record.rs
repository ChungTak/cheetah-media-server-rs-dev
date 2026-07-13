//! Record route handlers for the native media HTTP adapter.
//!
//! native HTTP adapter 的 Record 路由处理器。

use cheetah_media_api::command::{
    DeleteRecordRequest, RecordFileQuery, RecordPlaybackCommand, RecordTaskQuery,
    StartRecordRequest, StopRecordRequest,
};
use cheetah_media_api::ids::{RecordFileId, RecordTaskId};
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{json_response, parse_body, parse_query, record_id_from_path, NativeMediaHttpService};

impl NativeMediaHttpService {
    pub(crate) async fn record_tasks(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let mut query: RecordTaskQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_tasks(&ctx, query).await?;
        Ok(json_response(&page))
    }
    pub(crate) async fn record_files(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let mut query: RecordFileQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = record_api.query_record_files(&ctx, query).await?;
        Ok(json_response(&page))
    }
    pub(crate) async fn record_start(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let request: StartRecordRequest = parse_body(&req)?;
        let task = record_api.start_record(&ctx, request).await?;
        Ok(json_response(&task))
    }
    pub(crate) async fn record_stop(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/tasks/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing task_id".to_string()))?;
        let request = StopRecordRequest {
            task_id: RecordTaskId(id),
        };
        let task = record_api.stop_record(&ctx, request).await?;
        Ok(json_response(&task))
    }
    pub(crate) async fn record_file_delete(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let id = record_id_from_path(&req.path, "/record/files/", "")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        record_api
            .delete_record_file(
                &ctx,
                DeleteRecordRequest {
                    file_id: RecordFileId(id),
                },
            )
            .await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }
    pub(crate) async fn record_playback_control(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let record_api = self.record()?;
        let file_id = record_id_from_path(&req.path, "/record/playback/", "/control")
            .ok_or_else(|| AdapterError::InvalidRequest("missing file_id".to_string()))?;
        let command: RecordPlaybackCommand = parse_body(&req)?;
        record_api
            .control_record_playback(&ctx, &RecordFileId(file_id), command)
            .await?;
        Ok(json_response(&serde_json::json!({ "controlled": true })))
    }
}
