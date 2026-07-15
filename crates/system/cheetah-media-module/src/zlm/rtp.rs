//! ZLMediaKit-compatible RTP endpoint handlers.
//!
//! ZLMediaKit 兼容的 RTP 端点处理函数。

use std::collections::HashMap;
use std::net::SocketAddr;

use cheetah_media_api::command::{RtpReceiverRequest, RtpSenderMode, RtpSenderRequest};
use cheetah_media_api::ids::{MediaKey, RtpSessionId, StreamKeyBridge};
use cheetah_media_api::model::RtpTcpMode;
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;
use crate::zlm::ZlmMediaHttpService;

impl ZlmMediaHttpService {
    pub(crate) async fn open_rtp_server(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let tcp_mode = parse_zlm_rtp_tcp_mode(&params);
        let request = RtpReceiverRequest {
            media_key: key,
            port: parse_zlm_u16(&params, "port")?,
            ip: params["ip"].as_str().map(String::from),
            ssrc: parse_zlm_u32(&params, "ssrc")?,
            enable_rtcp: crate::util::parse_json_bool(&params["enable_rtcp"]).unwrap_or(false),
            tcp_mode,
            payload_type: parse_zlm_u8(&params, "payload_type")?,
            codec_hint: params["codec_hint"]
                .as_str()
                .or_else(|| params["payload_mode"].as_str())
                .map(String::from),
            reuse_port: crate::util::parse_json_bool(&params["reuse_port"]).unwrap_or(false),
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
        };
        let session = rtp_api.open_rtp_receiver(ctx, request).await?;
        Ok(super::zlm_response(
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
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let session_id = zlm_rtp_session_id(&key, "recv");
        rtp_api
            .stop_rtp_session(ctx, &RtpSessionId(session_id))
            .await?;
        Ok(super::zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn start_send_rtp(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let destination = parse_zlm_destination(&params)?;
        let ssrc = parse_zlm_u32(&params, "ssrc")?;
        let codec_hint = params["codec_hint"]
            .as_str()
            .or_else(|| params["payload_mode"].as_str())
            .or_else(|| {
                if crate::util::parse_json_bool(&params["use_ps"]).unwrap_or(true) {
                    Some("ps")
                } else {
                    Some("es")
                }
            })
            .map(String::from);
        let tcp_mode = parse_zlm_rtp_tcp_mode(&params);
        let mut transport_options = HashMap::new();
        let mode = match tcp_mode {
            Some(RtpTcpMode::Passive) => {
                transport_options.insert("tcp".to_string(), "true".to_string());
                RtpSenderMode::Passive
            }
            Some(RtpTcpMode::Active) => {
                transport_options.insert("tcp".to_string(), "true".to_string());
                RtpSenderMode::Active
            }
            None => RtpSenderMode::Active,
        };
        let request = RtpSenderRequest {
            media_key: key,
            destination_endpoint: destination,
            ssrc,
            payload_type: parse_zlm_u8(&params, "payload_type")?,
            codec_hint,
            mode,
            transport_options,
        };
        let session = rtp_api.open_rtp_sender(ctx, request).await?;
        Ok(super::zlm_response(
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
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let session_id = zlm_rtp_session_id(&key, "send");
        rtp_api
            .stop_rtp_session(ctx, &RtpSessionId(session_id))
            .await?;
        Ok(super::zlm_response(
            0,
            "success",
            serde_json::json!({"result": true}),
        ))
    }

    pub(crate) async fn get_rtp_info(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let mut info = None;
        for kind in ["recv", "send"] {
            let id = zlm_rtp_session_id(&key, kind);
            if let Ok(session) = rtp_api.get_rtp_session(ctx, &RtpSessionId(id)).await {
                info = Some(session);
                break;
            }
        }
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
        Ok(super::zlm_response(0, "success", data))
    }
}

fn parse_zlm_u16(params: &serde_json::Value, key: &str) -> Result<Option<u16>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u16::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

fn parse_zlm_u32(params: &serde_json::Value, key: &str) -> Result<Option<u32>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u32::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

fn parse_zlm_u8(params: &serde_json::Value, key: &str) -> Result<Option<u8>, AdapterError> {
    if params[key].is_null() {
        return Ok(None);
    }
    let v = crate::util::parse_json_u64(&params[key])
        .ok_or_else(|| AdapterError::InvalidRequest(format!("{key} is not a valid number")))?;
    u8::try_from(v)
        .map(Some)
        .map_err(|_| AdapterError::InvalidRequest(format!("{key} is out of range")))
}

fn parse_zlm_rtp_tcp_mode(params: &serde_json::Value) -> Option<RtpTcpMode> {
    if let Some(s) = params["tcp_mode"].as_str() {
        match s.to_lowercase().as_str() {
            "0" => return None,
            "passive" | "1" => return Some(RtpTcpMode::Passive),
            "active" | "2" => return Some(RtpTcpMode::Active),
            _ => {}
        }
    }
    if let Some(n) = crate::util::parse_json_u64(&params["tcp_mode"]) {
        match n {
            0 => return None,
            1 => return Some(RtpTcpMode::Passive),
            2 => return Some(RtpTcpMode::Active),
            _ => {}
        }
    }
    if crate::util::parse_json_bool(&params["tcp"]).unwrap_or(false)
        || crate::util::parse_json_bool(&params["enable_tcp"]).unwrap_or(false)
    {
        return Some(RtpTcpMode::Passive);
    }
    if crate::util::parse_json_bool(&params["is_udp"]).unwrap_or(true) {
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

fn zlm_rtp_session_id(key: &MediaKey, kind: &str) -> String {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
    format!("{kind}/{namespace}/{path}")
}
