//! Tokio-based RTP/RTCP driver.
//!
//! 基于 Tokio 的 RTP/RTCP 驱动。

mod load_limiter;
mod port_lease;

pub use load_limiter::DriverLimits;

use bytes::{Bytes, BytesMut};
use load_limiter::LoadLimiter;
use port_lease::PortManager;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use cheetah_codec::MonoTime;
use cheetah_rtp_core::{
    rtcp::RtcpCompoundPacket, RtpClientSpec, RtpConnectionType, RtpCore, RtpCoreCommand,
    RtpCoreEvent, RtpCoreInput, RtpCoreOutput, RtpDatagram, RtpSendFrame, RtpServerSpec,
    RtpSourcePolicy, RtpTcpChunk,
};
use cheetah_runtime_api::{CancellationToken, RuntimeApi};

/// Inclusive UDP port range used for per-session socket allocation.
///
/// 用于每会话套接字分配的 UDP 端口范围（含边界）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    /// Create a new port range. Returns `None` if `start > end` or `start == 0`.
    ///
    /// 创建新的端口范围。`start > end` 或 `start == 0` 时返回 `None`。
    pub fn new(start: u16, end: u16) -> Option<Self> {
        if start == 0 || start > end {
            None
        } else {
            Some(Self { start, end })
        }
    }

    /// Number of ports in the range.
    pub fn count(&self) -> usize {
        (self.end as usize).saturating_sub(self.start as usize) + 1
    }
}

/// Configuration for the Tokio RTP driver.
///
/// Tokio RTP 驱动配置。
#[derive(Debug, Clone)]
pub struct RtpDriverConfig {
    pub listen_udp: SocketAddr,
    pub listen_tcp: SocketAddr,
    /// Optional separate RTCP listening UDP socket (`rtcpPort` config). When `None`, RTCP is
    /// expected to flow on the same UDP socket as RTP and gets routed by the core based on
    /// payload type.
    ///
    /// 可选的独立 RTCP UDP 监听端口。为 `None` 时，RTCP 与 RTP 共用同一 UDP 套接字，由 core 根据负载类型路由。
    pub listen_rtcp_udp: Option<SocketAddr>,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub session_idle_timeout_ms: u64,
    pub max_sessions: usize,
    /// Driver tick interval in milliseconds. The core receives a `Tick` input at this cadence.
    ///
    /// 驱动定时器间隔（毫秒）。core 按此频率接收 `Tick` 输入。
    pub tick_interval_ms: u64,
    /// Interval between RTCP sender/receiver reports in milliseconds.
    ///
    /// RTCP Sender/Receiver Report 生成间隔（毫秒）。
    pub rtcp_report_interval_ms: u64,
    /// Default TCP framing applied by the core when deframing inbound TCP RTP traffic. Defaults
    /// to `AutoDetect` so we accept both 2-byte length prefixes and 4-byte interleaved frames
    /// without explicit per-session negotiation.
    ///
    /// TCP 入站 RTP 分帧模式，默认 `AutoDetect`，支持 2 字节长度前缀与 4 字节交错帧自动识别。
    pub tcp_framing: cheetah_rtp_core::RtpTcpFraming,
    /// Hard upper bound on the dynamic `nMaxRtpLength` learner (defaults to 65 536 bytes).
    ///
    /// 动态 `nMaxRtpLength` 学习器的硬上限（默认 65536 字节）。
    pub max_rtp_len_cap: usize,
    /// Driver-wide resource limits (sessions, TCP connections, incoming byte rate).
    /// A limit of `0` disables it.
    pub limits: DriverLimits,
    /// Optional bounded UDP port pool for per-session sockets. When present and a
    /// request uses port `0`, the driver allocates from this range instead of the
    /// OS ephemeral pool. Explicit non-zero ports are bound directly.
    ///
    /// 可选的每会话 UDP 端口池。请求端口为 `0` 时从此范围分配；显式非零端口直接绑定。
    pub udp_port_pool: Option<PortRange>,
}

/// Default values for `RtpDriverConfig`.
///
/// `RtpDriverConfig` 默认值。
impl Default for RtpDriverConfig {
    fn default() -> Self {
        Self {
            listen_udp: SocketAddr::from(([127, 0, 0, 1], 20000)),
            listen_tcp: SocketAddr::from(([127, 0, 0, 1], 20000)),
            listen_rtcp_udp: None,
            write_queue_capacity: 256,
            read_buffer_size: 65536,
            session_idle_timeout_ms: 30000,
            max_sessions: 1024,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        }
    }
}

/// Error returned by the Tokio RTP driver when a bind or command cannot be completed.
///
/// Tokio RTP 驱动 bind 或命令失败时返回的错误。
#[derive(Debug)]
pub struct RtpDriverError {
    pub message: String,
}

impl std::fmt::Display for RtpDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RtpDriverError {}

/// Whether a bound per-session UDP socket may be shared with other sessions on the same address.
///
/// 绑定的每会话 UDP 套接字是否可与其他会话共享同一地址。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RtpSocketReuse {
    /// Bind a fresh, exclusive socket for this session.
    ///
    /// 为该会话绑定一个独占的新套接字。
    #[default]
    Exclusive,
    /// Reuse an existing per-session socket already bound to the same address.
    ///
    /// 复用已绑定到同一地址的现有每会话套接字。
    Reuse,
}

/// Acknowledgement payload returned by a successful `UpdateSession`.
///
/// 成功 `UpdateSession` 后返回的确认负载。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpSessionUpdateAck {
    pub generation: u64,
    pub ssrc: Option<u32>,
    pub payload_type: Option<u8>,
    pub pause_check: Option<bool>,
}

/// Commands sent to the RTP driver loop.
///
/// 发送给 RTP 驱动循环的命令。
pub enum RtpDriverCommand {
    CreateServer {
        spec: RtpServerSpec,
        bind_addr: Option<SocketAddr>,
        reuse: RtpSocketReuse,
        ack: Option<oneshot::Sender<Result<SocketAddr, String>>>,
    },
    CreateClient(RtpClientSpec),
    SendFrame(Box<RtpSendFrame>),
    StopSession(String),
    UpdateSession {
        session_key: String,
        expected_generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
        source_policy: Option<RtpSourcePolicy>,
        ack: Option<oneshot::Sender<Result<RtpSessionUpdateAck, String>>>,
    },
    PauseCheck {
        session_key: String,
        paused: bool,
    },
}

impl std::fmt::Debug for RtpDriverCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateServer {
                spec,
                bind_addr,
                reuse,
                ..
            } => f
                .debug_struct("CreateServer")
                .field("spec", spec)
                .field("reuse", reuse)
                .field("bind_addr", bind_addr)
                .finish(),
            Self::CreateClient(spec) => f.debug_tuple("CreateClient").field(spec).finish(),
            Self::SendFrame(frame) => f.debug_tuple("SendFrame").field(frame).finish(),
            Self::StopSession(key) => f.debug_tuple("StopSession").field(key).finish(),
            Self::UpdateSession {
                session_key,
                expected_generation,
                ssrc,
                payload_type,
                pause_check,
                source_policy,
                ..
            } => f
                .debug_struct("UpdateSession")
                .field("session_key", session_key)
                .field("expected_generation", expected_generation)
                .field("ssrc", ssrc)
                .field("payload_type", payload_type)
                .field("pause_check", pause_check)
                .field("source_policy", source_policy)
                .finish(),
            Self::PauseCheck {
                session_key,
                paused,
            } => f
                .debug_struct("PauseCheck")
                .field("session_key", session_key)
                .field("paused", paused)
                .finish(),
        }
    }
}

/// Handle to the running RTP driver.
///
/// 运行中 RTP 驱动的句柄。
pub struct RtpDriverHandle {
    cmd_tx: mpsc::Sender<RtpDriverCommand>,
    event_rx: Mutex<mpsc::Receiver<RtpCoreEvent>>,
}

/// `RtpDriverHandle` API.
///
/// `RtpDriverHandle` API。
impl RtpDriverHandle {
    /// Send a command to the driver loop.
    ///
    /// 向驱动循环发送命令。
    pub async fn send_command(&self, cmd: RtpDriverCommand) {
        let _ = self.cmd_tx.send(cmd).await;
    }

    /// Try to send a command without blocking.
    ///
    /// Returns `true` when the command was accepted by the driver queue. When the
    /// queue is full the command is dropped and `false` is returned so callers can
    /// apply their own backpressure / slow-peer isolation policy.
    ///
    /// 尝试非阻塞地向驱动循环发送命令。队列满时返回 false，调用方可自行决定反压策略。
    pub fn try_send_command(&self, cmd: RtpDriverCommand) -> bool {
        match self.cmd_tx.try_send(cmd) {
            Ok(()) => true,
            Err(e) => {
                // Drop the rejected command to avoid back-propagating pressure.
                drop(e);
                false
            }
        }
    }

    /// Bind a server socket for the given spec and wait for the driver to confirm
    /// the actual local address. `bind_addr` of `None` reuses the driver's default UDP socket.
    ///
    /// 绑定服务端套接字并等待驱动返回实际本地地址；`bind_addr` 为 `None` 时复用默认 UDP 套接字。
    pub async fn create_server(
        &self,
        spec: RtpServerSpec,
        bind_addr: Option<SocketAddr>,
        reuse: RtpSocketReuse,
    ) -> Result<SocketAddr, RtpDriverError> {
        let (tx, rx) = oneshot::channel();
        let cmd = RtpDriverCommand::CreateServer {
            spec,
            bind_addr,
            reuse,
            ack: Some(tx),
        };
        if self.cmd_tx.send(cmd).await.is_err() {
            return Err(RtpDriverError {
                message: "driver command channel closed".to_string(),
            });
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(addr))) => Ok(addr),
            Ok(Ok(Err(reason))) => Err(RtpDriverError { message: reason }),
            Ok(Err(_)) => Err(RtpDriverError {
                message: "driver dropped bind acknowledgement".to_string(),
            }),
            Err(_) => Err(RtpDriverError {
                message: "bind acknowledgement timed out".to_string(),
            }),
        }
    }

    /// Update a session and wait for the core to acknowledge the new generation.
    ///
    /// 更新会话并等待 core 返回新的 generation。
    pub async fn update_session(
        &self,
        session_key: String,
        expected_generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
    ) -> Result<RtpSessionUpdateAck, RtpDriverError> {
        self.update_session_with_source_policy(
            session_key,
            expected_generation,
            ssrc,
            payload_type,
            pause_check,
            None,
        )
        .await
    }

    /// Update a session, optionally changing the source-address binding policy.
    ///
    /// 更新会话，可一并修改源地址绑定策略。
    pub async fn update_session_with_source_policy(
        &self,
        session_key: String,
        expected_generation: u64,
        ssrc: Option<u32>,
        payload_type: Option<u8>,
        pause_check: Option<bool>,
        source_policy: Option<RtpSourcePolicy>,
    ) -> Result<RtpSessionUpdateAck, RtpDriverError> {
        let (tx, rx) = oneshot::channel();
        let cmd = RtpDriverCommand::UpdateSession {
            session_key,
            expected_generation,
            ssrc,
            payload_type,
            pause_check,
            source_policy,
            ack: Some(tx),
        };
        if self.cmd_tx.send(cmd).await.is_err() {
            return Err(RtpDriverError {
                message: "driver command channel closed".to_string(),
            });
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(Ok(ack))) => Ok(ack),
            Ok(Ok(Err(reason))) => Err(RtpDriverError { message: reason }),
            Ok(Err(_)) => Err(RtpDriverError {
                message: "driver dropped update acknowledgement".to_string(),
            }),
            Err(_) => Err(RtpDriverError {
                message: "update acknowledgement timed out".to_string(),
            }),
        }
    }

    /// Receive the next event from the driver loop.
    ///
    /// 从驱动循环接收下一个事件。
    pub async fn recv_event(&self) -> Option<RtpCoreEvent> {
        self.event_rx.lock().await.recv().await
    }

    /// Try to receive the next event without blocking.
    ///
    /// 尝试非阻塞地从驱动循环接收下一个事件。
    pub async fn try_recv_event(&self) -> Option<RtpCoreEvent> {
        self.event_rx.lock().await.try_recv().ok()
    }
}

/// Start the Tokio RTP driver using the default `TokioRuntime` and return a handle.
///
/// 使用默认 `TokioRuntime` 启动 Tokio RTP 驱动并返回句柄。
pub fn start_driver(config: RtpDriverConfig, cancel: CancellationToken) -> RtpDriverHandle {
    start_driver_with_runtime(
        config,
        cancel,
        Arc::new(cheetah_runtime_tokio::TokioRuntime::new()),
    )
}

/// Start the Tokio RTP driver with an injected `RuntimeApi` and return a handle.
///
/// This allows tests to drive time via a fake or paused `RuntimeApi` (e.g. `TokioRuntime`
/// under `tokio::time::pause`) instead of wall-clock time.
///
/// 使用注入的 `RuntimeApi` 启动 Tokio RTP 驱动并返回句柄。
/// 测试可以通过 fake 或 paused 的 `RuntimeApi`（如 `tokio::time::pause` 下的 `TokioRuntime`）控制时间。
pub fn start_driver_with_runtime(
    config: RtpDriverConfig,
    cancel: CancellationToken,
    runtime: Arc<dyn RuntimeApi>,
) -> RtpDriverHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let (event_tx, event_rx) = mpsc::channel(256);

    tokio::spawn(run_driver_loop(config, cmd_rx, event_tx, cancel, runtime));

    RtpDriverHandle {
        cmd_tx,
        event_rx: Mutex::new(event_rx),
    }
}

/// Spawn a UDP recv task that forwards datagrams into the core input channels.
///
/// When `rtcp_rx_tx` is `Some`, datagrams that look like RTCP are routed to the RTCP
/// channel; otherwise they are sent as RTP. `mux` enables RTCP/RTP mux detection on this
/// socket; when false the socket is assumed to be a dedicated RTCP socket and all
/// datagrams are forwarded as RTCP.
///
/// 生成 UDP 接收任务，将数据报转发到 core 输入通道。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_udp_reader(
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
    udp_rx_tx: mpsc::Sender<RtpDatagram>,
    rtcp_rx_tx: Option<mpsc::Sender<RtpDatagram>>,
    mux: bool,
    buf_size: usize,
    runtime: Arc<dyn RuntimeApi>,
    load_limiter: LoadLimiter,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; buf_size];
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                res = socket.recv_from(&mut buf) => {
                    match res {
                        Ok((len, src)) => {
                            if !load_limiter.try_consume_bytes(len) {
                                continue;
                            }
                            let received_at_ms = runtime.now().as_micros() / 1000;
                            let data = Bytes::copy_from_slice(&buf[..len]);

                            let datagram = RtpDatagram { source: src, data, received_at_ms };

                            if let Some(rtcp_tx) = rtcp_rx_tx.as_ref() {
                                if mux {
                                    // RTP/RTCP mux: RFC 5761 disambiguation first, then parse.
                                    // Only route to the RTCP path when the packet-type byte is
                                    // in an RTCP range and the compound packet parses cleanly.
                                    if looks_like_rtcp(&datagram.data)
                                        && RtcpCompoundPacket::parse(datagram.data.clone()).is_ok()
                                    {
                                        if rtcp_tx.send(datagram).await.is_err() {
                                            break;
                                        }
                                        continue;
                                    }
                                } else {
                                    // Dedicated RTCP socket.
                                    if rtcp_tx.send(datagram).await.is_err() {
                                        break;
                                    }
                                    continue;
                                }
                            }

                            if udp_rx_tx.send(datagram).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("UDP receive error: {e}");
                        }
                    }
                }
            }
        }
    });
}

/// Release any dedicated UDP socket associated with a stopped/closed session.
///
/// 释放与会话关联的专用 UDP 套接字。
async fn release_session_socket(
    key: &str,
    session_bind_addrs: &Mutex<HashMap<String, Option<SocketAddr>>>,
    port_manager: &PortManager,
) {
    if let Some(Some(addr)) = session_bind_addrs.lock().await.remove(key) {
        port_manager.release(addr).await;
    }
}

/// Return the per-session UDP socket for `session_key` if one was bound, otherwise the default
/// shared UDP socket.
///
/// 返回 `session_key` 绑定的每会话 UDP 套接字；若不存在则返回默认共享 UDP 套接字。
async fn resolve_udp_socket(
    session_key: &str,
    session_bind_addrs: &Mutex<HashMap<String, Option<SocketAddr>>>,
    port_manager: &PortManager,
    default_socket: &Arc<UdpSocket>,
) -> Arc<UdpSocket> {
    let maybe_addr = session_bind_addrs
        .lock()
        .await
        .get(session_key)
        .copied()
        .flatten();
    if let Some(addr) = maybe_addr {
        if let Some(socket) = port_manager.get_socket(addr).await {
            return socket;
        }
    }
    default_socket.clone()
}

/// Derive the RTCP destination from an RTP peer address.
///
/// RTP conventionally uses an even port and RTCP the next odd port. When the supplied
/// address already looks like an RTCP port (odd) we leave it unchanged; otherwise we
/// map the even RTP port to `port + 1`.
///
/// 由 RTP 对端地址推导 RTCP 目的地址。
fn resolve_rtcp_destination(rtp_dest: SocketAddr) -> SocketAddr {
    let mut dest = rtp_dest;
    if dest.port().is_multiple_of(2) {
        dest.set_port(dest.port().saturating_add(1));
    }
    dest
}

/// Return the current wall-clock time in milliseconds from the supplied runtime.
fn now_ms(runtime: &Arc<dyn RuntimeApi>) -> u64 {
    runtime.now().as_micros() / 1000
}

/// RFC 5761-style disambiguation for RTP/RTCP-muxed UDP sockets.
///
/// Returns true when the packet looks like an RTCP compound packet rather than RTP:
/// RTP version is 2 and the packet-type byte falls in the RTCP ranges (64-95, 192-223).
///
/// RFC 5761 风格的 RTP/RTCP 复用端口判别。
fn looks_like_rtcp(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let version = data[0] >> 6;
    let pt = data[1];
    version == 2 && (matches!(pt, 64..=95) || matches!(pt, 192..=223))
}

/// Main Tokio driver loop: bind sockets, spawn I/O tasks, and dispatch core I/O.
///
/// 主 Tokio 驱动循环：绑定套接字、生成 I/O 任务并调度 core 的输入/输出。
async fn run_driver_loop(
    config: RtpDriverConfig,
    cmd_rx: mpsc::Receiver<RtpDriverCommand>,
    event_tx: mpsc::Sender<RtpCoreEvent>,
    cancel: CancellationToken,
    runtime: Arc<dyn RuntimeApi>,
) {
    let udp_socket = match UdpSocket::bind(config.listen_udp).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!("RTP UDP Driver bind failed on {}: {e}", config.listen_udp);
            return;
        }
    };

    let tcp_listener = match TcpListener::bind(config.listen_tcp).await {
        Ok(l) => {
            info!("RTP TCP Driver listening on {}", config.listen_tcp);
            Some(Arc::new(l))
        }
        Err(e) => {
            error!("RTP TCP Driver bind failed on {}: {e}", config.listen_tcp);
            None
        }
    };

    let (cmd_tx, mut cmd_rx_internal) = mpsc::channel::<RtpDriverCommand>(256);
    {
        let cmd_tx_inner = cmd_tx.clone();
        let cancel_inner = cancel.clone();
        tokio::spawn(async move {
            let mut cmd_rx = cmd_rx;
            loop {
                tokio::select! {
                    _ = cancel_inner.cancelled() => break,
                    cmd = cmd_rx.recv() => {
                        if let Some(c) = cmd {
                            if cmd_tx_inner.send(c).await.is_err() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        });
    }

    let mut core = RtpCore::new(config.max_sessions, config.session_idle_timeout_ms);
    core.set_tcp_framing(config.tcp_framing);
    core.set_max_rtp_len_cap(config.max_rtp_len_cap);
    core.set_rtcp_report_interval_ms(config.rtcp_report_interval_ms);
    let wall_clock_offset_ms = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64)
        .saturating_sub(runtime.now().as_micros() / 1000);
    core.set_wall_clock_offset_ms(wall_clock_offset_ms);

    let tick_interval_us = config.tick_interval_ms.max(1).saturating_mul(1000);
    let mut next_tick = runtime.now().as_micros().saturating_add(tick_interval_us);
    let mut tick_timer = runtime.sleep_until(MonoTime::from_micros(next_tick));

    // Optional separate RTCP UDP socket. When configured, RTCP datagrams arriving on this socket
    // are dispatched into the core via `RtpCoreInput::RtcpPacket` so that RR-timeout sender
    // lifecycle can react to peer feedback.
    let rtcp_socket = match config.listen_rtcp_udp {
        Some(addr) => match UdpSocket::bind(addr).await {
            Ok(s) => {
                info!("RTP RTCP UDP listening on {}", addr);
                Some(Arc::new(s))
            }
            Err(e) => {
                error!("RTP RTCP UDP bind failed on {}: {e}", addr);
                None
            }
        },
        None => None,
    };

    // Channels for async socket read streams to multiplex into the main thread
    let (udp_rx_tx, mut udp_rx_rx) = mpsc::channel::<RtpDatagram>(256);
    let (tcp_rx_tx, mut tcp_rx_rx) = mpsc::channel::<RtpCoreInput>(256);
    let (rtcp_rx_tx, mut rtcp_rx_rx) = mpsc::channel::<RtpDatagram>(64);

    // Active TCP connection writers: conn_id -> Writer Channel
    let tcp_writers: Arc<Mutex<HashMap<u64, mpsc::Sender<Bytes>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_conn_id = Arc::new(Mutex::new(1u64));
    let load_limiter = LoadLimiter::new(runtime.clone(), config.limits);

    let rtcp_mux = config.listen_rtcp_udp.is_none();

    // Per-session UDP sockets bound to explicit addresses (e.g. GB28181 port allocations).
    // Each socket has a cancellation token and a session refcount so it is closed when the
    // last session using it stops.
    let port_manager = PortManager::new(
        udp_rx_tx.clone(),
        if rtcp_mux {
            Some(rtcp_rx_tx.clone())
        } else {
            None
        },
        rtcp_mux,
        config.read_buffer_size,
        runtime.clone(),
        cancel.clone(),
        load_limiter.clone(),
        config.udp_port_pool,
    );
    let session_bind_addrs: Arc<Mutex<HashMap<String, Option<SocketAddr>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Per-session cancellation tokens. Stopping or closing a session cancels only that
    // session's outbound tasks without tearing down the whole driver.
    let session_cancels: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Fallback token shared by any egress spawn whose session token is missing. It is a
    // child of the driver-wide cancel so it only fires during driver shutdown, preserving
    // the previous fail-open behavior for untracked keys.
    let fallback = cancel.child_token();

    // Spawn default UDP recv task.
    let default_udp_addr = udp_socket.local_addr().unwrap_or(config.listen_udp);
    spawn_udp_reader(
        udp_socket.clone(),
        cancel.clone(),
        udp_rx_tx.clone(),
        if rtcp_mux {
            Some(rtcp_rx_tx.clone())
        } else {
            None
        },
        rtcp_mux,
        config.read_buffer_size,
        runtime.clone(),
        load_limiter.clone(),
    );

    // Spawn dedicated RTCP UDP reader if configured.
    if let Some(rtcp_socket) = rtcp_socket.clone() {
        spawn_udp_reader(
            rtcp_socket,
            cancel.clone(),
            udp_rx_tx.clone(),
            Some(rtcp_rx_tx.clone()),
            false,
            config.read_buffer_size,
            runtime.clone(),
            load_limiter.clone(),
        );
    }
    drop(rtcp_rx_tx);

    // Spawn TCP accept task
    if let Some(tcp_listener) = tcp_listener {
        let cancel = cancel.clone();
        let tcp_writers = tcp_writers.clone();
        let next_conn_id = next_conn_id.clone();
        let tcp_rx_tx = tcp_rx_tx.clone();
        let buf_size = config.read_buffer_size;
        let wq_cap = config.write_queue_capacity;
        let runtime = runtime.clone();
        let load_limiter = load_limiter.clone();

        tokio::spawn(async move {
            let load_limiter = load_limiter;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    res = tcp_listener.accept() => {
                        match res {
                            Ok((stream, addr)) => {
                                if !load_limiter.try_new_tcp_connection() {
                                    warn!("RTP TCP connection limit reached; dropping peer {addr}");
                                    drop(stream);
                                    continue;
                                }
                                let _tcp_guard = load_limiter.tcp_connection_guard();
                                debug!("RTP TCP client connected from {}", addr);
                                let conn_id = {
                                    let mut id_guard = next_conn_id.lock().await;
                                    let id = *id_guard;
                                    *id_guard += 1;
                                    id
                                };

                                let (reader, mut writer) = tokio::io::split(stream);
                                let (writer_tx, mut writer_rx) = mpsc::channel::<Bytes>(wq_cap);
                                tcp_writers.lock().await.insert(conn_id, writer_tx);

                                // Spawn TCP Reader task
                                {
                                    let tcp_rx_tx = tcp_rx_tx.clone();
                                    let cancel = cancel.child_token();
                                    let runtime = runtime.clone();
                                    let load_limiter = load_limiter.clone();
                                    tokio::spawn(async move {
                                        let _tcp_guard = _tcp_guard;
                                        let mut reader = reader;
                                        let mut buf = vec![0u8; buf_size];
                                        let mut remaining = BytesMut::new();
                                        let mut is_ehome = false;
                                        let mut checked_ehome = false;
                                        loop {
                                            tokio::select! {
                                                _ = cancel.cancelled() => break,
                                                res = reader.read(&mut buf) => {
                                                    match res {
                                                        Ok(0) => {
                                                            // Peer half-closed the write side; notify the core so it can close
                                                            // any sessions bound to this connection while the write side may still
                                                            // be draining outbound frames.
                                                            let now_ms = now_ms(&runtime);
                                                            let _ = tcp_rx_tx.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                            break;
                                                        }
                                                        Ok(n) => {
                                                            if !load_limiter.try_consume_bytes(n) {
                                                                // Byte-rate budget exceeded. TCP is a byte stream, so dropping
                                                                // bytes mid-stream would corrupt framing; tear down the connection.
                                                                let now_ms = now_ms(&runtime);
                                                                let _ = tcp_rx_tx.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                                break;
                                                            }
                                                            let received_at_ms = now_ms(&runtime);
                                                            remaining.extend_from_slice(&buf[..n]);

                                                            if !checked_ehome {
                                                                if remaining.len() >= 3 {
                                                                    // Sticky Ehome detection: only the Ehome2 256-byte prefix
                                                                    // (0x01 0x00 0x01/0x02 ...) signals an Ehome stream. The
                                                                    // historical 0x00 0x00 heuristic has been removed because
                                                                    // it false-positives on RTP-over-TCP frames whose length
                                                                    // high byte is zero (small audio packets).
                                                                    if remaining[0] == 0x01
                                                                        && remaining[1] == 0x00
                                                                        && (remaining[2] == 0x01 || remaining[2] == 0x02)
                                                                    {
                                                                        is_ehome = true;
                                                                    }
                                                                    checked_ehome = true;
                                                                } else {
                                                                    continue;
                                                                }
                                                            }

                                                            if is_ehome {
                                                                loop {
                                                                    // Check for Ehome2 256-byte header
                                                                    if remaining.len() >= 256 && remaining[0] == 0x01 && remaining[1] == 0x00 && (remaining[2] == 0x01 || remaining[2] == 0x02) {
                                                                        let data = remaining.split_to(256).freeze();
                                                                        if tcp_rx_tx.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await.is_err() {
                                                                            break;
                                                                        }
                                                                        continue;
                                                                    }

                                                                    if remaining.len() >= 4 {
                                                                        let len = u16::from_be_bytes([remaining[2], remaining[3]]) as usize;
                                                                        if remaining.len() >= 4 + len {
                                                                            let data = remaining.split_to(4 + len).freeze();
                                                                            if tcp_rx_tx.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await.is_err() {
                                                                                break;
                                                                            }
                                                                        } else {
                                                                            break;
                                                                        }
                                                                    } else {
                                                                        break;
                                                                    }
                                                                }
                                                            } else {
                                                                // Auto-detect 2-byte / 4-byte interleaved framing per chunk. We send each
                                                                // complete frame to the core which then deframes it via its configured
                                                                // `RtpTcpFraming`. Picking the right size here keeps the chunk boundary
                                                                // aligned to a single RTP packet.
                                                                while !remaining.is_empty() {
                                                                    if remaining[0] == b'$' {
                                                                        if remaining.len() < 4 {
                                                                            break;
                                                                        }
                                                                        let len = u16::from_be_bytes([remaining[2], remaining[3]]) as usize;
                                                                        if remaining.len() < 4 + len {
                                                                            break;
                                                                        }
                                                                        let data = remaining.split_to(4 + len).freeze();
                                                                        if tcp_rx_tx.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await.is_err() {
                                                                            break;
                                                                        }
                                                                    } else if remaining.len() >= 2 {
                                                                        let len = u16::from_be_bytes([remaining[0], remaining[1]]) as usize;
                                                                        if remaining.len() < 2 + len {
                                                                            break;
                                                                        }
                                                                        let data = remaining.split_to(2 + len).freeze();
                                                                        if tcp_rx_tx.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await.is_err() {
                                                                            break;
                                                                        }
                                                                    } else {
                                                                        break;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        Err(_) => {
                                                            let now_ms = now_ms(&runtime);
                                                            let _ = tcp_rx_tx.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    });
                                }

                                // Spawn TCP Writer task
                                {
                                    let tcp_writers = tcp_writers.clone();
                                    let cancel = cancel.child_token();
                                    tokio::spawn(async move {
                                        loop {
                                            tokio::select! {
                                                _ = cancel.cancelled() => break,
                                                msg = writer_rx.recv() => {
                                                    match msg {
                                                        Some(data) => {
                                                            if writer.write_all(&data).await.is_err() {
                                                                break;
                                                            }
                                                        }
                                                        None => break,
                                                    }
                                                }
                                            }
                                        }
                                        tcp_writers.lock().await.remove(&conn_id);
                                    });
                                }
                            }
                            Err(e) => {
                                warn!("TCP accept error: {e}");
                            }
                        }
                    }
                }
            }
        });
    }

    loop {
        let mut inputs = Vec::new();
        let mut pending_update_ack: Option<(
            String,
            oneshot::Sender<Result<RtpSessionUpdateAck, String>>,
        )> = None;

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tick_timer.wait() => {
                let now = runtime.now();
                let now_ms = now.as_micros() / 1000;
                inputs.push(RtpCoreInput::Tick { now_ms });
                next_tick = now.as_micros().saturating_add(tick_interval_us);
                tick_timer = runtime.sleep_until(MonoTime::from_micros(next_tick));
            }
            Some(cmd) = cmd_rx_internal.recv() => {
                match cmd {
                    RtpDriverCommand::CreateServer {
                        spec,
                        bind_addr,
                        reuse,
                        ack,
                    } => {
                        let key = spec.session_key.clone();
                        let explicit_bind = bind_addr.is_some();

                        // Avoid binding a second socket for a session key that already has a
                        // dedicated UDP socket. Reusing the key would leave the new socket leased
                        // without a session to release it.
                        if explicit_bind && session_bind_addrs.lock().await.contains_key(&key) {
                            if let Some(ack) = ack {
                                let _ = ack.send(Err(format!(
                                    "session {key} already has a bound socket"
                                )));
                            }
                            continue;
                        }

                        if !load_limiter.allow_new_session() {
                            if let Some(ack) = ack {
                                let _ = ack.send(Err(
                                    "RTP session limit reached".to_string()
                                ));
                            }
                            continue;
                        }

                        let actual_addr = match bind_addr {
                            None => default_udp_addr,
                            Some(addr) => {
                                match port_manager.acquire(addr, reuse == RtpSocketReuse::Reuse).await {
                                    Ok(lease) => {
                                        let actual = lease.addr();
                                        lease.commit();
                                        actual
                                    }
                                    Err(reason) => {
                                        if let Some(ack) = ack {
                                            let _ = ack.send(Err(reason));
                                        }
                                        continue;
                                    }
                                }
                            }
                        };
                        if let Some(ack) = ack {
                            let _ = ack.send(Ok(actual_addr));
                        }
                        session_bind_addrs
                            .lock()
                            .await
                            .insert(key.clone(), if explicit_bind { Some(actual_addr) } else { None });
                        session_cancels
                            .lock()
                            .await
                            .entry(key)
                            .or_insert_with(|| cancel.child_token());
                        inputs.push(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));
                    }
                    RtpDriverCommand::CreateClient(spec) => {
                        // If it's a TCP client connect, we need to spin up the connection first
                        if spec.tcp_conn_id.is_none() && spec.connection_type == Some(RtpConnectionType::TcpActive) {
                            // Active TCP Client connect
                            if !load_limiter.allow_new_session() {
                                warn!("RTP session limit reached; dropping active TCP connect to {}", spec.destination);
                                continue;
                            }
                            let dest = spec.destination;
                            let mut spec_clone = spec.clone();
                            let tcp_writers_clone = tcp_writers.clone();
                            let next_conn_id_clone = next_conn_id.clone();
                            let tcp_rx_tx_clone = tcp_rx_tx.clone();
                            let cmd_tx_clone = cmd_tx.clone();
                            let cancel_clone = cancel.clone();
                            let runtime = runtime.clone();
                            let load_limiter = load_limiter.clone();
                                                tokio::spawn(async move {
                                match TcpStream::connect(dest).await {
                                    Ok(stream) => {
                                        let conn_id = {
                                            let mut id_guard = next_conn_id_clone.lock().await;
                                            let id = *id_guard;
                                            *id_guard += 1;
                                            id
                                        };

                                        // Register the connection session in the state machine
                                        spec_clone.tcp_conn_id = Some(conn_id);
                                        let _ = cmd_tx_clone.send(RtpDriverCommand::CreateClient(spec_clone)).await;

                                        let (reader, writer) = tokio::io::split(stream);

                                        // Register the writer before spawning the reader so any
                                        // outbound data produced by the core (RTCP feedback, etc.)
                                        // can be sent as soon as the connection is live.
                                        let (writer_tx, mut writer_rx) =
                                            mpsc::channel::<Bytes>(config.write_queue_capacity);
                                        tcp_writers_clone.lock().await.insert(conn_id, writer_tx);

                                        // Spawn TCP client reader
                                        let cancel_reader = cancel_clone.child_token();
                                        let runtime = runtime.clone();
                                        let load_limiter = load_limiter.clone();
                                        tokio::spawn(async move {
                                            let mut reader = reader;
                                            let mut buf = vec![0u8; config.read_buffer_size];
                                            let mut remaining = BytesMut::new();
                                            let mut is_ehome = false;
                                            let mut checked_ehome = false;
                                            loop {
                                                tokio::select! {
                                                    _ = cancel_reader.cancelled() => break,
                                                    res = reader.read(&mut buf) => {
                                                        match res {
                                                            Ok(0) => {
                                                                let now_ms = now_ms(&runtime);
                                                                let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                                break;
                                                            }
                                                            Ok(n) => {
                                                                if !load_limiter.try_consume_bytes(n) {
                                                                    let now_ms = now_ms(&runtime);
                                                                    let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                                    break;
                                                                }
                                                                let received_at_ms = now_ms(&runtime);
                                                                remaining.extend_from_slice(&buf[..n]);

                                                                if !checked_ehome {
                                                                    if remaining.len() >= 3 {
                                                                        // Sticky Ehome detection — see server-side note above.
                                                                        if remaining[0] == 0x01
                                                                            && remaining[1] == 0x00
                                                                            && (remaining[2] == 0x01 || remaining[2] == 0x02)
                                                                        {
                                                                            is_ehome = true;
                                                                        }
                                                                        checked_ehome = true;
                                                                    } else {
                                                                        continue;
                                                                    }
                                                                }

                                                                if is_ehome {
                                                                    loop {
                                                                        if remaining.len() >= 256 && remaining[0] == 0x01 && remaining[1] == 0x00 && (remaining[2] == 0x01 || remaining[2] == 0x02) {
                                                                            let data = remaining.split_to(256).freeze();
                                                                            let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await;
                                                                            continue;
                                                                        }
                                                                        if remaining.len() >= 4 {
                                                                            let len = u16::from_be_bytes([remaining[2], remaining[3]]) as usize;
                                                                            if remaining.len() >= 4 + len {
                                                                                let data = remaining.split_to(4 + len).freeze();
                                                                                let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await;
                                                                            } else {
                                                                                break;
                                                                            }
                                                                        } else {
                                                                            break;
                                                                        }
                                                                    }
                                                                } else {
                                                                    // Auto-detect 2-byte / 4-byte interleaved framing per chunk on the
                                                                    // active TCP client read path.
                                                                    while !remaining.is_empty() {
                                                                        if remaining[0] == b'$' {
                                                                            if remaining.len() < 4 {
                                                                                break;
                                                                            }
                                                                            let len = u16::from_be_bytes([remaining[2], remaining[3]]) as usize;
                                                                            if remaining.len() < 4 + len {
                                                                                break;
                                                                            }
                                                                            let data = remaining.split_to(4 + len).freeze();
                                                                            let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await;
                                                                        } else if remaining.len() >= 2 {
                                                                            let len = u16::from_be_bytes([remaining[0], remaining[1]]) as usize;
                                                                            if remaining.len() < 2 + len {
                                                                                break;
                                                                            }
                                                                            let data = remaining.split_to(2 + len).freeze();
                                                                            let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpBytes(RtpTcpChunk { conn_id, data, received_at_ms })).await;
                                                                        } else {
                                                                            break;
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            Err(_) => {
                                                                let now_ms = now_ms(&runtime);
                                                                let _ = tcp_rx_tx_clone.send(RtpCoreInput::TcpConnectionClosed { conn_id, received_at_ms: now_ms }).await;
                                                                break;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        });

                                        // Spawn TCP client writer so the core can send RTCP
                                        // feedback and, for send-capable sessions, RTP data back.
                                        let cancel_writer = cancel_clone.child_token();
                                        let tcp_writers_remove = tcp_writers_clone.clone();
                                        tokio::spawn(async move {
                                            let mut writer = writer;
                                            loop {
                                                tokio::select! {
                                                    _ = cancel_writer.cancelled() => break,
                                                    msg = writer_rx.recv() => {
                                                        match msg {
                                                            Some(data) => {
                                                                if writer.write_all(&data).await.is_err() {
                                                                    break;
                                                                }
                                                            }
                                                            None => break,
                                                        }
                                                    }
                                                }
                                            }
                                            tcp_writers_remove.lock().await.remove(&conn_id);
                                        });
                                    }
                                    Err(e) => {
                                        error!("Failed to connect TCP to client {dest}: {e}");
                                    }
                                }
                            });
                        } else {
                            session_cancels
                                .lock()
                                .await
                                .entry(spec.session_key.clone())
                                .or_insert_with(|| cancel.child_token());
                            inputs.push(RtpCoreInput::Command(RtpCoreCommand::CreateClient(spec)));
                        }
                    }
                    RtpDriverCommand::SendFrame(send_frame) => {
                        inputs.push(RtpCoreInput::Command(RtpCoreCommand::SendFrame(*send_frame)));
                    }
                    RtpDriverCommand::StopSession(key) => {
                        if let Some(token) = session_cancels.lock().await.remove(&key) {
                            token.cancel();
                        }
                        release_session_socket(&key, &session_bind_addrs, &port_manager).await;
                        inputs.push(RtpCoreInput::Command(RtpCoreCommand::StopSession(key)));
                    }
                    RtpDriverCommand::UpdateSession {
                        session_key,
                        expected_generation,
                        ssrc,
                        payload_type,
                        pause_check,
                        source_policy,
                        ack,
                    } => {
                        if let Some(ack) = ack {
                            pending_update_ack = Some((session_key.clone(), ack));
                        }
                        inputs.push(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
                            session_key,
                            expected_generation,
                            ssrc,
                            payload_type,
                            pause_check,
                            source_policy,
                        }));
                    }
                    RtpDriverCommand::PauseCheck { session_key, paused } => {
                        inputs.push(RtpCoreInput::Command(RtpCoreCommand::PauseCheck {
                            session_key,
                            paused,
                        }));
                    }
                }
            }
            Some(datagram) = udp_rx_rx.recv() => {
                inputs.push(RtpCoreInput::UdpPacket(datagram));
            }
            Some(input) = tcp_rx_rx.recv() => {
                inputs.push(input);
            }
            Some(rtcp) = rtcp_rx_rx.recv() => {
                inputs.push(RtpCoreInput::RtcpPacket(rtcp));
            }
        }

        for input in inputs {
            let outputs = core.handle_input(input);
            for output in outputs {
                match output {
                    RtpCoreOutput::SendUdp(udp_send) => {
                        let socket = resolve_udp_socket(
                            &udp_send.session_key,
                            &session_bind_addrs,
                            &port_manager,
                            &udp_socket,
                        )
                        .await;
                        let session_cancel = session_cancels
                            .lock()
                            .await
                            .get(&udp_send.session_key)
                            .cloned()
                            .unwrap_or_else(|| fallback.clone());
                        let core_input_tx = tcp_rx_tx.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = session_cancel.cancelled() => {}
                                result = socket.send_to(&udp_send.data, udp_send.destination) => {
                                    if let Err(e) = result {
                                        debug!("UDP send error for {}: {e}", udp_send.session_key);
                                        let _ = core_input_tx.send(RtpCoreInput::Command(
                                            RtpCoreCommand::ReportSendFailure {
                                                session_key: udp_send.session_key,
                                                reason: e.to_string(),
                                            }
                                        )).await;
                                    }
                                }
                            }
                        });
                    }
                    RtpCoreOutput::SendTcp(tcp_send) => {
                        let writers = tcp_writers.clone();
                        let session_cancel = session_cancels
                            .lock()
                            .await
                            .get(&tcp_send.session_key)
                            .cloned()
                            .unwrap_or_else(|| fallback.clone());
                        let core_input_tx = tcp_rx_tx.clone();
                        tokio::spawn(async move {
                            let map = writers.lock().await;
                            if let Some(tx) = map.get(&tcp_send.conn_id) {
                                let session_key = tcp_send.session_key.clone();
                                tokio::select! {
                                    _ = session_cancel.cancelled() => {}
                                    result = tx.send(tcp_send.data) => {
                                        if result.is_err() {
                                            debug!("TCP writer {} closed", tcp_send.conn_id);
                                            let _ = core_input_tx.send(RtpCoreInput::Command(
                                                RtpCoreCommand::ReportSendFailure {
                                                    session_key,
                                                    reason: "tcp writer closed".to_string(),
                                                }
                                            )).await;
                                        }
                                    }
                                }
                            }
                        });
                    }
                    RtpCoreOutput::SendRtcp(rtcp_send) => {
                        if let Some(conn_id) = rtcp_send.conn_id {
                            // TCP RTCP frames are RFC 4571 length-prefixed like RTP so the
                            // peer can delimit the compound packet on the byte stream.
                            let writers = tcp_writers.clone();
                            let session_key = rtcp_send.session_key.clone();
                            let session_cancel = session_cancels
                                .lock()
                                .await
                                .get(&session_key)
                                .cloned()
                                .unwrap_or_else(|| fallback.clone());
                            let data = cheetah_codec::encode_tcp_rtcp_frame(&rtcp_send.data);
                            let core_input_tx = tcp_rx_tx.clone();
                            tokio::spawn(async move {
                                let map = writers.lock().await;
                                if let Some(tx) = map.get(&conn_id) {
                                    tokio::select! {
                                        _ = session_cancel.cancelled() => {}
                                        result = tx.send(data) => {
                                            if result.is_err() {
                                                debug!("TCP RTCP writer {conn_id} closed");
                                                let _ = core_input_tx.send(RtpCoreInput::Command(
                                                    RtpCoreCommand::ReportSendFailure {
                                                        session_key,
                                                        reason: "tcp rtcp writer closed".to_string(),
                                                    }
                                                )).await;
                                            }
                                        }
                                    }
                                }
                            });
                        } else if let Some(rtcp_socket) = rtcp_socket.clone() {
                            // Dedicated RTCP socket: use the observed RTCP source address
                            // directly if known; otherwise derive the RTCP port from the
                            // RTP destination.
                            let dest = rtcp_send
                                .rtcp_destination
                                .unwrap_or_else(|| resolve_rtcp_destination(rtcp_send.destination));
                            let session_cancel = session_cancels
                                .lock()
                                .await
                                .get(&rtcp_send.session_key)
                                .cloned()
                                .unwrap_or_else(|| fallback.clone());
                            let core_input_tx = tcp_rx_tx.clone();
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = session_cancel.cancelled() => {}
                                    result = rtcp_socket.send_to(&rtcp_send.data, dest) => {
                                        if let Err(e) = result {
                                            debug!("RTCP send error: {e}");
                                            let _ = core_input_tx.send(RtpCoreInput::Command(
                                                RtpCoreCommand::ReportSendFailure {
                                                    session_key: rtcp_send.session_key,
                                                    reason: e.to_string(),
                                                }
                                            )).await;
                                        }
                                    }
                                }
                            });
                        } else {
                            // RTP/RTCP mux: reuse the same UDP socket (or per-session socket).
                            let socket = resolve_udp_socket(
                                &rtcp_send.session_key,
                                &session_bind_addrs,
                                &port_manager,
                                &udp_socket,
                            )
                            .await;
                            let dest = rtcp_send.rtcp_destination.unwrap_or(rtcp_send.destination);
                            let session_cancel = session_cancels
                                .lock()
                                .await
                                .get(&rtcp_send.session_key)
                                .cloned()
                                .unwrap_or_else(|| fallback.clone());
                            let core_input_tx = tcp_rx_tx.clone();
                            tokio::spawn(async move {
                                tokio::select! {
                                    _ = session_cancel.cancelled() => {}
                                    result = socket.send_to(&rtcp_send.data, dest) => {
                                        if let Err(e) = result {
                                            debug!("RTCP mux send error: {e}");
                                            let _ = core_input_tx.send(RtpCoreInput::Command(
                                                RtpCoreCommand::ReportSendFailure {
                                                    session_key: rtcp_send.session_key,
                                                    reason: e.to_string(),
                                                }
                                            )).await;
                                        }
                                    }
                                }
                            });
                        }
                    }
                    RtpCoreOutput::Event(ev) => {
                        if let RtpCoreEvent::SessionCreated { session_key, .. } = &ev {
                            load_limiter.session_created();
                            session_cancels
                                .lock()
                                .await
                                .entry(session_key.clone())
                                .or_insert_with(|| cancel.child_token());
                        }
                        if let RtpCoreEvent::SessionClosed { session_key, .. } = &ev {
                            if let Some(token) = session_cancels.lock().await.remove(session_key) {
                                token.cancel();
                            }
                        }
                        if let Some((pending_key, ack)) = pending_update_ack.take() {
                            match ev {
                                RtpCoreEvent::SessionUpdated {
                                    session_key,
                                    generation,
                                    ssrc,
                                    payload_type,
                                    pause_check,
                                    ..
                                } if session_key == pending_key => {
                                    let _ = ack.send(Ok(RtpSessionUpdateAck {
                                        generation,
                                        ssrc,
                                        payload_type,
                                        pause_check,
                                    }));
                                }
                                RtpCoreEvent::SessionUpdateFailed {
                                    session_key,
                                    reason,
                                } if session_key == pending_key => {
                                    let _ = ack.send(Err(reason));
                                }
                                other => {
                                    let _ = event_tx.send(other).await;
                                }
                            }
                        } else {
                            let _ = event_tx.send(ev).await;
                        }
                    }
                    RtpCoreOutput::Diagnostic(diag) => {
                        debug!("RTP Diagnostic: {}", diag);
                    }
                    RtpCoreOutput::CloseSession(key) => {
                        debug!("Closing RTP session key: {key}");
                        load_limiter.session_closed();
                        if let Some(token) = session_cancels.lock().await.remove(&key) {
                            token.cancel();
                        }
                        release_session_socket(&key, &session_bind_addrs, &port_manager).await;
                    }
                    RtpCoreOutput::CloseTcpConnection { conn_id } => {
                        debug!("Closing TCP connection: {conn_id}");
                        // Dropping the writer sender causes the per-connection writer task to
                        // exit, which closes the write half of the socket and frees the entry.
                        if let Some(writer_tx) = tcp_writers.lock().await.remove(&conn_id) {
                            drop(writer_tx);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{
        AVFrame, CodecId, FrameFormat, MediaKind, RtpHeader, RtpPacket, Timebase, TrackId,
    };
    use cheetah_rtp_core::{
        RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpSendFrame, RtpServerSpec,
        RtpSessionCloseReason, RtpTrackFilter, RtpTransportMode,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn test_rtp_driver_udp_and_tcp_ingress() {
        let cancel = CancellationToken::new();

        // 1. Choose dynamic port by binding to 127.0.0.1:0
        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        // --- UDP TEST ---
        // Send a UDP RTP packet
        let client_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc: 8888,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x01, 0x02, 0x03]),
        };
        client_socket
            .send_to(&rtp.encode(), udp_addr)
            .await
            .unwrap();

        // Wait for SessionCreated event
        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                ..
            } => {
                assert_eq!(session_key, "live/8888");
                assert_eq!(ssrc, 8888);
                assert_eq!(payload_mode, RtpPayloadMode::Ps);
            }
            _ => panic!("Expected SessionCreated event"),
        }

        // --- TCP TEST ---
        // Send a TCP RTP packet
        let mut tcp_stream = tokio::net::TcpStream::connect(tcp_addr).await.unwrap();
        let rtp_tcp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc: 7777,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x05, 0x06, 0x07]),
        };
        let framed = cheetah_codec::encode_tcp_rtp_frame(&rtp_tcp);
        tcp_stream.write_all(&framed).await.unwrap();

        // Wait for SessionCreated event
        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                ..
            } => {
                assert_eq!(session_key, "live/7777");
                assert_eq!(ssrc, 7777);
                assert_eq!(payload_mode, RtpPayloadMode::Ps);
            }
            _ => panic!("Expected SessionCreated event for TCP session"),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_rtp_driver_tcp_passive_half_close_closes_session() {
        let cancel = CancellationToken::new();

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        // We still need a UDP address for the driver, but it won't be used in this test.
        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        // Give the driver a moment to bind its TCP listener.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut tcp_stream = tokio::net::TcpStream::connect(tcp_addr).await.unwrap();
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc: 4444,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x05, 0x06, 0x07]),
        };
        let framed = cheetah_codec::encode_tcp_rtp_frame(&rtp);
        tcp_stream.write_all(&framed).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            RtpCoreEvent::SessionCreated { ssrc: 4444, .. }
        ));

        // Half-close the TCP connection from the client side.
        tcp_stream.shutdown().await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();
        match event {
            RtpCoreEvent::SessionClosed { reason, .. } => {
                assert_eq!(reason, RtpSessionCloseReason::ConnectionClosed);
            }
            _ => panic!("Expected SessionClosed after TCP half-close, got {event:?}"),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_create_server_binds_and_returns_actual_port() {
        let cancel = CancellationToken::new();

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        let config = RtpDriverConfig {
            listen_udp: "127.0.0.1:20000".parse().unwrap(),
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        // Ask the driver to bind an ephemeral UDP socket for this server session.
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let spec = RtpServerSpec {
            session_key: "live/port-ack".to_string(),
            ssrc: Some(4242),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        let actual_addr = handle
            .create_server(spec, Some(bind_addr), RtpSocketReuse::Exclusive)
            .await
            .expect("create_server should acknowledge the bound address");
        assert_ne!(
            actual_addr.port(),
            0,
            "ephemeral bind should return a real port"
        );

        // Send a packet to the returned port and confirm the pre-created session receives it.
        let client_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc: 4242,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x01, 0x02, 0x03]),
        };
        client_socket
            .send_to(&rtp.encode(), actual_addr)
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                ..
            } => {
                assert_eq!(session_key, "live/port-ack");
                assert_eq!(ssrc, 4242);
                assert_eq!(payload_mode, RtpPayloadMode::Ps);
            }
            _ => panic!("Expected SessionCreated event"),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_update_session_acknowledges_generation_and_payload_type() {
        let cancel = CancellationToken::new();

        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        let spec = RtpServerSpec {
            session_key: "ack/1".to_string(),
            ssrc: Some(1111),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        let addr = handle
            .create_server(
                spec,
                Some("127.0.0.1:0".parse().unwrap()),
                RtpSocketReuse::Exclusive,
            )
            .await
            .expect("create_server should bind");
        assert_ne!(addr.port(), 0);

        // Drain the SessionCreated event so the event channel does not fill.
        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, RtpCoreEvent::SessionCreated { .. }));

        let ack = handle
            .update_session("ack/1".to_string(), 1, None, Some(96), None)
            .await
            .expect("update should be acknowledged");
        assert_eq!(ack.generation, 2);
        assert_eq!(ack.ssrc, None);
        assert_eq!(ack.payload_type, Some(96));

        // Second update with the correct new generation advances again.
        let ack2 = handle
            .update_session("ack/1".to_string(), 2, Some(2222), None, None)
            .await
            .expect("second update should be acknowledged");
        assert_eq!(ack2.generation, 3);
        assert_eq!(ack2.ssrc, Some(2222));

        // A stale generation must be rejected by the core and returned as an error.
        let err = handle
            .update_session("ack/1".to_string(), 1, None, Some(97), None)
            .await;
        assert!(err.is_err(), "stale expected_generation should be rejected");

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_rtp_driver_tcp_active_connect_and_ingress() {
        let cancel = CancellationToken::new();

        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        let fake_server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = fake_server.local_addr().unwrap();

        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        let spec = RtpClientSpec {
            session_key: "active/5555".to_string(),
            destination: server_addr,
            ssrc: 5555,
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::TcpActive),
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        handle
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        // The driver must initiate the TCP connection towards our fake server.
        let (mut server_stream, _) =
            tokio::time::timeout(Duration::from_secs(5), fake_server.accept())
                .await
                .unwrap()
                .unwrap();

        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 100,
                ssrc: 5555,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x01, 0x02, 0x03]),
        };
        let framed = cheetah_codec::encode_tcp_rtp_frame(&rtp);
        server_stream.write_all(&framed).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap()
            .unwrap();

        match event {
            RtpCoreEvent::SessionCreated {
                session_key,
                ssrc,
                payload_mode,
                ..
            } => {
                assert_eq!(session_key, "active/5555");
                assert_eq!(ssrc, 5555);
                assert_eq!(payload_mode, RtpPayloadMode::Ps);
            }
            _ => panic!("Expected SessionCreated event for active TCP client"),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_rtp_driver_cancellation_releases_sockets() {
        let cancel = CancellationToken::new();

        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 10,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        let spec = RtpServerSpec {
            session_key: "release/1".to_string(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        let actual_addr = handle
            .create_server(
                spec,
                Some("127.0.0.1:0".parse().unwrap()),
                RtpSocketReuse::Exclusive,
            )
            .await
            .expect("create_server should bind");

        // Drain the SessionCreated event so the event channel is clean.
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap();

        cancel.cancel();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // After cancellation the driver must drop its sockets, allowing the OS to
        // release the ports for immediate reuse.
        assert!(
            tokio::net::UdpSocket::bind(actual_addr).await.is_ok(),
            "UDP socket should be released after cancellation"
        );
        assert!(
            tokio::net::TcpListener::bind(tcp_addr).await.is_ok(),
            "TCP listener should be released after cancellation"
        );
    }

    #[tokio::test]
    async fn test_rtp_driver_tcp_active_backpressure_does_not_block_command_path() {
        let cancel = CancellationToken::new();

        let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_addr = temp_udp.local_addr().unwrap();
        drop(temp_udp);

        let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_addr = temp_tcp.local_addr().unwrap();
        drop(temp_tcp);

        // Accept but never read, so the kernel / writer buffers fill up.
        let fake_server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = fake_server.local_addr().unwrap();

        // Use a very small write queue so the backpressure limit is reached quickly.
        let config = RtpDriverConfig {
            listen_udp: udp_addr,
            listen_tcp: tcp_addr,
            listen_rtcp_udp: None,
            write_queue_capacity: 2,
            read_buffer_size: 4096,
            session_idle_timeout_ms: 5000,
            max_sessions: 5,
            tick_interval_ms: 100,
            rtcp_report_interval_ms: 5000,
            tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
            max_rtp_len_cap: 65536,
            limits: DriverLimits::default(),
            udp_port_pool: None,
        };

        let handle = start_driver(config, cancel.clone());

        let spec = RtpClientSpec {
            session_key: "backpressure/1".to_string(),
            destination: server_addr,
            ssrc: 2000,
            payload_mode: RtpPayloadMode::Es,
            transport_mode: RtpTransportMode::SendOnly,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::TcpActive),
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        handle
            .send_command(RtpDriverCommand::CreateClient(spec))
            .await;

        let (_server_stream, _) =
            tokio::time::timeout(Duration::from_secs(5), fake_server.accept())
                .await
                .unwrap()
                .unwrap();

        // Drain SessionCreated so the channel is empty.
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
            .await
            .unwrap();

        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from(vec![0u8; 1024]),
        );

        for _ in 0..10 {
            let send = RtpSendFrame {
                session_key: "backpressure/1".to_string(),
                frame: frame.clone(),
            };
            handle
                .send_command(RtpDriverCommand::SendFrame(Box::new(send)))
                .await;
        }

        // Even though the TCP writer path is saturated, the driver loop must remain
        // responsive to new commands.
        let spec2 = RtpServerSpec {
            session_key: "backpressure/server".to_string(),
            ssrc: Some(3000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        };

        let actual_addr = tokio::time::timeout(
            Duration::from_millis(500),
            handle.create_server(
                spec2,
                Some("127.0.0.1:0".parse().unwrap()),
                RtpSocketReuse::Exclusive,
            ),
        )
        .await
        .expect("driver command path should not be blocked by a slow TCP consumer")
        .expect("create_server should bind");

        assert_ne!(actual_addr.port(), 0);

        cancel.cancel();
    }
}
