//! Rtp route handlers for the native media HTTP adapter.
//!
//! native HTTP adapter 的 Rtp 路由处理器。

use cheetah_media_api::command::{RtpQuery, RtpReceiverRequest, RtpSenderRequest};
use cheetah_media_api::ids::RtpSessionId;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{json_response, parse_body, parse_query, rtp_id_from_path, NativeMediaHttpService};

impl NativeMediaHttpService {
    pub(crate) async fn rtp_receivers(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let request: RtpReceiverRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_receiver(&ctx, request).await?;
        Ok(json_response(&session))
    }
    pub(crate) async fn rtp_senders(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let request: RtpSenderRequest = parse_body(&req)?;
        let session = rtp_api.open_rtp_sender(&ctx, request).await?;
        Ok(json_response(&session))
    }
    pub(crate) async fn rtp_session_stop(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let id = rtp_id_from_path(&req.path, "/rtp/sessions/", "/stop")
            .ok_or_else(|| AdapterError::InvalidRequest("missing session_id".to_string()))?;
        rtp_api.stop_rtp_session(&ctx, &RtpSessionId(id)).await?;
        Ok(json_response(&serde_json::json!({ "stopped": true })))
    }
    pub(crate) async fn rtp_sessions(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let rtp_api = self.rtp()?;
        let mut query: RtpQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(&ctx, query).await?;
        Ok(json_response(&page))
    }
}
