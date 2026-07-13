//! Webrtc route handlers for the native media HTTP adapter.
//!
//! native HTTP adapter 的 Webrtc 路由处理器。

use cheetah_media_api::command::{WebRtcRoomQuery, WebRtcRoomRequest, WhepRequest, WhipRequest};
use cheetah_media_api::ids::WebRtcRoomId;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{json_response, parse_body, parse_query, room_id_from_path, NativeMediaHttpService};

impl NativeMediaHttpService {
    pub(crate) async fn webrtc_whip(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let request: WhipRequest = parse_body(&req)?;
        let response = api.whip_publish(&ctx, request).await?;
        Ok(json_response(&response))
    }
    pub(crate) async fn webrtc_whep(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let request: WhepRequest = parse_body(&req)?;
        let response = api.whep_subscribe(&ctx, request).await?;
        Ok(json_response(&response))
    }
    pub(crate) async fn webrtc_rooms_create(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.webrtc()?;
        let request: WebRtcRoomRequest = parse_body(&req)?;
        let room = api.create_room(&ctx, request).await?;
        Ok(json_response(&room))
    }
    pub(crate) async fn webrtc_rooms_list(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        let api = self.webrtc()?;
        let mut query: WebRtcRoomQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = api.list_rooms(&ctx, query).await?;
        Ok(json_response(&page))
    }
    pub(crate) async fn webrtc_room_delete(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.webrtc()?;
        let id = room_id_from_path(&req.path, "/webrtc/rooms/", "")
            .ok_or_else(|| AdapterError::InvalidRequest("missing room_id".to_string()))?;
        api.delete_room(&ctx, &WebRtcRoomId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }
}
