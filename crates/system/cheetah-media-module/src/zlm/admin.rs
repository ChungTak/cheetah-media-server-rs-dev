//! ZLMediaKit-compatible admin endpoint handlers.
//!
//! ZLMediaKit 兼容的管理端点处理函数。

use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{
    routes, zlm_response, ApiListData, Data, VersionInfo, ZlmMediaHttpService, ZlmResponse,
};

impl ZlmMediaHttpService {
    pub(crate) async fn version(
        &self,
        _ctx: &MediaRequestContext,
        _req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        Ok(zlm_response(ZlmResponse::ok(Data::new(
            VersionInfo::default(),
        ))))
    }

    pub(crate) async fn get_api_list(
        &self,
        _ctx: &MediaRequestContext,
        _req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let routes = routes::zlm_http_routes();
        let paths: Vec<String> = routes.into_iter().map(|r| r.path).collect();
        let capabilities = self.ctx.media_services.capabilities();
        Ok(zlm_response(ZlmResponse::ok(Data::new(ApiListData {
            apis: paths,
            capabilities,
        }))))
    }
}
