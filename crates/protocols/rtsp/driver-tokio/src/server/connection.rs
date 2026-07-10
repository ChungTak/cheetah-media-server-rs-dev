use std::collections::VecDeque;
use std::time::Duration;

use bytes::Bytes;
use cheetah_rtsp_core::{CoreInput, CoreOutput, RtspCore};
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use super::command::{ConnectionCommand, ConnectionMap};
use super::{DriverConfig, DriverEvent, RtspConnectionId};

/// Runtime resources passed to each connection task.
///
/// Contains the event sender, the shared connection map, the cancellation token, and
/// the driver configuration used by the connection loop.
///
/// 传递给每个连接任务的运行时资源。
///
/// 包含事件发送器、共享连接映射、取消令牌以及连接循环使用的驱动配置。
pub(super) struct ConnectionRuntime {
    pub(super) event_tx: mpsc::Sender<DriverEvent>,
    pub(super) conn_map: ConnectionMap,
    pub(super) cancel: CancellationToken,
    pub(super) config: DriverConfig,
}

/// Run a single plain TCP or TLS server connection.
///
/// The connection owns a `RtspCore` instance. It feeds incoming bytes and outbound core
/// commands into the core, then flushes the resulting outputs (bytes to write, events to
/// emit, or close). The write loop is the same as the client: one pending write at a time,
/// with `try_recv` and zero-timeout reads while the queue is non-empty.
///
/// 运行单个普通 TCP 或 TLS 服务器连接。
///
/// 连接拥有一个 `RtspCore` 实例。将入站字节与出站核心命令输入核心，然后刷新产生的
/// 输出（待写字节、待发出事件或关闭）。写入循环与客户端一致：一次处理一个待写项，
/// 队列非空时使用 `try_recv` 与零超时读取。
pub(super) async fn run_connection(
    connection_id: RtspConnectionId,
    mut stream: Box<dyn AsyncTcpStream>,
    initial_input: Bytes,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    runtime: ConnectionRuntime,
) {
    let mut pending_writes = VecDeque::<Bytes>::new();
    let max_write_queue = runtime.config.write_queue_capacity.max(8);
    let mut core = RtspCore::new();
    let mut read_buf = vec![0_u8; runtime.config.read_buffer_size.max(1024)];
    let mut close_requested = false;

    if !initial_input.is_empty() {
        match core.handle_input(CoreInput::Bytes(initial_input)) {
            Ok(outputs) => {
                if let Err(reason) = flush_outputs(
                    outputs,
                    connection_id,
                    &runtime.event_tx,
                    &mut pending_writes,
                    max_write_queue,
                )
                .await
                {
                    let _ = stream.shutdown().await;
                    runtime.conn_map.lock().remove(&connection_id);
                    let _ = runtime
                        .event_tx
                        .send(DriverEvent::ConnectionClosed {
                            connection_id,
                            reason,
                        })
                        .await;
                    return;
                }
            }
            Err(err) => {
                let reason = format!("core read error: {err}");
                let _ = stream.shutdown().await;
                runtime.conn_map.lock().remove(&connection_id);
                let _ = runtime
                    .event_tx
                    .send(DriverEvent::ConnectionClosed {
                        connection_id,
                        reason,
                    })
                    .await;
                return;
            }
        }
    }

    let mut consecutive_write_errors: u32 = 0;
    let max_write_errors: u32 = 30;

    let reason = loop {
        if close_requested && pending_writes.is_empty() {
            break "closed by command".to_string();
        }
        if let Some(bytes) = pending_writes.front().cloned() {
            if let Err(reason) =
                write_pending_bytes(stream.as_mut(), &bytes, &runtime.cancel, close_requested).await
            {
                consecutive_write_errors += 1;
                if consecutive_write_errors >= max_write_errors {
                    break format!("{reason} ({consecutive_write_errors} consecutive)");
                }
                pending_writes.pop_front();
                continue;
            }
            consecutive_write_errors = 0;
            pending_writes.pop_front();

            if !close_requested {
                match cmd_rx.try_recv() {
                    Ok(command) => {
                        match handle_connection_command(
                            Some(command),
                            &mut core,
                            connection_id,
                            &runtime.event_tx,
                            &mut pending_writes,
                            max_write_queue,
                            &mut close_requested,
                        )
                        .await
                        {
                            Ok(Some(reason)) => break reason,
                            Ok(None) => {}
                            Err(reason) => break reason,
                        }
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        break "command channel closed".to_string();
                    }
                }

                if let Ok(read_res) =
                    tokio::time::timeout(Duration::from_millis(0), stream.read(&mut read_buf)).await
                {
                    match handle_connection_read(
                        read_res,
                        &mut core,
                        connection_id,
                        &runtime.event_tx,
                        &mut pending_writes,
                        max_write_queue,
                        &read_buf,
                    )
                    .await
                    {
                        Ok(Some(reason)) => break reason,
                        Ok(None) => {}
                        Err(reason) => break reason,
                    }
                }
            }
        } else {
            tokio::select! {
                _ = runtime.cancel.cancelled() => {
                    break "cancelled".to_string();
                }
                maybe_cmd = cmd_rx.recv(), if !close_requested => {
                    match handle_connection_command(
                        maybe_cmd,
                        &mut core,
                        connection_id,
                        &runtime.event_tx,
                        &mut pending_writes,
                        max_write_queue,
                        &mut close_requested,
                    )
                    .await
                    {
                        Ok(Some(reason)) => break reason,
                        Ok(None) => {}
                        Err(reason) => break reason,
                    }
                }
                read_res = stream.read(&mut read_buf), if !close_requested => {
                    match handle_connection_read(
                        read_res,
                        &mut core,
                        connection_id,
                        &runtime.event_tx,
                        &mut pending_writes,
                        max_write_queue,
                        &read_buf,
                    )
                    .await
                    {
                        Ok(Some(reason)) => break reason,
                        Ok(None) => {}
                        Err(reason) => break reason,
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

/// Write a queued byte slice to the stream, aborting if the cancellation token fires.
///
/// 将队列中的字节切片写入流，若取消令牌触发则中止。
async fn write_pending_bytes(
    stream: &mut dyn AsyncTcpStream,
    bytes: &[u8],
    cancel: &CancellationToken,
    _close_requested: bool,
) -> Result<(), String> {
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".to_string()),
        write_res = stream.write_all(bytes) => {
            write_res.map_err(|err| format!("write failed: {err}"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
pub(super) async fn write_pending_bytes_for_test(
    mut stream: Box<dyn AsyncTcpStream>,
    cancel: CancellationToken,
    close_requested: bool,
) -> Result<(), String> {
    write_pending_bytes(
        stream.as_mut(),
        b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n",
        &cancel,
        close_requested,
    )
    .await
}

/// Handle a command delivered to the connection task.
///
/// `Core` commands are passed into `RtspCore` and the resulting outputs are flushed.
/// `Close` sets `close_requested` so the loop drains writes. `None` means the command
/// channel closed.
///
/// 处理传递到连接任务的命令。
///
/// `Core` 命令被输入 `RtspCore` 并刷新其输出。`Close` 设置 `close_requested`，使循环
/// 刷新写入。`None` 表示命令通道已关闭。
async fn handle_connection_command(
    maybe_cmd: Option<ConnectionCommand>,
    core: &mut RtspCore,
    connection_id: RtspConnectionId,
    event_tx: &mpsc::Sender<DriverEvent>,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
    close_requested: &mut bool,
) -> Result<Option<String>, String> {
    match maybe_cmd {
        Some(ConnectionCommand::Core(command)) => {
            let outputs = core
                .handle_input(CoreInput::Command(command))
                .map_err(|err| format!("core command error: {err}"))?;
            flush_outputs(
                outputs,
                connection_id,
                event_tx,
                pending_writes,
                max_write_queue,
            )
            .await?;
            Ok(None)
        }
        Some(ConnectionCommand::Close) => {
            *close_requested = true;
            Ok(None)
        }
        None => Ok(Some("command channel closed".to_string())),
    }
}

/// Handle a result from `AsyncTcpStream::read`.
///
/// EOF forwards `PeerClosed` into the core so it can flush any final response. A successful
/// read is fed into `CoreInput::Bytes`. Errors are treated as fatal read failures.
///
/// 处理 `AsyncTcpStream::read` 的结果。
///
/// EOF 将 `PeerClosed` 输入核心，使其可刷新最终响应。成功读取的字节输入
/// `CoreInput::Bytes`。错误被视为致命的读取失败。
async fn handle_connection_read(
    read_res: Result<usize, std::io::Error>,
    core: &mut RtspCore,
    connection_id: RtspConnectionId,
    event_tx: &mpsc::Sender<DriverEvent>,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
    read_buf: &[u8],
) -> Result<Option<String>, String> {
    match read_res {
        Ok(0) => {
            if let Ok(outputs) = core.handle_input(CoreInput::PeerClosed) {
                let _ = flush_outputs(
                    outputs,
                    connection_id,
                    event_tx,
                    pending_writes,
                    max_write_queue,
                )
                .await;
            }
            Ok(Some("peer closed".to_string()))
        }
        Ok(n) => {
            let bytes = Bytes::copy_from_slice(&read_buf[..n]);
            let outputs = core
                .handle_input(CoreInput::Bytes(bytes))
                .map_err(|err| format!("core read error: {err}"))?;
            flush_outputs(
                outputs,
                connection_id,
                event_tx,
                pending_writes,
                max_write_queue,
            )
            .await?;
            Ok(None)
        }
        Err(err) => Ok(Some(format!("read failed: {err}"))),
    }
}

/// Flush `CoreOutput` values produced by `RtspCore`.
///
/// `Write` outputs are queued to be sent over the transport. `Event` outputs are forwarded
/// as `DriverEvent::Core`. `Close` returns immediately to let the connection loop exit.
///
/// 刷新 `RtspCore` 产生的 `CoreOutput`。
///
/// `Write` 输出被排队并通过传输发送。`Event` 输出作为 `DriverEvent::Core` 转发。
/// `Close` 立即返回，使连接循环退出。
async fn flush_outputs(
    outputs: Vec<CoreOutput>,
    connection_id: RtspConnectionId,
    event_tx: &mpsc::Sender<DriverEvent>,
    pending_writes: &mut VecDeque<Bytes>,
    max_write_queue: usize,
) -> Result<(), String> {
    for output in outputs {
        match output {
            CoreOutput::Write(bytes) => {
                if pending_writes.len() >= max_write_queue {
                    return Err("write queue overflow".to_string());
                }
                pending_writes.push_back(bytes);
            }
            CoreOutput::Event(event) => {
                event_tx
                    .send(DriverEvent::Core {
                        connection_id,
                        event,
                    })
                    .await
                    .map_err(|_| "event channel closed".to_string())?;
            }
            CoreOutput::Close => {
                return Ok(());
            }
        }
    }
    Ok(())
}
