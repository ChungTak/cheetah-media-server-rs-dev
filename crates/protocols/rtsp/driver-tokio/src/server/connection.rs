use std::collections::VecDeque;
use std::time::Duration;

use bytes::Bytes;
use cheetah_rtsp_core::{CoreInput, CoreOutput, RtspCore};
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use super::command::{ConnectionCommand, ConnectionMap};
use super::{DriverConfig, DriverEvent, RtspConnectionId};

/// `ConnectionRuntime` data structure.
/// `ConnectionRuntime` 数据结构.
pub(super) struct ConnectionRuntime {
    /// `event_tx` field.
    /// `event_tx` 字段.
    pub(super) event_tx: mpsc::Sender<DriverEvent>,
    /// `conn_map` field of type `ConnectionMap`.
    /// `conn_map` 字段，类型为 `ConnectionMap`.
    pub(super) conn_map: ConnectionMap,
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub(super) cancel: CancellationToken,
    /// `config` field of type `DriverConfig`.
    /// `config` 字段，类型为 `DriverConfig`.
    pub(super) config: DriverConfig,
}

/// `run_connection` function.
/// `run_connection` 函数.
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

/// `write_pending_bytes_for_test` function.
/// `write_pending_bytes_for_test` 函数.
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
