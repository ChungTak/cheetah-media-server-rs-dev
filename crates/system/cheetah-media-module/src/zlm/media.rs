//! ZLMediaKit-compatible media list/info endpoint handlers.
//!
//! ZLMediaKit 兼容的媒体列表/信息端点处理函数。

use cheetah_media_api::command::MediaQuery;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{page_from_params, page_size_from_params, zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn get_media_list(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let mut query = MediaQuery {
            vhost: params["vhost"].as_str().map(String::from),
            app: params["app"].as_str().map(String::from),
            stream: params["stream"].as_str().map(String::from),
            schema: params["schema"].as_str().map(String::from),
            page: page_from_params(&params),
            page_size: page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = self.control()?.get_media_list(ctx, query).await?;
        Ok(zlm_response(0, "success", page))
    }

    pub(crate) async fn is_media_online(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let online = self.control()?.is_media_online(ctx, &key).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "online": online == cheetah_media_api::model::OnlineState::Online }),
        ))
    }

    pub(crate) async fn get_media_info(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let info = self.control()?.get_media(ctx, &key).await?;
        Ok(zlm_response(0, "success", info))
    }
}
