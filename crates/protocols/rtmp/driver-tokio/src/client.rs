use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::MonoTime;
use cheetah_rtmp_core::{
    CoreInput, CoreOutput, RtmpClientHandshake, RtmpClientState, RtmpCore, RtmpCoreCommand,
    RtmpEvent, RtmpMediaType, RtmpMessage, RtmpMessageDecoder, RtmpMessageStreamId, RtmpUrl,
    TimerId,
};
use cheetah_runtime_api::{
    AsyncTcpStream, CancellationToken, JoinHandle, RuntimeApi, TaskJoinError,
};
use tokio::sync::mpsc;
use tracing::warn;

const DEFAULT_FLASH_VER: &str = "FMLE/3.0 (compatible; FME/3.0)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Direction of an RTMP client connection.
///
/// RTMP 客户端连接方向。
pub enum RtmpClientMode {
    Play,
    Publish,
}

#[derive(Debug, Clone)]
/// Configuration for the RTMP client driver.
///
/// RTMP 客户端驱动配置。
pub struct RtmpClientDriverConfig {
    pub command_queue_capacity: usize,
    pub event_queue_capacity: usize,
    pub write_queue_capacity: usize,
    pub read_buffer_size: usize,
    pub ack_window_size: u32,
    pub chunk_size: u32,
}

impl Default for RtmpClientDriverConfig {
    fn default() -> Self {
        Self {
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
            write_queue_capacity: 256,
            read_buffer_size: 64 * 1024,
            ack_window_size: 5_000_000,
            chunk_size: 4096,
        }
    }
}

#[derive(Debug)]
/// Events emitted by the RTMP client driver to the caller.
///
/// RTMP 客户端驱动向调用者发出的事件。
pub enum ClientDriverEvent {
    Connected { peer: Option<SocketAddr> },
    Core { event: RtmpEvent },
    Closed { reason: String },
}

#[derive(Debug, Clone)]
/// Commands sent from the caller into the RTMP client driver.
///
/// 从调用者发送到 RTMP 客户端驱动的命令。
pub enum RtmpClientDriverCommand {
    Core(RtmpCoreCommand),
    CloseConnection,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Error returned when the client command channel is closed.
///
/// 客户端命令通道关闭时返回的错误。
pub enum ClientSendError {
    ChannelClosed,
}

#[derive(Clone)]
/// MPSC sender for commands to the client driver loop.
///
/// 向客户端驱动循环发送命令的 MPSC 发送端。
pub struct RtmpClientCommandSender {
    tx: mpsc::Sender<RtmpClientDriverCommand>,
}

/// `RtmpClientCommandSender` API: send core and lifecycle commands.
///
/// `RtmpClientCommandSender` API：发送 core 与生命周期命令。
impl RtmpClientCommandSender {
    pub async fn send(&self, command: RtmpClientDriverCommand) -> Result<(), ClientSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| ClientSendError::ChannelClosed)
    }

    pub async fn send_core(&self, command: RtmpCoreCommand) -> Result<(), ClientSendError> {
        self.send(RtmpClientDriverCommand::Core(command)).await
    }

    pub async fn close_connection(&self) -> Result<(), ClientSendError> {
        self.send(RtmpClientDriverCommand::CloseConnection).await
    }
}

/// Handle for the RTMP client: receive events and send commands.
///
/// RTMP 客户端句柄：接收事件并发送命令。
pub struct RtmpClientHandle {
    events_rx: mpsc::Receiver<ClientDriverEvent>,
    cmd_tx: RtmpClientCommandSender,
    cancel: CancellationToken,
    join: Box<dyn JoinHandle>,
}

/// `RtmpClientHandle` API: event reception, command send, and lifecycle.
///
/// `RtmpClientHandle` API：事件接收、命令发送与生命周期。
impl RtmpClientHandle {
    pub async fn recv_event(&mut self) -> Option<ClientDriverEvent> {
        self.events_rx.recv().await
    }

    pub async fn send_driver_command(
        &self,
        command: RtmpClientDriverCommand,
    ) -> Result<(), ClientSendError> {
        self.cmd_tx.send(command).await
    }

    pub fn core_command_sender(&self) -> RtmpClientCommandSender {
        self.cmd_tx.clone()
    }

    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}

#[derive(Debug, Clone, Copy)]
/// Timer event with generation to filter stale firings.
///
/// 带 generation 的定时器事件，用于过滤过期触发。
struct TimerFired {
    id: TimerId,
    generation: u64,
}

/// Mutable state used by `flush_outputs` for a client connection.
///
/// `flush_outputs` 为客户端连接使用的可变状态。
struct OutputState<'a> {
    event_tx: &'a mpsc::Sender<ClientDriverEvent>,
    write_tx: &'a mpsc::Sender<Bytes>,
    runtime_api: &'a Arc<dyn RuntimeApi>,
    timer_tx: &'a mpsc::Sender<TimerFired>,
    timers: &'a mut HashMap<TimerId, u64>,
    timer_generation_seed: &'a mut u64,
    automation: &'a mut ClientAutomation,
}

/// State machine that automates the RTMP handshake/transaction sequence.
///
/// 自动执行 RTMP 握手/事务序列的状态机。
struct ClientAutomation {
    mode: RtmpClientMode,
    url: RtmpUrl,
    next_transaction_id: i64,
    peer_ack_window_size: u32,
    total_bytes_received: u32,
    last_ack_sent: u32,
    chunk_size: u32,
    ack_window_size: u32,
}

/// `ClientAutomation` API: generate commands and react to events/bytes.
///
/// `ClientAutomation` API：生成命令并响应事件/字节。
impl ClientAutomation {
    /// Create an automation state machine for the given mode and URL.
    ///
    /// 为指定模式与 URL 创建自动化状态机。
    fn new(mode: RtmpClientMode, url: RtmpUrl, cfg: &RtmpClientDriverConfig) -> Self {
        Self {
            mode,
            url,
            next_transaction_id: 2,
            peer_ack_window_size: u32::MAX,
            total_bytes_received: 0,
            last_ack_sent: 0,
            chunk_size: cfg.chunk_size,
            ack_window_size: cfg.ack_window_size,
        }
    }

    /// Return the next command transaction id.
    ///
    /// 返回下一个命令事务 ID。
    fn take_transaction_id(&mut self) -> f64 {
        let id = self.next_transaction_id;
        self.next_transaction_id = self.next_transaction_id.saturating_add(1);
        id as f64
    }

    /// Generate the initial connect/publish/play commands for the session.
    ///
    /// 为会话生成初始 connect/publish/play 命令。
    fn initial_commands(&mut self) -> Vec<RtmpCoreCommand> {
        let tc_url = format!(
            "{}://{}:{}/{}",
            if self.url.tls { "rtmps" } else { "rtmp" },
            self.url.host,
            self.url.port,
            self.url.app
        );
        vec![
            RtmpCoreCommand::SetWindowAckSize {
                size: self.ack_window_size,
            },
            RtmpCoreCommand::SetPeerBandwidth {
                size: self.ack_window_size,
            },
            RtmpCoreCommand::SetChunkSize {
                size: self.chunk_size,
            },
            RtmpCoreCommand::ClientConnect {
                app: self.url.app.clone(),
                flash_ver: DEFAULT_FLASH_VER.to_string(),
                tc_url,
            },
        ]
    }

    /// Generate an acknowledgement command when bytes are read.
    ///
    /// 读取字节时生成确认命令。
    fn on_bytes_read(&mut self, n: usize) -> Option<RtmpCoreCommand> {
        self.total_bytes_received = self.total_bytes_received.wrapping_add(n as u32);
        let unacked = self.total_bytes_received.wrapping_sub(self.last_ack_sent);
        if unacked > self.peer_ack_window_size / 2 {
            self.last_ack_sent = self.total_bytes_received;
            Some(RtmpCoreCommand::SendAck {
                sequence_number: self.total_bytes_received,
            })
        } else {
            None
        }
    }

    /// React to a core event by returning the next command, if any.
    ///
    /// 响应 core 事件，若有则返回下一个命令。
    fn on_core_event(&mut self, event: &RtmpEvent) -> Option<RtmpCoreCommand> {
        match event {
            RtmpEvent::PeerAckWindowUpdated { size } => {
                self.peer_ack_window_size = *size;
                None
            }
            RtmpEvent::ClientStateChanged { state } => match state {
                RtmpClientState::Connected => Some(RtmpCoreCommand::ClientCreateStream {
                    transaction_id: self.take_transaction_id(),
                }),
                RtmpClientState::MediaStreamCreated => match self.mode {
                    RtmpClientMode::Publish => Some(RtmpCoreCommand::ClientPublish {
                        stream_id: RtmpMessageStreamId::MEDIA.get(),
                        transaction_id: self.take_transaction_id(),
                        stream_name: self.url.stream_name.clone(),
                    }),
                    RtmpClientMode::Play => Some(RtmpCoreCommand::ClientPlay {
                        stream_id: RtmpMessageStreamId::MEDIA.get(),
                        transaction_id: self.take_transaction_id(),
                        stream_name: self.url.stream_name.clone(),
                        start: -1.0,
                    }),
                },
                RtmpClientState::Publishing | RtmpClientState::Playing => None,
            },
            _ => None,
        }
    }
}

/// Start a TCP RTMP client and return a handle.
///
/// 启动 TCP RTMP 客户端并返回句柄。
pub fn start_client(
    runtime_api: Arc<dyn RuntimeApi>,
    url: RtmpUrl,
    mode: RtmpClientMode,
    config: RtmpClientDriverConfig,
    cancel: CancellationToken,
) -> io::Result<RtmpClientHandle> {
    let addr = resolve_url_addr(&url)?;
    let stream = runtime_api.connect_tcp(addr)?;
    let peer = stream.peer_addr().ok();

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtmpClientCommandSender { tx: cmd_tx.clone() };

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let runtime_api = runtime_api.clone();
        async move {
            let _ = event_tx.send(ClientDriverEvent::Connected { peer }).await;
            let reason = run_client_connection(ClientConnectionParams {
                stream,
                runtime_api: &runtime_api,
                mode,
                url,
                config,
                cancel: join_cancel.clone(),
                event_tx: &event_tx,
                cmd_rx: &mut cmd_rx,
            })
            .await;
            let _ = event_tx.send(ClientDriverEvent::Closed { reason }).await;
        }
    }));

    Ok(RtmpClientHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

pub(crate) struct ClientConnectionParams<'a> {
    pub stream: Box<dyn AsyncTcpStream>,
    pub runtime_api: &'a Arc<dyn RuntimeApi>,
    pub mode: RtmpClientMode,
    pub url: RtmpUrl,
    pub config: RtmpClientDriverConfig,
    pub cancel: CancellationToken,
    pub event_tx: &'a mpsc::Sender<ClientDriverEvent>,
    pub cmd_rx: &'a mut mpsc::Receiver<RtmpClientDriverCommand>,
}

/// Main client connection loop: read, drive automation, flush outputs.
///
/// 客户端连接主循环：读取、驱动自动化、刷新输出。
async fn run_client_connection(params: ClientConnectionParams<'_>) -> String {
    let ClientConnectionParams {
        mut stream,
        runtime_api,
        mode,
        url,
        config,
        cancel,
        event_tx,
        cmd_rx,
    } = params;
    let mut handshake = RtmpClientHandshake::new();
    let mut in_ready_state = false;

    let (write_tx, mut write_rx) = mpsc::channel::<Bytes>(config.write_queue_capacity.max(8));
    let (timer_tx, mut timer_rx) =
        mpsc::channel::<TimerFired>(config.command_queue_capacity.max(64));

    let mut decoder = RtmpMessageDecoder::default();
    let mut core = RtmpCore::new_client();
    let mut read_buf = vec![0u8; config.read_buffer_size.max(1024)];
    let mut timer_generation_seed = 1u64;
    let mut timers: HashMap<TimerId, u64> = HashMap::new();
    let mut automation = ClientAutomation::new(mode, url, &config);

    if !handshake.send_buf().is_empty() {
        let bytes = Bytes::copy_from_slice(handshake.send_buf());
        handshake.advance_send_buf(bytes.len());
        if write_tx.try_send(bytes).is_err() {
            return "write queue overflow before handshake".to_string();
        }
    }

    let reason = 'run: loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break 'run "cancelled".to_string();
            }
            maybe_cmd = cmd_rx.recv() => {
                let Some(cmd) = maybe_cmd else {
                    break 'run "command channel closed".to_string();
                };
                match cmd {
                    RtmpClientDriverCommand::Core(command) => {
                        if let Err(err) = apply_core_command(
                            command,
                            &mut core,
                            &mut OutputState {
                                event_tx,
                                write_tx: &write_tx,
                                runtime_api,
                                timer_tx: &timer_tx,
                                timers: &mut timers,
                                timer_generation_seed: &mut timer_generation_seed,
                                automation: &mut automation,
                            },
                        ).await {
                            break 'run err;
                        }
                    }
                    RtmpClientDriverCommand::CloseConnection => {
                        break 'run "closed by command".to_string();
                    }
                    RtmpClientDriverCommand::Shutdown => {
                        break 'run "shutdown by command".to_string();
                    }
                }
            }
            maybe_fired = timer_rx.recv() => {
                let Some(fired) = maybe_fired else {
                    break 'run "timer channel closed".to_string();
                };
                if !is_timer_active(&timers, fired) {
                    continue;
                }
                timers.remove(&fired.id);
                if let Err(err) = apply_core_input(
                    CoreInput::Timeout { id: fired.id },
                    &mut core,
                    &mut OutputState {
                        event_tx,
                        write_tx: &write_tx,
                        runtime_api,
                        timer_tx: &timer_tx,
                        timers: &mut timers,
                        timer_generation_seed: &mut timer_generation_seed,
                        automation: &mut automation,
                    },
                ).await {
                    break 'run err;
                }
            }
            maybe_write = write_rx.recv() => {
                let Some(bytes) = maybe_write else {
                    break 'run "write queue closed".to_string();
                };
                tokio::select! {
                    _ = cancel.cancelled() => {
                        break 'run "cancelled".to_string();
                    }
                    write_res = stream.write_all(&bytes) => {
                        if let Err(err) = write_res {
                            break 'run format!("write failed: {err}");
                        }
                    }
                }
            }
            read_res = stream.read(&mut read_buf) => {
                match read_res {
                    Ok(0) => {
                        break 'run "peer closed".to_string();
                    }
                    Ok(n) => {
                        let incoming = &read_buf[..n];
                        if !in_ready_state {
                            if let Err(err) = handshake.feed_recv_buf(incoming) {
                                break 'run format!("handshake failed: {err}");
                            }
                            if !handshake.send_buf().is_empty() {
                                let bytes = Bytes::copy_from_slice(handshake.send_buf());
                                handshake.advance_send_buf(bytes.len());
                                if write_tx.try_send(bytes).is_err() {
                                    break 'run "write queue overflow during handshake".to_string();
                                }
                            }
                            if handshake.is_recv_complete() {
                                in_ready_state = true;
                                let mut init_commands = automation.initial_commands();
                                for cmd in init_commands.drain(..) {
                                    if let Err(err) = apply_core_command(
                                        cmd,
                                        &mut core,
                                        &mut OutputState {
                                            event_tx,
                                            write_tx: &write_tx,
                                            runtime_api,
                                            timer_tx: &timer_tx,
                                            timers: &mut timers,
                                            timer_generation_seed: &mut timer_generation_seed,
                                            automation: &mut automation,
                                        },
                                    ).await {
                                        break 'run err;
                                    }
                                }
                                let remaining = handshake.take_recv_buf();
                                if !remaining.is_empty() {
                                    if let Err(err) = process_ready_bytes(
                                        &remaining,
                                        &mut decoder,
                                        &mut core,
                                        &mut OutputState {
                                            event_tx,
                                            write_tx: &write_tx,
                                            runtime_api,
                                            timer_tx: &timer_tx,
                                            timers: &mut timers,
                                            timer_generation_seed: &mut timer_generation_seed,
                                            automation: &mut automation,
                                        },
                                    ).await {
                                        break 'run err;
                                    }
                                }
                            }
                        } else if let Err(err) = process_ready_bytes(
                            incoming,
                            &mut decoder,
                            &mut core,
                            &mut OutputState {
                                event_tx,
                                write_tx: &write_tx,
                                runtime_api,
                                timer_tx: &timer_tx,
                                timers: &mut timers,
                                timer_generation_seed: &mut timer_generation_seed,
                                automation: &mut automation,
                            },
                        ).await {
                            break 'run err;
                        }
                    }
                    Err(err) => {
                        break 'run format!("read failed: {err}");
                    }
                }
            }
        }
    };

    let _ = stream.shutdown().await;
    reason
}

/// Push accumulated bytes into the core and flush any produced outputs.
///
/// 将累积字节推入 core 并刷新所有产生的输出。
async fn process_ready_bytes(
    incoming: &[u8],
    decoder: &mut RtmpMessageDecoder,
    core: &mut RtmpCore,
    output_state: &mut OutputState<'_>,
) -> Result<(), String> {
    if let Some(ack_cmd) = output_state.automation.on_bytes_read(incoming.len()) {
        apply_core_command(ack_cmd, core, output_state).await?;
    }

    decoder.feed_buf(incoming);
    while let Some(message) = decoder
        .decode()
        .map_err(|err| format!("message decode failed: {err}"))?
    {
        let command = map_message_to_client_command(message)?;
        apply_core_command(command, core, output_state).await?;
    }
    Ok(())
}

/// Convert an inbound message into a core command for the client.
///
/// 将入站消息转换为客户端的 core 命令。
fn map_message_to_client_command(message: RtmpMessage) -> Result<RtmpCoreCommand, String> {
    match message {
        RtmpMessage::Command {
            header,
            name,
            transaction_id,
            object,
            args,
            ..
        } => Ok(RtmpCoreCommand::ClientHandleWireCommand {
            message_stream_id: header.stream_id.get(),
            name,
            transaction_id,
            object,
            args,
        }),
        RtmpMessage::Audio {
            header,
            frame: _,
            payload,
        } => Ok(RtmpCoreCommand::ClientObserveMediaData {
            stream_id: header.stream_id.get(),
            timestamp_ms: header.timestamp.as_millis(),
            media_type: RtmpMediaType::Audio,
            payload,
        }),
        RtmpMessage::Video {
            header,
            frame: _,
            payload,
        } => Ok(RtmpCoreCommand::ClientObserveMediaData {
            stream_id: header.stream_id.get(),
            timestamp_ms: header.timestamp.as_millis(),
            media_type: RtmpMediaType::Video,
            payload,
        }),
        RtmpMessage::WinAckSize { size, .. } => {
            Ok(RtmpCoreCommand::ClientObserveWinAckSize { size })
        }
        RtmpMessage::Ack {
            sequence_number, ..
        } => Ok(RtmpCoreCommand::ClientObserveAck { sequence_number }),
        RtmpMessage::SetPeerBandwidth { size, .. } => {
            Ok(RtmpCoreCommand::ClientHandleSetPeerBandwidth {
                size,
                response_window_size: 5_000_000,
            })
        }
        RtmpMessage::UserControl { event, .. } => {
            Ok(RtmpCoreCommand::ClientHandleUserControl { event })
        }
        RtmpMessage::SetChunkSize { .. } => {
            Ok(RtmpCoreCommand::ClientHandleUnhandledMessage { message })
        }
        message => Ok(RtmpCoreCommand::ClientHandleUnhandledMessage { message }),
    }
}

/// Push bytes into the core and process the resulting outputs.
///
/// 将字节推入 core 并处理结果输出。
async fn apply_core_input(
    input: CoreInput,
    core: &mut RtmpCore,
    output_state: &mut OutputState<'_>,
) -> Result<(), String> {
    let outputs = core
        .handle_input(input)
        .map_err(|err| format!("core failed: {err}"))?;
    flush_outputs(outputs, core, output_state).await
}

/// Push a command into the core and process the resulting outputs.
///
/// 将命令推入 core 并处理结果输出。
async fn apply_core_command(
    command: RtmpCoreCommand,
    core: &mut RtmpCore,
    output_state: &mut OutputState<'_>,
) -> Result<(), String> {
    apply_core_input(CoreInput::Command(command), core, output_state).await
}

/// Dispatch core outputs to the write queue, events, and timers.
///
/// 将 core 输出分发到写入队列、事件和定时器。
async fn flush_outputs(
    outputs: Vec<CoreOutput>,
    core: &mut RtmpCore,
    output_state: &mut OutputState<'_>,
) -> Result<(), String> {
    let mut pending_outputs: VecDeque<Vec<CoreOutput>> = VecDeque::new();
    let mut follow_up: VecDeque<RtmpCoreCommand> = VecDeque::new();
    pending_outputs.push_back(outputs);

    while let Some(outputs) = pending_outputs.pop_front() {
        for output in outputs {
            match output {
                CoreOutput::Write(bytes) => {
                    output_state
                        .write_tx
                        .try_send(bytes)
                        .map_err(|_| "write queue overflow".to_string())?;
                }
                CoreOutput::Event(event) => {
                    if let Some(next) = output_state.automation.on_core_event(&event) {
                        follow_up.push_back(next);
                    }
                    output_state
                        .event_tx
                        .send(ClientDriverEvent::Core { event })
                        .await
                        .map_err(|_| "event channel closed".to_string())?;
                }
                CoreOutput::SetTimer { id, at_micros } => {
                    let generation = next_timer_generation(output_state.timer_generation_seed);
                    output_state.timers.insert(id, generation);
                    schedule_timer(
                        output_state.runtime_api.clone(),
                        output_state.timer_tx.clone(),
                        id,
                        generation,
                        at_micros,
                    );
                }
                CoreOutput::CancelTimer { id } => {
                    output_state.timers.remove(&id);
                }
            }
        }

        while let Some(command) = follow_up.pop_front() {
            let outputs = core
                .handle_input(CoreInput::Command(command))
                .map_err(|err| format!("core follow-up failed: {err}"))?;
            pending_outputs.push_back(outputs);
        }
    }
    Ok(())
}

/// Resolve the host/port of an `RtmpUrl` into a TCP socket address.
///
/// 将 `RtmpUrl` 的主机/端口解析为 TCP 套接字地址。
fn resolve_url_addr(url: &RtmpUrl) -> io::Result<SocketAddr> {
    let host = url.host.trim_matches(|c| c == '[' || c == ']');
    let addrs = (host, url.port).to_socket_addrs()?;
    // Prefer IPv4 when both families are available: in many test/dev
    // environments an IPv6 localhost address is returned first but the
    // server may only be listening on IPv4.
    let mut preferred = None;
    let mut fallback = None;
    for addr in addrs {
        if addr.is_ipv4() {
            preferred = Some(addr);
            break;
        }
        fallback = fallback.or(Some(addr));
    }
    preferred.or(fallback).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("failed to resolve {}", url.host),
        )
    })
}

/// Return the next monotonic timer generation, skipping zero.
///
/// 返回下一个单调定时器 generation，跳过零。
fn next_timer_generation(seed: &mut u64) -> u64 {
    let generation = *seed;
    *seed = seed.wrapping_add(1);
    if *seed == 0 {
        *seed = 1;
    }
    generation
}

/// Spawn a runtime timer that sends a `TimerFired` on expiration.
///
/// 派生运行时定时器，到期时发送 `TimerFired`。
fn schedule_timer(
    runtime_api: Arc<dyn RuntimeApi>,
    timer_tx: mpsc::Sender<TimerFired>,
    id: TimerId,
    generation: u64,
    at_micros: u64,
) {
    let deadline = MonoTime::from_micros(at_micros);
    let runtime_for_timer = runtime_api.clone();
    let _ = runtime_api.spawn(Box::pin(async move {
        let mut timer = runtime_for_timer.sleep_until(deadline);
        timer.wait().await;
        if timer_tx.send(TimerFired { id, generation }).await.is_err() {
            warn!(timer_id = id, "client timer event channel closed");
        }
    }));
}

/// Verify that a fired timer still matches the current generation.
///
/// 验证触发的定时器仍与当前 generation 匹配。
fn is_timer_active(timers: &HashMap<TimerId, u64>, fired: TimerFired) -> bool {
    timers
        .get(&fired.id)
        .is_some_and(|generation| *generation == fired.generation)
}

/// Start an RTMPS client that connects to a remote server over TLS.
pub fn start_tls_client(
    runtime_api: Arc<dyn RuntimeApi>,
    url: RtmpUrl,
    mode: RtmpClientMode,
    config: RtmpClientDriverConfig,
    tls_config: crate::tls::RtmpTlsClientConfig,
    cancel: CancellationToken,
) -> io::Result<RtmpClientHandle> {
    let addr = resolve_url_addr(&url)?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtmpClientCommandSender { tx: cmd_tx.clone() };

    let host = url.host.clone();
    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let runtime_api = runtime_api.clone();
        async move {
            // Connect TCP then wrap with TLS
            let tcp_stream = match tokio::net::TcpStream::connect(addr).await {
                Ok(s) => s,
                Err(err) => {
                    let _ = event_tx
                        .send(ClientDriverEvent::Closed {
                            reason: format!("tcp connect failed: {err}"),
                        })
                        .await;
                    return;
                }
            };
            let peer = tcp_stream.peer_addr().ok();

            let server_name = match rustls::pki_types::ServerName::try_from(host.clone()) {
                Ok(name) => name,
                Err(err) => {
                    let _ = event_tx
                        .send(ClientDriverEvent::Closed {
                            reason: format!("invalid server name '{host}': {err}"),
                        })
                        .await;
                    return;
                }
            };

            let tls_stream =
                match crate::tls::connect_tls(tcp_stream, addr, server_name, &tls_config.connector)
                    .await
                {
                    Ok(s) => s,
                    Err(err) => {
                        let _ = event_tx
                            .send(ClientDriverEvent::Closed {
                                reason: format!("tls handshake failed: {err}"),
                            })
                            .await;
                        return;
                    }
                };

            let _ = event_tx.send(ClientDriverEvent::Connected { peer }).await;
            let stream: Box<dyn AsyncTcpStream> = Box::new(tls_stream);
            let reason = run_client_connection(ClientConnectionParams {
                stream,
                runtime_api: &runtime_api,
                mode,
                url,
                config,
                cancel: join_cancel.clone(),
                event_tx: &event_tx,
                cmd_rx: &mut cmd_rx,
            })
            .await;
            let _ = event_tx.send(ClientDriverEvent::Closed { reason }).await;
        }
    }));

    Ok(RtmpClientHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}
