use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cheetah_codec::MonoTime;
use cheetah_runtime_api::{AsyncTcpStream, CancellationToken, RuntimeApi};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

use super::command::{
    handle_driver_command, ConnectionCommand, ConnectionHandle, ConnectionMap,
    RtspCoreCommandSender,
};
use super::connection::{run_connection, ConnectionRuntime};
use super::http_tunnel::{
    build_http_tunnel_get_ok_response, build_http_tunnel_post_ok_response,
    looks_like_http_tunnel_candidate, probe_http_tunnel_open_request, run_http_tunnel_connection,
    HttpTunnelMethod, HttpTunnelParseResult, HttpTunnelProbeResult, HttpTunnelRegistry,
    HttpTunnelRegistryConfig, PendingPair,
};
use super::{DriverConfig, DriverEvent, RtspServerHandle};

const INITIAL_READ_TIMEOUT: Duration = Duration::from_millis(20);
const TUNNEL_PROBE_TIMEOUT_AFTER_INITIAL_TIMEOUT: Duration = Duration::from_millis(250);

/// Starts the `server`.
/// 启动 `server`。
pub fn start_server(
    runtime_api: Arc<dyn RuntimeApi>,
    listen: SocketAddr,
    config: DriverConfig,
    cancel: CancellationToken,
) -> io::Result<RtspServerHandle> {
    let listener = runtime_api.bind_tcp(listen)?;

    let (event_tx, event_rx) = mpsc::channel(config.event_queue_capacity.max(64));
    let (cmd_tx, mut cmd_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let command_sender = RtspCoreCommandSender::new(cmd_tx.clone());

    let conn_map: ConnectionMap = Arc::new(Mutex::new(HashMap::new()));
    let conn_ids = Arc::new(AtomicU64::new(1));
    let tunnel_registry = Arc::new(Mutex::new(HttpTunnelRegistry::new(
        HttpTunnelRegistryConfig::from_driver_config(&config),
    )));
    let mut tunnel_gc_tick = tokio::time::interval(Duration::from_millis(250));

    let join_cancel = cancel.clone();
    let join = runtime_api.spawn(Box::pin({
        let conn_map = conn_map.clone();
        let conn_ids = conn_ids.clone();
        let tunnel_registry = tunnel_registry.clone();
        let runtime_api = runtime_api.clone();
        let config = config.clone();
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
                        if handle_driver_command(cmd, &conn_map, &join_cancel).await {
                            break;
                        }
                    }
                    _ = tunnel_gc_tick.tick() => {
                        let now_micros = runtime_api.now().as_micros();
                        let expired = tunnel_registry.lock().drain_expired(now_micros);
                        for (get, post) in expired {
                            if let Some(mut get_half) = get {
                                let _ = get_half.stream.shutdown().await;
                            }
                            if let Some(mut post_half) = post {
                                let _ = post_half.stream.shutdown().await;
                            }
                        }
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((mut stream, peer)) => {
                                let initial_read = match read_initial_bytes(
                                    stream.as_mut(),
                                    &join_cancel,
                                    config.read_buffer_size.max(1024),
                                ).await {
                                    Ok(initial_read) => initial_read,
                                    Err(reason) => {
                                        let _ = stream.shutdown().await;
                                        warn!(%reason, %peer, "read initial bytes failed");
                                        continue;
                                    }
                                };
                                let should_probe = (!initial_read.bytes.is_empty()
                                    && looks_like_http_tunnel_candidate(&initial_read.bytes))
                                    || initial_read.timed_out;
                                if should_probe {
                                    let event_tx = event_tx.clone();
                                    let conn_map = conn_map.clone();
                                    let join_cancel = join_cancel.clone();
                                    let config = config.clone();
                                    let runtime_api = runtime_api.clone();
                                    let runtime_api_for_task = runtime_api.clone();
                                    let conn_ids = conn_ids.clone();
                                    let tunnel_registry = tunnel_registry.clone();
                                    let _ = runtime_api.spawn(Box::pin(async move {
                                        handle_tunnel_probe_connection(
                                            stream,
                                            peer,
                                            initial_read,
                                            event_tx,
                                            conn_map,
                                            join_cancel,
                                            config,
                                            runtime_api_for_task,
                                            conn_ids,
                                            tunnel_registry,
                                        )
                                        .await;
                                    }));
                                    continue;
                                }

                                if spawn_direct_connection(
                                    connection_id_alloc(&conn_ids),
                                    peer,
                                    stream,
                                    initial_read.bytes,
                                    &event_tx,
                                    &conn_map,
                                    &join_cancel,
                                    &config,
                                    &runtime_api,
                                )
                                .await
                                {
                                    break;
                                }
                            }
                            Err(err) => {
                                warn!(%err, "rtsp listener accept failed");
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

            let connections: Vec<ConnectionHandle> = conn_map.lock().values().cloned().collect();
            for connection in connections {
                connection.cancel.cancel();
                let _ = connection.tx.try_send(ConnectionCommand::Close);
            }
        }
    }));

    Ok(RtspServerHandle {
        events_rx: event_rx,
        cmd_tx: command_sender,
        cancel,
        join,
    })
}

fn connection_id_alloc(conn_ids: &AtomicU64) -> u64 {
    conn_ids.fetch_add(1, Ordering::Relaxed)
}

async fn read_initial_bytes(
    stream: &mut dyn AsyncTcpStream,
    cancel: &CancellationToken,
    read_buffer_size: usize,
) -> Result<InitialRead, String> {
    let mut buf = vec![0u8; read_buffer_size];
    tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".to_string()),
        read_res = tokio::time::timeout(INITIAL_READ_TIMEOUT, stream.read(&mut buf)) => {
            match read_res {
                Ok(Ok(n)) => {
                    if n == 0 {
                        return Ok(InitialRead {
                            bytes: Bytes::new(),
                            timed_out: false,
                        });
                    }
                    Ok(InitialRead {
                        bytes: Bytes::copy_from_slice(&buf[..n]),
                        timed_out: false,
                    })
                }
                Ok(Err(err)) => Err(format!("read failed: {err}")),
                Err(_) => Ok(InitialRead {
                    bytes: Bytes::new(),
                    timed_out: true,
                }),
            }
        }
    }
}

struct InitialRead {
    bytes: Bytes,
    timed_out: bool,
}

#[allow(clippy::too_many_arguments)]
async fn handle_tunnel_probe_connection(
    mut stream: Box<dyn AsyncTcpStream>,
    peer: SocketAddr,
    initial_read: InitialRead,
    event_tx: mpsc::Sender<DriverEvent>,
    conn_map: ConnectionMap,
    join_cancel: CancellationToken,
    config: DriverConfig,
    runtime_api: Arc<dyn RuntimeApi>,
    conn_ids: Arc<AtomicU64>,
    tunnel_registry: Arc<Mutex<HttpTunnelRegistry>>,
) {
    let probe_seed = if initial_read.timed_out {
        Bytes::new()
    } else {
        initial_read.bytes
    };
    let parse_res = probe_http_tunnel_open_request(
        &mut stream,
        probe_seed,
        TUNNEL_PROBE_TIMEOUT_AFTER_INITIAL_TIMEOUT,
    )
    .await;

    match parse_res {
        HttpTunnelProbeResult::Parsed(Ok(HttpTunnelParseResult::Tunnel(open_req))) => {
            let now_micros = runtime_api.now().as_micros();
            let pair = match open_req.method {
                HttpTunnelMethod::Get => {
                    if stream
                        .write_all(&build_http_tunnel_get_ok_response())
                        .await
                        .is_err()
                    {
                        let _ = stream.shutdown().await;
                        return;
                    }
                    let upsert = {
                        tunnel_registry.lock().upsert_get(
                            open_req.cookie,
                            stream,
                            open_req.path,
                            now_micros,
                        )
                    };
                    match upsert {
                        Ok(pair) => pair,
                        Err(_) => return,
                    }
                }
                HttpTunnelMethod::Post => {
                    if stream
                        .write_all(&build_http_tunnel_post_ok_response())
                        .await
                        .is_err()
                    {
                        let _ = stream.shutdown().await;
                        return;
                    }
                    let upsert = {
                        tunnel_registry.lock().upsert_post(
                            open_req.cookie,
                            stream,
                            peer,
                            open_req.path,
                            open_req.initial_post_body,
                            now_micros,
                        )
                    };
                    match upsert {
                        Ok(pair) => pair,
                        Err(_) => return,
                    }
                }
            };

            if let Some(pair) = pair {
                let decoder_limits = {
                    let registry = tunnel_registry.lock();
                    (
                        registry.config().max_base64_buffer_bytes,
                        registry.config().max_decoded_chunk_bytes,
                    )
                };
                if spawn_http_tunnel_connection(
                    connection_id_alloc(conn_ids.as_ref()),
                    pair,
                    &event_tx,
                    &conn_map,
                    &join_cancel,
                    &config,
                    &runtime_api,
                    decoder_limits,
                )
                .await
                {
                    join_cancel.cancel();
                }
            }
        }
        HttpTunnelProbeResult::Parsed(Ok(HttpTunnelParseResult::NotTunnel(initial_bytes)))
        | HttpTunnelProbeResult::TimedOut(initial_bytes) => {
            if spawn_direct_connection(
                connection_id_alloc(conn_ids.as_ref()),
                peer,
                stream,
                initial_bytes,
                &event_tx,
                &conn_map,
                &join_cancel,
                &config,
                &runtime_api,
            )
            .await
            {
                join_cancel.cancel();
            }
        }
        HttpTunnelProbeResult::Parsed(Err(_)) => {
            let _ = stream
                .write_all(b"HTTP/1.0 400 Bad Request\r\nConnection: close\r\n\r\n")
                .await;
            let _ = stream.shutdown().await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_http_tunnel_connection(
    connection_id: u64,
    mut pair: PendingPair,
    event_tx: &mpsc::Sender<DriverEvent>,
    conn_map: &ConnectionMap,
    join_cancel: &CancellationToken,
    config: &DriverConfig,
    runtime_api: &Arc<dyn RuntimeApi>,
    decoder_limits: (usize, usize),
) -> bool {
    let (conn_tx, conn_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let child_cancel = join_cancel.child_token();
    let _cookie = &pair.cookie;
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: conn_tx,
            cancel: child_cancel.clone(),
        },
    );

    if event_tx
        .send(DriverEvent::ConnectionOpened {
            connection_id,
            peer: Some(pair.post.peer),
        })
        .await
        .is_err()
    {
        conn_map.lock().remove(&connection_id);
        child_cancel.cancel();
        let _ = pair.get.stream.shutdown().await;
        let _ = pair.post.stream.shutdown().await;
        return true;
    }

    let runtime = ConnectionRuntime {
        event_tx: event_tx.clone(),
        conn_map: conn_map.clone(),
        cancel: child_cancel,
        config: config.clone(),
    };
    let _ = runtime_api.spawn(Box::pin(async move {
        run_http_tunnel_connection(
            connection_id,
            pair.get.stream,
            pair.post.stream,
            pair.post.initial_body,
            conn_rx,
            runtime,
            decoder_limits,
        )
        .await;
    }));
    false
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_direct_connection(
    connection_id: u64,
    peer: SocketAddr,
    mut stream: Box<dyn AsyncTcpStream>,
    initial_bytes: Bytes,
    event_tx: &mpsc::Sender<DriverEvent>,
    conn_map: &ConnectionMap,
    join_cancel: &CancellationToken,
    config: &DriverConfig,
    runtime_api: &Arc<dyn RuntimeApi>,
) -> bool {
    let (conn_tx, conn_rx) = mpsc::channel(config.command_queue_capacity.max(64));
    let child_cancel = join_cancel.child_token();
    conn_map.lock().insert(
        connection_id,
        ConnectionHandle {
            tx: conn_tx,
            cancel: child_cancel.clone(),
        },
    );

    if event_tx
        .send(DriverEvent::ConnectionOpened {
            connection_id,
            peer: Some(peer),
        })
        .await
        .is_err()
    {
        conn_map.lock().remove(&connection_id);
        child_cancel.cancel();
        let _ = stream.shutdown().await;
        return true;
    }

    let runtime = ConnectionRuntime {
        event_tx: event_tx.clone(),
        conn_map: conn_map.clone(),
        cancel: child_cancel,
        config: config.clone(),
    };
    let _ = runtime_api.spawn(Box::pin(async move {
        run_connection(connection_id, stream, initial_bytes, conn_rx, runtime).await;
    }));
    false
}
