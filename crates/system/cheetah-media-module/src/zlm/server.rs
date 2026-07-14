//! Server ops route handlers for the ZLMediaKit-compatible adapter.
//!
//! 为 ZLMediaKit 兼容适配器实现的服务器管理路由处理器。

use std::collections::HashMap;

use cheetah_media_api::model::ServerConfig;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn get_server_load(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        let data = serde_json::json!({
            "cpu": info.load.cpu_percent,
            "mem": info.load.memory_bytes,
            "net_in": info.load.network_in,
            "net_out": info.load.network_out,
        });
        Ok(zlm_response(0, "success", data))
    }

    pub(crate) async fn get_work_threads_load(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let info = api.server_info(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({ "threads": info.load.threads }),
        ))
    }

    pub(crate) async fn get_server_config(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let mut config = api.server_config(&ctx).await?;
        crate::util::filter_sensitive_config_values(&mut config.values);
        Ok(zlm_response(0, "success", config.values))
    }

    pub(crate) async fn set_server_config(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        let params = self.extract_params(&req)?;
        let mut values = HashMap::new();
        if let (Some(key), Some(value)) = (params["key"].as_str(), params["value"].as_str()) {
            if !crate::util::is_sensitive_config_key(key) {
                values.insert(key.to_string(), value.to_string());
            }
        } else if let Some(obj) = params.as_object() {
            for (k, v) in obj {
                if k == "restart" || crate::util::is_sensitive_config_key(k) {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    values.insert(k.clone(), s.to_string());
                }
            }
        }
        let config = ServerConfig { values };
        api.set_server_config(&ctx, config).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn restart_server(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.restart_server(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn shutdown_server(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let ctx = self.request_context(&req);
        self.require_principal(&ctx)?;
        let api = self.server_admin()?;
        api.shutdown_server(&ctx).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }
}
