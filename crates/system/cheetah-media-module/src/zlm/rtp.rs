//! RTP route handlers for the ZLMediaKit-compatible adapter.
//!
//! 为 ZLMediaKit 兼容适配器实现的 RTP 路由处理器。

use std::net::SocketAddr;

use cheetah_media_api::command::{RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest};
use cheetah_media_api::ids::RtpSessionId;
use cheetah_media_api::model::RtpTcpMode;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;

use super::{parse_zlm_u16, parse_zlm_u32, parse_zlm_u8, zlm_response, ZlmMediaHttpService};

impl ZlmMediaHttpService {
    pub(crate) async fn open_rtp_server(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let tcp_mode = parse_zlm_rtp_tcp_mode(&params);
        let request = RtpReceiverRequest {
            media_key: key,
            port: parse_zlm_u16(&params, "port")?,
            ip: params["ip"].as_str().map(String::from),
            ssrc: parse_zlm_u32(&params, "ssrc")?,
            enable_rtcp: params["enable_rtcp"].as_bool().unwrap_or(false),
            tcp_mode,
            payload_type: parse_zlm_u8(&params, "payload_type")?,
            codec_hint: params["codec_hint"]
                .as_str()
                .or_else(|| params["payload_mode"].as_str())
                .map(String::from),
            reuse_port: params["reuse_port"].as_bool().unwrap_or(false),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
        };
        let session = rtp_api.open_rtp_receiver(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "port": session.local_port,
                "ssrc": session.ssrc,
                "session_id": session.session_id.0,
            }),
        ))
    }

    pub(crate) async fn close_rtp_server(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let session_id = super::zlm_key_string(&key);
        rtp_api
            .stop_rtp_session(&ctx, &RtpSessionId(session_id))
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn start_send_rtp(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let destination = parse_zlm_destination(&params)?;
        let ssrc = parse_zlm_u32(&params, "ssrc")?;
        let codec_hint = params["codec_hint"]
            .as_str()
            .or_else(|| params["payload_mode"].as_str())
            .or_else(|| {
                if params["use_ps"].as_bool().unwrap_or(true) {
                    Some("ps")
                } else {
                    Some("es")
                }
            })
            .map(String::from);
        let request = RtpSenderRequest {
            media_key: key,
            destination_endpoint: destination,
            ssrc,
            payload_type: parse_zlm_u8(&params, "payload_type")?,
            codec_hint,
            mode: RtpSenderMode::Active,
            transport_options: std::collections::HashMap::new(),
        };
        let session = rtp_api.open_rtp_sender(&ctx, request).await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({
                "ssrc": session.ssrc,
                "session_id": session.session_id.0,
            }),
        ))
    }

    pub(crate) async fn stop_send_rtp(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let session_id = super::zlm_key_string(&key);
        rtp_api
            .stop_rtp_session(&ctx, &RtpSessionId(session_id))
            .await?;
        Ok(zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn get_rtp_info(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let ctx = self.request_context(&req);
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut query = RtpQuery {
            page_size: RtpQuery::MAX_PAGE_SIZE,
            ..Default::default()
        };
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(&ctx, query).await?;
        let info = page.items.into_iter().find(|s| s.media_key == key);
        let data = info
            .map(|s| {
                serde_json::json!({
                    "session_id": s.session_id.0,
                    "port": s.local_port,
                    "ssrc": s.ssrc,
                    "remote_endpoint": s.remote_endpoint,
                    "state": s.state,
                })
            })
            .unwrap_or_else(|| serde_json::json!({"exists": false}));
        Ok(zlm_response(0, "success", data))
    }
}

fn parse_zlm_rtp_tcp_mode(params: &serde_json::Value) -> Option<RtpTcpMode> {
    if let Some(s) = params["tcp_mode"].as_str() {
        match s.to_lowercase().as_str() {
            "passive" | "0" => return Some(RtpTcpMode::Passive),
            "active" | "1" => return Some(RtpTcpMode::Active),
            _ => {}
        }
    }
    if let Some(n) = crate::util::parse_json_u64(&params["tcp_mode"]) {
        match n {
            0 => return Some(RtpTcpMode::Passive),
            1 => return Some(RtpTcpMode::Active),
            _ => {}
        }
    }
    if params["tcp"].as_bool().unwrap_or(false) || params["enable_tcp"].as_bool().unwrap_or(false) {
        return Some(RtpTcpMode::Passive);
    }
    if params["is_udp"].as_bool().unwrap_or(true) {
        return None;
    }
    Some(RtpTcpMode::Passive)
}

fn parse_zlm_destination(params: &serde_json::Value) -> Result<String, AdapterError> {
    if let Some(url) = params["dst_url"].as_str() {
        if url.parse::<SocketAddr>().is_err() {
            return Err(AdapterError::InvalidRequest(format!(
                "invalid destination endpoint: {url}"
            )));
        }
        return Ok(url.to_string());
    }
    let ip = params["dst_ip"]
        .as_str()
        .ok_or_else(|| AdapterError::InvalidRequest("dst_ip is required".to_string()))?;
    let port = parse_zlm_u16(params, "dst_port")?
        .ok_or_else(|| AdapterError::InvalidRequest("dst_port is required".to_string()))?;
    let endpoint = format!("{ip}:{port}");
    if endpoint.parse::<SocketAddr>().is_err() {
        return Err(AdapterError::InvalidRequest(format!(
            "invalid destination endpoint: {endpoint}"
        )));
    }
    Ok(endpoint)
}
