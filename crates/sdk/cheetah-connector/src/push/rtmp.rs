//! RTMP push-side adapter implementing [`cheetah_sdk::PublisherSink`].
//!
//! Bridges the synchronous `PublisherSink` API with the async `RtmpClient` driver
//! by running a background task that forwards `RtmpCoreCommand`s produced from
//! `AVFrame` values and `TrackInfo` sequence headers.
//!
//! RTMP push 端适配器，实现 [`cheetah_sdk::PublisherSink`]。
//!
//! 通过后台任务将 `AVFrame` 与 `TrackInfo` 序列头生成的 `RtmpCoreCommand`
//! 转发给异步 `RtmpClient` 驱动。

use std::collections::VecDeque;
use std::sync::Arc;

use cheetah_codec::{
    build_track_bootstrap_payloads, map_frame_to_rtmp_flv_payload, AVFrame, RtmpFlvPayload,
    RtmpFlvPayloadKind, RtmpFlvPlayMode, TrackInfo,
};
use cheetah_rtmp_core::{
    RtmpClientState, RtmpCoreCommand, RtmpEvent, RtmpMessageStreamId, RtmpUrl,
};
use cheetah_rtmp_driver_tokio::{
    start_client, ClientDriverEvent, ClientSendError, RtmpClientCommandSender,
    RtmpClientDriverCommand, RtmpClientDriverConfig, RtmpClientHandle, RtmpClientMode,
};
use cheetah_runtime_api::{CancellationToken, JoinHandle};
use cheetah_sdk::{DispatchResult, PublisherSink, SdkError};
use futures::channel::mpsc;
use futures::future::{BoxFuture, Fuse, FusedFuture, OptionFuture};
use futures::{select_biased, FutureExt, StreamExt};
use parking_lot::Mutex;

use crate::error::{ConnectorError, Operation};
use crate::handles::{map_sdk_error, PushHandle};
use crate::options::{ConnectorPushOptions, ProtocolPushExtras, RtmpPushExtras};
use crate::protocol::Protocol;

const DEFAULT_COMMAND_QUEUE: usize = 256;
const DEFAULT_EVENT_QUEUE: usize = 1024;

struct SinkState {
    closed: bool,
    ready: bool,
    emit_play_metadata: bool,
    tracks: Vec<TrackInfo>,
    buffer: VecDeque<RtmpCoreCommand>,
    buffer_capacity: usize,
}

/// Synchronous RTMP publisher sink.
///
/// A background task (started by [`open_rtmp_push`]) performs the async
/// handshake and forwards commands to the `RtmpClient` driver.
pub struct RtmpPublisherSink {
    cmd_tx: Mutex<mpsc::Sender<RtmpCoreCommand>>,
    cancel: CancellationToken,
    state: Arc<Mutex<SinkState>>,
    _join: Mutex<Option<Box<dyn JoinHandle>>>,
}

impl RtmpPublisherSink {
    fn new(
        cmd_tx: mpsc::Sender<RtmpCoreCommand>,
        cancel: CancellationToken,
        state: Arc<Mutex<SinkState>>,
        join: Box<dyn JoinHandle>,
    ) -> Self {
        Self {
            cmd_tx: Mutex::new(cmd_tx),
            cancel,
            state,
            _join: Mutex::new(Some(join)),
        }
    }

    fn send_command(&self, command: RtmpCoreCommand) -> Result<(), SdkError> {
        let mut guard = self.cmd_tx.lock();
        if guard.is_closed() {
            return Err(SdkError::Internal(
                "rtmp push command channel closed".to_string(),
            ));
        }
        guard.try_send(command).map_err(|err| {
            if err.is_full() {
                SdkError::Internal("rtmp push command queue full".to_string())
            } else {
                SdkError::Internal("rtmp push command channel closed".to_string())
            }
        })
    }
}

impl PublisherSink for RtmpPublisherSink {
    fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
        {
            let mut guard = self.state.lock();
            if guard.closed {
                return Err(SdkError::Internal("rtmp push sink closed".to_string()));
            }
            guard.tracks = tracks;
        }

        let bootstrap = {
            let guard = self.state.lock();
            build_track_bootstrap_payloads(
                &guard.tracks,
                RtmpFlvPlayMode::Normal,
                false,
                guard.emit_play_metadata,
            )
        };

        for payload in bootstrap {
            let command = payload_to_core_command(payload, RtmpMessageStreamId::MEDIA.get());
            self.send_command(command)?;
        }

        Ok(())
    }

    fn push_frame(&self, frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
        let (closed, tracks) = {
            let guard = self.state.lock();
            (guard.closed, guard.tracks.clone())
        };
        if closed {
            return Ok(DispatchResult::RejectedClosed);
        }

        let Some(payload) = map_frame_to_rtmp_flv_payload(&frame, RtmpFlvPlayMode::Normal, &tracks)
        else {
            return Ok(DispatchResult::DroppedByPolicy);
        };

        let command = payload_to_core_command(payload, RtmpMessageStreamId::MEDIA.get());
        match self.send_command(command) {
            Ok(()) => Ok(DispatchResult::Accepted),
            Err(SdkError::Internal(msg)) if msg.contains("queue full") => {
                Ok(DispatchResult::DroppedByPolicy)
            }
            Err(err) => Err(err),
        }
    }

    fn close(&self) -> Result<(), SdkError> {
        self.state.lock().closed = true;
        self.cancel.cancel();
        Ok(())
    }

    fn take_keyframe_requests(&self) -> u64 {
        0
    }
}

/// Open an RTMP push handle for `url` and `options`.
///
/// 为 `url` 和 `options` 打开一个 RTMP push 句柄。
pub async fn open_rtmp_push(
    engine: Arc<cheetah_engine::Engine>,
    url: &str,
    options: ConnectorPushOptions,
) -> Result<PushHandle, ConnectorError> {
    open_rtmp_push_with_runtime(engine.runtime_api(), url, options).await
}

/// Open an RTMP push using only a runtime handle (no full `Engine` required).
///
/// 仅使用 runtime 句柄打开 RTMP 推流（不需要完整 `Engine`）。
pub async fn open_rtmp_push_with_runtime(
    runtime_api: Arc<dyn cheetah_runtime_api::RuntimeApi>,
    url: &str,
    options: ConnectorPushOptions,
) -> Result<PushHandle, ConnectorError> {
    let parsed_url = RtmpUrl::parse(url).map_err(|err| ConnectorError::InvalidUrl {
        protocol: Protocol::Rtmp,
        url: url.to_string(),
        reason: err.to_string(),
    })?;
    let rtmp_url = parsed_url.to_string();

    let cancel = options.cancel.clone().unwrap_or_default().child_token();

    let extras = match options.protocol {
        ProtocolPushExtras::Rtmp(extras) => extras,
        _ => RtmpPushExtras::default(),
    };

    let command_queue_capacity = extras
        .command_queue_capacity
        .unwrap_or(DEFAULT_COMMAND_QUEUE)
        .max(64);
    let write_queue_capacity = extras
        .write_queue_capacity
        .unwrap_or(command_queue_capacity)
        .max(8);
    let read_buffer_size = extras.read_buffer_size.unwrap_or(64 * 1024).max(1024);
    let chunk_size = extras.chunk_size.unwrap_or(4096).max(1) as u32;
    let ack_window_size = extras.ack_window_size.unwrap_or(5_000_000).max(1) as u32;

    let config = RtmpClientDriverConfig {
        command_queue_capacity,
        event_queue_capacity: DEFAULT_EVENT_QUEUE,
        write_queue_capacity,
        read_buffer_size,
        ack_window_size,
        chunk_size,
    };

    let client = start_client(
        runtime_api.clone(),
        parsed_url,
        RtmpClientMode::Publish,
        config,
        cancel.clone(),
    )
    .map_err(|err| ConnectorError::Connect {
        protocol: Protocol::Rtmp,
        endpoint: rtmp_url.clone(),
        source: Box::new(err),
    })?;

    let (cmd_tx, cmd_rx) = mpsc::channel(command_queue_capacity);
    let state = Arc::new(Mutex::new(SinkState {
        closed: false,
        ready: false,
        emit_play_metadata: options.publisher.announce_tracks,
        tracks: Vec::new(),
        buffer: VecDeque::with_capacity(command_queue_capacity),
        buffer_capacity: command_queue_capacity,
    }));

    let cmd_tx_client = client.core_command_sender();
    let (ready_tx, ready_rx) = tokio::sync::watch::channel(false);
    let run = run_client(
        client,
        cmd_rx,
        cmd_tx_client,
        cancel.clone(),
        state.clone(),
        ready_tx,
    );

    let join = runtime_api.spawn(Box::pin(run));

    let sink = RtmpPublisherSink::new(cmd_tx, cancel, state, join);
    sink.update_tracks(options.tracks)
        .map_err(|e| map_sdk_error(Protocol::Rtmp, Operation::Open, e))?;

    Ok(PushHandle::new(
        Protocol::Rtmp,
        rtmp_url,
        Box::new(sink),
        Arc::new(ready_rx),
    ))
}

async fn run_client(
    mut client: RtmpClientHandle,
    mut cmd_rx: mpsc::Receiver<RtmpCoreCommand>,
    cmd_tx: RtmpClientCommandSender,
    cancel: CancellationToken,
    state: Arc<Mutex<SinkState>>,
    ready_tx: tokio::sync::watch::Sender<bool>,
) {
    let mut send_fut: OptionFuture<Fuse<BoxFuture<'static, Result<(), ClientSendError>>>> =
        OptionFuture::from(None);

    loop {
        select_biased! {
            _ = cancel.cancelled().fuse() => break,
            event = client.recv_event().fuse() => {
                let Some(event) = event else { break; };
                if let ClientDriverEvent::Core {
                    event: RtmpEvent::ClientStateChanged { state: RtmpClientState::Publishing },
                } = event
                {
                    state.lock().ready = true;
                    let _ = ready_tx.send(true);
                    try_send_next(&mut send_fut, &cmd_tx, &state);
                }
            }
            cmd = cmd_rx.next().fuse() => {
                let Some(cmd) = cmd else { break; };
                {
                    let mut guard = state.lock();
                    if guard.closed {
                        continue;
                    }
                    if guard.buffer.len() < guard.buffer_capacity {
                        guard.buffer.push_back(cmd);
                    }
                }
                try_send_next(&mut send_fut, &cmd_tx, &state);
            }
            send = send_fut => {
                if let Some(Err(_)) = send {
                    break;
                }
                try_send_next(&mut send_fut, &cmd_tx, &state);
            }
        }
    }

    client.shutdown();
    let _ = client.wait().await;
}

fn try_send_next(
    send_fut: &mut OptionFuture<Fuse<BoxFuture<'static, Result<(), ClientSendError>>>>,
    cmd_tx: &RtmpClientCommandSender,
    state: &Arc<Mutex<SinkState>>,
) {
    if !send_fut.is_terminated() {
        return;
    }
    let next = {
        let mut guard = state.lock();
        if guard.ready {
            guard.buffer.pop_front()
        } else {
            None
        }
    };
    if let Some(next) = next {
        let cmd_tx = cmd_tx.clone();
        let fut: BoxFuture<'static, Result<(), ClientSendError>> =
            Box::pin(async move { cmd_tx.send(RtmpClientDriverCommand::Core(next)).await });
        *send_fut = OptionFuture::from(Some(fut.fuse()));
    }
}

fn payload_to_core_command(payload: RtmpFlvPayload, stream_id: u32) -> RtmpCoreCommand {
    match payload.kind {
        RtmpFlvPayloadKind::Audio => RtmpCoreCommand::SendAudio {
            stream_id,
            timestamp_ms: payload.timestamp_ms,
            payload: payload.payload,
        },
        RtmpFlvPayloadKind::Video => RtmpCoreCommand::SendVideo {
            stream_id,
            timestamp_ms: payload.timestamp_ms,
            payload: payload.payload,
        },
        RtmpFlvPayloadKind::Data => RtmpCoreCommand::SendMetadata {
            stream_id,
            timestamp_ms: payload.timestamp_ms,
            payload: payload.payload,
        },
    }
}
