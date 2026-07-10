use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cheetah_runtime_api::CancellationToken;
use cheetah_srt_core::{SrtKeyLength, SrtSessionOptions};
use shiguredo_srt::{
    ConnectionEvent, ConnectionOptions, ConnectionOutput, SrtConnection, TimerId, Timestamp,
};
use tokio::sync::mpsc;

use crate::config::SrtDriverConfig;

/// `SrtPeerId` data structure.
/// `SrtPeerId` 数据结构.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SrtPeerId(pub u64);

/// `SrtDriverStats` data structure.
/// `SrtDriverStats` 数据结构.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrtDriverStats {
    /// `bytes_in` field of type `u64`.
    /// `bytes_in` 字段，类型为 `u64`.
    pub bytes_in: u64,
    /// `bytes_out` field of type `u64`.
    /// `bytes_out` 字段，类型为 `u64`.
    pub bytes_out: u64,
    /// `packets_in` field of type `u64`.
    /// `packets_in` 字段，类型为 `u64`.
    pub packets_in: u64,
    /// `packets_out` field of type `u64`.
    /// `packets_out` 字段，类型为 `u64`.
    pub packets_out: u64,
    /// `sender_packets_in_buffer` field of type `u32`.
    /// `sender_packets_in_buffer` 字段，类型为 `u32`.
    pub sender_packets_in_buffer: u32,
    /// `sender_packets_in_loss_list` field of type `u32`.
    /// `sender_packets_in_loss_list` 字段，类型为 `u32`.
    pub sender_packets_in_loss_list: u32,
    /// `sender_total_retransmits` field of type `u32`.
    /// `sender_total_retransmits` 字段，类型为 `u32`.
    pub sender_total_retransmits: u32,
    /// `sender_total_sent` field of type `u64`.
    /// `sender_total_sent` 字段，类型为 `u64`.
    pub sender_total_sent: u64,
    /// `sender_total_bytes_sent` field of type `u64`.
    /// `sender_total_bytes_sent` 字段，类型为 `u64`.
    pub sender_total_bytes_sent: u64,
    /// `receiver_packets_in_buffer` field of type `u32`.
    /// `receiver_packets_in_buffer` 字段，类型为 `u32`.
    pub receiver_packets_in_buffer: u32,
    /// `receiver_packets_in_loss_list` field of type `u32`.
    /// `receiver_packets_in_loss_list` 字段，类型为 `u32`.
    pub receiver_packets_in_loss_list: u32,
    /// `receiver_total_received` field of type `u64`.
    /// `receiver_total_received` 字段，类型为 `u64`.
    pub receiver_total_received: u64,
    /// `receiver_total_lost` field of type `u64`.
    /// `receiver_total_lost` 字段，类型为 `u64`.
    pub receiver_total_lost: u64,
    /// `receiver_total_duplicates` field of type `u64`.
    /// `receiver_total_duplicates` 字段，类型为 `u64`.
    pub receiver_total_duplicates: u64,
    /// `receiver_total_bytes_received` field of type `u64`.
    /// `receiver_total_bytes_received` 字段，类型为 `u64`.
    pub receiver_total_bytes_received: u64,
    /// `receiver_rtt_micros` field of type `u32`.
    /// `receiver_rtt_micros` 字段，类型为 `u32`.
    pub receiver_rtt_micros: u32,
    /// `receiver_rtt_var_micros` field of type `u32`.
    /// `receiver_rtt_var_micros` 字段，类型为 `u32`.
    pub receiver_rtt_var_micros: u32,
    /// `receiver_loss_rate_percent_x100` field of type `u32`.
    /// `receiver_loss_rate_percent_x100` 字段，类型为 `u32`.
    pub receiver_loss_rate_percent_x100: u32,
    /// `receiver_jitter_micros` field of type `u32`.
    /// `receiver_jitter_micros` 字段，类型为 `u32`.
    pub receiver_jitter_micros: u32,
}

/// `SrtDriverCommand` enumeration.
/// `SrtDriverCommand` 枚举.
#[derive(Debug, Clone)]
pub enum SrtDriverCommand {
    /// `ConnectCaller` variant.
    /// `ConnectCaller` 变体.
    ConnectCaller {
        peer_id: SrtPeerId,
        remote: SocketAddr,
        stream_id: Option<String>,
        options: SrtSessionOptions,
    },
    /// `SendPayload` variant.
    /// `SendPayload` 变体.
    SendPayload { peer_id: SrtPeerId, payload: Bytes },
    /// `Close` variant.
    /// `Close` 变体.
    Close { peer_id: SrtPeerId, reason: String },
}

/// `SrtDriverEvent` enumeration.
/// `SrtDriverEvent` 枚举.
#[derive(Debug, Clone)]
pub enum SrtDriverEvent {
    /// `ListenerStarted` variant.
    /// `ListenerStarted` 变体.
    ListenerStarted { local_addr: SocketAddr },
    /// `CallerConnecting` variant.
    /// `CallerConnecting` 变体.
    CallerConnecting {
        peer_id: SrtPeerId,
        remote: SocketAddr,
    },
    /// `Connected` variant.
    /// `Connected` 变体.
    Connected {
        peer_id: SrtPeerId,
        remote: SocketAddr,
        stream_id: Option<String>,
    },
    /// `Payload` variant.
    /// `Payload` 变体.
    Payload { peer_id: SrtPeerId, payload: Bytes },
    /// `KeyRefreshNeeded` variant.
    /// `KeyRefreshNeeded` 变体.
    KeyRefreshNeeded { peer_id: SrtPeerId },
    /// `Stats` variant.
    /// `Stats` 变体.
    Stats {
        peer_id: SrtPeerId,
        stats: SrtDriverStats,
    },
    /// `Disconnected` variant.
    /// `Disconnected` 变体.
    Disconnected { peer_id: SrtPeerId, reason: String },
    /// `Error` variant.
    /// `Error` 变体.
    Error {
        peer_id: Option<SrtPeerId>,
        message: String,
    },
}

/// `SrtDriverHandle` data structure.
/// `SrtDriverHandle` 数据结构.
#[derive(Clone)]
pub struct SrtDriverHandle {
    /// `command_tx` field.
    /// `command_tx` 字段.
    command_tx: mpsc::Sender<SrtDriverCommand>,
}

impl SrtDriverHandle {
    /// `send` function.
    /// `send` 函数.
    pub async fn send(&self, command: SrtDriverCommand) {
        let _ = self.command_tx.send(command).await;
    }
}

/// `spawn_driver` function.
/// `spawn_driver` 函数.
pub fn spawn_driver(
    config: SrtDriverConfig,
    cancel: CancellationToken,
) -> (SrtDriverHandle, mpsc::Receiver<SrtDriverEvent>) {
    let (command_tx, command_rx) = mpsc::channel(256);
    let (event_tx, event_rx) = mpsc::channel(256);
    tokio::spawn(run_driver(config, command_rx, event_tx, cancel));
    (SrtDriverHandle { command_tx }, event_rx)
}

struct ConnectionSlot {
    peer_id: SrtPeerId,
    remote: SocketAddr,
    connection: SrtConnection,
    timers: HashMap<TimerId, Instant>,
    next_stats_at: Option<Instant>,
    connect_deadline: Option<Instant>,
    connected: bool,
    last_activity: Instant,
    stream_id: Option<String>,
    stats: SrtDriverStats,
}

async fn run_driver(
    config: SrtDriverConfig,
    mut command_rx: mpsc::Receiver<SrtDriverCommand>,
    event_tx: mpsc::Sender<SrtDriverEvent>,
    cancel: CancellationToken,
) {
    if config.encryption.enabled && config.encryption.passphrase.is_empty() {
        let _ = event_tx
            .send(SrtDriverEvent::Error {
                peer_id: None,
                message: "SRT encryption passphrase must not be empty".to_string(),
            })
            .await;
        return;
    }

    let socket = match tokio::net::UdpSocket::bind(config.listen).await {
        Ok(socket) => socket,
        Err(err) => {
            let _ = event_tx
                .send(SrtDriverEvent::Error {
                    peer_id: None,
                    message: format!("bind {} failed: {err}", config.listen),
                })
                .await;
            return;
        }
    };
    let local_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            let _ = event_tx
                .send(SrtDriverEvent::Error {
                    peer_id: None,
                    message: format!("local_addr failed: {err}"),
                })
                .await;
            return;
        }
    };
    let _ = event_tx
        .send(SrtDriverEvent::ListenerStarted { local_addr })
        .await;

    let start = Instant::now();
    let mut next_listener_peer_id = 1_u64;
    let mut by_peer: HashMap<SrtPeerId, ConnectionSlot> = HashMap::new();
    let mut by_remote: HashMap<SocketAddr, SrtPeerId> = HashMap::new();
    let mut recv_buf = vec![0_u8; 2048.max(config.recv_buffer_packets.min(8192))];

    loop {
        let next_deadline = nearest_deadline(&by_peer, &config);
        tokio::select! {
            _ = cancel.cancelled() => break,
            recv = socket.recv_from(&mut recv_buf) => {
                let Ok((len, remote)) = recv else { continue };
                let recv_at = Instant::now();
                let now = timestamp(start);
                let peer_id = if let Some(peer_id) = by_remote.get(&remote).copied() {
                    peer_id
                } else {
                    if by_peer.len() >= config.max_connections {
                        let _ = event_tx.send(SrtDriverEvent::Error {
                            peer_id: None,
                            message: "SRT max_connections reached".to_string(),
                        }).await;
                        continue;
                    }
                    let peer_id = SrtPeerId(next_listener_peer_id);
                    next_listener_peer_id += 1;
                    let mut connection = SrtConnection::new_listener(connection_options(
                        peer_id,
                        None,
                        &config,
                    ));
                    let _ = connection.feed_recv_buf(&recv_buf[..len], now);
                    let mut slot = ConnectionSlot {
                        peer_id,
                        remote,
                        connection,
                        timers: HashMap::new(),
                        next_stats_at: next_stats_deadline(&config),
                        connect_deadline: None,
                        connected: false,
                        last_activity: recv_at,
                        stream_id: None,
                        stats: SrtDriverStats {
                            bytes_in: len as u64,
                            packets_in: 1,
                            ..Default::default()
                        },
                    };
                    drain_slot_outputs(&socket, &event_tx, &mut slot, start).await;
                    if !drain_slot_events(&event_tx, &mut slot).await {
                        by_remote.insert(remote, peer_id);
                        by_peer.insert(peer_id, slot);
                    }
                    continue;
                };
                let mut disconnected = false;
                if let Some(slot) = by_peer.get_mut(&peer_id) {
                    slot.last_activity = recv_at;
                    slot.stats.bytes_in += len as u64;
                    slot.stats.packets_in += 1;
                    if let Err(err) = slot.connection.feed_recv_buf(&recv_buf[..len], now) {
                        let _ = event_tx.send(SrtDriverEvent::Error {
                            peer_id: Some(peer_id),
                            message: err.to_string(),
                        }).await;
                    }
                    drain_slot_outputs(&socket, &event_tx, slot, start).await;
                    disconnected = drain_slot_events(&event_tx, slot).await;
                }
                if disconnected {
                    remove_slot(&mut by_peer, &mut by_remote, peer_id);
                }
            }
            Some(command) = command_rx.recv() => {
                handle_command(&socket, &event_tx, &mut by_peer, &mut by_remote, command, &config, start).await;
            }
            _ = sleep_until_optional(next_deadline), if next_deadline.is_some() => {
                let now_instant = Instant::now();
                let now = timestamp(start);
                let due: Vec<(SrtPeerId, TimerId)> = by_peer
                    .iter()
                    .flat_map(|(peer_id, slot)| {
                        slot.timers
                            .iter()
                            .filter_map(move |(timer_id, deadline)| (*deadline <= now_instant).then_some((*peer_id, *timer_id)))
                    })
                    .collect();
                for (peer_id, timer_id) in due {
                    let mut disconnected = false;
                    if let Some(slot) = by_peer.get_mut(&peer_id) {
                        slot.timers.remove(&timer_id);
                        if let Err(err) = slot.connection.handle_timer(timer_id, now) {
                            let _ = event_tx.send(SrtDriverEvent::Error {
                                peer_id: Some(peer_id),
                                message: err.to_string(),
                            }).await;
                        }
                        drain_slot_outputs(&socket, &event_tx, slot, start).await;
                        disconnected = drain_slot_events(&event_tx, slot).await;
                    }
                    if disconnected {
                        remove_slot(&mut by_peer, &mut by_remote, peer_id);
                    }
                }
                emit_due_stats(&event_tx, &mut by_peer, &config, now_instant).await;
                disconnect_connect_timeouts(&socket, &event_tx, &mut by_peer, &mut by_remote, now_instant, start).await;
                disconnect_idle_slots(&socket, &event_tx, &mut by_peer, &mut by_remote, &config, now_instant, start).await;
            }
        }
    }
}

async fn handle_command(
    socket: &tokio::net::UdpSocket,
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    by_peer: &mut HashMap<SrtPeerId, ConnectionSlot>,
    by_remote: &mut HashMap<SocketAddr, SrtPeerId>,
    command: SrtDriverCommand,
    config: &SrtDriverConfig,
    start: Instant,
) {
    match command {
        SrtDriverCommand::ConnectCaller {
            peer_id,
            remote,
            stream_id,
            options,
        } => {
            if !by_peer.contains_key(&peer_id) && by_peer.len() >= config.max_connections {
                let _ = event_tx
                    .send(SrtDriverEvent::Error {
                        peer_id: Some(peer_id),
                        message: "SRT max_connections reached".to_string(),
                    })
                    .await;
                return;
            }
            let now = timestamp(start);
            if options.encryption.enabled && options.encryption.passphrase.is_empty() {
                let _ = event_tx
                    .send(SrtDriverEvent::Error {
                        peer_id: Some(peer_id),
                        message: "SRT encryption passphrase must not be empty".to_string(),
                    })
                    .await;
                return;
            }
            let mut connection = SrtConnection::new_caller(caller_connection_options(
                peer_id,
                stream_id.clone(),
                &options,
                config,
            ));
            let _ = event_tx
                .send(SrtDriverEvent::CallerConnecting { peer_id, remote })
                .await;
            if let Err(err) = connection.connect(now) {
                let _ = event_tx
                    .send(SrtDriverEvent::Error {
                        peer_id: Some(peer_id),
                        message: err.to_string(),
                    })
                    .await;
                return;
            }
            let mut slot = ConnectionSlot {
                peer_id,
                remote,
                connection,
                timers: HashMap::new(),
                next_stats_at: next_stats_deadline(config),
                connect_deadline: connect_deadline(config),
                connected: false,
                last_activity: Instant::now(),
                stream_id,
                stats: SrtDriverStats::default(),
            };
            drain_slot_outputs(socket, event_tx, &mut slot, start).await;
            if !drain_slot_events(event_tx, &mut slot).await {
                by_remote.insert(remote, peer_id);
                by_peer.insert(peer_id, slot);
            }
        }
        SrtDriverCommand::SendPayload { peer_id, payload } => {
            let mut disconnected = false;
            if let Some(slot) = by_peer.get_mut(&peer_id) {
                if is_send_queue_full(slot, config) {
                    let _ = event_tx
                        .send(SrtDriverEvent::Error {
                            peer_id: Some(peer_id),
                            message: "SRT send queue full".to_string(),
                        })
                        .await;
                    return;
                }
                let now = timestamp(start);
                if let Err(err) = slot.connection.send(&payload, now) {
                    let _ = event_tx
                        .send(SrtDriverEvent::Error {
                            peer_id: Some(peer_id),
                            message: err.to_string(),
                        })
                        .await;
                }
                drain_slot_outputs(socket, event_tx, slot, start).await;
                disconnected = drain_slot_events(event_tx, slot).await;
            }
            if disconnected {
                remove_slot(by_peer, by_remote, peer_id);
            }
        }
        SrtDriverCommand::Close { peer_id, reason } => {
            if let Some(mut slot) = by_peer.remove(&peer_id) {
                by_remote.remove(&slot.remote);
                slot.connection.disconnect(timestamp(start));
                drain_slot_outputs(socket, event_tx, &mut slot, start).await;
                let _ = event_tx
                    .send(SrtDriverEvent::Disconnected { peer_id, reason })
                    .await;
            }
        }
    }
}

fn remove_slot(
    by_peer: &mut HashMap<SrtPeerId, ConnectionSlot>,
    by_remote: &mut HashMap<SocketAddr, SrtPeerId>,
    peer_id: SrtPeerId,
) {
    if let Some(slot) = by_peer.remove(&peer_id) {
        by_remote.remove(&slot.remote);
    }
}

fn is_send_queue_full(slot: &ConnectionSlot, config: &SrtDriverConfig) -> bool {
    slot.connection
        .sender_stats()
        .map(|stats| stats.packets_in_buffer as usize >= config.send_queue_capacity)
        .unwrap_or(config.send_queue_capacity == 0)
}

fn connection_options(
    peer_id: SrtPeerId,
    stream_id: Option<String>,
    config: &SrtDriverConfig,
) -> ConnectionOptions {
    let key_length = match config.encryption.key_length {
        SrtKeyLength::Aes128 => shiguredo_srt::KeyLength::Aes128,
        SrtKeyLength::Aes256 => shiguredo_srt::KeyLength::Aes256,
    };
    ConnectionOptions {
        socket_id: (0xC000_0000_u32).wrapping_add(peer_id.0 as u32),
        initial_seq: Some(1 + peer_id.0 as u32),
        syn_cookie: Some(0x5A17_0000_u32.wrapping_add(peer_id.0 as u32)),
        passphrase: config
            .encryption
            .enabled
            .then(|| config.encryption.passphrase.clone()),
        key_length,
        tsbpd_delay: config.latency_ms.min(u16::MAX as u64) as u16,
        stream_id,
        ..Default::default()
    }
}

fn caller_connection_options(
    peer_id: SrtPeerId,
    stream_id: Option<String>,
    options: &SrtSessionOptions,
    config: &SrtDriverConfig,
) -> ConnectionOptions {
    let encryption_enabled = options.encryption.enabled || config.encryption.enabled;
    let passphrase = if options.encryption.enabled {
        options.encryption.passphrase.clone()
    } else {
        config.encryption.passphrase.clone()
    };
    let key_length = if options.encryption.enabled {
        options.encryption.key_length
    } else {
        config.encryption.key_length
    };
    let key_length = match key_length {
        SrtKeyLength::Aes128 => shiguredo_srt::KeyLength::Aes128,
        SrtKeyLength::Aes256 => shiguredo_srt::KeyLength::Aes256,
    };
    ConnectionOptions {
        socket_id: (0xC000_0000_u32).wrapping_add(peer_id.0 as u32),
        initial_seq: Some(1 + peer_id.0 as u32),
        syn_cookie: Some(0x5A17_0000_u32.wrapping_add(peer_id.0 as u32)),
        passphrase: encryption_enabled.then_some(passphrase),
        key_length,
        tsbpd_delay: options.latency_ms.min(u16::MAX as u64) as u16,
        stream_id,
        ..Default::default()
    }
}

async fn drain_slot_outputs(
    socket: &tokio::net::UdpSocket,
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    slot: &mut ConnectionSlot,
    start: Instant,
) {
    while let Some(output) = slot.connection.poll_output() {
        match output {
            ConnectionOutput::SendPacket(packet) => {
                match socket.send_to(&packet, slot.remote).await {
                    Ok(sent) => {
                        slot.stats.bytes_out += sent as u64;
                        slot.stats.packets_out += 1;
                    }
                    Err(err) => {
                        let _ = event_tx
                            .send(SrtDriverEvent::Error {
                                peer_id: Some(slot.peer_id),
                                message: err.to_string(),
                            })
                            .await;
                    }
                }
            }
            ConnectionOutput::SetTimer {
                id,
                duration_micros,
            } => {
                slot.timers.insert(
                    id,
                    Instant::now() + Duration::from_micros(duration_micros.max(1)),
                );
            }
            ConnectionOutput::ClearTimer { id } => {
                slot.timers.remove(&id);
            }
        }
    }

    let _ = start;
}

async fn drain_slot_events(
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    slot: &mut ConnectionSlot,
) -> bool {
    let mut disconnected = false;
    while let Some(event) = slot.connection.poll_event() {
        match event {
            ConnectionEvent::Connected => {
                slot.connected = true;
                slot.connect_deadline = None;
                if slot.stream_id.is_none() {
                    slot.stream_id = slot.connection.peer_stream_id().map(ToOwned::to_owned);
                }
                let _ = event_tx
                    .send(SrtDriverEvent::Connected {
                        peer_id: slot.peer_id,
                        remote: slot.remote,
                        stream_id: slot.stream_id.clone(),
                    })
                    .await;
            }
            ConnectionEvent::DataReceived { payload, .. } => {
                let _ = event_tx
                    .send(SrtDriverEvent::Payload {
                        peer_id: slot.peer_id,
                        payload: Bytes::from(payload),
                    })
                    .await;
            }
            ConnectionEvent::StateChanged(_) => {}
            ConnectionEvent::Error(message) => {
                let _ = event_tx
                    .send(SrtDriverEvent::Error {
                        peer_id: Some(slot.peer_id),
                        message,
                    })
                    .await;
            }
            ConnectionEvent::Disconnected { reason } => {
                disconnected = true;
                let _ = event_tx
                    .send(SrtDriverEvent::Disconnected {
                        peer_id: slot.peer_id,
                        reason,
                    })
                    .await;
            }
            ConnectionEvent::KeyRefreshNeeded { .. } => {
                let _ = event_tx
                    .send(SrtDriverEvent::KeyRefreshNeeded {
                        peer_id: slot.peer_id,
                    })
                    .await;
            }
        }
    }
    disconnected
}

fn nearest_deadline(
    by_peer: &HashMap<SrtPeerId, ConnectionSlot>,
    config: &SrtDriverConfig,
) -> Option<Instant> {
    by_peer
        .values()
        .flat_map(|slot| {
            slot.timers
                .values()
                .copied()
                .chain(slot.next_stats_at)
                .chain(slot.connect_deadline)
                .chain(idle_deadline(slot, config))
        })
        .min()
}

fn next_stats_deadline(config: &SrtDriverConfig) -> Option<Instant> {
    (config.stats_interval_ms > 0)
        .then(|| Instant::now() + Duration::from_millis(config.stats_interval_ms))
}

fn connect_deadline(config: &SrtDriverConfig) -> Option<Instant> {
    (config.connect_timeout_ms > 0)
        .then(|| Instant::now() + Duration::from_millis(config.connect_timeout_ms))
}

async fn emit_due_stats(
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    by_peer: &mut HashMap<SrtPeerId, ConnectionSlot>,
    config: &SrtDriverConfig,
    now: Instant,
) {
    let interval = Duration::from_millis(config.stats_interval_ms.max(1));
    for slot in by_peer.values_mut() {
        let Some(deadline) = slot.next_stats_at else {
            continue;
        };
        if deadline > now {
            continue;
        }
        refresh_slot_stats(slot);
        match event_tx.try_send(SrtDriverEvent::Stats {
            peer_id: slot.peer_id,
            stats: slot.stats.clone(),
        }) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
        slot.next_stats_at = Some(now + interval);
    }
}

fn refresh_slot_stats(slot: &mut ConnectionSlot) {
    if let Some(sender) = slot.connection.sender_stats() {
        slot.stats.sender_packets_in_buffer = sender.packets_in_buffer;
        slot.stats.sender_packets_in_loss_list = sender.packets_in_loss_list;
        slot.stats.sender_total_retransmits = sender.total_retransmits;
        slot.stats.sender_total_sent = sender.total_sent;
        slot.stats.sender_total_bytes_sent = sender.total_bytes_sent;
    }
    if let Some(receiver) = slot.connection.receiver_stats() {
        slot.stats.receiver_packets_in_buffer = receiver.packets_in_buffer;
        slot.stats.receiver_packets_in_loss_list = receiver.packets_in_loss_list;
        slot.stats.receiver_total_received = receiver.total_received;
        slot.stats.receiver_total_lost = receiver.total_lost;
        slot.stats.receiver_total_duplicates = receiver.total_duplicates;
        slot.stats.receiver_total_bytes_received = receiver.total_bytes_received;
        slot.stats.receiver_rtt_micros = receiver.rtt;
        slot.stats.receiver_rtt_var_micros = receiver.rtt_var;
        slot.stats.receiver_loss_rate_percent_x100 = receiver.loss_rate_percent_x100;
        slot.stats.receiver_jitter_micros = receiver.jitter;
    }
}

fn idle_deadline(slot: &ConnectionSlot, config: &SrtDriverConfig) -> Option<Instant> {
    (config.idle_timeout_ms > 0)
        .then(|| slot.last_activity + Duration::from_millis(config.idle_timeout_ms))
}

async fn disconnect_idle_slots(
    socket: &tokio::net::UdpSocket,
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    by_peer: &mut HashMap<SrtPeerId, ConnectionSlot>,
    by_remote: &mut HashMap<SocketAddr, SrtPeerId>,
    config: &SrtDriverConfig,
    now: Instant,
    start: Instant,
) {
    let due: Vec<SrtPeerId> = by_peer
        .iter()
        .filter_map(|(peer_id, slot)| {
            idle_deadline(slot, config).and_then(|deadline| (deadline <= now).then_some(*peer_id))
        })
        .collect();

    for peer_id in due {
        if let Some(mut slot) = by_peer.remove(&peer_id) {
            by_remote.remove(&slot.remote);
            slot.connection.disconnect(timestamp(start));
            drain_slot_outputs(socket, event_tx, &mut slot, start).await;
            let _ = event_tx
                .send(SrtDriverEvent::Disconnected {
                    peer_id,
                    reason: "idle timeout".to_string(),
                })
                .await;
        }
    }
}

async fn disconnect_connect_timeouts(
    socket: &tokio::net::UdpSocket,
    event_tx: &mpsc::Sender<SrtDriverEvent>,
    by_peer: &mut HashMap<SrtPeerId, ConnectionSlot>,
    by_remote: &mut HashMap<SocketAddr, SrtPeerId>,
    now: Instant,
    start: Instant,
) {
    let due: Vec<SrtPeerId> = by_peer
        .iter()
        .filter_map(|(peer_id, slot)| {
            (!slot.connected)
                .then_some(slot.connect_deadline)
                .flatten()
                .and_then(|deadline| (deadline <= now).then_some(*peer_id))
        })
        .collect();

    for peer_id in due {
        if let Some(mut slot) = by_peer.remove(&peer_id) {
            by_remote.remove(&slot.remote);
            slot.connection.disconnect(timestamp(start));
            drain_slot_outputs(socket, event_tx, &mut slot, start).await;
            let _ = event_tx
                .send(SrtDriverEvent::Disconnected {
                    peer_id,
                    reason: "connect timeout".to_string(),
                })
                .await;
        }
    }
}

async fn sleep_until_optional(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
    }
}

fn timestamp(start: Instant) -> Timestamp {
    Timestamp::from_micros(start.elapsed().as_micros() as u64)
}
