//! ZLMediaKit-compatible snapshot endpoint handlers.
//!
//! ZLMediaKit 兼容的截图端点处理函数。

use cheetah_media_api::command::{DeleteSnapshotRequest, SnapshotRequest};
use cheetah_media_api::media_file_store::{DeleteBatchResult, DeleteFailure};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpHeader, HttpRequest, HttpResponse};
use serde::Serialize;

use super::{zlm_response, ZlmMediaHttpService, ZlmResponse};
use crate::error::AdapterError;

impl ZlmMediaHttpService {
    pub(crate) async fn get_snap(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let timeout_ms = parse_zlm_timeout_ms(&params);
        let format = params["format"].as_str().unwrap_or("jpg").to_string();
        let quality = crate::util::parse_json_u64(&params["quality"])
            .or_else(|| crate::util::parse_json_u64(&params["scale"]))
            .map(|v| v.min(100) as u8);
        let request = SnapshotRequest {
            media_key: key,
            timeout_ms,
            format: format.clone(),
            quality,
            max_width: None,
            max_height: None,
            storage_policy: Default::default(),
            capture_policy: Default::default(),
        };
        let handle = snapshot_api.take_snapshot(ctx, request).await?;

        // ZLM getSnap returns the actual image bytes, not a JSON handle.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let filename = format!("{}.{}", handle.snapshot_id.0, format);
        let download = self
            .ctx
            .media_file_store
            .resolve_download(ctx, &handle.path_handle, None, Some(filename), now)
            .map_err(AdapterError::Media)?;

        Ok(HttpResponse {
            status: 200,
            headers: vec![
                HttpHeader {
                    name: "content-type".to_string(),
                    value: download.content_type,
                },
                HttpHeader {
                    name: "content-length".to_string(),
                    value: download.body.len().to_string(),
                },
            ],
            body: download.body,
        })
    }

    pub(crate) async fn delete_snap_directory(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let request = DeleteSnapshotRequest {
            media_key: key,
            directory: params["directory"].as_str().map(String::from),
            retain_count: params["retain_count"].as_u64().map(|v| v as u32),
        };
        let result = snapshot_api.delete_snapshots(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(DeleteDirectoryResult::from(
            result,
        ))))
    }
}

/// ZLM-compatible delete response that preserves the original `result` flag
/// while also exposing the per-handle batch result.
#[derive(Serialize)]
struct DeleteDirectoryResult {
    result: bool,
    matched: u64,
    deleted: u64,
    failed: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failures: Vec<DeleteFailure>,
}

impl From<DeleteBatchResult> for DeleteDirectoryResult {
    fn from(r: DeleteBatchResult) -> Self {
        Self {
            result: r.failed == 0,
            matched: r.matched,
            deleted: r.deleted,
            failed: r.failed,
            failures: r.failures,
        }
    }
}

fn parse_zlm_timeout_ms(params: &serde_json::Value) -> u64 {
    crate::util::parse_json_u64(&params["timeout_sec"])
        .or_else(|| crate::util::parse_json_u64(&params["timeout"]))
        .map(|s| s * 1000)
        .unwrap_or(10_000)
}
