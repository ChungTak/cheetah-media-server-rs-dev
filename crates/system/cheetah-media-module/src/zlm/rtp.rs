//! ZLMediaKit-compatible RTP endpoint handlers.
//!
//! ZLMediaKit 兼容的 RTP 端点处理函数。

use std::collections::HashMap;
use std::net::SocketAddr;

use cheetah_media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest,
    UpdateRtpRequest,
};
use cheetah_media_api::ids::{MediaKey, RtpSessionId, StreamKeyBridge};
use cheetah_media_api::model::{RtpSessionKind, RtpTcpMode};
use cheetah_media_api::port::MediaRequestContext;
use cheetah_sdk::{HttpRequest, HttpResponse};

use crate::error::AdapterError;
use crate::zlm::{
    zlm_response, Data, Empty, HitResult, OpenRtpServerResult, RtpInfo, RtpPauseResult,
    RtpServerItem, RtpUpdateResult, StartSendRtpResult, ZlmMediaHttpService, ZlmResponse,
};

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
        Ok(zlm_response(ZlmResponse::ok(OpenRtpServerResult {
            port: session.local_port.unwrap_or(0),
            ssrc: session.ssrc,
            session_id: session.session_id.0,
        })))
    }

    pub(crate) async fn open_rtp_server_multiplex(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.open_rtp_server_internal(ctx, req, true).await
    }

    async fn open_rtp_server_internal(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
        reuse_port: bool,
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
            reuse_port,
            timeout_ms: crate::util::parse_json_u64(&params["timeout_ms"]).unwrap_or(10_000),
        };
        let session = rtp_api.open_rtp_receiver(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(OpenRtpServerResult {
            port: session.local_port.unwrap_or(0),
            ssrc: session.ssrc,
            session_id: session.session_id.0,
        })))
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
        Ok(zlm_response(ZlmResponse::ok(HitResult { hit: 1 })))
    }

    pub(crate) async fn connect_rtp_server(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let session_id = params["session_id"]
            .as_str()
            .or_else(|| params["stream_id"].as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| zlm_rtp_session_id(&key, "recv"));
        let remote_endpoint = parse_zlm_destination_optional(&params).unwrap_or_default();
        let request = RtpConnectRequest {
            session_id: RtpSessionId(session_id),
            remote_endpoint,
            ssrc: parse_zlm_u32(&params, "ssrc")?,
        };
        let session = rtp_api.connect_rtp_receiver(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(OpenRtpServerResult {
            port: session.local_port.unwrap_or(0),
            ssrc: session.ssrc,
            session_id: session.session_id.0,
        })))
    }

    pub(crate) async fn list_rtp_server(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let mut query = RtpQuery {
            kind: Some(RtpSessionKind::Receiver),
            page: super::page_from_params(&params),
            page_size: super::page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(ctx, query).await?;
        let items: Vec<RtpServerItem> = page.items.into_iter().map(RtpServerItem::from).collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
    }

    pub(crate) async fn start_send_rtp(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.start_send_rtp_with_mode(ctx, req, RtpSenderMode::Active)
            .await
    }

    pub(crate) async fn start_send_rtp_passive(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.start_send_rtp_with_mode(ctx, req, RtpSenderMode::Passive)
            .await
    }

    pub(crate) async fn start_send_rtp_talk(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.start_send_rtp_with_mode(ctx, req, RtpSenderMode::Talk)
            .await
    }

    async fn start_send_rtp_with_mode(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
        mode: RtpSenderMode,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let key = self.parse_media_key(&params)?;
        let destination = parse_zlm_destination_optional(&params).unwrap_or_default();
        let ssrc = parse_zlm_u32(&params, "ssrc")?;
        let codec_hint = params["codec"]
            .as_str()
            .or_else(|| params["codec_hint"].as_str())
            .or_else(|| params["payload_mode"].as_str())
            .map(String::from);
        let mut transport_options = HashMap::new();
        let tcp_mode = parse_zlm_rtp_tcp_mode(&params);
        if matches!(
            tcp_mode,
            Some(RtpTcpMode::Passive) | Some(RtpTcpMode::Active)
        ) || crate::util::parse_json_bool(&params["tcp"]).unwrap_or(false)
        {
            transport_options.insert("tcp".to_string(), "true".to_string());
        }
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
        Ok(zlm_response(ZlmResponse::ok(StartSendRtpResult {
            local_port: session.local_port.unwrap_or(0),
            ssrc: session.ssrc,
            session_id: session.session_id.0,
        })))
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
        Ok(zlm_response(ZlmResponse::ok(Empty)))
    }

    pub(crate) async fn list_rtp_sender(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let mut query = RtpQuery {
            kind: Some(RtpSessionKind::Sender),
            page: super::page_from_params(&params),
            page_size: super::page_size_from_params(&params),
            ..Default::default()
        };
        query.clamp_page_size();
        let page = rtp_api.list_rtp_sessions(ctx, query).await?;
        let items: Vec<RtpServerItem> = page.items.into_iter().map(RtpServerItem::from).collect();
        Ok(zlm_response(ZlmResponse::ok(Data::new(items))))
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
        let body = if let Some(s) = info {
            RtpInfo::from(s)
        } else {
            RtpInfo {
                exist: false,
                peer_ip: None,
                peer_port: None,
                local_ip: None,
                local_port: None,
            }
        };
        Ok(zlm_response(ZlmResponse::ok(body)))
    }

    pub(crate) async fn update_rtp_server_ssrc(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let session_id = self.rtp_session_id_from_params(&params)?;
        let expected_generation = rtp_api
            .get_rtp_session(ctx, &session_id)
            .await
            .map(|s| s.generation)
            .unwrap_or(0);
        let request = UpdateRtpRequest {
            session_id,
            expected_generation,
            ssrc: parse_zlm_u32(&params, "ssrc")?,
            payload_type: parse_zlm_u8(&params, "payload_type")?,
            pause_check: None,
        };
        let session = rtp_api.update_rtp_session(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(RtpUpdateResult {
            session_id: session.session_id.0,
            ssrc: session.ssrc,
        })))
    }

    pub(crate) async fn pause_rtp_check(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.set_rtp_check(ctx, req, true).await
    }

    pub(crate) async fn resume_rtp_check(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
    ) -> Result<HttpResponse, AdapterError> {
        self.set_rtp_check(ctx, req, false).await
    }

    async fn set_rtp_check(
        &self,
        ctx: &MediaRequestContext,
        req: HttpRequest,
        paused: bool,
    ) -> Result<HttpResponse, AdapterError> {
        let rtp_api = self.rtp()?;
        let params = self.extract_params(&req)?;
        let session_id = self.rtp_session_id_from_params(&params)?;
        let expected_generation = rtp_api
            .get_rtp_session(ctx, &session_id)
            .await
            .map(|s| s.generation)
            .unwrap_or(0);
        let request = UpdateRtpRequest {
            session_id,
            expected_generation,
            ssrc: None,
            payload_type: None,
            pause_check: Some(paused),
        };
        let session = rtp_api.update_rtp_session(ctx, request).await?;
        Ok(zlm_response(ZlmResponse::ok(RtpPauseResult {
            session_id: session.session_id.0,
            check_paused: session.check_paused,
        })))
    }

    fn rtp_session_id_from_params(
        &self,
        params: &serde_json::Value,
    ) -> Result<RtpSessionId, AdapterError> {
        if let Some(id) = params["session_id"]
            .as_str()
            .or_else(|| params["stream_id"].as_str())
        {
            return Ok(RtpSessionId(id.to_string()));
        }
        let key = self.parse_media_key(params)?;
        Ok(RtpSessionId(zlm_rtp_session_id(&key, "recv")))
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

fn parse_zlm_destination_optional(params: &serde_json::Value) -> Option<String> {
    if let Some(url) = params["dst_url"].as_str() {
        if url.parse::<SocketAddr>().is_ok() {
            return Some(url.to_string());
        }
    }
    let ip = params["dst_ip"].as_str()?;
    let port = parse_zlm_u16(params, "dst_port").ok()??;
    let endpoint = format!("{ip}:{port}");
    if endpoint.parse::<SocketAddr>().is_ok() {
        Some(endpoint)
    } else {
        None
    }
}

fn zlm_rtp_session_id(key: &MediaKey, kind: &str) -> String {
    let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
    format!("{kind}:{namespace}:{path}")
}
