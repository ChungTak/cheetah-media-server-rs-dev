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
use cheetah_rtp_driver_tokio::{RtpDriverCommand, RtpDriverHandle};
use cheetah_sdk::media_api::command::{
    RtpConnectRequest, RtpQuery, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest,
    UpdateRtpRequest,
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
    pub(crate) sessions: Arc<Mutex<HashMap<RtpSessionId, RtpSession>>>,
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
        format!("{kind}:{namespace}:{path}")
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
        let now = self.now_ms();
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
            check_paused: false,
            generation: 1,
            created_at: now,
            updated_at: now,
            last_error: None,
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
        let actual_addr = driver
            .create_server(spec, bind_addr, crate::egress::reuse_from_flag(reuse_port))
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
        let bind_addr = if connection_type == Some(RtpConnectionType::TcpActive) {
            None
        } else {
            self.receiver_bind_addr(request.ip.as_deref(), request.port)?
        };
        let state = if connection_type == Some(RtpConnectionType::TcpActive) {
            RtpSessionState::Created
        } else {
            RtpSessionState::Listening
        };
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
            state,
        )
        .await
    }

    /// Connect an RTP receiver to a remote endpoint. Used for TCP active mode.
    ///
    /// 为 RTP 接收端主动连接到远端地址（TCP active 模式）。
    pub async fn connect_rtp_receiver(&self, request: RtpConnectRequest) -> Result<RtpSession> {
        let destination: SocketAddr = request
            .remote_endpoint
            .parse()
            .map_err(|e| MediaError::invalid_argument(format!("invalid remote endpoint: {e}")))?;

        let (session_key, ssrc, payload_mode, tcp_mode) = {
            let mut sessions = self.sessions.lock();
            let session = sessions
                .get_mut(&request.session_id)
                .ok_or_else(|| MediaError::not_found("rtp session"))?;
            if session.kind != RtpSessionKind::Receiver {
                return Err(MediaError::invalid_argument("session is not a receiver"));
            }
            if session.ssrc.is_none() && request.ssrc.is_some() {
                session.ssrc = request.ssrc;
            }
            session.remote_endpoint = Some(request.remote_endpoint.clone());
            if session.state != RtpSessionState::Created {
                session.state = RtpSessionState::Created;
            }
            session.generation += 1;
            session.updated_at = self.now_ms();
            let ssrc = session.ssrc.unwrap_or(0);
            let payload_mode = Self::parse_payload_mode(&None, session.payload_type);
            (
                session.session_id.0.clone(),
                ssrc,
                payload_mode,
                session.tcp_mode,
            )
        };

        let connection_type = match tcp_mode {
            Some(RtpTcpMode::Active) => Some(RtpConnectionType::TcpActive),
            Some(RtpTcpMode::Passive) => {
                return Err(MediaError::invalid_argument(
                    "connect_rtp_receiver requires a TCP active session",
                ));
            }
            None => {
                return Err(MediaError::invalid_argument(
                    "connect_rtp_receiver requires a TCP active session",
                ));
            }
        };
        let spec = RtpClientSpec {
            session_key,
            destination,
            ssrc,
            payload_mode,
            transport_mode: RtpTransportMode::RecvOnly,
            tcp_conn_id: None,
            connection_type,
            track_filter: RtpTrackFilter::All,
        };

        let driver = self.driver()?;
        driver
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        let sessions = self.sessions.lock();
        sessions
            .get(&request.session_id)
            .cloned()
            .ok_or_else(|| MediaError::not_found("rtp session"))
    }

    /// Set the state of a tracked RTP session.
    ///
    /// 设置已跟踪 RTP 会话的状态。
    pub fn set_session_state(
        &self,
        id: &RtpSessionId,
        state: RtpSessionState,
    ) -> Result<RtpSession> {
        let mut sessions = self.sessions.lock();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found("rtp session"))?;
        if session.state != state {
            session.state = state;
            session.generation += 1;
            session.updated_at = self.now_ms();
        }
        Ok(session.clone())
    }

    /// Record the peer address observed for a session and move it to Connected.
    ///
    /// 记录会话观测到的对端地址，并将其状态推进到 Connected。
    pub fn set_session_remote_endpoint(
        &self,
        id: &RtpSessionId,
        remote: SocketAddr,
    ) -> Result<RtpSession> {
        let mut sessions = self.sessions.lock();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| MediaError::not_found("rtp session"))?;
        let new_remote = Some(remote.to_string());
        let mut changed = session.remote_endpoint != new_remote;
        session.remote_endpoint = new_remote;
        if matches!(
            session.state,
            RtpSessionState::Listening | RtpSessionState::Created
        ) {
            session.state = RtpSessionState::Connected;
            changed = true;
        }
        if changed {
            session.generation += 1;
            session.updated_at = self.now_ms();
        }
        Ok(session.clone())
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
        // A missing port or port 0 means "allocate a dedicated per-session UDP socket";
        // only TCP active mode bypasses UDP binding by passing `None` from the caller.
        let ip = parsed_ip.unwrap_or(self.default_bind_addr.ip());
        let port = port.unwrap_or(0);
        Ok(Some(SocketAddr::new(ip, port)))
    }

    /// Open an RTP sender from a domain request.
    ///
    /// 通过领域请求打开 RTP 发送端。
    pub async fn open_rtp_sender(&self, request: RtpSenderRequest) -> Result<RtpSession> {
        if request.mode == RtpSenderMode::Talk {
            return self.open_rtp_talk(request).await;
        }

        let session_key = Self::session_key_from_media_key(&request.media_key, "send");
        let destination: SocketAddr = request.destination_endpoint.parse().map_err(|e| {
            MediaError::invalid_argument(format!("invalid destination endpoint: {e}"))
        })?;
        let payload_mode = Self::parse_payload_mode(&request.codec_hint, request.payload_type);
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
            RtpTransportMode::SendOnly,
            connection_type,
            RtpTrackFilter::All,
        )
        .await
    }

    /// Upgrade an existing inbound session to bidirectional talkback audio.
    ///
    /// 将现有入站会话升级为双向对讲音频。
    pub async fn open_rtp_talk(&self, request: RtpSenderRequest) -> Result<RtpSession> {
        let recv_key = Self::session_key_from_media_key(&request.media_key, "recv");
        let id = RtpSessionId(recv_key.clone());

        // Lock the directory, validate and mutate the receiver session, then release the
        // guard before any `.await` because `parking_lot::MutexGuard` is not `Send`.
        let destination = {
            let mut sessions = self.sessions.lock();
            let session = sessions
                .get_mut(&id)
                .ok_or_else(|| MediaError::not_found("rtp receiver session"))?;

            if session.kind != RtpSessionKind::Receiver && session.kind != RtpSessionKind::Talk {
                return Err(MediaError::invalid_argument("session is not a receiver"));
            }

            let destination = session
                .remote_endpoint
                .as_deref()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| {
                    MediaError::unavailable("receiver has not received any traffic yet")
                })?;

            session.kind = RtpSessionKind::Talk;
            session.state = RtpSessionState::Connected;
            session.generation += 1;
            session.updated_at = self.now_ms();
            destination
        };

        let payload_mode = Self::parse_payload_mode(&request.codec_hint, request.payload_type);
        let ssrc = request.ssrc.unwrap_or(0);
        let spec = RtpClientSpec {
            session_key: recv_key.clone(),
            destination,
            ssrc,
            payload_mode,
            transport_mode: RtpTransportMode::SendRecv,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::VoiceTalk),
            track_filter: RtpTrackFilter::OnlyAudio,
        };

        let driver = self.driver()?;
        driver
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        let sessions = self.sessions.lock();
        sessions
            .get(&id)
            .cloned()
            .ok_or_else(|| MediaError::not_found("rtp session"))
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
        if let Some(ref id) = query.session_id {
            items.retain(|s| &s.session_id == id);
        }
        if let Some(ref key) = query.media_key {
            items.retain(|s| &s.media_key == key);
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

    /// Update an RTP session.
    ///
    /// `expected_generation` is compared to the local snapshot before the core update is
    /// attempted; the core compares it again atomically. A conflicting or failed update
    /// leaves both the core and the snapshot unchanged.
    ///
    /// 更新 RTP 会话。
    pub async fn update_rtp_session(&self, request: UpdateRtpRequest) -> Result<RtpSession> {
        if request.ssrc.is_none() && request.payload_type.is_none() && request.pause_check.is_none()
        {
            return Err(MediaError::invalid_argument("empty patch"));
        }

        let driver = self.driver()?;
        let (session_key, generation) = {
            let sessions = self.sessions.lock();
            let session = sessions
                .get(&request.session_id)
                .ok_or_else(|| MediaError::not_found("rtp session"))?;
            (session.session_id.0.clone(), session.generation)
        };

        if request.expected_generation != generation {
            return Err(MediaError::conflict("generation mismatch"));
        }

        let ack = driver
            .update_session(
                session_key,
                request.expected_generation,
                request.ssrc,
                request.payload_type,
                request.pause_check,
            )
            .await
            .map_err(|e| MediaError::unavailable(e.to_string()))?;

        let mut sessions = self.sessions.lock();
        let session = sessions
            .get_mut(&request.session_id)
            .ok_or_else(|| MediaError::not_found("rtp session"))?;
        if let Some(ssrc) = ack.ssrc {
            session.ssrc = Some(ssrc);
        }
        if let Some(payload_type) = ack.payload_type {
            session.payload_type = Some(payload_type);
        }
        if let Some(pause_check) = ack.pause_check {
            session.check_paused = pause_check;
        }
        session.generation = ack.generation;
        session.updated_at = self.now_ms();
        session.last_error = None;
        Ok(session.clone())
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use super::*;
    use cheetah_rtp_driver_tokio::{start_driver, RtpDriverConfig};
    use cheetah_runtime_api::CancellationToken;
    use cheetah_sdk::media_api::command::{
        RtpConnectRequest, RtpReceiverRequest, RtpSenderMode, RtpSenderRequest, UpdateRtpRequest,
    };
    use cheetah_sdk::media_api::error::MediaErrorCode;
    use cheetah_sdk::media_api::ids::{MediaKey, RtpSessionId};
    use cheetah_sdk::media_api::model::{RtpSessionKind, RtpSessionState, RtpTcpMode};

    fn test_driver_config() -> RtpDriverConfig {
        RtpDriverConfig {
            listen_udp: "127.0.0.1:0".parse().unwrap(),
            listen_tcp: "127.0.0.1:0".parse().unwrap(),
            listen_rtcp_udp: None,
            write_queue_capacity: 256,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 10,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
        }
    }

    fn make_orchestrator() -> (RtpSessionOrchestrator, CancellationToken) {
        let config = test_driver_config();
        let cancel = CancellationToken::new();
        let handle = Arc::new(start_driver(config, cancel.clone()));
        let driver_slot: Arc<Mutex<Option<Arc<RtpDriverHandle>>>> =
            Arc::new(Mutex::new(Some(handle)));
        let default_bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let orchestrator = RtpSessionOrchestrator::new(driver_slot, default_bind);
        (orchestrator, cancel)
    }

    fn receiver_request(media_key: MediaKey) -> RtpReceiverRequest {
        RtpReceiverRequest {
            media_key,
            port: None,
            ip: None,
            ssrc: Some(1000),
            enable_rtcp: false,
            tcp_mode: None,
            payload_type: Some(96),
            codec_hint: Some("ps".to_string()),
            reuse_port: false,
            timeout_ms: 0,
        }
    }

    #[tokio::test]
    async fn open_rtp_receiver_creates_session_with_generation_one_and_listening_state() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch1", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key.clone()))
            .await
            .expect("open receiver");

        assert_eq!(
            session.session_id,
            RtpSessionId("recv:test:ch1".to_string())
        );
        assert_eq!(session.kind, RtpSessionKind::Receiver);
        assert_eq!(session.state, RtpSessionState::Listening);
        assert_eq!(session.generation, 1);
        assert!(session.local_port.is_some());
        assert!(session.last_error.is_none());

        cancel.cancel();
    }

    #[tokio::test]
    async fn connect_rtp_receiver_bumps_generation_and_sets_remote_endpoint() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch2", None).unwrap();
        let mut req = receiver_request(media_key.clone());
        req.tcp_mode = Some(RtpTcpMode::Active);
        let session = orchestrator
            .open_rtp_receiver(req)
            .await
            .expect("open receiver");
        assert_eq!(session.state, RtpSessionState::Created);

        let session_id = session.session_id.clone();
        let connected = orchestrator
            .connect_rtp_receiver(RtpConnectRequest {
                session_id,
                remote_endpoint: "127.0.0.1:54321".to_string(),
                ssrc: Some(2000),
            })
            .await
            .expect("connect receiver");

        assert_eq!(connected.generation, 2);
        assert_eq!(
            connected.remote_endpoint,
            Some("127.0.0.1:54321".to_string())
        );

        cancel.cancel();
    }

    #[tokio::test]
    async fn set_session_state_bumps_generation() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch3", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key))
            .await
            .expect("open receiver");

        let updated = orchestrator
            .set_session_state(&session.session_id, RtpSessionState::Connected)
            .expect("set state");

        assert_eq!(updated.state, RtpSessionState::Connected);
        assert_eq!(updated.generation, 2);

        cancel.cancel();
    }

    #[tokio::test]
    async fn set_session_remote_endpoint_transitions_to_connected_and_bumps_generation() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch4", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key))
            .await
            .expect("open receiver");

        let addr: SocketAddr = "127.0.0.1:16666".parse().unwrap();
        let updated = orchestrator
            .set_session_remote_endpoint(&session.session_id, addr)
            .expect("set remote endpoint");

        assert_eq!(updated.remote_endpoint, Some("127.0.0.1:16666".to_string()));
        assert_eq!(updated.state, RtpSessionState::Connected);
        assert_eq!(updated.generation, 2);

        cancel.cancel();
    }

    #[tokio::test]
    async fn update_rtp_session_with_expected_generation_updates_snapshot() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch5", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key))
            .await
            .expect("open receiver");

        let updated = orchestrator
            .update_rtp_session(UpdateRtpRequest {
                session_id: session.session_id,
                expected_generation: 1,
                ssrc: Some(3000),
                payload_type: Some(97),
                pause_check: None,
            })
            .await
            .expect("update session");

        assert_eq!(updated.generation, 2);
        assert_eq!(updated.ssrc, Some(3000));
        assert_eq!(updated.payload_type, Some(97));
        assert!(updated.last_error.is_none());

        cancel.cancel();
    }

    #[tokio::test]
    async fn update_rtp_session_rejects_stale_generation() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch6", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key))
            .await
            .expect("open receiver");

        let err = orchestrator
            .update_rtp_session(UpdateRtpRequest {
                session_id: session.session_id,
                expected_generation: 99,
                ssrc: Some(4000),
                payload_type: None,
                pause_check: None,
            })
            .await
            .expect_err("stale generation should fail");

        assert_eq!(err.code, MediaErrorCode::Conflict);

        cancel.cancel();
    }

    #[tokio::test]
    async fn open_rtp_talk_upgrades_receiver_to_talk_and_bumps_generation() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch_talk", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key.clone()))
            .await
            .expect("open receiver");

        let addr: SocketAddr = "127.0.0.1:16666".parse().unwrap();
        let _ = orchestrator
            .set_session_remote_endpoint(&session.session_id, addr)
            .expect("set remote endpoint");

        let talk = orchestrator
            .open_rtp_talk(RtpSenderRequest {
                media_key,
                destination_endpoint: addr.to_string(),
                ssrc: Some(2000),
                payload_type: Some(8),
                codec_hint: Some("raw_audio".to_string()),
                mode: RtpSenderMode::Talk,
                transport_options: HashMap::new(),
            })
            .await
            .expect("open talk");

        assert_eq!(talk.kind, RtpSessionKind::Talk);
        assert_eq!(talk.state, RtpSessionState::Connected);
        assert_eq!(talk.generation, 3);

        cancel.cancel();
    }

    #[tokio::test]
    async fn stop_rtp_session_removes_session() {
        let (orchestrator, cancel) = make_orchestrator();

        let media_key = MediaKey::with_default_vhost("test", "ch_stop", None).unwrap();
        let session = orchestrator
            .open_rtp_receiver(receiver_request(media_key))
            .await
            .expect("open receiver");

        orchestrator
            .stop_rtp_session(&session.session_id)
            .await
            .expect("stop session");

        assert!(orchestrator.get_rtp_session(&session.session_id).is_err());

        cancel.cancel();
    }

    #[tokio::test]
    async fn list_rtp_sessions_filters_by_kind_and_state() {
        let (orchestrator, cancel) = make_orchestrator();

        let recv_key = MediaKey::with_default_vhost("list", "recv", None).unwrap();
        let send_key = MediaKey::with_default_vhost("list", "send", None).unwrap();

        let recv = orchestrator
            .open_rtp_receiver(receiver_request(recv_key))
            .await
            .expect("open receiver");
        let _sender = orchestrator
            .open_rtp_sender(RtpSenderRequest {
                media_key: send_key,
                destination_endpoint: "127.0.0.1:16666".to_string(),
                ssrc: Some(5000),
                payload_type: Some(96),
                codec_hint: Some("ps".to_string()),
                mode: RtpSenderMode::Active,
                transport_options: HashMap::new(),
            })
            .await
            .expect("open sender");

        let all = orchestrator
            .list_rtp_sessions(cheetah_sdk::media_api::command::RtpQuery {
                kind: None,
                state: None,
                session_id: None,
                media_key: None,
                page: 1,
                page_size: 10,
            })
            .expect("list all");
        assert_eq!(all.total, 2);

        let receivers = orchestrator
            .list_rtp_sessions(cheetah_sdk::media_api::command::RtpQuery {
                kind: Some(RtpSessionKind::Receiver),
                state: None,
                session_id: None,
                media_key: None,
                page: 1,
                page_size: 10,
            })
            .expect("list receivers");
        assert_eq!(receivers.total, 1);
        assert_eq!(receivers.items[0].session_id, recv.session_id);

        cancel.cancel();
    }

    #[tokio::test]
    async fn list_rtp_sessions_filters_by_session_id_and_media_key() {
        let (orchestrator, cancel) = make_orchestrator();

        let recv_key = MediaKey::with_default_vhost("list", "recv", None).unwrap();
        let send_key = MediaKey::with_default_vhost("list", "send", None).unwrap();

        let recv = orchestrator
            .open_rtp_receiver(receiver_request(recv_key.clone()))
            .await
            .expect("open receiver");
        let sender = orchestrator
            .open_rtp_sender(RtpSenderRequest {
                media_key: send_key.clone(),
                destination_endpoint: "127.0.0.1:16666".to_string(),
                ssrc: Some(5000),
                payload_type: Some(96),
                codec_hint: Some("ps".to_string()),
                mode: RtpSenderMode::Active,
                transport_options: HashMap::new(),
            })
            .await
            .expect("open sender");

        let by_session_id = orchestrator
            .list_rtp_sessions(cheetah_sdk::media_api::command::RtpQuery {
                kind: None,
                state: None,
                session_id: Some(recv.session_id.clone()),
                media_key: None,
                page: 1,
                page_size: 10,
            })
            .expect("list by session id");
        assert_eq!(by_session_id.total, 1);
        assert_eq!(by_session_id.items[0].session_id, recv.session_id);

        let by_media_key = orchestrator
            .list_rtp_sessions(cheetah_sdk::media_api::command::RtpQuery {
                kind: None,
                state: None,
                session_id: None,
                media_key: Some(send_key.clone()),
                page: 1,
                page_size: 10,
            })
            .expect("list by media key");
        assert_eq!(by_media_key.total, 1);
        assert_eq!(by_media_key.items[0].session_id, sender.session_id);

        cancel.cancel();
    }
}
