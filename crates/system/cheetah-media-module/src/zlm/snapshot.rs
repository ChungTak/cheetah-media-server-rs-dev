//! ZLMediaKit-compatible snapshot endpoint handlers.
//!
//! ZLMediaKit 兼容的截图端点处理函数。

use cheetah_media_api::command::{DeleteSnapshotRequest, SnapshotRequest};
use cheetah_sdk::{HttpRequest, HttpResponse};

use super::{zlm_response, ZlmMediaHttpService};
use crate::error::AdapterError;

impl ZlmMediaHttpService {
    pub(crate) async fn get_snap(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let ctx = self.authorize_request(&req)?;
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
            format,
            quality,
            storage_policy: Default::default(),
            capture_policy: Default::default(),
        };
        let handle = snapshot_api.take_snapshot(&ctx, request).await?;
        Ok(zlm_response(0, "success", handle))
    }

    pub(crate) async fn delete_snap_directory(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let snapshot_api = self.snapshot()?;
        let ctx = self.authorize_request(&req)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let request = DeleteSnapshotRequest {
            media_key: key,
            directory: params["directory"].as_str().map(String::from),
            retain_count: params["retain_count"].as_u64().map(|v| v as u32),
        };
        snapshot_api
            .delete_snapshot_directory(&ctx, request)
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }
}

fn parse_zlm_timeout_ms(params: &serde_json::Value) -> u64 {
    crate::util::parse_json_u64(&params["timeout_sec"])
        .or_else(|| crate::util::parse_json_u64(&params["timeout"]))
        .map(|s| s * 1000)
        .unwrap_or(10_000)
}
