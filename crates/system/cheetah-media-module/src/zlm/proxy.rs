//! Proxy route handlers for the ZLMediaKit-compatible adapter.
//!
//! 为 ZLMediaKit 兼容适配器实现的代理路由处理器。

use cheetah_media_api::command::{FfmpegProxyRequest, ProxyQuery, PullProxyRequest};
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::ProxyKind;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{zlm_key_string, zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn get_all_stream_proxy(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let mut query = ProxyQuery {
            kind: Some(ProxyKind::Pull),
            ..Default::default()
        };
        query.page_size =
            crate::util::parse_json_u64(&params["page_size"]).unwrap_or(ProxyQuery::MAX_PAGE_SIZE);
        query.page = crate::util::parse_json_u64(&params["page"]).unwrap_or(0);
        query.clamp_page_size();
        let page = proxy_api.list_proxies(&ctx, query).await?;
        Ok(zlm_response(0, "success", page.items))
    }

    pub(crate) async fn add_stream_proxy(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let source_url = params["url"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("url is required".to_string()))?;
        crate::util::validate_ffmpeg_url(source_url)?;
        let request = PullProxyRequest {
            source_url: source_url.to_string(),
            destination: key.clone(),
            retry_policy: Default::default(),
            heartbeat_ms: crate::util::parse_json_u64(&params["heartbeat_ms"]),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
            transcode_policy: Default::default(),
            output_policy: Default::default(),
            record_policy: None,
        };
        let info = proxy_api.create_pull_proxy(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "key": zlm_key_string(&key),
                "proxy_id": info.proxy_id.0,
                "result": true,
            }),
        ))
    }

    pub(crate) async fn del_stream_proxy(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let proxy_id = ProxyId(zlm_key_string(&key));
        proxy_api.delete_pull_proxy(&ctx, &proxy_id).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn add_ffmpeg_source(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let (source_url, input_options, output_options) = crate::util::parse_ffmpeg_request(
            params["ffmpeg_cmd"].as_str(),
            params["src_url"].as_str(),
        )?;
        crate::util::validate_ffmpeg_options(&input_options)?;
        crate::util::validate_ffmpeg_options(&output_options)?;
        let request = FfmpegProxyRequest {
            source_url,
            destination: key.clone(),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(0),
            input_options,
            output_options,
            transcode_policy: Default::default(),
            output_policy: Default::default(),
        };
        let info = proxy_api.create_ffmpeg_proxy(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "key": zlm_key_string(&key),
                "proxy_id": info.proxy_id.0,
                "result": true,
            }),
        ))
    }

    pub(crate) async fn del_ffmpeg_source(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let proxy_id = ProxyId(zlm_key_string(&key));
        proxy_api.delete_ffmpeg_proxy(&ctx, &proxy_id).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }
}
