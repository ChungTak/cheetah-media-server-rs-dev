//! Proxy route handlers for the native media HTTP adapter.
//!
//! native HTTP adapter 的 Proxy 路由处理器。

use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest,
};
use cheetah_media_api::ids::ProxyId;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{json_response, parse_body, parse_query, proxy_id_from_path, NativeMediaHttpService};

impl NativeMediaHttpService {
    pub(crate) async fn proxies_list(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let mut query: ProxyQuery = parse_query(&req)?;
        query.clamp_page_size();
        let page = proxy_api.list_proxies(&ctx, query).await?;
        Ok(json_response(&page))
    }
    pub(crate) async fn proxies_pull(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let mut request: PullProxyRequest = parse_body(&req)?;
        request.source_url = request.source_url.trim().to_string();
        crate::util::validate_ffmpeg_url(&request.source_url)?;
        let info = proxy_api.create_pull_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }
    pub(crate) async fn proxies_push(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let mut request: PushProxyRequest = parse_body(&req)?;
        request.destination_url = request.destination_url.trim().to_string();
        crate::util::validate_ffmpeg_url(&request.destination_url)?;
        let info = proxy_api.create_push_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }
    pub(crate) async fn proxies_ffmpeg(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let mut request: FfmpegProxyRequest = parse_body(&req)?;
        crate::util::validate_ffmpeg_options(&request.input_options)?;
        crate::util::validate_ffmpeg_options(&request.output_options)?;
        // For a native API call, the user should not be able to pass a raw command string.
        request.source_url = request.source_url.trim().to_string();
        crate::util::validate_ffmpeg_url(&request.source_url)?;
        let info = proxy_api.create_ffmpeg_proxy(&ctx, request).await?;
        Ok(json_response(&info))
    }
    pub(crate) async fn proxies_pull_delete(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/pull")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_pull_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }
    pub(crate) async fn proxies_push_delete(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/push")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_push_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }
    pub(crate) async fn proxies_ffmpeg_delete(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let proxy_api = self.proxy()?;
        let id = proxy_id_from_path(&req.path, "/proxies/", "/ffmpeg")
            .ok_or_else(|| AdapterError::InvalidRequest("missing proxy_id".to_string()))?;
        proxy_api.delete_ffmpeg_proxy(&ctx, &ProxyId(id)).await?;
        Ok(json_response(&serde_json::json!({ "deleted": true })))
    }
}
