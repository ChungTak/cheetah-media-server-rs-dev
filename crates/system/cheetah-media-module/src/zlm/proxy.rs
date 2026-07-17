//! ZLMediaKit-compatible proxy endpoint handlers.
//!
//! ZLMediaKit 兼容的代理端点处理函数。

use cheetah_media_api::command::{
    FfmpegProxyRequest, ProxyQuery, PullProxyRequest, PushProxyRequest, RetryPolicy,
};
use cheetah_media_api::ids::ProxyId;
use cheetah_media_api::model::{AdmissionAction, OutputPolicy, ProxyKind, TranscodePolicy};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;
use crate::zlm::{
    page_from_params, page_size_from_params, zlm_response, Data, KeyData, ProxyItem,
    ZlmMediaHttpService, ZlmResponse, ZlmResult,
};

impl ZlmMediaHttpService {
    pub(crate) async fn add_stream_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let url = params["url"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("url is required".to_string()))?;
        let mut ctx = ctx.clone();
        ctx.idempotency_key = params["key"].as_str().map(|s| s.to_string());

        let request = PullProxyRequest {
            source_url: url.to_string(),
            destination: key.clone(),
            retry_policy: RetryPolicy::default(),
            heartbeat_ms: None,
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
            transcode_policy: TranscodePolicy::default(),
            output_policy: OutputPolicy::default(),
            record_policy: None,
        };
        self.check_admission(
            &ctx,
            AdmissionAction::CreatePullProxy,
            key,
            "proxy-pull".to_string(),
            Some(url.to_string()),
        )
        .await?;
        let info = proxy_api.create_pull_proxy(&ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(KeyData {
            key: info.proxy_id.0,
        }))))
    }

    pub(crate) async fn del_stream_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let id = proxy_id_from_params(&params)?;
        proxy_api.delete_pull_proxy(ctx, &id).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(ZlmResult {
            result: true,
        }))))
    }

    pub(crate) async fn list_stream_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let mut query = ProxyQuery {
            kind: Some(ProxyKind::Pull),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = proxy_api.list_pull_proxies(ctx, query).await?;
        let items: Vec<_> = page
            .items
            .into_iter()
            .map(|info| {
                let key = info.proxy_id.0.clone();
                ProxyItem::from_info(&info, Some(key))
            })
            .collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
    }

    pub(crate) async fn get_proxy_info(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let id = proxy_id_from_params(&params)?;
        let info = proxy_api.get_pull_proxy(ctx, &id).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(
            ProxyItem::from_info(&info, Some(id.0.clone())),
        ))))
    }

    pub(crate) async fn add_stream_pusher_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let dst_url = params["dst_url"]
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest("dst_url is required".to_string()))?;
        let mut ctx = ctx.clone();
        ctx.idempotency_key = params["key"].as_str().map(|s| s.to_string());

        let protocol = params["schema"].as_str().unwrap_or("rtmp").to_string();
        let request = PushProxyRequest {
            source_media_key: key.clone(),
            destination_url: dst_url.to_string(),
            protocol: protocol.clone(),
            retry_policy: RetryPolicy::default(),
            protocol_options: Default::default(),
        };
        self.check_admission(
            &ctx,
            AdmissionAction::CreatePushProxy,
            key,
            protocol,
            Some(dst_url.to_string()),
        )
        .await?;
        let info = proxy_api.create_push_proxy(&ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(KeyData {
            key: info.proxy_id.0,
        }))))
    }

    pub(crate) async fn del_stream_pusher_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let id = proxy_id_from_params(&params)?;
        proxy_api.delete_push_proxy(ctx, &id).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(ZlmResult {
            result: true,
        }))))
    }

    pub(crate) async fn list_stream_pusher_proxy(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let mut query = ProxyQuery {
            kind: Some(ProxyKind::Push),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = proxy_api.list_push_proxies(ctx, query).await?;
        let items: Vec<_> = page
            .items
            .into_iter()
            .map(|info| {
                let key = info.proxy_id.0.clone();
                ProxyItem::from_info(&info, Some(key))
            })
            .collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
    }

    pub(crate) async fn get_proxy_pusher_info(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let id = proxy_id_from_params(&params)?;
        let info = proxy_api.get_push_proxy(ctx, &id).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(
            ProxyItem::from_info(&info, Some(id.0.clone())),
        ))))
    }

    pub(crate) async fn add_ffmpeg_source(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let src_url = params["src_url"]
            .as_str()
            .or_else(|| params["url"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("src_url is required".to_string()))?;
        let mut ctx = ctx.clone();
        ctx.idempotency_key = params["key"].as_str().map(|s| s.to_string());

        let request = FfmpegProxyRequest {
            source_url: src_url.to_string(),
            destination: key.clone(),
            input_options: Vec::new(),
            output_options: Vec::new(),
            transcode_policy: TranscodePolicy::default(),
            output_policy: OutputPolicy::default(),
        };
        self.check_admission(
            &ctx,
            AdmissionAction::CreateFfmpegProxy,
            key,
            "proxy-ffmpeg".to_string(),
            Some(src_url.to_string()),
        )
        .await?;
        let info = proxy_api.create_ffmpeg_proxy(&ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(KeyData {
            key: info.proxy_id.0,
        }))))
    }

    pub(crate) async fn del_ffmpeg_source(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let id = proxy_id_from_params(&params)?;
        proxy_api.delete_ffmpeg_proxy(ctx, &id).await?;
        Ok(zlm_response(ZlmResponse::ok(Data::new(ZlmResult {
            result: true,
        }))))
    }

    pub(crate) async fn list_ffmpeg_source(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let proxy_api = self.proxy()?;
        let params = self.extract_params(&req)?;
        let mut query = ProxyQuery {
            kind: Some(ProxyKind::Ffmpeg),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = proxy_api.list_ffmpeg_proxies(ctx, query).await?;
        let items: Vec<_> = page
            .items
            .into_iter()
            .map(|info| {
                let key = info.proxy_id.0.clone();
                ProxyItem::from_info(&info, Some(key))
            })
            .collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
    }
}

fn proxy_id_from_params(params: &serde_json::Value) -> Result<ProxyId, AdapterError> {
    params["key"]
        .as_str()
        .map(|s| ProxyId(s.to_string()))
        .ok_or_else(|| AdapterError::InvalidRequest("key is required".to_string()))
}
