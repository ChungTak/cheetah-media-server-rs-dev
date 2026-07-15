//! ZLMediaKit-compatible login/logout session handlers.
//!
//! ZLMediaKit 兼容的登录/登出 session 处理函数。
//! Session state is kept inside the adapter module; cookies are not passed into
//! the domain layer.

use cheetah_media_api::port::MediaRequestContext;
use cheetah_media_api::{MediaScope, Principal};
use cheetah_sdk::{HttpHeader, HttpRequest, HttpResponse};
use serde_json::json;

use crate::error::AdapterError;

use super::{constant_time_eq_str, cookie_from_header, zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn login(
        &self,
        _ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let cfg = self.config.read().unwrap();
        let session_cfg = cfg.auth.session.as_ref().ok_or_else(|| {
            AdapterError::Media(
                cheetah_media_api::error::MediaError::unsupported_capability("session auth"),
            )
        })?;
        let params = self.extract_params(&req)?;
        let username = params["username"]
            .as_str()
            .or_else(|| params["user"].as_str())
            .or_else(|| params["userName"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("username is required".to_string()))?;
        let password = params["password"]
            .as_str()
            .or_else(|| params["passwd"].as_str())
            .or_else(|| params["pass"].as_str())
            .ok_or_else(|| AdapterError::InvalidRequest("password is required".to_string()))?;

        if !constant_time_eq_str(username, &session_cfg.username)
            || !constant_time_eq_str(password, &session_cfg.password)
        {
            return Err(AdapterError::Media(
                cheetah_media_api::error::MediaError::new(
                    cheetah_media_api::error::MediaErrorCode::Unauthenticated,
                    "invalid credentials",
                ),
            ));
        }

        let token = crate::util::generate_session_token();
        let principal = Principal {
            identity: session_cfg.username.clone(),
            scopes: vec![
                MediaScope::MediaRead,
                MediaScope::MediaControl,
                MediaScope::MediaPublish,
                MediaScope::MediaConsume,
                MediaScope::RecordManage,
                MediaScope::FileRead,
                MediaScope::FileDelete,
                MediaScope::ServerAdmin,
            ],
        };
        let expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(session_cfg.session_ttl_sec);
        self.sessions.write().unwrap().insert(
            token.clone(),
            super::SessionEntry {
                principal,
                expires_at,
            },
        );

        let cookie_value = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            session_cfg.cookie_name, token
        );
        let mut response = zlm_response(
            0,
            "success",
            json!({
                "cookie_name": session_cfg.cookie_name,
                "cookie": token,
            }),
        );
        response.headers.push(HttpHeader {
            name: "set-cookie".to_string(),
            value: cookie_value,
        });
        Ok(response)
    }

    pub(crate) async fn logout(
        &self,
        _ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let cfg = self.config.read().unwrap();
        if let Some(session_cfg) = cfg.auth.session.as_ref() {
            if let Some(token) = cookie_from_header(&req, &session_cfg.cookie_name) {
                self.sessions.write().unwrap().remove(&token);
            }
        }
        Ok(zlm_response(0, "success", json!({"result": true})))
    }
}
