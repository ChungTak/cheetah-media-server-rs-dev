use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::MonoTime;
use cheetah_rtmp_core::{CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpEvent, TimerId};
use cheetah_runtime_api::{
    AsyncTcpStream, CancellationToken, JoinHandle, RuntimeApi, TaskJoinError,
};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;

pub type RtmpConnectionId = u64;

const MAX_CONSECUTIVE_WRITE_ERRORS: u32 = 30;

#[derive(Debug, Clone)]
/// Configuration for the RTMP server driver: queue sizes and read buffer.
///
/// RTMP 服务器驱动配置：队列大小与读缓冲区。
pub struct DriverConfig {
    pub write_queue_capacity: usize,
    pub command_queue_capacity: usize,
    pub event_queue_capacity: usize,
    pub read_buffer_size: usize,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            write_queue_capacity: 256,
            command_queue_capacity: 256,
            event_queue_capacity: 1024,
            read_buffer_size: 64 * 1024,
        }
    }
}

#[derive(Debug)]
/// Events emitted by the RTMP server driver to the module.
///
/// RTMP 服务器驱动向模块发出的事件。
pub enum DriverEvent {
    ConnectionOpened {
        connection_id: RtmpConnectionId,
        peer: Option<SocketAddr>,
    },
    ConnectionClosed {
        connection_id: RtmpConnectionId,
        reason: String,
    },
    Core {
        connection_id: RtmpConnectionId,
        event: RtmpEvent,
    },
}

#[derive(Debug, Clone)]
/// Commands sent from the module into the RTMP server driver.
///
/// 从模块发送到 RTMP 服务器驱动的命令。
pub enum RtmpDriverCommand {
    Core {
        connection_id: RtmpConnectionId,
        command: RtmpCoreCommand,
    },
    CloseConnection {
        connection_id: RtmpConnectionId,
    },
    Shutdown,
}

#[derive(Clone)]
/// MPSC sender handle for issuing commands to the driver loop.
///
/// 用于向驱动循环发送命令的 MPSC 发送端句柄。
pub struct RtmpCoreCommandSender {
    tx: mpsc::Sender<RtmpDriverCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Error returned when the command channel has closed.
///
/// 当命令通道关闭时返回的错误。
pub enum DriverSendError {
    ChannelClosed,
}

/// `RtmpCoreCommandSender` API: send commands and close connections.
///
/// `RtmpCoreCommandSender` API：发送命令并关闭连接。
impl RtmpCoreCommandSender {
    pub async fn send(&self, command: RtmpDriverCommand) -> Result<(), DriverSendError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| DriverSendError::ChannelClosed)
    }

    pub async fn send_core(
        &self,
        connection_id: RtmpConnectionId,
        command: RtmpCoreCommand,
    ) -> Result<(), DriverSendError> {
        self.send(RtmpDriverCommand::Core {
            connection_id,
            command,
        })
        .await
    }

    pub async fn close_connection(
        &self,
        connection_id: RtmpConnectionId,
    ) -> Result<(), DriverSendError> {
        self.send(RtmpDriverCommand::CloseConnection { connection_id })
            .await
    }
}

#[derive(Debug)]
/// Internal command routed to a per-connection task.
///
/// 路由到每个连接任务的内置命令。
enum ConnectionCommand {
    Core(RtmpCoreCommand),
    Close,
}

/// Handle for the RTMP server: receive events, send commands, and shutdown.
///
/// RTMP 服务器句柄：接收事件、发送命令并关闭。
pub struct RtmpServerHandle {
    events_rx: mpsc::Receiver<DriverEvent>,
    cmd_tx: RtmpCoreCommandSender,
    cancel: CancellationToken,
    join: Box<dyn JoinHandle>,
}

/// `RtmpServerHandle` API: event reception, command send, and lifecycle.
///
/// `RtmpServerHandle` API：事件接收、命令发送与生命周期。
impl RtmpServerHandle {
    pub async fn recv_event(&mut self) -> Option<DriverEvent> {
        self.events_rx.recv().await
    }

    pub async fn send_driver_command(
        &self,
        command: RtmpDriverCommand,
    ) -> Result<(), DriverSendError> {
        self.cmd_tx.send(command).await
    }

    pub fn core_command_sender(&self) -> RtmpCoreCommandSender {
        self.cmd_tx.clone()
    }

    pub fn shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(self) -> Result<(), TaskJoinError> {
        self.join.wait().await
    }
}

/// Start a TCP RTMP server and return a handle for the driver.
///
/// 启动 TCP RTMP 服务器并返回驱动句柄。
pub fn start_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: DriverConfig,
    cancel: CancellationToken,
) -> io::Result<RtmpServerHandle> {
    let listener = runtime_api.bind_tcp(listen)?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtmpCoreCommandSender { tx: cmd_tx.clone() };

    let conn_map: Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let conn_ids = Arc::new(AtomicU64::new(1));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let config = config.clone();
        let runtime_api = runtime_api.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => {
                        break;
                    }
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else {
                            break;
                        };
                        if handle_driver_command(cmd, &conn_map, &join_cancel, &runtime_api) {
                            break;
                        }
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let (conn_tx, conn_rx) = mpsc::channel(config.command_queue_capacity.max(64));
                                conn_map.lock().insert(connection_id, conn_tx);

                                if event_tx.send(DriverEvent::ConnectionOpened {
                                    connection_id,
                                    peer: Some(peer),
                                }).await.is_err() {
                                    break;
                                }

                                let child_cancel = join_cancel.child_token();
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let runtime_api2 = runtime_api.clone();
                                let runtime_api_for_task = runtime_api2.clone();
                                let config2 = config.clone();
                                let _ = runtime_api2.spawn(Box::pin(async move {
                                    let runtime = ConnectionRuntime {
                                        event_tx: event_tx2,
                                        conn_map: conn_map2,
                                        runtime_api: runtime_api_for_task,
                                        cancel: child_cancel,
                                        config: config2,
                                    };
                                    run_connection(
                                        connection_id,
                                        stream,
                                        conn_rx,
                                        runtime,
                                    )
                                    .await;
                                }));
                            }
                            Err(err) => {
                                warn!(%err, "rtmp listener accept failed");
                                let deadline = MonoTime::from_micros(
                                    runtime_api
                                        .now()
                                        .as_micros()
                                        .saturating_add(Duration::from_millis(200).as_micros() as u64),
                                );
                                let mut backoff = runtime_api.sleep_until(deadline);
                                tokio::select! {
                                    _ = join_cancel.cancelled() => {
                                        break;
                                    }
                                    _ = backoff.wait() => {}
                                }
                            }
                        }
                    }
                }
            }

            let senders: Vec<mpsc::Sender<ConnectionCommand>> =
                conn_map.lock().values().cloned().collect();
            for tx in senders {
                let _ = tx.try_send(ConnectionCommand::Close);
            }
        }
    }));

    Ok(RtmpServerHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

/// Route a driver-level command to the right connection or shutdown.
///
/// 将驱动级命令路由到正确的连接或关闭。
fn handle_driver_command(
    cmd: RtmpDriverCommand,
    conn_map: &Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>>,
    cancel: &CancellationToken,
    runtime_api: &Arc<dyn RuntimeApi>,
) -> bool {
    match cmd {
        RtmpDriverCommand::Core {
            connection_id,
            command,
        } => {
            send_connection_command(
                connection_id,
                ConnectionCommand::Core(command),
                conn_map,
                runtime_api,
            );
            false
        }
        RtmpDriverCommand::CloseConnection { connection_id } => {
            send_connection_command(
                connection_id,
                ConnectionCommand::Close,
                conn_map,
                runtime_api,
            );
            false
        }
        RtmpDriverCommand::Shutdown => {
            cancel.cancel();
            true
        }
    }
}

/// Forward a command to a connection task, dropping/ closing on full or closed.
///
/// 将命令转发到连接任务，在队列满或关闭时丢弃/关闭。
fn send_connection_command(
    connection_id: RtmpConnectionId,
    command: ConnectionCommand,
    conn_map: &Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>>,
    runtime_api: &Arc<dyn RuntimeApi>,
) {
    let tx = conn_map.lock().get(&connection_id).cloned();
    let Some(tx) = tx else {
        return;
    };

    match tx.try_send(command) {
        Ok(()) => {}
        Err(TrySendError::Closed(_)) => {
            conn_map.lock().remove(&connection_id);
        }
        Err(TrySendError::Full(ConnectionCommand::Core(_))) => {
            warn!(
                connection_id,
                "rtmp connection command queue is full, dropping command and closing connection"
            );
            force_close_connection(connection_id, tx, conn_map, runtime_api);
        }
        Err(TrySendError::Full(ConnectionCommand::Close)) => {
            force_close_connection(connection_id, tx, conn_map, runtime_api);
        }
    }
}

/// Enqueue a close command, even if the command queue is full.
///
/// 即使命令队列已满，也强制入队关闭命令。
fn force_close_connection(
    connection_id: RtmpConnectionId,
    tx: mpsc::Sender<ConnectionCommand>,
    conn_map: &Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>>,
    runtime_api: &Arc<dyn RuntimeApi>,
) {
    match tx.try_send(ConnectionCommand::Close) {
        Ok(()) => {}
        Err(TrySendError::Full(close_cmd)) => {
            let tx_for_task = tx.clone();
            let _ = runtime_api.spawn(Box::pin(async move {
                let _ = tx_for_task.send(close_cmd).await;
            }));
        }
        Err(TrySendError::Closed(_)) => {}
    }

    conn_map.lock().remove(&connection_id);
}

#[derive(Debug, Clone, Copy)]
/// Identifier for a timer firing, paired with a generation to detect stale events.
///
/// 定时器触发标识，附带 generation 以检测过期事件。
struct TimerFired {
    id: TimerId,
    generation: u64,
}

/// Shared resources passed to a connection task.
///
/// 传递给连接任务的共享资源。
struct ConnectionRuntime {
    event_tx: mpsc::Sender<DriverEvent>,
    conn_map: Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>>,
    runtime_api: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
    config: DriverConfig,
}

/// Per-connection task: read bytes, push to core, flush outputs, handle timers.
///
/// 每个连接的任务：读取字节、推入 core、刷新输出、处理定时器。
async fn run_connection(
    connection_id: RtmpConnectionId,
    mut stream: Box<dyn AsyncTcpStream>,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    runtime: ConnectionRuntime,
) {
    let (write_tx, mut write_rx) =
        mpsc::channel::<Bytes>(runtime.config.write_queue_capacity.max(8));
    let (timer_tx, mut timer_rx) =
        mpsc::channel::<TimerFired>(runtime.config.command_queue_capacity.max(64));

    let mut core = RtmpCore::new();
    let mut read_buf = vec![0u8; runtime.config.read_buffer_size.max(1024)];
    let mut timer_generation_seed = 1u64;
    let mut timers: HashMap<TimerId, u64> = HashMap::new();
    let mut consecutive_write_errors: u32 = 0;

    let reason = loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => {
                break "cancelled".to_string();
            }
            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(ConnectionCommand::Core(command)) => {
                        match core.handle_input(CoreInput::Command(command)) {
                            Ok(outputs) => {
                                let mut output_state = OutputState {
                                    connection_id,
                                    event_tx: &runtime.event_tx,
                                    write_tx: &write_tx,
                                    runtime_api: &runtime.runtime_api,
                                    timer_tx: &timer_tx,
                                    timers: &mut timers,
                                    timer_generation_seed: &mut timer_generation_seed,
                                };
                                if let Err(err) = flush_outputs(outputs, &mut output_state).await {
                                    break err;
                                }
                            }
                            Err(err) => {
                                break format!("core command error: {err}");
                            }
                        }
                    }
                    Some(ConnectionCommand::Close) => {
                        break "closed by command".to_string();
                    }
                    None => {
                        break "command channel closed".to_string();
                    }
                }
            }
            maybe_fired = timer_rx.recv() => {
                let Some(fired) = maybe_fired else {
                    break "timer channel closed".to_string();
                };
                if !is_timer_active(&timers, fired) {
                    continue;
                }
                timers.remove(&fired.id);
                match core.handle_input(CoreInput::Timeout { id: fired.id }) {
                    Ok(outputs) => {
                        let mut output_state = OutputState {
                            connection_id,
                            event_tx: &runtime.event_tx,
                            write_tx: &write_tx,
                            runtime_api: &runtime.runtime_api,
                            timer_tx: &timer_tx,
                            timers: &mut timers,
                            timer_generation_seed: &mut timer_generation_seed,
                        };
                        if let Err(err) = flush_outputs(outputs, &mut output_state).await {
                            break err;
                        }
                    }
                    Err(err) => {
                        break format!("core timeout error: {err}");
                    }
                }
            }
            maybe_write = write_rx.recv() => {
                let Some(bytes) = maybe_write else {
                    break "write queue closed".to_string();
                };
                tokio::select! {
                    _ = runtime.cancel.cancelled() => {
                        break "cancelled".to_string();
                    }
                    write_res = stream.write_all(&bytes) => {
                        if let Err(err) = write_res {
                            consecutive_write_errors += 1;
                            if consecutive_write_errors >= MAX_CONSECUTIVE_WRITE_ERRORS {
                                break format!("write failed ({consecutive_write_errors} consecutive): {err}");
                            }
                        } else {
                            consecutive_write_errors = 0;
                        }
                    }
                }
            }
            read_res = stream.read(&mut read_buf) => {
                match read_res {
                    Ok(0) => {
                        break "peer closed".to_string();
                    }
                    Ok(n) => {
                        let bytes = Bytes::copy_from_slice(&read_buf[..n]);
                        match core.handle_input(CoreInput::Bytes(bytes)) {
                            Ok(outputs) => {
                                let mut output_state = OutputState {
                                    connection_id,
                                    event_tx: &runtime.event_tx,
                                    write_tx: &write_tx,
                                    runtime_api: &runtime.runtime_api,
                                    timer_tx: &timer_tx,
                                    timers: &mut timers,
                                    timer_generation_seed: &mut timer_generation_seed,
                                };
                                if let Err(err) = flush_outputs(outputs, &mut output_state).await {
                                    break err;
                                }
                            }
                            Err(err) => {
                                break format!("core read error: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        break format!("read failed: {err}");
                    }
                }
            }
        }
    };

    let _ = stream.shutdown().await;
    runtime.conn_map.lock().remove(&connection_id);
    let _ = runtime
        .event_tx
        .send(DriverEvent::ConnectionClosed {
            connection_id,
            reason,
        })
        .await;
}

/// Mutable state borrowed by `flush_outputs` for a single connection.
///
/// `flush_outputs` 为单个连接借用的可变状态。
struct OutputState<'a> {
    connection_id: RtmpConnectionId,
    event_tx: &'a mpsc::Sender<DriverEvent>,
    write_tx: &'a mpsc::Sender<Bytes>,
    runtime_api: &'a Arc<dyn RuntimeApi>,
    timer_tx: &'a mpsc::Sender<TimerFired>,
    timers: &'a mut HashMap<TimerId, u64>,
    timer_generation_seed: &'a mut u64,
}

/// Dispatch core outputs: write bytes, emit events, set/cancel timers.
///
/// 分发 core 输出：写入字节、发出事件、设置/取消定时器。
async fn flush_outputs(
    outputs: Vec<CoreOutput>,
    state: &mut OutputState<'_>,
) -> Result<(), String> {
    for output in outputs {
        match output {
            CoreOutput::Write(bytes) => {
                state
                    .write_tx
                    .try_send(bytes)
                    .map_err(|_| "write queue overflow".to_string())?;
            }
            CoreOutput::Event(event) => {
                state
                    .event_tx
                    .send(DriverEvent::Core {
                        connection_id: state.connection_id,
                        event,
                    })
                    .await
                    .map_err(|_| "event channel closed".to_string())?;
            }
            CoreOutput::SetTimer { id, at_micros } => {
                let generation = next_timer_generation(state.timer_generation_seed);
                state.timers.insert(id, generation);
                schedule_timer(
                    state.runtime_api.clone(),
                    state.timer_tx.clone(),
                    id,
                    generation,
                    at_micros,
                );
            }
            CoreOutput::CancelTimer { id } => {
                state.timers.remove(&id);
            }
        }
    }
    Ok(())
}

/// Return a monotonically increasing timer generation, skipping zero.
///
/// 返回单调递增的定时器 generation，跳过零。
fn next_timer_generation(seed: &mut u64) -> u64 {
    let generation = *seed;
    *seed = seed.wrapping_add(1);
    if *seed == 0 {
        *seed = 1;
    }
    generation
}

/// Spawn a runtime timer that sends a `TimerFired` when it expires.
///
/// 派生一个运行时定时器，到期时发送 `TimerFired`。
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
        let _ = timer_tx.send(TimerFired { id, generation }).await;
    }));
}

/// Check if a fired timer still matches the current generation.
///
/// 检查触发的定时器是否仍与当前 generation 匹配。
fn is_timer_active(timers: &HashMap<TimerId, u64>, fired: TimerFired) -> bool {
    timers
        .get(&fired.id)
        .is_some_and(|generation| *generation == fired.generation)
}

/// Start a TLS-enabled RTMP server (RTMPS) on the given address.
///
/// Returns a handle identical to `start_server`. Connections are TLS-wrapped
/// before entering the standard RTMP connection handler.
pub fn start_tls_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: DriverConfig,
    tls_config: crate::tls::RtmpTlsConfig,
    tls_handshake_timeout: Duration,
    cancel: CancellationToken,
) -> io::Result<RtmpServerHandle> {
    let tcp_listener = std::net::TcpListener::bind(listen)?;
    tcp_listener.set_nonblocking(true)?;
    let tokio_listener = tokio::net::TcpListener::from_std(tcp_listener)?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtmpCoreCommandSender { tx: cmd_tx.clone() };

    let conn_map: Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let conn_ids = Arc::new(AtomicU64::new(1));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let config = config.clone();
        let runtime_api = runtime_api.clone();
        let acceptor = tls_config.acceptor.clone();
        async move {
            loop {
                tokio::select! {
                    _ = join_cancel.cancelled() => {
                        break;
                    }
                    maybe_cmd = cmd_rx.recv() => {
                        let Some(cmd) = maybe_cmd else { break; };
                        if handle_driver_command(cmd, &conn_map, &join_cancel, &runtime_api) {
                            break;
                        }
                    }
                    accept_res = tokio_listener.accept() => {
                        match accept_res {
                            Ok((tcp_stream, peer)) => {
                                let connection_id = conn_ids.fetch_add(1, Ordering::Relaxed);
                                let acceptor = acceptor.clone();
                                let event_tx2 = event_tx.clone();
                                let conn_map2 = conn_map.clone();
                                let runtime_api2 = runtime_api.clone();
                                let config2 = config.clone();
                                let child_cancel = join_cancel.child_token();
                                let timeout_dur = tls_handshake_timeout;

                                let _ = runtime_api.spawn(Box::pin(async move {
                                    let tls_stream = match crate::tls::accept_tls(
                                        tcp_stream, peer, &acceptor, timeout_dur,
                                    ).await {
                                        Ok(s) => s,
                                        Err(err) => {
                                            warn!(%err, %peer, "rtmps tls handshake failed");
                                            return;
                                        }
                                    };

                                    let (conn_tx, conn_rx) = mpsc::channel(config2.command_queue_capacity.max(64));
                                    conn_map2.lock().insert(connection_id, conn_tx);

                                    if event_tx2.send(DriverEvent::ConnectionOpened {
                                        connection_id,
                                        peer: Some(peer),
                                    }).await.is_err() {
                                        return;
                                    }

                                    let runtime = ConnectionRuntime {
                                        event_tx: event_tx2,
                                        conn_map: conn_map2,
                                        runtime_api: runtime_api2,
                                        cancel: child_cancel,
                                        config: config2,
                                    };
                                    run_connection(
                                        connection_id,
                                        Box::new(tls_stream),
                                        conn_rx,
                                        runtime,
                                    ).await;
                                }));
                            }
                            Err(err) => {
                                warn!(%err, "rtmps listener accept failed");
                                let deadline = MonoTime::from_micros(
                                    runtime_api.now().as_micros().saturating_add(
                                        Duration::from_millis(200).as_micros() as u64,
                                    ),
                                );
                                let mut backoff = runtime_api.sleep_until(deadline);
                                tokio::select! {
                                    _ = join_cancel.cancelled() => { break; }
                                    _ = backoff.wait() => {}
                                }
                            }
                        }
                    }
                }
            }

            let senders: Vec<mpsc::Sender<ConnectionCommand>> =
                conn_map.lock().values().cloned().collect();
            for tx in senders {
                let _ = tx.try_send(ConnectionCommand::Close);
            }
        }
    }));

    Ok(RtmpServerHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use cheetah_rtmp_core::{
        encode_all, Amf0Value, ErrorKind, RtmpChunk, RtmpChunkDecoder, RtmpChunkEncoder,
        RtmpChunkSize, RtmpChunkStreamId, RtmpMessageStreamId, RtmpMessageType, RtmpTimestamp,
    };
    use std::sync::Arc;

    use cheetah_runtime_api::{CancellationToken, RuntimeApi};
    use cheetah_runtime_tokio::TokioRuntime;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{timeout, Duration};

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn accepts_handshake_and_closes_by_command() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let listen = probe.local_addr().expect("probe addr");
        drop(probe);

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut handle = start_server(
            runtime_api,
            listen,
            DriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start server");

        let command_sender = handle.core_command_sender();
        let mut stream = tokio::net::TcpStream::connect(listen)
            .await
            .expect("tcp connect");

        let connection_id = timeout(Duration::from_secs(1), async {
            loop {
                match handle.recv_event().await {
                    Some(DriverEvent::ConnectionOpened { connection_id, .. }) => {
                        break connection_id;
                    }
                    Some(_) => {}
                    None => panic!("driver event channel closed"),
                }
            }
        })
        .await
        .expect("connection open timeout");

        let mut c0c1 = vec![0u8; 1537];
        c0c1[0] = 3;
        stream.write_all(&c0c1).await.expect("write c0c1");

        let mut s0s1s2 = vec![0u8; 3073];
        stream.read_exact(&mut s0s1s2).await.expect("read s0s1s2");
        assert_eq!(s0s1s2[0], 3);

        let c2 = vec![0u8; 1536];
        stream.write_all(&c2).await.expect("write c2");

        command_sender
            .close_connection(connection_id)
            .await
            .expect("send close command");

        let reason = timeout(Duration::from_secs(1), async {
            loop {
                match handle.recv_event().await {
                    Some(DriverEvent::ConnectionClosed {
                        connection_id: id,
                        reason,
                    }) if id == connection_id => break reason,
                    Some(_) => {}
                    None => panic!("driver event channel closed"),
                }
            }
        })
        .await
        .expect("connection close timeout");

        assert!(!reason.is_empty());
        handle.shutdown();
        let _ = handle.wait().await;
    }

    fn command_wire(message_stream_id: u32, values: &[Amf0Value]) -> Bytes {
        let payload = encode_all(values);
        encode_chunk_wire(
            3,
            RtmpMessageType::CommandAmf0,
            message_stream_id,
            0,
            payload.as_ref(),
            128,
        )
    }

    fn media_wire(message_type: u8, message_stream_id: u32, payload: &[u8]) -> Bytes {
        let message_type =
            RtmpMessageType::from_type_id(message_type).expect("valid media message type");
        encode_chunk_wire(6, message_type, message_stream_id, 0, payload, 128)
    }

    fn encode_chunk_wire(
        csid: u32,
        message_type: RtmpMessageType,
        message_stream_id: u32,
        timestamp_ms: u32,
        payload: &[u8],
        out_chunk_size: usize,
    ) -> Bytes {
        let chunk_stream_id = RtmpChunkStreamId::new(csid).expect("valid csid");
        let chunk = RtmpChunk {
            chunk_stream_id,
            message_stream_id: RtmpMessageStreamId::new(message_stream_id),
            message_type,
            timestamp: RtmpTimestamp::from_millis(timestamp_ms),
            payload: Bytes::from(payload.to_vec()),
        };
        let mut encoder = RtmpChunkEncoder::default();
        encoder.set_chunk_size(RtmpChunkSize::saturating_new(out_chunk_size));
        let mut wire = Vec::new();
        encoder.encode(&mut wire, &chunk);
        Bytes::from(wire)
    }

    async fn client_handshake(stream: &mut tokio::net::TcpStream) {
        let mut c0c1 = vec![0u8; 1537];
        c0c1[0] = 3;
        stream.write_all(&c0c1).await.expect("write c0c1");
        let mut s0s1s2 = vec![0u8; 3073];
        stream.read_exact(&mut s0s1s2).await.expect("read s0s1s2");
        assert_eq!(s0s1s2[0], 3);
        stream.write_all(&vec![0u8; 1536]).await.expect("write c2");
    }

    async fn recv_conn_open(handle: &mut RtmpServerHandle) -> RtmpConnectionId {
        timeout(Duration::from_secs(1), async {
            loop {
                match handle.recv_event().await {
                    Some(DriverEvent::ConnectionOpened { connection_id, .. }) => {
                        break connection_id;
                    }
                    Some(_) => {}
                    None => panic!("driver event channel closed"),
                }
            }
        })
        .await
        .expect("connection open timeout")
    }

    async fn recv_core_event<F>(
        handle: &mut RtmpServerHandle,
        connection_id: RtmpConnectionId,
        mut pred: F,
    ) -> RtmpEvent
    where
        F: FnMut(&RtmpEvent) -> bool,
    {
        timeout(Duration::from_secs(1), async {
            loop {
                match handle.recv_event().await {
                    Some(DriverEvent::Core {
                        connection_id: id,
                        event,
                    }) if id == connection_id && pred(&event) => break event,
                    Some(_) => {}
                    None => panic!("driver event channel closed"),
                }
            }
        })
        .await
        .expect("core event timeout")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publish_flow_emits_publish_and_media_events() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let listen = probe.local_addr().expect("probe addr");
        drop(probe);

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut handle = start_server(
            runtime_api,
            listen,
            DriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start server");

        let command_sender = handle.core_command_sender();
        let mut stream = tokio::net::TcpStream::connect(listen)
            .await
            .expect("tcp connect");
        let connection_id = recv_conn_open(&mut handle).await;
        client_handshake(&mut stream).await;

        stream
            .write_all(&command_wire(
                0,
                &[
                    Amf0Value::String("connect".to_string()),
                    Amf0Value::Number(1.0),
                    Amf0Value::empty_object(),
                ],
            ))
            .await
            .expect("write connect");
        let _ = recv_core_event(&mut handle, connection_id, |event| {
            matches!(event, RtmpEvent::Connected { .. })
        })
        .await;

        stream
            .write_all(&command_wire(
                0,
                &[
                    Amf0Value::String("createStream".to_string()),
                    Amf0Value::Number(2.0),
                    Amf0Value::Null,
                ],
            ))
            .await
            .expect("write createStream");
        tokio::time::sleep(Duration::from_millis(20)).await;

        stream
            .write_all(&command_wire(
                1,
                &[
                    Amf0Value::String("publish".to_string()),
                    Amf0Value::Number(3.0),
                    Amf0Value::Null,
                    Amf0Value::String("test".to_string()),
                ],
            ))
            .await
            .expect("write publish");
        let publish_event = recv_core_event(&mut handle, connection_id, |event| {
            matches!(event, RtmpEvent::PublishRequested { .. })
        })
        .await;
        assert!(matches!(
            publish_event,
            RtmpEvent::PublishRequested {
                stream_id: 1,
                ref stream_name,
                ..
            } if stream_name == "test"
        ));

        command_sender
            .send_core(
                connection_id,
                RtmpCoreCommand::AcceptPublish { stream_id: 1 },
            )
            .await
            .expect("send accept publish");

        let video_payload = [0x17, 0x01, 0x00, 0x00, 0x00, 1, 2, 3, 4];
        stream
            .write_all(&media_wire(9, 1, &video_payload))
            .await
            .expect("write video");
        let media_event = recv_core_event(&mut handle, connection_id, |event| {
            matches!(event, RtmpEvent::MediaData { .. })
        })
        .await;
        assert!(matches!(
            media_event,
            RtmpEvent::MediaData {
                stream_id: 1,
                media_type: cheetah_rtmp_core::RtmpMediaType::Video,
                ref payload,
                ..
            } if payload.as_ref() == video_payload
        ));

        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn play_flow_can_send_video_back_to_client() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let listen = probe.local_addr().expect("probe addr");
        drop(probe);

        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        let mut handle = start_server(
            runtime_api,
            listen,
            DriverConfig::default(),
            CancellationToken::new(),
        )
        .expect("start server");

        let command_sender = handle.core_command_sender();
        let mut stream = tokio::net::TcpStream::connect(listen)
            .await
            .expect("tcp connect");
        let connection_id = recv_conn_open(&mut handle).await;
        client_handshake(&mut stream).await;

        stream
            .write_all(&command_wire(
                0,
                &[
                    Amf0Value::String("connect".to_string()),
                    Amf0Value::Number(1.0),
                    Amf0Value::empty_object(),
                ],
            ))
            .await
            .expect("write connect");
        let _ = recv_core_event(&mut handle, connection_id, |event| {
            matches!(event, RtmpEvent::Connected { .. })
        })
        .await;

        stream
            .write_all(&command_wire(
                0,
                &[
                    Amf0Value::String("createStream".to_string()),
                    Amf0Value::Number(2.0),
                    Amf0Value::Null,
                ],
            ))
            .await
            .expect("write createStream");
        tokio::time::sleep(Duration::from_millis(20)).await;

        stream
            .write_all(&command_wire(
                1,
                &[
                    Amf0Value::String("play".to_string()),
                    Amf0Value::Number(3.0),
                    Amf0Value::Null,
                    Amf0Value::String("test".to_string()),
                ],
            ))
            .await
            .expect("write play");
        let play_event = recv_core_event(&mut handle, connection_id, |event| {
            matches!(event, RtmpEvent::PlayRequested { .. })
        })
        .await;
        assert!(matches!(
            play_event,
            RtmpEvent::PlayRequested {
                stream_id: 1,
                ref stream_name,
                ..
            } if stream_name == "test"
        ));

        command_sender
            .send_core(connection_id, RtmpCoreCommand::AcceptPlay { stream_id: 1 })
            .await
            .expect("send accept play");

        let expected = [0x17, 0x01, 0x00, 0x00, 0x00, 0xaa, 0xbb];
        command_sender
            .send_core(
                connection_id,
                RtmpCoreCommand::SendVideo {
                    stream_id: 1,
                    timestamp_ms: 0,
                    payload: Bytes::copy_from_slice(&expected),
                },
            )
            .await
            .expect("send video");

        let mut decoder = RtmpChunkDecoder::default();
        decoder.set_chunk_size(RtmpChunkSize::saturating_new(60_000));
        let mut pending = Vec::new();
        let mut read_buf = [0u8; 4096];
        let mut saw_expected_video = false;
        for _ in 0..20 {
            let n = timeout(Duration::from_millis(200), stream.read(&mut read_buf))
                .await
                .expect("read timeout")
                .expect("read failed");
            if n == 0 {
                break;
            }
            pending.extend_from_slice(&read_buf[..n]);
            loop {
                match decoder.decode(&pending) {
                    Ok((consumed, maybe_chunk)) => {
                        pending.drain(..consumed);
                        if let Some(chunk) = maybe_chunk {
                            let matched = chunk.message_type == RtmpMessageType::Video
                                && chunk.message_stream_id.get() == 1
                                && chunk.payload.as_ref() == expected;
                            if matched {
                                saw_expected_video = true;
                                break;
                            }
                        }
                    }
                    Err(err) if err.kind == ErrorKind::InsufficientBuffer => break,
                    Err(err) => panic!("decode outbound chunk failed: {err:?}"),
                }
            }
            if saw_expected_video {
                break;
            }
        }
        assert!(
            saw_expected_video,
            "did not receive expected outbound video"
        );

        handle.shutdown();
        let _ = handle.wait().await;
    }

    #[test]
    fn timer_generation_filters_stale_events() {
        let mut timers = HashMap::new();
        timers.insert(1, 4);
        assert!(is_timer_active(
            &timers,
            TimerFired {
                id: 1,
                generation: 4
            }
        ));
        assert!(!is_timer_active(
            &timers,
            TimerFired {
                id: 1,
                generation: 3
            }
        ));
        assert!(!is_timer_active(
            &timers,
            TimerFired {
                id: 9,
                generation: 4
            }
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_command_cancels_shared_token() {
        let conn_map: Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let cancel = CancellationToken::new();
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());
        assert!(!cancel.is_cancelled());
        let stop = handle_driver_command(
            RtmpDriverCommand::Shutdown,
            &conn_map,
            &cancel,
            &runtime_api,
        );
        assert!(stop);
        assert!(cancel.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn full_connection_command_queue_does_not_block_driver_loop() {
        let connection_id = 7;
        let conn_map: Arc<Mutex<HashMap<RtmpConnectionId, mpsc::Sender<ConnectionCommand>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(ConnectionCommand::Close)
            .expect("pre-fill command queue");
        conn_map.lock().insert(connection_id, tx);

        let cancel = CancellationToken::new();
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(TokioRuntime::new());

        timeout(Duration::from_millis(50), async {
            let stop = handle_driver_command(
                RtmpDriverCommand::Core {
                    connection_id,
                    command: RtmpCoreCommand::CloseConnection,
                },
                &conn_map,
                &cancel,
                &runtime_api,
            );
            assert!(!stop);
        })
        .await
        .expect("driver command handling must not block on full queue");

        assert!(conn_map.lock().get(&connection_id).is_none());
        assert!(rx.recv().await.is_some());
    }
}
