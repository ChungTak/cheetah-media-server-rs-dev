//! Bridge `cheetah_media_api::port::RtpApi` to the Tokio RTP driver.
//!
//! The provider is registered in `RtpModule::init` and is backed by the same
//! `RtpDriverHandle` that the module's HTTP service uses. When the module is
//! stopped the handle is dropped and the provider returns `Unavailable`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest,
    UpdateRtpRequest,
};
use cheetah_media_api::error::{MediaError, Result};
use cheetah_media_api::ids::{MediaKey, RtpSessionId, StreamKeyBridge};
use cheetah_media_api::model::{Page, RtpSession, RtpSessionKind, RtpSessionState, RtpTcpMode};
use cheetah_media_api::port::{MediaRequestContext, RtpApi};
use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpServerSpec, RtpTrackFilter,
    RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle};
use parking_lot::Mutex;

/// Media-domain `RtpApi` provider backed by the module's Tokio driver.
///
/// 由模块 Tokio 驱动支撑的 `RtpApi` provider。
pub struct RtpMediaProvider {
    driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>,
    listen_port: u16,
}

impl RtpMediaProvider {
    /// Create a provider bound to the shared driver handle.
    ///
    /// 创建绑定到共享驱动句柄的 provider。
    pub fn new(driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>, listen_port: u16) -> Self {
        Self {
            driver_handle,
            listen_port,
        }
    }

    fn driver(&self) -> Result<Arc<RtpDriverHandle>> {
        self.driver_handle
            .lock()
            .clone()
            .ok_or_else(|| MediaError::unavailable("RTP driver is not running"))
    }

    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn session_key_from_media_key(key: &MediaKey) -> String {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
        format!("{namespace}/{path}")
    }

    fn parse_payload_mode(hint: &Option<String>, payload_type: Option<u8>) -> RtpPayloadMode {
        if let Some(s) = hint {
            return parse_payload_mode_str(s);
        }
        // Best-effort mapping from a handful of common RTP payload numbers.
        match payload_type {
            Some(0) => RtpPayloadMode::RawAudio,
            Some(8) => RtpPayloadMode::RawAudio,
            Some(33) => RtpPayloadMode::Ts,
            Some(96) | Some(97) | Some(98) | Some(99) => RtpPayloadMode::Es,
            _ => RtpPayloadMode::Ps,
        }
    }

    fn receiver_connection_type(tcp_mode: Option<RtpTcpMode>) -> Option<RtpConnectionType> {
        match tcp_mode {
            Some(RtpTcpMode::Passive) => Some(RtpConnectionType::TcpPassive),
            Some(RtpTcpMode::Active) => Some(RtpConnectionType::TcpActive),
            None => Some(RtpConnectionType::UdpPassive),
        }
    }

    fn sender_connection_type(
        mode: RtpSenderMode,
        transport_options: &HashMap<String, String>,
    ) -> Option<RtpConnectionType> {
        let tcp = transport_options
            .get("tcp")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        match mode {
            RtpSenderMode::Active if tcp => Some(RtpConnectionType::TcpActive),
            RtpSenderMode::Active => Some(RtpConnectionType::UdpActive),
            RtpSenderMode::Passive if tcp => Some(RtpConnectionType::TcpPassive),
            RtpSenderMode::Passive => Some(RtpConnectionType::UdpPassive),
            RtpSenderMode::Talk => Some(RtpConnectionType::VoiceTalk),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_session(
        &self,
        session_id: RtpSessionId,
        kind: RtpSessionKind,
        media_key: MediaKey,
        remote_endpoint: Option<String>,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        tcp_mode: Option<RtpTcpMode>,
        reuse_port: bool,
        state: RtpSessionState,
    ) -> RtpSession {
        RtpSession {
            session_id,
            kind,
            media_key,
            local_port: Some(self.listen_port),
            remote_endpoint,
            ssrc,
            payload_type,
            tcp_mode,
            reuse_port,
            state,
            created_at: self.now_ms(),
        }
    }
}

#[async_trait]
impl RtpApi for RtpMediaProvider {
    async fn open_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpReceiverRequest,
    ) -> Result<RtpSession> {
        let driver = self.driver()?;
        let session_key = Self::session_key_from_media_key(&request.media_key);
        let session_id = RtpSessionId(session_key.clone());

        let spec = RtpServerSpec {
            session_key,
            ssrc: request.ssrc,
            payload_mode: Self::parse_payload_mode(&request.codec_hint, request.payload_type),
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: Self::receiver_connection_type(request.tcp_mode),
            track_filter: RtpTrackFilter::All,
        };

        driver
            .send_command(RtpDriverCommand::CreateServer(spec))
            .await;

        Ok(self.build_session(
            session_id,
            RtpSessionKind::Receiver,
            request.media_key,
            None,
            request.ssrc,
            request.payload_type,
            request.tcp_mode,
            request.reuse_port,
            RtpSessionState::Listening,
        ))
    }

    async fn connect_rtp_receiver(
        &self,
        _ctx: &MediaRequestContext,
        _request: RtpConnectRequest,
    ) -> Result<RtpSession> {
        Err(MediaError::unsupported("active RTP receiver connection"))
    }

    async fn open_rtp_sender(
        &self,
        _ctx: &MediaRequestContext,
        request: RtpSenderRequest,
    ) -> Result<RtpSession> {
        let driver = self.driver()?;
        let session_key = Self::session_key_from_media_key(&request.media_key);
        let session_id = RtpSessionId(session_key.clone());

        let destination: SocketAddr = request.destination_endpoint.parse().map_err(|e| {
            MediaError::invalid_argument(format!("invalid destination endpoint: {e}"))
        })?;

        let ssrc = request.ssrc.unwrap_or(0);
        let payload_mode = Self::parse_payload_mode(&request.codec_hint, request.payload_type);
        let transport_mode = if request.mode == RtpSenderMode::Talk {
            RtpTransportMode::SendRecv
        } else {
            RtpTransportMode::SendOnly
        };

        let spec = RtpClientSpec {
            session_key,
            destination,
            ssrc,
            payload_mode,
            transport_mode,
            tcp_conn_id: None,
            connection_type: Self::sender_connection_type(request.mode, &request.transport_options),
            track_filter: RtpTrackFilter::All,
        };

        driver
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        Ok(self.build_session(
            session_id,
            RtpSessionKind::Sender,
            request.media_key,
            Some(request.destination_endpoint),
            request.ssrc,
            request.payload_type,
            None,
            false,
            RtpSessionState::Created,
        ))
    }

    async fn stop_rtp_session(&self, _ctx: &MediaRequestContext, id: &RtpSessionId) -> Result<()> {
        let driver = self.driver()?;
        driver
            .send_command(RtpDriverCommand::StopSession(id.0.clone()))
            .await;
        Ok(())
    }

    async fn list_rtp_sessions(
        &self,
        _ctx: &MediaRequestContext,
        query: RtpQuery,
    ) -> Result<Page<RtpSession>> {
        let mut query = query;
        query.clamp_page_size();
        Ok(Page {
            items: Vec::new(),
            page: query.page,
            page_size: query.page_size,
            total: 0,
            next_cursor: None,
        })
    }

    async fn update_rtp_session(
        &self,
        _ctx: &MediaRequestContext,
        _request: UpdateRtpRequest,
    ) -> Result<RtpSession> {
        Err(MediaError::unsupported("RTP session update"))
    }
}

fn parse_payload_mode_str(s: &str) -> RtpPayloadMode {
    match s.to_lowercase().as_str() {
        "ps" | "1" => RtpPayloadMode::Ps,
        "ts" | "2" => RtpPayloadMode::Ts,
        "es" | "3" => RtpPayloadMode::Es,
        "ehome" | "4" => RtpPayloadMode::Ehome,
        "xhb" | "hk" => RtpPayloadMode::Xhb,
        "jtt1078" | "1078" => RtpPayloadMode::Jtt1078,
        "raw_audio" | "audio" => RtpPayloadMode::RawAudio,
        "raw_video" | "video" => RtpPayloadMode::RawVideo,
        _ => RtpPayloadMode::Ps,
    }
}
