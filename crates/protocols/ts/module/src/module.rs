//! TS module factory and implementation.
//!
//! TS 模块工厂与实现。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::{
    MonoTime, MpegTsDemuxEvent, MpegTsDemuxer, MpegTsDemuxerConfig, MpegTsMuxEvent, MpegTsMuxer,
    MpegTsMuxerConfig, TrackInfo,
};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability,
    ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, PublisherOptions, PublisherSink, RuntimeApi, SdkError,
    StreamKey, StreamSnapshot, SubscriberOptions,
};
use cheetah_ts_core::StreamKeyParts;
use cheetah_ts_driver_tokio::{
    start_server, TsCommandSender, TsConnectionId, TsDriverCommand, TsDriverConfig, TsDriverEvent,
    TsPullClient, TsPullClientConfig, TsPullEvent, TsTlsConfig as DriverTsTlsConfig,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::{debug, warn};

use crate::config::{TsModuleConfig, TsPullJobConfig};

const MODULE_ID: &str = "ts";

/// Factory for creating TS protocol modules.
///
/// TS 协议模块工厂。
pub struct TsModuleFactory;

/// `TsModuleFactory` implementation.
///
/// `TsModuleFactory` 实现。
impl ModuleFactory for TsModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "TS Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "ts".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![
                ModuleCapability::Subscribe,
                ModuleCapability::Publish,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(TsModule::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "ts-module".to_string(),
            default_value: TsModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let config =
                    TsModuleConfig::from_value(value.clone()).map_err(|err| err.to_string())?;
                config.validate()
            })),
        })
    }
}

/// TS module runtime state.
///
/// TS 模块运行时状态。
struct TsModule {
    state: ModuleState,
    config: TsModuleConfig,
    ctx: Option<EngineContext>,
}

impl TsModule {
    fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: TsModuleConfig::default(),
            ctx: None,
        }
    }
}

/// Translate module TLS config into the driver TLS config.
///
/// 将模块 TLS 配置转换为驱动 TLS 配置。
fn driver_tls_config(config: &TsModuleConfig) -> Result<Option<DriverTsTlsConfig>, SdkError> {
    let Some(tls) = &config.tls else {
        return Ok(None);
    };
    if !tls.enabled {
        return Ok(None);
    }

    let listen = tls
        .listen
        .parse()
        .map_err(|e| SdkError::InvalidArgument(format!("invalid tls.listen: {e}")))?;

    Ok(Some(DriverTsTlsConfig {
        listen,
        cert_path: tls.cert_path.clone(),
        key_path: tls.key_path.clone(),
        handshake_timeout_ms: tls.handshake_timeout_ms,
    }))
}

/// `Module` lifecycle and runtime loop implementation for TS.
///
/// TS 的 `Module` 生命周期与运行时循环实现。
#[async_trait]
impl Module for TsModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "TS Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = TsModuleConfig::from_value(ctx.initial_config.clone())
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        self.ctx = Some(ctx.engine);
        self.state = ModuleState::Initialized;
        Ok(())
    }

    async fn start(&mut self, cancel: CancellationToken) -> Result<(), SdkError> {
        if !self.config.enabled {
            self.state = ModuleState::Running;
            cancel.cancelled().await;
            return Ok(());
        }

        let Some(ctx) = self.ctx.clone() else {
            return Err(SdkError::InvalidArgument(
                "module not initialized".to_string(),
            ));
        };
        let config = self.config.clone();

        self.state = ModuleState::Running;

        let listen_addr = config
            .listen
            .parse()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen: {e}")))?;

        let (cmd_sender, mut handle) = start_server(
            TsDriverConfig {
                listen: listen_addr,
                write_queue_capacity: config.write_queue_capacity,
                read_buffer_size: config.read_buffer_size,
                tls: driver_tls_config(&config)?,
            },
            cancel.clone(),
        );

        // Spawn pull jobs
        for job in &config.pull_jobs {
            if !job.enabled {
                continue;
            }
            let job = job.clone();
            let ctx2 = ctx.clone();
            let cancel2 = cancel.clone();
            let demux_config = MpegTsDemuxerConfig {
                max_reassembly_bytes: config.max_reassembly_bytes,
                strict_crc: config.strict_crc,
            };
            ctx.runtime_api.spawn(Box::pin(async move {
                run_pull_job_supervisor(&ctx2, &job, &demux_config, cancel2).await;
            }));
        }

        // Main event loop
        loop {
            let cancel_fut = cancel.cancelled().fuse();
            let event_fut = handle.recv_event().fuse();
            pin_mut!(cancel_fut, event_fut);

            let event = select_biased! {
                _ = cancel_fut => break,
                ev = event_fut => match ev {
                    Some(ev) => ev,
                    None => break,
                },
            };

            match event {
                TsDriverEvent::PlayRequested {
                    connection_id,
                    stream_key,
                    transport: _,
                } => {
                    let ctx2 = ctx.clone();
                    let config2 = config.clone();
                    let cmd2 = cmd_sender.clone();
                    let cancel2 = cancel.clone();
                    ctx.runtime_api.spawn(Box::pin(async move {
                        run_play_session(ctx2, config2, cmd2, connection_id, stream_key, cancel2)
                            .await;
                    }));
                }
                TsDriverEvent::ConnectionClosed { .. } => {}
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = TsModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_config != self.config {
            self.config = new_config;
            Ok(ConfigEffect::ModuleRestartRequired)
        } else {
            Ok(ConfigEffect::Immediate)
        }
    }
}

/// Run a TS play session: subscribe to engine stream, mux to TS, send via driver.
///
/// 运行 TS 播放会话：订阅引擎流、复用为 TS 并通过驱动发送。
async fn run_play_session(
    ctx: EngineContext,
    config: TsModuleConfig,
    cmd_sender: TsCommandSender,
    conn_id: TsConnectionId,
    stream_key: StreamKeyParts,
    cancel: CancellationToken,
) {
    let sk = StreamKey::new(&stream_key.namespace, &stream_key.stream_path);

    // Wait for stream
    let timeout = Duration::from_millis(config.play_wait_source_timeout_ms);
    let Some(snapshot) = wait_for_stream(&ctx, &sk, &cancel, timeout).await else {
        cmd_sender
            .send(TsDriverCommand::CloseConnection {
                connection_id: conn_id,
            })
            .await;
        return;
    };

    // Subscribe
    let queue_cap = config
        .subscriber_queue_capacity
        .max(config.bootstrap_max_frames.max(1));
    let mut subscriber = match ctx
        .subscriber_api
        .subscribe(
            sk.clone(),
            SubscriberOptions {
                queue_capacity: queue_cap,
                bootstrap_policy: BootstrapPolicy::live_tail(config.bootstrap_max_frames, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!(%sk, "TS subscribe failed: {e}");
            cmd_sender
                .send(TsDriverCommand::CloseConnection {
                    connection_id: conn_id,
                })
                .await;
            return;
        }
    };

    // Initialize muxer from tracks
    let tracks: Vec<TrackInfo> = snapshot
        .tracks
        .iter()
        .filter(|t| {
            t.media_kind == cheetah_codec::MediaKind::Video
                || t.media_kind == cheetah_codec::MediaKind::Audio
        })
        .take(config.max_tracks)
        .cloned()
        .collect();

    let mut muxer = MpegTsMuxer::new(&MpegTsMuxerConfig::default(), &tracks);

    // Send initial PAT/PMT
    send_tables(&mut muxer, &cmd_sender, conn_id).await;

    let pat_pmt_interval_us = (config.pat_pmt_interval_ms * 1000) as i64;
    let mut last_pat_pmt_us = ctx.runtime_api.now().as_micros() as i64;

    // Frame loop
    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);

        let next = select_biased! {
            _ = cancel_fut => break,
            r = recv_fut => r,
        };

        match next {
            Ok(Some(frame)) => {
                let now_us = ctx.runtime_api.now().as_micros() as i64;
                let elapsed = now_us.saturating_sub(last_pat_pmt_us);
                let is_keyframe = frame.flags.contains(cheetah_codec::FrameFlags::KEY)
                    && frame.media_kind == cheetah_codec::MediaKind::Video;

                // Resend PAT/PMT on keyframe if interval elapsed, or periodically
                if (is_keyframe && elapsed >= pat_pmt_interval_us)
                    || elapsed >= pat_pmt_interval_us * 2
                {
                    send_tables(&mut muxer, &cmd_sender, conn_id).await;
                    last_pat_pmt_us = now_us;
                }

                for ev in muxer.push_frame(frame.as_ref()) {
                    if let MpegTsMuxEvent::Packet(data) = ev {
                        cmd_sender
                            .send(TsDriverCommand::SendBytes {
                                connection_id: conn_id,
                                data,
                            })
                            .await;
                    }
                }
            }
            Ok(None) | Err(_) => break,
        }
    }

    let _ = subscriber.close().await;
}

/// Send PAT/PMT tables from the muxer to the connection.
///
/// 将复用器中的 PAT/PMT 表发送到连接。
async fn send_tables(
    muxer: &mut MpegTsMuxer,
    cmd_sender: &TsCommandSender,
    conn_id: TsConnectionId,
) {
    for ev in muxer.write_tables() {
        if let MpegTsMuxEvent::Packet(data) = ev {
            cmd_sender
                .send(TsDriverCommand::SendBytes {
                    connection_id: conn_id,
                    data,
                })
                .await;
        }
    }
}

/// Run a pull job with retry logic.
///
/// 以重试逻辑运行拉流任务。
async fn run_pull_job_supervisor(
    ctx: &EngineContext,
    job: &TsPullJobConfig,
    demux_config: &MpegTsDemuxerConfig,
    cancel: CancellationToken,
) {
    let mut backoff_ms = job.retry_backoff_ms;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        match run_single_pull(ctx, job, demux_config, &cancel).await {
            Ok(()) => return, // clean exit (cancelled)
            Err(e) => {
                warn!(name = %job.name, "TS pull error: {e}, retry in {backoff_ms}ms");
            }
        }

        // Backoff
        if sleep_or_cancel(
            ctx.runtime_api.as_ref(),
            &cancel,
            Duration::from_millis(backoff_ms),
        )
        .await
        {
            return;
        }
        backoff_ms = (backoff_ms * 2).min(job.max_retry_backoff_ms);
    }
}

/// Single pull attempt: connect, demux, publish.
///
/// 单次拉流尝试：连接、解复用、发布。
async fn run_single_pull(
    ctx: &EngineContext,
    job: &TsPullJobConfig,
    demux_config: &MpegTsDemuxerConfig,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let sk = match job.target_stream_key.split_once('/') {
        Some((ns, path)) => StreamKey::new(ns, path),
        None => StreamKey::new("live", &job.target_stream_key),
    };

    // Acquire publisher lease
    let (lease, sink) = ctx
        .publisher_api
        .acquire_publisher(sk.clone(), PublisherOptions::default())
        .await
        .map_err(|e| format!("acquire publisher: {e}"))?;

    let result = run_pull_inner(ctx, job, demux_config, cancel, &*sink).await;

    // Release lease
    let _ = ctx.publisher_api.release_publisher(&lease).await;
    result
}

/// Inner pull loop: read TS bytes, demux, and push frames into the engine.
///
/// 内部拉流循环：读取 TS 字节、解复用并将帧推入引擎。
async fn run_pull_inner(
    _ctx: &EngineContext,
    job: &TsPullJobConfig,
    demux_config: &MpegTsDemuxerConfig,
    cancel: &CancellationToken,
    sink: &dyn PublisherSink,
) -> Result<(), String> {
    let pull_config = TsPullClientConfig {
        url: job.source_url.clone(),
        read_buffer_size: 65536,
        insecure_tls: job.insecure_tls,
    };

    let mut rx = TsPullClient::connect(pull_config)
        .await
        .map_err(|e| format!("connect: {e}"))?;

    let mut demuxer = MpegTsDemuxer::new(demux_config.clone());
    let mut accumulated_tracks: Vec<TrackInfo> = Vec::new();
    let mut tracks_published = false;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        let event = match rx.recv().await {
            Some(ev) => ev,
            None => return Err("channel closed".to_string()),
        };

        match event {
            TsPullEvent::Bytes(data) => {
                for ev in demuxer.push(&data) {
                    match ev {
                        MpegTsDemuxEvent::TrackFound(track_info) => {
                            debug!(name = %job.name, codec = ?track_info.codec, "track found");
                            accumulated_tracks.push(track_info);
                        }
                        MpegTsDemuxEvent::TrackRemoved(ids) => {
                            let before = accumulated_tracks.len();
                            accumulated_tracks.retain(|t| !ids.contains(&t.track_id));
                            if accumulated_tracks.len() != before {
                                tracks_published = false;
                            }
                        }
                        MpegTsDemuxEvent::Frame(frame) => {
                            // Publish accumulated tracks on first frame
                            if !tracks_published && !accumulated_tracks.is_empty() {
                                sink.update_tracks(accumulated_tracks.clone())
                                    .map_err(|e| format!("update_tracks: {e}"))?;
                                tracks_published = true;
                            }
                            let _ = sink
                                .push_frame(Arc::new(frame))
                                .map_err(|e| format!("push_frame: {e}"))?;
                        }
                        MpegTsDemuxEvent::Diagnostic(_) => {}
                    }
                }
            }
            TsPullEvent::Closed { reason } => {
                // Flush demuxer on close
                for ev in demuxer.flush() {
                    if let MpegTsDemuxEvent::Frame(frame) = ev {
                        if !tracks_published && !accumulated_tracks.is_empty() {
                            sink.update_tracks(accumulated_tracks.clone())
                                .map_err(|e| format!("update_tracks: {e}"))?;
                            tracks_published = true;
                        }
                        let _ = sink.push_frame(Arc::new(frame));
                    }
                }
                return Err(format!("remote closed: {reason}"));
            }
        }
    }
}

/// Wait for a stream to appear in the engine, with timeout and cancel support.
///
/// 等待引擎中的流出现，支持超时与取消。
async fn wait_for_stream(
    ctx: &EngineContext,
    stream_key: &StreamKey,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Option<StreamSnapshot> {
    let start = ctx.runtime_api.now().as_micros();
    let timeout_us = timeout.as_micros() as u64;

    loop {
        if cancel.is_cancelled() {
            return None;
        }
        if let Ok(Some(snapshot)) = ctx.stream_manager_api.get_stream(stream_key).await {
            return Some(snapshot);
        }
        let elapsed = ctx.runtime_api.now().as_micros().saturating_sub(start);
        if elapsed >= timeout_us {
            return None;
        }
        if sleep_or_cancel(ctx.runtime_api.as_ref(), cancel, Duration::from_millis(100)).await {
            return None;
        }
    }
}

/// Sleep until `duration` or cancellation, whichever comes first.
///
/// 睡眠直到 `duration` 或取消，以先到者为准。
async fn sleep_or_cancel(
    runtime_api: &dyn RuntimeApi,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let now = runtime_api.now().as_micros();
    let delta = duration.as_micros() as u64;
    let deadline = MonoTime::from_micros(now.saturating_add(delta));
    let mut timer = runtime_api.sleep_until(deadline);
    let cancel_fut = cancel.cancelled().fuse();
    let wait_fut = timer.wait().fuse();
    pin_mut!(cancel_fut, wait_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = wait_fut => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TsTlsConfig;

    #[test]
    fn enabled_tls_config_is_passed_to_driver() {
        let config = TsModuleConfig {
            tls: Some(TsTlsConfig {
                enabled: true,
                listen: "127.0.0.1:18443".to_string(),
                cert_path: "cert.pem".to_string(),
                key_path: "key.pem".to_string(),
                handshake_timeout_ms: 1234,
            }),
            ..Default::default()
        };

        let tls = driver_tls_config(&config)
            .expect("valid tls config")
            .expect("enabled tls should produce driver config");
        assert_eq!(tls.listen.to_string(), "127.0.0.1:18443");
        assert_eq!(tls.cert_path, "cert.pem");
        assert_eq!(tls.key_path, "key.pem");
        assert_eq!(tls.handshake_timeout_ms, 1234);
    }

    #[test]
    fn disabled_tls_config_is_not_passed_to_driver() {
        let config = TsModuleConfig {
            tls: Some(TsTlsConfig {
                enabled: false,
                listen: "127.0.0.1:18443".to_string(),
                cert_path: "cert.pem".to_string(),
                key_path: "key.pem".to_string(),
                handshake_timeout_ms: 1234,
            }),
            ..Default::default()
        };

        assert!(driver_tls_config(&config)
            .expect("disabled tls config should be accepted")
            .is_none());
    }
}
