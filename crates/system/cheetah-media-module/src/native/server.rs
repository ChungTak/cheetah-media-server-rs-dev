//! Server route handlers for the native media HTTP adapter.
//!
//! native HTTP adapter 的 Server 路由处理器。

use cheetah_media_api::command::ServerConfigUpdate;
use cheetah_media_api::model::ServerConfig;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{json_response, parse_body, NativeMediaHttpService};

impl NativeMediaHttpService {
    pub(crate) async fn server_info(&self, req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        Ok(json_response(&info))
    }
    pub(crate) async fn server_config(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let mut config = api.server_config(&ctx).await?;
        crate::util::filter_sensitive_config_values(&mut config.values);
        Ok(json_response(&config))
    }
    pub(crate) async fn server_config_update(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let mut update: ServerConfigUpdate = parse_body(&req)?;
        crate::util::filter_sensitive_config_values(&mut update.values);
        let config = ServerConfig {
            values: update.values,
        };
        api.set_server_config(&ctx, config).await?;
        if update.restart {
            api.restart_server(&ctx).await?;
        }
        Ok(json_response(&serde_json::json!({ "updated": true })))
    }
    pub(crate) async fn server_restart(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.restart_server(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "restarting": true })))
    }
    pub(crate) async fn server_shutdown(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.shutdown_server(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "shutting_down": true })))
    }
    pub(crate) async fn server_ports(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let ports = api.list_ports(&ctx).await?;
        Ok(json_response(&serde_json::json!({ "ports": ports })))
    }
}
