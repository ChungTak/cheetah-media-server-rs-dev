//! Central RTP session orchestrator shared by the module HTTP service and the
//! `RtpApi` provider.
//!
//! `RtpSessionOrchestrator` owns the session directory, the driver handle, and
//! the common command-building logic so that HTTP, native, and ZLM adapters all
//! drive the same RTP driver state machine.
//!
//! 中央 RTP 会话编排器，供模块 HTTP 服务与 `RtpApi` provider 共享。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpServerSpec, RtpTrackFilter,
    RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle, RtpSocketReuse};
use cheetah_sdk::media_api::command::{
    RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest, UpdateRtpRequest,
};
use cheetah_sdk::media_api::error::{MediaError, Result};
use cheetah_sdk::media_api::ids::{MediaKey, RtpSessionId, StreamKeyBridge};
use cheetah_sdk::media_api::model::{
    Page, RtpSession, RtpSessionKind, RtpSessionState, RtpTcpMode,
};
use parking_lot::Mutex;

/// Shared RTP session state and driver command dispatcher.
///
/// 共享的 RTP 会话状态与驱动命令分发器。
pub struct RtpSessionOrchestrator {
    driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>,
    sessions: Arc<Mutex<HashMap<RtpSessionId, RtpSession>>>,
    /// Default address used when a caller does not supply an explicit IP/port.
    default_bind_addr: SocketAddr,
}

impl RtpSessionOrchestrator {
    /// Maximum number of tracked RTP sessions before rejecting new ones.
    const MAX_SESSIONS: usize = 10_000;

    /// Create an orchestrator bound to the shared driver handle.
    ///
    /// 创建绑定到共享驱动句柄的编排器。
    pub fn new(
        driver_handle: Arc<Mutex<Option<Arc<RtpDriverHandle>>>>,
        default_bind_addr: SocketAddr,
    ) -> Self {
        Self {
            driver_handle,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_bind_addr,
        }
    }

    /// Install the concrete driver handle once the module has started it.
    ///
    /// 模块启动驱动后，安装具体驱动句柄。
    pub fn set_driver_handle(&self, handle: Arc<RtpDriverHandle>) {
        *self.driver_handle.lock() = Some(handle);
    }

    /// Clear the driver handle during module shutdown.
    ///
    /// 模块关闭期间清除驱动句柄。
    pub fn clear_driver_handle(&self) {
        *self.driver_handle.lock() = None;
    }

    pub fn driver(&self) -> Result<Arc<RtpDriverHandle>> {
        self.driver_handle
            .lock()
            .clone()
            .ok_or_else(|| MediaError::unavailable("RTP driver is not running"))
    }

    /// Return the default bind address used when callers do not request an explicit IP/port.
    ///
    /// 返回调用方未显式请求 IP/port 时使用的默认绑定地址。
    pub fn default_bind_addr(&self) -> SocketAddr {
        self.default_bind_addr
    }

    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn session_key_from_media_key(key: &MediaKey, kind: &str) -> String {
        let (namespace, path) = StreamKeyBridge::to_namespace_path(key);
        format!("{kind}/{namespace}/{path}")
    }

    fn parse_payload_mode(hint: &Option<String>, payload_type: Option<u8>) -> RtpPayloadMode {
        if let Some(s) = hint {
            return parse_payload_mode_str(s);
        }
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
        local_port: Option<u16>,
        tcp_mode: Option<RtpTcpMode>,
        reuse_port: bool,
        state: RtpSessionState,
    ) -> RtpSession {
        RtpSession {
            session_id,
            kind,
            media_key,
            local_port,
            remote_endpoint,
            ssrc,
            payload_type,
            tcp_mode,
            reuse_port,
            state,
            created_at: self.now_ms(),
        }
    }

    fn insert_session(&self, session: RtpSession) -> Result<()> {
        let mut sessions = self.sessions.lock();
        if sessions.len() >= Self::MAX_SESSIONS {
            return Err(MediaError::unavailable("rtp session limit reached"));
        }
        sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    fn remove_session(&self, id: &RtpSessionId) {
        self.sessions.lock().remove(id);
    }

    /// Create a server (receiver) session, bind the requested local socket, and
    /// wait for the driver to confirm the actual bound port.
    ///
    /// 创建服务端（接收端）会话，绑定请求的本地端口，并等待驱动返回实际端口。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_server_session(
        &self,
        session_key: String,
        media_key: MediaKey,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        payload_mode: RtpPayloadMode,
        transport_mode: RtpTransportMode,
        connection_type: Option<RtpConnectionType>,
        track_filter: RtpTrackFilter,
        tcp_mode: Option<RtpTcpMode>,
        bind_addr: Option<SocketAddr>,
        reuse_port: bool,
        state: RtpSessionState,
    ) -> Result<RtpSession> {
        let driver = self.driver()?;
        let spec = RtpServerSpec {
            session_key: session_key.clone(),
            ssrc,
            payload_mode,
            transport_mode,
            connection_type,
            track_filter,
        };
        let socket_reuse = if reuse_port {
            RtpSocketReuse::Reuse
        } else {
            RtpSocketReuse::Exclusive
        };
        let actual_addr = driver
            .create_server(spec, bind_addr, socket_reuse)
            .await
            .map_err(|e| MediaError::unavailable(e.to_string()))?;

        let session_id = RtpSessionId(session_key);
        let session = self.build_session(
            session_id,
            RtpSessionKind::Receiver,
            media_key,
            None,
            ssrc,
            payload_type,
            Some(actual_addr.port()),
            tcp_mode,
            reuse_port,
            state,
        );
        self.insert_session(session.clone())?;
        Ok(session)
    }

    /// Create a client (sender) session and send `CreateClient` to the driver.
    ///
    /// 创建客户端（发送端）会话并向驱动发送 `CreateClient`。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_client_session(
        &self,
        session_key: String,
        media_key: MediaKey,
        destination: SocketAddr,
        remote_endpoint: String,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        payload_mode: RtpPayloadMode,
        transport_mode: RtpTransportMode,
        connection_type: Option<RtpConnectionType>,
        track_filter: RtpTrackFilter,
    ) -> Result<RtpSession> {
        let driver = self.driver()?;
        let session_id = RtpSessionId(session_key.clone());
        let session = self.build_session(
            session_id,
            RtpSessionKind::Sender,
            media_key,
            Some(remote_endpoint),
            ssrc,
            payload_type,
            None,
            None,
            false,
            RtpSessionState::Created,
        );
        self.insert_session(session.clone())?;

        let spec = RtpClientSpec {
            session_key,
            destination,
            ssrc: ssrc.unwrap_or(0),
            payload_mode,
            transport_mode,
            tcp_conn_id: None,
            connection_type,
            track_filter,
        };
        driver
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;
        Ok(session)
    }

    /// Stop a session by its opaque session key.
    ///
    /// 通过会话键停止会话。
    pub async fn stop_session_by_key(&self, session_key: &str) -> Result<()> {
        let id = RtpSessionId(session_key.to_string());

        // Best-effort stop command: if the driver is up, send the command before
        // removing the record. If the driver is not running (shutdown, not yet
        // started, or restart), there is no active session to stop, so we still
        // remove the local record to keep the directory consistent.
        if let Ok(driver) = self.driver() {
            driver
                .send_command(RtpDriverCommand::StopSession(session_key.to_string()))
                .await;
        }
        self.remove_session(&id);
        Ok(())
    }

    /// Open an RTP receiver from a domain request.
    ///
    /// 通过领域请求打开 RTP 接收端。
    pub async fn open_rtp_receiver(&self, request: RtpReceiverRequest) -> Result<RtpSession> {
        let session_key = Self::session_key_from_media_key(&request.media_key, "recv");
        let payload_mode = Self::parse_payload_mode(&request.codec_hint, request.payload_type);
        let connection_type = Self::receiver_connection_type(request.tcp_mode);
        let bind_addr = self.receiver_bind_addr(request.ip.as_deref(), request.port)?;
        self.create_server_session(
            session_key,
            request.media_key,
            request.ssrc,
            request.payload_type,
            payload_mode,
            RtpTransportMode::RecvOnly,
            connection_type,
            RtpTrackFilter::All,
            request.tcp_mode,
            bind_addr,
            request.reuse_port,
            RtpSessionState::Listening,
        )
        .await
    }

    /// Resolve a receiver bind address from an optional explicit `ip`/`port`.
    /// `port` of `None` or `0` asks the driver to allocate an ephemeral port from
    /// the default interface.
    ///
    /// 从可选的显式 ip/port 解析接收端绑定地址；port 为 None 或 0 时让驱动在默认接口上
    /// 分配临时端口。
    fn receiver_bind_addr(
        &self,
        ip: Option<&str>,
        port: Option<u16>,
    ) -> Result<Option<SocketAddr>> {
        let parsed_ip =
            match ip {
                Some(s) => Some(s.parse::<IpAddr>().map_err(|e| {
                    MediaError::invalid_argument(format!("invalid rtp bind ip: {e}"))
                })?),
                None => None,
            };
        match (parsed_ip, port) {
            (None, None) | (None, Some(0)) => Ok(None),
            _ => {
                let ip = parsed_ip.unwrap_or(self.default_bind_addr.ip());
                let port = port.unwrap_or(0);
                Ok(Some(SocketAddr::new(ip, port)))
            }
        }
    }

    /// Open an RTP sender from a domain request.
    ///
    /// 通过领域请求打开 RTP 发送端。
    pub async fn open_rtp_sender(&self, request: RtpSenderRequest) -> Result<RtpSession> {
        let session_key = Self::session_key_from_media_key(&request.media_key, "send");
        let destination: SocketAddr = request.destination_endpoint.parse().map_err(|e| {
            MediaError::invalid_argument(format!("invalid destination endpoint: {e}"))
        })?;
        let payload_mode = Self::parse_payload_mode(&request.codec_hint, request.payload_type);
        let transport_mode = if request.mode == RtpSenderMode::Talk {
            RtpTransportMode::SendRecv
        } else {
            RtpTransportMode::SendOnly
        };
        let connection_type =
            Self::sender_connection_type(request.mode, &request.transport_options);
        self.create_client_session(
            session_key,
            request.media_key,
            destination,
            request.destination_endpoint,
            request.ssrc,
            request.payload_type,
            payload_mode,
            transport_mode,
            connection_type,
            RtpTrackFilter::All,
        )
        .await
    }

    /// Stop an RTP session by domain identifier.
    ///
    /// 通过领域标识停止 RTP 会话。
    pub async fn stop_rtp_session(&self, id: &RtpSessionId) -> Result<()> {
        self.stop_session_by_key(&id.0).await
    }

    /// List tracked RTP sessions, optionally filtered.
    ///
    /// 列出已跟踪的 RTP 会话，可选过滤。
    pub fn list_rtp_sessions(&self, mut query: RtpQuery) -> Result<Page<RtpSession>> {
        query.clamp_page_size();
        if query.page == 0 {
            query.page = 1;
        }

        let sessions = self.sessions.lock();
        let mut items: Vec<RtpSession> = sessions.values().cloned().collect();
        drop(sessions);

        if let Some(kind) = query.kind {
            items.retain(|s| s.kind == kind);
        }
        if let Some(state) = query.state {
            items.retain(|s| s.state == state);
        }

        let total = items.len() as u64;
        let start = (query.page - 1).saturating_mul(query.page_size) as usize;
        let page_items = if start >= items.len() {
            Vec::new()
        } else {
            let end = start
                .saturating_add(query.page_size as usize)
                .min(items.len());
            items[start..end].to_vec()
        };

        Ok(Page {
            items: page_items,
            page: query.page,
            page_size: query.page_size,
            total,
            next_cursor: None,
        })
    }

    /// Update an RTP session. Currently only metadata that does not require a
    /// restart is accepted.
    ///
    /// 更新 RTP 会话。当前只接受不需要重启的元数据更新。
    pub fn update_rtp_session(&self, request: UpdateRtpRequest) -> Result<RtpSession> {
        if request.ssrc.is_some() || request.payload_type.is_some() || request.pause_check.is_some()
        {
            return Err(MediaError::unsupported("rtp session update"));
        }

        let sessions = self.sessions.lock();
        sessions
            .get(&request.session_id)
            .cloned()
            .ok_or_else(|| MediaError::not_found("rtp session"))
    }

    /// Retrieve a single RTP session.
    ///
    /// 获取单个 RTP 会话。
    pub fn get_rtp_session(&self, id: &RtpSessionId) -> Result<RtpSession> {
        let sessions = self.sessions.lock();
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| MediaError::not_found("rtp session"))
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
