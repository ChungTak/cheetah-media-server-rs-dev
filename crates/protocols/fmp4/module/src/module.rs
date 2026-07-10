//! fMP4 module factory and implementation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::{
    annexb_from_payload, h26x_length_prefixed_from_payload, CodecId, Fmp4MuxEvent, Fmp4MuxSample,
    Fmp4Muxer, Fmp4MuxerConfig, FrameFlags, MediaKind, MonoTime, TrackInfo,
};
use cheetah_fmp4_core::StreamKeyParts;
use cheetah_fmp4_driver_tokio::{
    start_server, Fmp4CommandSender, Fmp4ConnectionId, Fmp4DriverCommand, Fmp4DriverConfig,
    Fmp4DriverEvent, Fmp4TlsConfig as DriverFmp4TlsConfig,
};
use cheetah_sdk::{
    BootstrapPolicy, CancellationToken, ConfigEffect, EngineContext, Module, ModuleCapability,
    ModuleConfigChange, ModuleFactory, ModuleId, ModuleInfo, ModuleInitContext, ModuleManifest,
    ModuleSchemaRegistration, ModuleState, RuntimeApi, SdkError, StreamKey, StreamSnapshot,
    SubscriberOptions,
};
use futures::{pin_mut, select_biased, FutureExt};
use tracing::warn;

use crate::config::Fmp4ModuleConfig;

const MODULE_ID: &str = "fmp4";

#[derive(Default)]
struct ActivePlaySessions {
    tokens: HashMap<Fmp4ConnectionId, CancellationToken>,
}

impl ActivePlaySessions {
    fn start(
        &mut self,
        connection_id: Fmp4ConnectionId,
        root: &CancellationToken,
    ) -> CancellationToken {
        self.cancel(connection_id);
        let token = root.child_token();
        self.tokens.insert(connection_id, token.clone());
        token
    }

    fn cancel(&mut self, connection_id: Fmp4ConnectionId) {
        if let Some(token) = self.tokens.remove(&connection_id) {
            token.cancel();
        }
    }
}

/// `Fmp4ModuleFactory` data structure.
/// `Fmp4ModuleFactory` 数据结构。
pub struct Fmp4ModuleFactory;

impl ModuleFactory for Fmp4ModuleFactory {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "fMP4 Module".to_string(),
            dependencies: Vec::new(),
            config_namespace: "fmp4".to_string(),
            routes_prefix: "/".to_string(),
            capabilities: vec![
                ModuleCapability::Subscribe,
                ModuleCapability::Publish,
                ModuleCapability::BackgroundJob,
            ],
        }
    }

    fn create(&self) -> Box<dyn Module> {
        Box::new(Fmp4Module::new())
    }

    fn config_schema(&self) -> Option<ModuleSchemaRegistration> {
        Some(ModuleSchemaRegistration {
            module_id: ModuleId::new(MODULE_ID),
            schema_name: "fmp4-module".to_string(),
            default_value: Fmp4ModuleConfig::default_json(),
            validator: Some(Arc::new(|value| {
                let config =
                    Fmp4ModuleConfig::from_value(value.clone()).map_err(|err| err.to_string())?;
                config.validate()
            })),
        })
    }
}

struct Fmp4Module {
    state: ModuleState,
    config: Fmp4ModuleConfig,
    ctx: Option<EngineContext>,
}

impl Fmp4Module {
    fn new() -> Self {
        Self {
            state: ModuleState::Created,
            config: Fmp4ModuleConfig::default(),
            ctx: None,
        }
    }
}

#[async_trait]
impl Module for Fmp4Module {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            module_id: ModuleId::new(MODULE_ID),
            display_name: "fMP4 Module".to_string(),
            state: self.state,
        }
    }

    fn state(&self) -> ModuleState {
        self.state
    }

    async fn init(&mut self, ctx: ModuleInitContext) -> Result<(), SdkError> {
        self.config = Fmp4ModuleConfig::from_value(ctx.initial_config.clone())
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

        let ctx = self.ctx.clone().unwrap();
        let config = self.config.clone();
        self.state = ModuleState::Running;

        let listen_addr = config
            .listen
            .parse()
            .map_err(|e| SdkError::InvalidArgument(format!("invalid listen: {e}")))?;

        let (cmd_sender, mut handle) = start_server(
            Fmp4DriverConfig {
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
            let read_buf = config.read_buffer_size;
            let max_box_bytes = config.max_box_bytes;
            ctx.runtime_api.spawn(Box::pin(async move {
                run_pull_job_supervisor(&ctx2, &job, read_buf, max_box_bytes, cancel2).await;
            }));
        }

        let mut active_play_sessions = ActivePlaySessions::default();

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
                Fmp4DriverEvent::PlayRequested {
                    connection_id,
                    stream_key,
                    transport: _,
                } => {
                    let ctx2 = ctx.clone();
                    let config2 = config.clone();
                    let cmd2 = cmd_sender.clone();
                    let cancel2 = active_play_sessions.start(connection_id, &cancel);
                    ctx.runtime_api.spawn(Box::pin(async move {
                        run_play_session(ctx2, config2, cmd2, connection_id, stream_key, cancel2)
                            .await;
                    }));
                }
                Fmp4DriverEvent::ConnectionClosed { connection_id } => {
                    active_play_sessions.cancel(connection_id);
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), SdkError> {
        self.state = ModuleState::Stopped;
        Ok(())
    }

    async fn apply_config(&mut self, change: ModuleConfigChange) -> Result<ConfigEffect, SdkError> {
        let new_config = Fmp4ModuleConfig::from_value(change.next)
            .map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        if new_config != self.config {
            self.config = new_config;
            Ok(ConfigEffect::ModuleRestartRequired)
        } else {
            Ok(ConfigEffect::Immediate)
        }
    }
}

fn driver_tls_config(config: &Fmp4ModuleConfig) -> Result<Option<DriverFmp4TlsConfig>, SdkError> {
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
    Ok(Some(DriverFmp4TlsConfig {
        listen,
        cert_path: tls.cert_path.clone(),
        key_path: tls.key_path.clone(),
        handshake_timeout_ms: tls.handshake_timeout_ms,
    }))
}

async fn run_play_session(
    ctx: EngineContext,
    config: Fmp4ModuleConfig,
    cmd_sender: Fmp4CommandSender,
    conn_id: Fmp4ConnectionId,
    stream_key: StreamKeyParts,
    cancel: CancellationToken,
) {
    let sk = StreamKey::new(&stream_key.namespace, &stream_key.stream_path);

    let timeout = Duration::from_millis(config.play_wait_source_timeout_ms);
    let Some(snapshot) = wait_for_stream(&ctx, &sk, &cancel, timeout).await else {
        cmd_sender
            .send(Fmp4DriverCommand::CloseConnection {
                connection_id: conn_id,
            })
            .await;
        return;
    };

    let queue_cap = config
        .subscriber_queue_capacity
        .max(config.bootstrap_max_frames.max(1));
    let mut subscriber = match ctx
        .subscriber_api
        .subscribe(
            sk.clone(),
            SubscriberOptions {
                queue_capacity: queue_cap,
                bootstrap_policy: BootstrapPolicy::full_gop(config.bootstrap_max_frames, None),
                ..Default::default()
            },
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!(%sk, "fMP4 subscribe failed: {e}");
            cmd_sender
                .send(Fmp4DriverCommand::CloseConnection {
                    connection_id: conn_id,
                })
                .await;
            return;
        }
    };

    let tracks: Vec<TrackInfo> = snapshot
        .tracks
        .iter()
        .filter(|t| t.media_kind == MediaKind::Video || t.media_kind == MediaKind::Audio)
        .take(config.max_tracks)
        .cloned()
        .collect();

    let mux_config = Fmp4MuxerConfig {
        include_styp: config.include_styp,
        include_sidx: config.include_sidx,
        ..Default::default()
    };
    let mut muxer = Fmp4Muxer::new(mux_config.clone(), &tracks);

    // Send init segment
    for event in muxer.init_segment() {
        if let Fmp4MuxEvent::InitSegment(data) = event {
            cmd_sender
                .send(Fmp4DriverCommand::SendData {
                    connection_id: conn_id,
                    data,
                })
                .await;
        }
    }

    // If there are video tracks, wait for a keyframe before sending media segments
    let has_video = tracks.iter().any(|t| t.media_kind == MediaKind::Video);

    // Frame loop — batch samples, flush on keyframe or duration threshold
    let mut pending_samples: Vec<Fmp4MuxSample> = Vec::with_capacity(1024);
    let mut fragment_start_us: Option<i64> = None;
    let max_frag_us = config.max_fragment_duration_ms as i64 * 1000;
    let mut waiting_for_keyframe = has_video;
    let mut current_tracks = tracks;
    let mut frames_since_track_check: u32 = 0;

    loop {
        let cancel_fut = cancel.cancelled().fuse();
        let recv_fut = subscriber.recv().fuse();
        pin_mut!(cancel_fut, recv_fut);

        let frame = select_biased! {
            _ = cancel_fut => break,
            r = recv_fut => match r {
                Ok(Some(f)) => f,
                _ => break,
            },
        };

        let is_key = frame.flags.contains(FrameFlags::KEY) && frame.media_kind == MediaKind::Video;

        // Skip frames until we see a video keyframe
        if waiting_for_keyframe {
            if is_key {
                waiting_for_keyframe = false;
            } else {
                continue;
            }
        }

        // Periodically check for track/config changes (every 300 frames or on keyframe)
        frames_since_track_check += 1;
        if is_key || frames_since_track_check >= 300 {
            frames_since_track_check = 0;
            if let Ok(Some(new_snapshot)) = ctx.stream_manager_api.get_stream(&sk).await {
                let new_tracks: Vec<TrackInfo> = new_snapshot
                    .tracks
                    .iter()
                    .filter(|t| {
                        t.media_kind == MediaKind::Video || t.media_kind == MediaKind::Audio
                    })
                    .take(config.max_tracks)
                    .cloned()
                    .collect();
                if tracks_changed(&current_tracks, &new_tracks) {
                    // Flush current fragment
                    if !pending_samples.is_empty() {
                        flush_segment(&mut muxer, &mut pending_samples, &cmd_sender, conn_id).await;
                        fragment_start_us = None;
                    }
                    // Rebuild muxer with new tracks
                    current_tracks = new_tracks;
                    muxer = Fmp4Muxer::new(mux_config.clone(), &current_tracks);
                    // Resend init segment
                    for event in muxer.init_segment() {
                        if let Fmp4MuxEvent::InitSegment(data) = event {
                            cmd_sender
                                .send(Fmp4DriverCommand::SendData {
                                    connection_id: conn_id,
                                    data,
                                })
                                .await;
                        }
                    }
                    // Wait for next keyframe if video present
                    let new_has_video = current_tracks
                        .iter()
                        .any(|t| t.media_kind == MediaKind::Video);
                    if new_has_video {
                        waiting_for_keyframe = true;
                        continue;
                    }
                }
            }
        }

        // Flush pending on video keyframe (start new fragment at keyframe boundary)
        if config.force_fragment_on_keyframe && is_key && !pending_samples.is_empty() {
            flush_segment(&mut muxer, &mut pending_samples, &cmd_sender, conn_id).await;
            fragment_start_us = None;
        }

        let sample = Fmp4MuxSample {
            track_id: frame.track_id.0,
            dts_us: frame.dts,
            pts_us: frame.pts,
            is_keyframe: frame.flags.contains(FrameFlags::KEY),
            data: fmp4_sample_payload(&frame),
        };

        if fragment_start_us.is_none() {
            fragment_start_us = Some(frame.dts);
        }
        pending_samples.push(sample);

        // Flush on duration threshold or sample count safety cap
        let should_flush = match fragment_start_us {
            Some(start) => frame.dts - start >= max_frag_us,
            None => false,
        } || pending_samples.len() >= 1024;

        if should_flush {
            flush_segment(&mut muxer, &mut pending_samples, &cmd_sender, conn_id).await;
            fragment_start_us = None;
        }
    }

    // Flush remaining
    if !pending_samples.is_empty() {
        flush_segment(&mut muxer, &mut pending_samples, &cmd_sender, conn_id).await;
    }

    let _ = subscriber.close().await;
}

async fn flush_segment(
    muxer: &mut Fmp4Muxer,
    samples: &mut Vec<Fmp4MuxSample>,
    cmd_sender: &Fmp4CommandSender,
    conn_id: Fmp4ConnectionId,
) {
    for event in muxer.write_segment(samples) {
        if let Fmp4MuxEvent::MediaSegment { data, .. } = event {
            cmd_sender
                .send(Fmp4DriverCommand::SendData {
                    connection_id: conn_id,
                    data,
                })
                .await;
        }
    }
    samples.clear();
}

/// Detect if tracks have changed (different count, IDs, codecs, or extradata).
fn tracks_changed(old: &[TrackInfo], new: &[TrackInfo]) -> bool {
    if old.len() != new.len() {
        return true;
    }
    for (a, b) in old.iter().zip(new.iter()) {
        if a.track_id != b.track_id || a.codec != b.codec || a.media_kind != b.media_kind {
            return true;
        }
        if a.extradata != b.extradata {
            return true;
        }
    }
    false
}

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

fn parse_stream_key(s: &str) -> StreamKey {
    match s.split_once('/') {
        Some((ns, path)) => StreamKey::new(ns, path),
        None => StreamKey::new("live", s),
    }
}

async fn run_pull_job_supervisor(
    ctx: &EngineContext,
    job: &crate::config::Fmp4PullJobConfig,
    read_buffer_size: usize,
    max_box_bytes: usize,
    cancel: CancellationToken,
) {
    use cheetah_codec::{Fmp4DemuxEvent, Fmp4Demuxer, Fmp4DemuxerConfig, TrackId};
    use cheetah_fmp4_driver_tokio::{connect_pull, Fmp4PullClientConfig, Fmp4PullEvent};
    use std::sync::Arc;

    let sk = parse_stream_key(&job.target_stream_key);
    let mut backoff_ms = job.retry_backoff_ms;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        // Acquire publisher lease
        let (lease, publisher) = match ctx
            .publisher_api
            .acquire_publisher(sk.clone(), cheetah_sdk::PublisherOptions::default())
            .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!(job = %job.name, "fMP4 pull publish failed: {e}");
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
                continue;
            }
        };

        // Connect to remote
        let pull_config = Fmp4PullClientConfig {
            url: job.source_url.clone(),
            read_buffer_size,
            insecure_tls: job.insecure_tls,
        };
        let mut rx = match connect_pull(pull_config).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!(job = %job.name, "fMP4 pull connect failed: {e}");
                let _ = publisher.close();
                let _ = ctx.publisher_api.release_publisher(&lease).await;
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
                continue;
            }
        };

        // Demux loop
        let mut demuxer = Fmp4Demuxer::new(Fmp4DemuxerConfig { max_box_bytes });
        let mut tracks_published = false;
        let mut mark_discontinuity = false;

        loop {
            let cancel_fut = cancel.cancelled().fuse();
            let recv_fut = rx.recv().fuse();
            pin_mut!(cancel_fut, recv_fut);

            let event = select_biased! {
                _ = cancel_fut => {
                    let _ = publisher.close();
                    let _ = ctx.publisher_api.release_publisher(&lease).await;
                    return;
                },
                ev = recv_fut => match ev {
                    Some(ev) => ev,
                    None => break,
                },
            };

            match event {
                Fmp4PullEvent::Bytes(data) => {
                    for demux_event in demuxer.push(&data) {
                        match demux_event {
                            Fmp4DemuxEvent::TrackInfo(tracks) => {
                                let track_infos: Vec<TrackInfo> =
                                    tracks.iter().map(demux_track_to_track_info).collect();
                                let _ = publisher.update_tracks(track_infos);
                                if tracks_published {
                                    // Repeated init — mark next frame as discontinuity
                                    mark_discontinuity = true;
                                }
                                tracks_published = true;
                            }
                            Fmp4DemuxEvent::Frame {
                                track_id,
                                media_kind,
                                codec,
                                pts_us,
                                dts_us,
                                keyframe,
                                data,
                            } => {
                                if !tracks_published {
                                    continue;
                                }
                                let tb = cheetah_codec::Timebase::new(1, 1_000_000);
                                let mut frame = cheetah_codec::AVFrame::new(
                                    TrackId(track_id),
                                    media_kind,
                                    codec,
                                    codec_to_format(codec),
                                    pts_us,
                                    dts_us,
                                    tb,
                                    demux_frame_payload(codec, data),
                                );
                                if keyframe {
                                    frame.flags |= FrameFlags::KEY;
                                }
                                if mark_discontinuity {
                                    frame.flags |= FrameFlags::DISCONTINUITY;
                                    mark_discontinuity = false;
                                }
                                let _ = publisher.push_frame(Arc::new(frame));
                            }
                            Fmp4DemuxEvent::Diagnostic(_) => {}
                        }
                    }
                }
                Fmp4PullEvent::Closed { reason } => {
                    warn!(job = %job.name, %reason, "fMP4 pull closed");
                    break;
                }
            }
        }

        let _ = publisher.close();
        let _ = ctx.publisher_api.release_publisher(&lease).await;

        // Retry with backoff
        if cancel.is_cancelled() {
            return;
        }
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

fn demux_track_to_track_info(t: &cheetah_codec::Fmp4DemuxTrack) -> TrackInfo {
    use cheetah_codec::track::{CodecExtradata, TrackId};

    let mut info = TrackInfo::new(TrackId(t.track_id), t.media_kind, t.codec, t.timescale);
    info.extradata = match t.codec {
        cheetah_codec::CodecId::H264 => CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(t.extradata.clone()),
        },
        cheetah_codec::CodecId::H265 => CodecExtradata::H265 {
            vps: vec![],
            sps: vec![],
            pps: vec![],
            hvcc: Some(t.extradata.clone()),
        },
        cheetah_codec::CodecId::AAC => CodecExtradata::AAC {
            asc: t.extradata.clone(),
        },
        cheetah_codec::CodecId::Opus => CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: Some(t.extradata.clone()),
        },
        _ => CodecExtradata::None,
    };
    info.readiness = cheetah_codec::TrackReadiness::Ready;
    info
}

fn fmp4_sample_payload(frame: &cheetah_codec::AVFrame) -> bytes::Bytes {
    if frame.format != cheetah_codec::FrameFormat::CanonicalH26x {
        return frame.payload.clone();
    }
    if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
        return frame.payload.clone();
    }
    h26x_length_prefixed_from_payload(frame.payload.clone())
}

fn demux_frame_payload(codec: CodecId, data: bytes::Bytes) -> bytes::Bytes {
    if matches!(codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
        annexb_from_payload(data)
    } else {
        data
    }
}

fn codec_to_format(codec: cheetah_codec::CodecId) -> cheetah_codec::FrameFormat {
    use cheetah_codec::FrameFormat;
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AV1 => FrameFormat::CanonicalAv1Obu,
        CodecId::VP8 => FrameFormat::CanonicalVp8Frame,
        CodecId::VP9 => FrameFormat::CanonicalVp9Frame,
        CodecId::MJPEG => FrameFormat::MjpegFrame,
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP2 => FrameFormat::Mp2Frame,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        _ => FrameFormat::Unknown,
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    use cheetah_codec::{AVFrame, FrameFormat, Timebase, TrackId};

    #[test]
    fn active_play_sessions_cancel_child_on_connection_close() {
        let root = CancellationToken::new();
        let mut sessions = ActivePlaySessions::default();

        let child = sessions.start(Fmp4ConnectionId(7), &root);
        assert!(!child.is_cancelled());

        sessions.cancel(Fmp4ConnectionId(7));

        assert!(child.is_cancelled());
    }

    #[test]
    fn active_play_sessions_root_cancel_cancels_child() {
        let root = CancellationToken::new();
        let mut sessions = ActivePlaySessions::default();

        let child = sessions.start(Fmp4ConnectionId(8), &root);
        root.cancel();

        assert!(child.is_cancelled());
    }

    #[test]
    fn fmp4_output_converts_h26x_annexb_to_length_prefixed() {
        let frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 1_000_000),
            bytes::Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xaa]),
        );

        let payload = fmp4_sample_payload(&frame);
        assert_eq!(payload.as_ref(), &[0, 0, 0, 2, 0x65, 0xaa]);
    }

    #[test]
    fn fmp4_pull_converts_h26x_length_prefixed_to_annexb() {
        let payload = demux_frame_payload(
            CodecId::H264,
            bytes::Bytes::from_static(&[0, 0, 0, 2, 0x65, 0xaa]),
        );
        assert_eq!(payload.as_ref(), &[0, 0, 0, 1, 0x65, 0xaa]);
    }
}
