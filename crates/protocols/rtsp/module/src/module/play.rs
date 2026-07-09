use super::*;
use cheetah_sdk::StreamKey;
use std::time::Duration;

const DESCRIBE_WAIT_SOURCE_POLL_INTERVAL_MS: u64 = 50;

struct DescribeTaskContext {
    connection_id: RtspConnectionId,
    req: RtspRequest,
    stream_key: StreamKey,
    base_uri: String,
    describe_pending: CancellationToken,
    engine: EngineContext,
    config: RtspModuleConfig,
    command_tx: RtspCoreCommandSender,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
}

fn selected_play_tracks_have_video(
    track_map: &HashMap<TrackId, TrackInfo>,
    play_tracks: &HashMap<TrackId, PlayTrackState>,
) -> bool {
    play_tracks.keys().any(|track_id| {
        track_map
            .get(track_id)
            .is_some_and(|track| matches!(track.media_kind, cheetah_codec::MediaKind::Video))
    })
}

fn selected_play_tracks_have_codec(
    track_map: &HashMap<TrackId, TrackInfo>,
    play_tracks: &HashMap<TrackId, PlayTrackState>,
    codec: cheetah_codec::CodecId,
) -> bool {
    play_tracks.keys().any(|track_id| {
        track_map
            .get(track_id)
            .is_some_and(|track| track.codec == codec)
    })
}

fn selected_play_tracks_video_bootstrap_floor(
    track_map: &HashMap<TrackId, TrackInfo>,
    play_tracks: &HashMap<TrackId, PlayTrackState>,
) -> usize {
    if !selected_play_tracks_have_video(track_map, play_tracks) {
        return 0;
    }

    if selected_play_tracks_have_codec(track_map, play_tracks, cheetah_codec::CodecId::VP9)
        || selected_play_tracks_have_codec(track_map, play_tracks, cheetah_codec::CodecId::AV1)
        || selected_play_tracks_have_codec(track_map, play_tracks, cheetah_codec::CodecId::H265)
        || selected_play_tracks_have_codec(track_map, play_tracks, cheetah_codec::CodecId::H266)
    {
        return 2048;
    }

    1024
}

fn release_replaced_multicast_play_track(
    multicast: &MulticastSenderRegistry,
    runtime_api: &Arc<dyn RuntimeApi>,
    connection_id: RtspConnectionId,
    map_track_id: TrackId,
    previous: Option<PlayTrackState>,
    replacement: &PlayTransport,
) {
    let Some(previous) = previous else {
        return;
    };
    let PlayTransport::UdpMulticast {
        stream_key,
        track_id: transport_track_id,
        ..
    } = previous.transport
    else {
        return;
    };

    let release_track_id = if transport_track_id == map_track_id {
        map_track_id
    } else {
        transport_track_id
    };
    if let PlayTransport::UdpMulticast {
        stream_key: replacement_stream_key,
        track_id: replacement_track_id,
        ..
    } = replacement
    {
        let replacement_track_id = if *replacement_track_id == map_track_id {
            map_track_id
        } else {
            *replacement_track_id
        };
        if *replacement_stream_key == stream_key && replacement_track_id == release_track_id {
            return;
        }
    }

    let now_micros = runtime_unix_time_micros(runtime_api);
    multicast.release(
        runtime_api,
        now_micros,
        connection_id,
        &stream_key,
        release_track_id,
    );
}

fn resolve_play_subscription_settings(
    has_video_track: bool,
    video_bootstrap_floor: usize,
    start_from_keyframe: bool,
    bootstrap_max_frames: usize,
    subscriber_queue_capacity: usize,
) -> (BootstrapPolicy, usize, bool) {
    let wait_for_next_random_access_point = has_video_track && start_from_keyframe;
    let bootstrap_max_frames = bootstrap_max_frames.max(video_bootstrap_floor);
    let subscriber_queue_capacity = subscriber_queue_capacity.max(bootstrap_max_frames);
    let bootstrap_policy = BootstrapPolicy {
        mode: BootstrapMode::LiveTail,
        max_bootstrap_age_ms: None,
        max_bootstrap_frames: bootstrap_max_frames,
        wait_for_next_random_access_point,
    };
    (
        bootstrap_policy,
        subscriber_queue_capacity,
        wait_for_next_random_access_point,
    )
}

fn rtsp_codec_supported_for_play(codec: cheetah_codec::CodecId) -> bool {
    matches!(
        codec,
        cheetah_codec::CodecId::H264
            | cheetah_codec::CodecId::H265
            | cheetah_codec::CodecId::H266
            | cheetah_codec::CodecId::AV1
            | cheetah_codec::CodecId::VP8
            | cheetah_codec::CodecId::VP9
            | cheetah_codec::CodecId::AAC
            | cheetah_codec::CodecId::Opus
            | cheetah_codec::CodecId::ADPCM
            | cheetah_codec::CodecId::G711A
            | cheetah_codec::CodecId::G711U
            | cheetah_codec::CodecId::MP3
    )
}

fn play_requires_keyframe_bootstrap(codec: cheetah_codec::CodecId) -> bool {
    matches!(
        codec,
        cheetah_codec::CodecId::H264
            | cheetah_codec::CodecId::H265
            | cheetah_codec::CodecId::H266
            | cheetah_codec::CodecId::AV1
            | cheetah_codec::CodecId::VP8
            | cheetah_codec::CodecId::VP9
    )
}

fn should_forward_play_frame(
    per_track: &HashMap<TrackId, PlayTrackState>,
    started_tracks: &mut HashSet<TrackId>,
    wait_for_video_keyframe: bool,
    track: &TrackInfo,
    track_id: TrackId,
    frame_flags: cheetah_codec::FrameFlags,
) -> bool {
    if !per_track.contains_key(&track_id) {
        return false;
    }
    if started_tracks.contains(&track_id) {
        return true;
    }
    if matches!(track.media_kind, cheetah_codec::MediaKind::Video)
        && wait_for_video_keyframe
        && play_requires_keyframe_bootstrap(track.codec)
        && !frame_flags.contains(cheetah_codec::FrameFlags::KEY)
    {
        return false;
    }
    started_tracks.insert(track_id);
    true
}

fn has_pending_selected_video_gate(
    track_map: &HashMap<TrackId, TrackInfo>,
    per_track: &HashMap<TrackId, PlayTrackState>,
    started_tracks: &HashSet<TrackId>,
    wait_for_video_keyframe: bool,
) -> bool {
    if !wait_for_video_keyframe {
        return false;
    }

    per_track.keys().any(|track_id| {
        !started_tracks.contains(track_id)
            && track_map.get(track_id).is_some_and(|track| {
                matches!(track.media_kind, cheetah_codec::MediaKind::Video)
                    && play_requires_keyframe_bootstrap(track.codec)
            })
    })
}

fn preserve_raw_rtp_timestamps(codec: cheetah_codec::CodecId) -> bool {
    matches!(
        codec,
        cheetah_codec::CodecId::H265
            | cheetah_codec::CodecId::H266
            | cheetah_codec::CodecId::AV1
            | cheetah_codec::CodecId::VP8
            | cheetah_codec::CodecId::VP9
            | cheetah_codec::CodecId::Opus
            | cheetah_codec::CodecId::ADPCM
            | cheetah_codec::CodecId::G711A
            | cheetah_codec::CodecId::G711U
            | cheetah_codec::CodecId::MP3
    )
}

fn source_rtp_timestamp_for_egress(
    frame: &cheetah_codec::AVFrame,
    codec: cheetah_codec::CodecId,
) -> Option<u32> {
    if !preserve_raw_rtp_timestamps(codec) {
        return None;
    }
    let cheetah_codec::SourceTimestamp::Rtp(source) = frame.source_timestamp()? else {
        return None;
    };
    Some(source.raw_timestamp)
}

fn play_packet_mtu(transport: &PlayTransport, configured_mtu: usize) -> usize {
    const RTSP_INTERLEAVED_LENGTH_LIMIT: usize = u16::MAX as usize;
    const RTP_HEADER_MARGIN_BYTES: usize = 12;
    match transport {
        PlayTransport::TcpInterleaved { .. } => {
            RTSP_INTERLEAVED_LENGTH_LIMIT.saturating_sub(RTP_HEADER_MARGIN_BYTES)
        }
        PlayTransport::UdpUnicast { .. } | PlayTransport::UdpMulticast { .. } => configured_mtu,
    }
}

fn media_ts_to_rtp_ticks(
    primary: i64,
    secondary: i64,
    timebase: cheetah_codec::Timebase,
    clock_rate: u32,
) -> u32 {
    cheetah_codec::media_ts_to_rtp_ticks(primary, secondary, timebase, clock_rate)
}

fn media_timestamp_priority(
    media_kind: cheetah_codec::MediaKind,
    pts: i64,
    dts: i64,
) -> (i64, i64) {
    cheetah_codec::select_egress_timestamps(media_kind, pts, dts)
}

const RTP_TIMESTAMP_BACKWARD_REPAIR_THRESHOLD_TICKS: u32 = 3_000;

/// Video codecs that may produce B-frames should not have their PTS-based RTP
/// timestamps forced monotonic, since PTS rollback is legitimate (RFC 6184).
fn should_skip_monotonic_repair_for_b_frames(
    media_kind: cheetah_codec::MediaKind,
    codec: cheetah_codec::CodecId,
) -> bool {
    matches!(media_kind, cheetah_codec::MediaKind::Video)
        && matches!(
            codec,
            cheetah_codec::CodecId::H264
                | cheetah_codec::CodecId::H265
                | cheetah_codec::CodecId::H266
        )
}

const RTSP_PLAY_PACING_BACKWARD_RESET_THRESHOLD_MS: u32 = 3_000;
const RTSP_PLAY_PACING_MAX_FORWARD_DELTA_MS: u32 = 30_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PlayStartPacingState {
    anchor_media_ms: Option<u32>,
    anchor_runtime_micros: u64,
    last_media_ms: Option<u32>,
}

impl PlayStartPacingState {
    fn delay_for(
        &mut self,
        media_timestamp_ms: u32,
        now_micros: u64,
        force_reset: bool,
    ) -> Duration {
        if force_reset || self.anchor_media_ms.is_none() {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        }

        let Some(anchor_media_ms) = self.anchor_media_ms else {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        };
        if let Some(last_media_ms) = self.last_media_ms {
            if media_timestamp_ms < last_media_ms
                && last_media_ms.wrapping_sub(media_timestamp_ms)
                    > RTSP_PLAY_PACING_BACKWARD_RESET_THRESHOLD_MS
            {
                self.reset_anchor(media_timestamp_ms, now_micros);
                return Duration::ZERO;
            }
        }

        let elapsed_media_ms = media_timestamp_ms.wrapping_sub(anchor_media_ms);
        if elapsed_media_ms > RTSP_PLAY_PACING_MAX_FORWARD_DELTA_MS {
            self.reset_anchor(media_timestamp_ms, now_micros);
            return Duration::ZERO;
        }

        self.last_media_ms = Some(media_timestamp_ms);
        let elapsed_media_micros = u64::from(elapsed_media_ms).saturating_mul(1_000);
        let target_runtime_micros = self
            .anchor_runtime_micros
            .saturating_add(elapsed_media_micros);
        if now_micros >= target_runtime_micros {
            Duration::ZERO
        } else {
            Duration::from_micros(target_runtime_micros - now_micros)
        }
    }

    fn reset_anchor(&mut self, media_timestamp_ms: u32, now_micros: u64) {
        self.anchor_media_ms = Some(media_timestamp_ms);
        self.anchor_runtime_micros = now_micros;
        self.last_media_ms = Some(media_timestamp_ms);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameObservabilityFields {
    track_id: u32,
    codec: cheetah_codec::CodecId,
    pts: i64,
    dts: i64,
}

fn frame_observability_fields(frame: &cheetah_codec::AVFrame) -> FrameObservabilityFields {
    FrameObservabilityFields {
        track_id: frame.track_id.0,
        codec: frame.codec,
        pts: frame.pts,
        dts: frame.dts,
    }
}

fn runtime_now_micros(runtime_api: &Arc<dyn RuntimeApi>) -> u64 {
    runtime_api.now().as_micros()
}

fn runtime_deadline_after(
    runtime_api: &Arc<dyn RuntimeApi>,
    duration: Duration,
) -> cheetah_codec::MonoTime {
    let duration_micros = duration.as_micros();
    let delta = duration_micros.min(u128::from(u64::MAX)) as u64;
    cheetah_codec::MonoTime::from_micros(runtime_now_micros(runtime_api).saturating_add(delta))
}

async fn runtime_sleep(runtime_api: &Arc<dyn RuntimeApi>, duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let mut timer = runtime_api.sleep_until(runtime_deadline_after(runtime_api, duration));
    timer.wait().await;
}

async fn wait_or_cancel(
    runtime_api: &Arc<dyn RuntimeApi>,
    cancel: &CancellationToken,
    duration: Duration,
) -> bool {
    let cancel_fut = cancel.cancelled().fuse();
    let sleep_fut = runtime_sleep(runtime_api, duration).fuse();
    pin_mut!(cancel_fut, sleep_fut);
    select_biased! {
        _ = cancel_fut => true,
        _ = sleep_fut => false,
    }
}

fn frame_media_timestamp_ms(
    media_kind: cheetah_codec::MediaKind,
    pts: i64,
    dts: i64,
    timebase: cheetah_codec::Timebase,
) -> Option<u32> {
    let (primary, secondary) = media_timestamp_priority(media_kind, pts, dts);
    let media_ts = if primary >= 0 {
        primary
    } else if secondary >= 0 {
        secondary
    } else {
        return None;
    };
    Some(cheetah_codec::dts_to_rtmp_timestamp_ms(media_ts, timebase))
}

pub(super) async fn handle_describe(
    connection_id: RtspConnectionId,
    req: RtspRequest,
    engine: &EngineContext,
    config: &RtspModuleConfig,
    command_tx: &RtspCoreCommandSender,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
) {
    let Some(stream_key) = parse_stream_key_from_uri(&req.uri) else {
        send_response(
            command_tx,
            connection_id,
            req.cseq,
            400,
            "Bad Request",
            Vec::new(),
            Bytes::from_static(b"invalid describe uri"),
        )
        .await;
        return;
    };
    let base_uri = req.uri.trim_end_matches('/').to_string();
    let describe_pending = CancellationToken::new();
    {
        let mut guard = sessions.lock();
        let state = guard
            .entry(connection_id)
            .or_insert_with(|| RtspConnectionState::new(connection_id));
        state.mode = Some(SessionMode::Play);
        state.stream_key = Some(stream_key.clone());
        state.describe_base_uri = Some(base_uri.clone());
        state.play_response_range = None;
        state.describe_tracks.clear();
        state.describe_control_to_track.clear();
        state.describe_pending = Some(describe_pending.clone());
    }
    let runtime_api = engine.runtime_api.clone();
    let engine = engine.clone();
    let config = config.clone();
    let command_tx = command_tx.clone();
    let _ = runtime_api.spawn(Box::pin(async move {
        handle_describe_task(DescribeTaskContext {
            connection_id,
            req,
            stream_key,
            base_uri,
            describe_pending,
            engine,
            config,
            command_tx,
            sessions,
        })
        .await;
    }));
}

async fn handle_describe_task(ctx: DescribeTaskContext) {
    let DescribeTaskContext {
        connection_id,
        req,
        stream_key,
        base_uri,
        describe_pending,
        engine,
        config,
        command_tx,
        sessions,
    } = ctx;
    let describe_wait_timeout_ms = config.play_wait_source_timeout_ms;
    let describe_wait_deadline_micros = if describe_wait_timeout_ms == 0 {
        None
    } else {
        Some(
            runtime_now_micros(&engine.runtime_api)
                .saturating_add(describe_wait_timeout_ms.saturating_mul(1_000)),
        )
    };
    let snapshot = loop {
        match engine.stream_manager_api.get_stream(&stream_key).await {
            Ok(Some(snapshot)) => break snapshot,
            Ok(None) => {
                let Some(deadline_micros) = describe_wait_deadline_micros else {
                    finish_pending_describe(connection_id, &sessions, &describe_pending);
                    send_response(
                        &command_tx,
                        connection_id,
                        req.cseq,
                        404,
                        "Not Found",
                        Vec::new(),
                        Bytes::from_static(b"stream not found"),
                    )
                    .await;
                    return;
                };
                let now_micros = runtime_now_micros(&engine.runtime_api);
                if now_micros >= deadline_micros {
                    finish_pending_describe(connection_id, &sessions, &describe_pending);
                    send_response(
                        &command_tx,
                        connection_id,
                        req.cseq,
                        404,
                        "Not Found",
                        Vec::new(),
                        Bytes::from_static(b"stream not found"),
                    )
                    .await;
                    return;
                }
                let remaining_micros = deadline_micros.saturating_sub(now_micros);
                let sleep_micros = remaining_micros
                    .min(DESCRIBE_WAIT_SOURCE_POLL_INTERVAL_MS.saturating_mul(1_000));
                if wait_or_cancel(
                    &engine.runtime_api,
                    &describe_pending,
                    Duration::from_micros(sleep_micros.max(1)),
                )
                .await
                {
                    finish_pending_describe(connection_id, &sessions, &describe_pending);
                    return;
                }
            }
            Err(err) => {
                finish_pending_describe(connection_id, &sessions, &describe_pending);
                send_response(
                    &command_tx,
                    connection_id,
                    req.cseq,
                    500,
                    "Internal Server Error",
                    Vec::new(),
                    Bytes::from(err.to_string()),
                )
                .await;
                return;
            }
        }
    };

    let supported_tracks: Vec<TrackInfo> = snapshot
        .tracks
        .into_iter()
        .filter(|track| rtsp_codec_supported_for_play(track.codec))
        .collect();

    // Wait for tracks to become ready (have codec config/extradata populated).
    // This handles cross-protocol scenarios where RTMP sequence headers arrive
    // after the first media frame, or RTSP in-band parameter sets haven't been
    // discovered yet.
    let supported_tracks = if supported_tracks
        .iter()
        .any(|t| t.readiness != cheetah_codec::TrackReadiness::Ready)
    {
        let track_ready_timeout_ms = config.track_ready_timeout_ms;
        if track_ready_timeout_ms == 0 {
            supported_tracks
        } else {
            let track_ready_deadline_micros = runtime_now_micros(&engine.runtime_api)
                .saturating_add(track_ready_timeout_ms.saturating_mul(1_000));
            let mut ready_tracks = supported_tracks;
            loop {
                if ready_tracks
                    .iter()
                    .all(|t| t.readiness == cheetah_codec::TrackReadiness::Ready)
                {
                    break ready_tracks;
                }
                let now_micros = runtime_now_micros(&engine.runtime_api);
                if now_micros >= track_ready_deadline_micros {
                    // Timeout: use all tracks as-is (best effort)
                    break ready_tracks;
                }
                let sleep_micros = track_ready_deadline_micros
                    .saturating_sub(now_micros)
                    .min(DESCRIBE_WAIT_SOURCE_POLL_INTERVAL_MS.saturating_mul(1_000));
                if wait_or_cancel(
                    &engine.runtime_api,
                    &describe_pending,
                    Duration::from_micros(sleep_micros.max(1)),
                )
                .await
                {
                    finish_pending_describe(connection_id, &sessions, &describe_pending);
                    return;
                }
                // Re-fetch stream snapshot to check updated track readiness
                match engine.stream_manager_api.get_stream(&stream_key).await {
                    Ok(Some(updated)) => {
                        ready_tracks = updated
                            .tracks
                            .into_iter()
                            .filter(|track| rtsp_codec_supported_for_play(track.codec))
                            .collect();
                    }
                    _ => break ready_tracks,
                }
            }
        }
    } else {
        supported_tracks
    };
    if supported_tracks.is_empty() {
        finish_pending_describe(connection_id, &sessions, &describe_pending);
        send_response(
            &command_tx,
            connection_id,
            req.cseq,
            415,
            "Unsupported Media Type",
            Vec::new(),
            Bytes::from_static(b"no rtsp-compatible tracks found in stream"),
        )
        .await;
        return;
    }

    // If enable_mute_audio and stream has video but no audio, inject a synthetic
    // silent AAC track so RTSP players don't stall on missing audio.
    let supported_tracks = if config.enable_mute_audio
        && supported_tracks
            .iter()
            .any(|t| t.media_kind == cheetah_codec::MediaKind::Video)
        && !supported_tracks
            .iter()
            .any(|t| t.media_kind == cheetah_codec::MediaKind::Audio)
    {
        let mut tracks = supported_tracks;
        let mute_track_id =
            cheetah_codec::TrackId(tracks.iter().map(|t| t.track_id.0).max().unwrap_or(0) + 1);
        let mut mute_track = cheetah_codec::TrackInfo::new(
            mute_track_id,
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::AAC,
            48_000,
        );
        let asc_bytes = cheetah_codec::MuteAudioMaker::with_params(mute_track_id, 48_000, 2)
            .audio_specific_config();
        mute_track.extradata = cheetah_codec::CodecExtradata::AAC {
            asc: Bytes::copy_from_slice(&asc_bytes),
        };
        mute_track.sample_rate = Some(48_000);
        mute_track.channels = Some(2);
        mute_track.refresh_readiness();
        tracks.push(mute_track);
        tracks
    } else {
        supported_tracks
    };

    let (sdp, control_map) = build_describe_sdp(&base_uri, &supported_tracks);
    let session_id = {
        let mut guard = sessions.lock();
        let Some(state) = guard.get_mut(&connection_id) else {
            describe_pending.cancel();
            return;
        };
        state.mode = Some(SessionMode::Play);
        state.stream_key = Some(stream_key);
        state.describe_base_uri = Some(base_uri.clone());
        state.play_response_range = None;
        state.describe_tracks = supported_tracks;
        state.describe_control_to_track = control_map
            .iter()
            .map(|(k, v)| (normalize_control(k), *v))
            .collect();
        state.describe_pending = None;
        state.session_id.clone()
    };
    describe_pending.cancel();

    send_response(
        &command_tx,
        connection_id,
        req.cseq,
        200,
        "OK",
        vec![
            ("Content-Type".to_string(), "application/sdp".to_string()),
            ("Content-Base".to_string(), format!("{base_uri}/")),
            (
                "Session".to_string(),
                session_header_value(&session_id, config.session_timeout_secs),
            ),
        ],
        Bytes::from(sdp),
    )
    .await;
}

fn finish_pending_describe(
    connection_id: RtspConnectionId,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    describe_pending: &CancellationToken,
) {
    if let Some(state) = sessions.lock().get_mut(&connection_id) {
        state.describe_pending = None;
    }
    describe_pending.cancel();
}

pub(super) async fn handle_setup(
    connection_id: RtspConnectionId,
    req: RtspRequest,
    engine: &EngineContext,
    config: &RtspModuleConfig,
    command_tx: &RtspCoreCommandSender,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    multicast: Arc<MulticastSenderRegistry>,
) {
    let Some(transport) = header_value(&req, "transport") else {
        send_response(
            command_tx,
            connection_id,
            req.cseq,
            461,
            "Unsupported Transport",
            Vec::new(),
            Bytes::from_static(b"missing Transport header"),
        )
        .await;
        return;
    };
    let Some(parsed_transport) = parse_setup_transport(transport) else {
        send_response(
            command_tx,
            connection_id,
            req.cseq,
            461,
            "Unsupported Transport",
            Vec::new(),
            Bytes::from_static(b"unsupported Transport"),
        )
        .await;
        return;
    };

    let control = parse_track_control_from_uri(&req.uri).map(|v| normalize_control(&v));
    let setup_result: Result<(String, String), (u16, &'static str, &'static [u8])> = {
        let mut guard = sessions.lock();
        let state = guard
            .entry(connection_id)
            .or_insert_with(|| RtspConnectionState::new(connection_id));

        match state.mode {
            Some(SessionMode::Publish) => {
                if let Some(publish) = state.publish.as_mut() {
                    let track_id = match control.as_deref() {
                        Some(token) => state.announced_control_to_track.get(token).copied(),
                        None if publish.tracks.len() == 1 => publish.tracks.keys().copied().next(),
                        None => None,
                    };
                    if let Some(track_id) = track_id {
                        if publish_track_is_already_setup(publish, track_id) {
                            Err((
                                455,
                                "Method Not Valid in This State",
                                b"track transport is already configured",
                            ))
                        } else {
                            match parsed_transport {
                                RtspSetupTransport::TcpInterleaved(interleaved) => {
                                    if interleaved_channels_in_use(
                                        &publish.track_channels,
                                        &publish.rtcp_channels,
                                        interleaved.rtp_channel,
                                        interleaved.rtcp_channel,
                                    ) {
                                        Err((
                                            461,
                                            "Unsupported Transport",
                                            b"interleaved channel is already in use",
                                        ))
                                    } else {
                                        publish
                                            .track_channels
                                            .insert(interleaved.rtp_channel, track_id);
                                        publish
                                            .rtcp_channels
                                            .insert(interleaved.rtcp_channel, track_id);
                                        publish.clocks.entry(track_id).or_default();
                                        if let Some(track) = publish.tracks.get(&track_id) {
                                            if track.codec == cheetah_codec::CodecId::H264 {
                                                publish
                                                    .h264_depacketizers
                                                    .entry(track_id)
                                                    .or_default();
                                            } else if matches!(
                                                track.codec,
                                                cheetah_codec::CodecId::H265
                                                    | cheetah_codec::CodecId::H266
                                            ) {
                                                publish
                                                    .h265_depacketizers
                                                    .entry(track_id)
                                                    .or_default();
                                            }
                                        }
                                        Ok((
                                            state.session_id.clone(),
                                            format!(
                                                "RTP/AVP/TCP;unicast;interleaved={}-{}",
                                                interleaved.rtp_channel, interleaved.rtcp_channel
                                            ),
                                        ))
                                    }
                                }
                                RtspSetupTransport::TcpInterleavedAuto => {
                                    match next_publish_interleaved_channels(
                                        &publish.track_channels,
                                        &publish.rtcp_channels,
                                    ) {
                                        Some(interleaved) => {
                                            if interleaved_channels_in_use(
                                                &publish.track_channels,
                                                &publish.rtcp_channels,
                                                interleaved.rtp_channel,
                                                interleaved.rtcp_channel,
                                            ) {
                                                Err((
                                                    461,
                                                    "Unsupported Transport",
                                                    b"interleaved channel is already in use",
                                                ))
                                            } else {
                                                publish
                                                    .track_channels
                                                    .insert(interleaved.rtp_channel, track_id);
                                                publish
                                                    .rtcp_channels
                                                    .insert(interleaved.rtcp_channel, track_id);
                                                publish.clocks.entry(track_id).or_default();
                                                if let Some(track) = publish.tracks.get(&track_id)
                                                {
                                                    if track.codec == cheetah_codec::CodecId::H264 {
                                                        publish
                                                            .h264_depacketizers
                                                            .entry(track_id)
                                                            .or_default();
                                                    } else if matches!(
                                                        track.codec,
                                                        cheetah_codec::CodecId::H265
                                                            | cheetah_codec::CodecId::H266
                                                    ) {
                                                        publish
                                                            .h265_depacketizers
                                                            .entry(track_id)
                                                            .or_default();
                                                    }
                                                }
                                                Ok((
                                                    state.session_id.clone(),
                                                    format!(
                                                        "RTP/AVP/TCP;unicast;interleaved={}-{}",
                                                        interleaved.rtp_channel,
                                                        interleaved.rtcp_channel
                                                    ),
                                                ))
                                            }
                                        }
                                        None => Err((
                                            461,
                                            "Unsupported Transport",
                                            b"interleaved channel space exhausted",
                                        )),
                                    }
                                }
                                RtspSetupTransport::UdpUnicast(udp_ports) => {
                                    (|| -> Result<
                                        (String, String),
                                        (u16, &'static str, &'static [u8]),
                                    > {
                                        let peer = match state.peer_addr {
                                            Some(peer) => peer,
                                            None => {
                                                return Err((
                                                    454,
                                                    "Session Not Found",
                                                    b"peer address is unavailable",
                                                ));
                                            }
                                        };
                                        let bind_addr = wildcard_bind_addr(peer);
                                        let bind_attempts = config
                                            .udp
                                            .bind_pair_attempts
                                            .min(MAX_UDP_PORT_PAIR_BIND_ATTEMPTS);
                                        let (rtp_socket, rtcp_socket, server_rtp_port, server_rtcp_port) =
                                            match bind_udp_socket_pair(
                                                &engine.runtime_api,
                                                bind_addr,
                                                config.udp.server_port_pool_start,
                                                config.udp.server_port_pool_end,
                                                bind_attempts,
                                            ) {
                                                Ok(value) => value,
                                                Err(UdpSocketPairBindError::PoolExhausted) => {
                                                    return Err((
                                                        461,
                                                        "Unsupported Transport",
                                                        b"udp server port pool exhausted",
                                                    ));
                                                }
                                                Err(UdpSocketPairBindError::BindFailure) => {
                                                    return Err((
                                                        500,
                                                        "Internal Server Error",
                                                        b"allocate udp rtp/rtcp port pair failed",
                                                    ));
                                                }
                                            };
                                        let target_ip = match resolve_udp_destination_ip(
                                            peer,
                                            udp_ports.destination,
                                        ) {
                                            Ok(ip) => ip,
                                            Err(message) => {
                                                return Err((461, "Unsupported Transport", message));
                                            }
                                        };
                                        let target_rtp =
                                            SocketAddr::new(target_ip, udp_ports.client_rtp_port);
                                        let target_rtcp =
                                            SocketAddr::new(target_ip, udp_ports.client_rtcp_port);
                                        if config.udp.enable_hole_punching_probe {
                                            let runtime_api = engine.runtime_api.clone();
                                            let rtp_probe_socket = rtp_socket.clone();
                                            let rtcp_probe_socket = rtcp_socket.clone();
                                            let _ = runtime_api.spawn(Box::pin(async move {
                                                if let Err(err) = send_udp_hole_punch_probe(
                                                    &rtp_probe_socket,
                                                    &rtcp_probe_socket,
                                                    target_rtp,
                                                    target_rtcp,
                                                )
                                                .await
                                                {
                                                    warn!(
                                                        connection_id,
                                                        track_id = %track_id.0,
                                                        "udp hole punching probe failed: {}",
                                                        String::from_utf8_lossy(err)
                                                    );
                                                }
                                            }));
                                        }
                                        publish.clocks.entry(track_id).or_default();
                                        if let Some(track) = publish.tracks.get(&track_id) {
                                            if track.codec == cheetah_codec::CodecId::H264 {
                                                publish
                                                    .h264_depacketizers
                                                    .entry(track_id)
                                                    .or_default();
                                            } else if matches!(
                                                track.codec,
                                                cheetah_codec::CodecId::H265
                                                    | cheetah_codec::CodecId::H266
                                            ) {
                                                publish
                                                    .h265_depacketizers
                                                    .entry(track_id)
                                                    .or_default();
                                            }
                                        }
                                        let udp_track = PublishUdpTrack {
                                            rtp_socket,
                                            rtcp_socket,
                                            target_rtp,
                                            target_rtcp,
                                        };
                                        publish.udp_tracks.insert(track_id, udp_track.clone());

                                        let task_cancel = publish.cancel.child_token();
                                        publish.udp_task_handles.push(spawn_publish_rtp_udp_task(
                                            PublishUdpRtpTaskContext {
                                                runtime_api: engine.runtime_api.clone(),
                                                sessions: sessions.clone(),
                                                connection_id,
                                                track_id,
                                                rtp_socket: udp_track.rtp_socket.clone(),
                                                expected_remote: udp_track.target_rtp,
                                                enable_reorder: config.udp.enable_reorder_buffer,
                                                cancel: task_cancel.child_token(),
                                            },
                                        ));
                                        publish.udp_task_handles.push(spawn_publish_rtcp_udp_task(
                                            engine.runtime_api.clone(),
                                            sessions.clone(),
                                            connection_id,
                                            track_id,
                                            udp_track.rtcp_socket.clone(),
                                            udp_track.target_rtcp,
                                            task_cancel,
                                        ));

                                        Ok((
                                            state.session_id.clone(),
                                            format!(
                                                "RTP/AVP;unicast;client_port={}-{};server_port={}-{}",
                                                udp_ports.client_rtp_port,
                                                udp_ports.client_rtcp_port,
                                                server_rtp_port,
                                                server_rtcp_port,
                                            ),
                                        ))
                                    })()
                                }
                                RtspSetupTransport::UdpMulticast(_) => Err((
                                    461,
                                    "Unsupported Transport",
                                    b"multicast publish is not supported",
                                )),
                            }
                        }
                    } else if control.is_none() && publish.tracks.len() > 1 {
                        Err((
                            459,
                            "Aggregate Operation Not Allowed",
                            b"SETUP requires track control uri",
                        ))
                    } else {
                        Err((400, "Bad Request", b"unknown publish track"))
                    }
                } else {
                    Err((
                        455,
                        "Method Not Valid in This State",
                        b"missing ANNOUNCE before SETUP",
                    ))
                }
            }
            Some(SessionMode::Play) => {
                let track_id = match control.as_deref() {
                    Some(token) => state.describe_control_to_track.get(token).copied(),
                    None if state.describe_tracks.len() == 1 => {
                        state.describe_tracks.first().map(|track| track.track_id)
                    }
                    None => None,
                };
                if let Some(track_id) = track_id {
                    if let Some(track) = state
                        .describe_tracks
                        .iter()
                        .find(|track| track.track_id == track_id)
                        .cloned()
                    {
                        let track_ssrc = 0x1122_3344_u32.wrapping_add(track_id.0);
                        let transport_state_result: Result<
                            (PlayTransport, String),
                            (u16, &'static str, &'static [u8]),
                        > = match parsed_transport {
                                RtspSetupTransport::TcpInterleaved(interleaved) => {
                                    if play_interleaved_channels_conflict(
                                        &state.play_tracks,
                                        track_id,
                                        interleaved.rtp_channel,
                                        interleaved.rtcp_channel,
                                    ) {
                                        Err((
                                            461,
                                            "Unsupported Transport",
                                            b"interleaved channel is already in use",
                                        ))
                                    } else {
                                        let transport_response = format!(
                                            "RTP/AVP/TCP;unicast;interleaved={}-{};ssrc={}",
                                            interleaved.rtp_channel,
                                            interleaved.rtcp_channel,
                                            format_rtp_ssrc(track_ssrc)
                                        );
                                        Ok((
                                            PlayTransport::TcpInterleaved {
                                                rtp_channel: interleaved.rtp_channel,
                                                rtcp_channel: interleaved.rtcp_channel,
                                            },
                                            transport_response,
                                        ))
                                    }
                                }
                                RtspSetupTransport::TcpInterleavedAuto => {
                                    match next_play_interleaved_channels(
                                        &state.play_tracks,
                                        track_id,
                                    ) {
                                        Some(interleaved) => {
                                            if play_interleaved_channels_conflict(
                                                &state.play_tracks,
                                                track_id,
                                                interleaved.rtp_channel,
                                                interleaved.rtcp_channel,
                                            ) {
                                                Err((
                                                    461,
                                                    "Unsupported Transport",
                                                    b"interleaved channel is already in use",
                                                ))
                                            } else {
                                                let transport_response = format!(
                                                    "RTP/AVP/TCP;unicast;interleaved={}-{};ssrc={}",
                                                    interleaved.rtp_channel,
                                                    interleaved.rtcp_channel,
                                                    format_rtp_ssrc(track_ssrc)
                                                );
                                                Ok((
                                                    PlayTransport::TcpInterleaved {
                                                        rtp_channel: interleaved.rtp_channel,
                                                        rtcp_channel: interleaved.rtcp_channel,
                                                    },
                                                    transport_response,
                                                ))
                                            }
                                        }
                                        None => Err((
                                            461,
                                            "Unsupported Transport",
                                            b"interleaved channel space exhausted",
                                        )),
                                    }
                                }
                            RtspSetupTransport::UdpUnicast(udp_ports) => {
                                (|| -> Result<
                                    (PlayTransport, String),
                                    (u16, &'static str, &'static [u8]),
                                > {
                                    let peer = match state.peer_addr {
                                        Some(peer) => peer,
                                        None => {
                                            return Err((
                                                454,
                                                "Session Not Found",
                                                b"peer address is unavailable".as_slice(),
                                            ));
                                        }
                                    };
                                    let bind_addr = wildcard_bind_addr(peer);
                                    let bind_attempts = config
                                        .udp
                                        .bind_pair_attempts
                                        .min(MAX_UDP_PORT_PAIR_BIND_ATTEMPTS);
                                    let (rtp_socket, rtcp_socket, server_rtp_port, server_rtcp_port) =
                                        match bind_udp_socket_pair(
                                            &engine.runtime_api,
                                            bind_addr,
                                            config.udp.server_port_pool_start,
                                            config.udp.server_port_pool_end,
                                            bind_attempts,
                                        ) {
                                            Ok(value) => value,
                                            Err(UdpSocketPairBindError::PoolExhausted) => {
                                                return Err((
                                                    461,
                                                    "Unsupported Transport",
                                                    b"udp server port pool exhausted",
                                                ));
                                            }
                                            Err(UdpSocketPairBindError::BindFailure) => {
                                                return Err((
                                                    500,
                                                    "Internal Server Error",
                                                    b"allocate udp rtp/rtcp port pair failed",
                                                ));
                                            }
                                        };
                                    let target_ip = match resolve_udp_destination_ip(
                                        peer,
                                        udp_ports.destination,
                                    ) {
                                        Ok(ip) => ip,
                                        Err(message) => {
                                            return Err((461, "Unsupported Transport", message));
                                        }
                                    };
                                    let target_rtp =
                                        SocketAddr::new(target_ip, udp_ports.client_rtp_port);
                                    let target_rtcp =
                                        SocketAddr::new(target_ip, udp_ports.client_rtcp_port);
                                    if config.udp.enable_hole_punching_probe {
                                        let runtime_api = engine.runtime_api.clone();
                                        let rtp_probe_socket = rtp_socket.clone();
                                        let rtcp_probe_socket = rtcp_socket.clone();
                                        let _ = runtime_api.spawn(Box::pin(async move {
                                            if let Err(err) = send_udp_hole_punch_probe(
                                                &rtp_probe_socket,
                                                &rtcp_probe_socket,
                                                target_rtp,
                                                target_rtcp,
                                            )
                                            .await
                                            {
                                                warn!(
                                                    connection_id,
                                                    track_id = %track_id.0,
                                                    "udp hole punching probe failed: {}",
                                                    String::from_utf8_lossy(err)
                                                );
                                            }
                                        }));
                                    }

                                    let transport_response = format!(
                                        "RTP/AVP;unicast;client_port={}-{};server_port={}-{};ssrc={}",
                                        udp_ports.client_rtp_port,
                                        udp_ports.client_rtcp_port,
                                        server_rtp_port,
                                        server_rtcp_port,
                                        format_rtp_ssrc(track_ssrc),
                                    );
                                    Ok((
                                        PlayTransport::UdpUnicast {
                                            rtp_socket,
                                            rtcp_socket,
                                            target_rtp,
                                            target_rtcp,
                                        },
                                        transport_response,
                                    ))
                                })()
                            }
                            RtspSetupTransport::UdpMulticast(requested) => {
                                (|| -> Result<
                                    (PlayTransport, String),
                                    (u16, &'static str, &'static [u8]),
                                > {
                                    if !config.multicast.enabled {
                                        return Err((
                                            461,
                                            "Unsupported Transport",
                                            b"multicast transport is disabled",
                                        ));
                                    }
                                    if let (Some(rtp_port), Some(rtcp_port)) =
                                        (requested.rtp_port, requested.rtcp_port)
                                    {
                                        if rtcp_port != rtp_port.saturating_add(1) {
                                            return Err((
                                                461,
                                                "Unsupported Transport",
                                                b"invalid multicast port pair",
                                            ));
                                        }
                                    }
                                    if let Some(requested_destination) = requested.destination {
                                        if !requested_destination.is_multicast() {
                                            return Err((
                                                461,
                                                "Unsupported Transport",
                                                b"invalid multicast destination",
                                            ));
                                        }
                                    }
                                    let Some(stream_key) = state.stream_key.clone() else {
                                        return Err((454, "Session Not Found", b""));
                                    };
                                    let now_micros = runtime_unix_time_micros(&engine.runtime_api);
                                    let sender = multicast
                                        .acquire(
                                            &engine.runtime_api,
                                            now_micros,
                                            connection_id,
                                            &stream_key,
                                            track_id,
                                        )
                                        .map_err(MulticastAcquireError::rtsp_response)?;
                                    if let Some(requested_ttl) = requested.ttl {
                                        if requested_ttl != sender.ttl {
                                            warn!(
                                                connection_id,
                                                requested_ttl,
                                                configured_ttl = sender.ttl,
                                                "multicast setup requested ttl differs from configured pool ttl"
                                            );
                                        }
                                    }
                                    let target_rtp = sender.target_rtp();
                                    let target_rtcp = sender.target_rtcp();
                                    let transport_response = format!(
                                        "RTP/AVP;multicast;destination={};port={}-{};ttl={};ssrc={}",
                                        sender.destination,
                                        sender.rtp_port,
                                        sender.rtcp_port,
                                        sender.ttl,
                                        format_rtp_ssrc(track_ssrc),
                                    );
                                    Ok((
                                        PlayTransport::UdpMulticast {
                                            rtp_socket: sender.rtp_socket.clone(),
                                            rtcp_socket: sender.rtcp_socket.clone(),
                                            target_rtp,
                                            target_rtcp,
                                            stream_key,
                                            track_id,
                                        },
                                        transport_response,
                                    ))
                                })()
                            }
                        };
                        match transport_state_result {
                            Ok(transport_state) => {
                                let payload_type = track
                                    .payload_type
                                    .unwrap_or_else(|| default_payload_type(track.codec));
                                let next_track = PlayTrackState {
                                    transport: transport_state.0,
                                    payload_type,
                                    seq: (connection_id as u16).wrapping_add(track_id.0 as u16),
                                    ssrc: track_ssrc,
                                    packets_sent: 0,
                                    octets_sent: 0,
                                    last_rtp_timestamp: 0,
                                    timestamp_repair_count: 0,
                                    sdes_sent: false,
                                };
                                let previous =
                                    state.play_tracks.insert(track_id, next_track.clone());
                                release_replaced_multicast_play_track(
                                    &multicast,
                                    &engine.runtime_api,
                                    connection_id,
                                    track_id,
                                    previous,
                                    &next_track.transport,
                                );
                                Ok((state.session_id.clone(), transport_state.1))
                            }
                            Err(err) => Err(err),
                        }
                    } else if control.is_none() && state.describe_tracks.len() > 1 {
                        Err((
                            459,
                            "Aggregate Operation Not Allowed",
                            b"SETUP requires track control uri",
                        ))
                    } else {
                        Err((404, "Not Found", b"track unavailable"))
                    }
                } else {
                    if control.is_none() && state.describe_tracks.len() > 1 {
                        Err((
                            459,
                            "Aggregate Operation Not Allowed",
                            b"SETUP requires track control uri",
                        ))
                    } else {
                        Err((404, "Not Found", b"unknown play track"))
                    }
                }
            }
            None => Err((
                455,
                "Method Not Valid in This State",
                b"missing ANNOUNCE or DESCRIBE before SETUP",
            )),
        }
    };

    let (session_id, transport_response) = match setup_result {
        Ok(result) => result,
        Err((code, reason, message)) => {
            send_response(
                command_tx,
                connection_id,
                req.cseq,
                code,
                reason,
                Vec::new(),
                Bytes::from_static(message),
            )
            .await;
            return;
        }
    };
    send_response(
        command_tx,
        connection_id,
        req.cseq,
        200,
        "OK",
        vec![
            ("Transport".to_string(), transport_response),
            (
                "Session".to_string(),
                session_header_value(&session_id, config.session_timeout_secs),
            ),
        ],
        Bytes::new(),
    )
    .await;
}

// `handle_play` mirrors the other RTSP method handlers in this module
// (`handle_describe`, `handle_setup`, `handle_pause`), which all receive the
// same per-request/session/config context. It additionally requires
// `module_cancel` because PLAY may spawn long-lived background tasks that
// outlive the request, pushing the argument count to 8. Introducing a
// context struct just for this one handler would hide the uniformity with
// its siblings without reducing coupling, so we explicitly allow the lint.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_play(
    connection_id: RtspConnectionId,
    request: PlayRequestMeta,
    engine: &EngineContext,
    config: &RtspModuleConfig,
    command_tx: &RtspCoreCommandSender,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    multicast: Arc<MulticastSenderRegistry>,
    module_cancel: CancellationToken,
) {
    let PlayRequestMeta {
        cseq,
        requested_range,
    } = request;
    let play_context: Result<_, (u16, &'static str, &'static [u8])> = {
        let guard = sessions.lock();
        if let Some(state) = guard.get(&connection_id) {
            if state.mode != Some(SessionMode::Play) {
                Err((
                    455,
                    "Method Not Valid in This State",
                    b"PLAY requires DESCRIBE/SETUP",
                ))
            } else if let Some(stream_key) = state.stream_key.clone() {
                let mut track_map = HashMap::new();
                for track in &state.describe_tracks {
                    track_map.insert(track.track_id, track.clone());
                }
                let rtp_info = build_rtp_info_header(
                    state.describe_base_uri.as_deref(),
                    &state.describe_control_to_track,
                    &state.play_tracks,
                );
                Ok((stream_key, state.play_tracks.clone(), track_map, rtp_info))
            } else {
                Err((454, "Session Not Found", b""))
            }
        } else {
            Err((454, "Session Not Found", b""))
        }
    };
    let (stream_key, play_tracks, track_map, rtp_info) = match play_context {
        Ok(context) => context,
        Err((code, reason, message)) => {
            send_response(
                command_tx,
                connection_id,
                cseq,
                code,
                reason,
                Vec::new(),
                Bytes::from_static(message),
            )
            .await;
            return;
        }
    };

    if play_tracks.is_empty() {
        send_response(
            command_tx,
            connection_id,
            cseq,
            455,
            "Method Not Valid in This State",
            Vec::new(),
            Bytes::from_static(b"PLAY requires SETUP"),
        )
        .await;
        return;
    }

    let has_video_track = selected_play_tracks_have_video(&track_map, &play_tracks);
    let video_bootstrap_floor =
        selected_play_tracks_video_bootstrap_floor(&track_map, &play_tracks);
    let (bootstrap_policy, subscriber_queue_capacity, wait_for_video_keyframe) =
        resolve_play_subscription_settings(
            has_video_track,
            video_bootstrap_floor,
            config.start_from_keyframe,
            config.bootstrap_max_frames,
            config.subscriber_queue_capacity,
        );
    let stream_key_for_logs = stream_key.clone();

    let mut subscriber = match engine
        .subscriber_api
        .subscribe(
            stream_key,
            SubscriberOptions {
                queue_capacity: subscriber_queue_capacity,
                backpressure: config.subscriber_backpressure,
                bootstrap_policy,
                ..Default::default()
            },
        )
        .await
    {
        Ok(subscriber) => subscriber,
        Err(err) => {
            send_response(
                command_tx,
                connection_id,
                cseq,
                404,
                "Not Found",
                Vec::new(),
                Bytes::from(err.to_string()),
            )
            .await;
            return;
        }
    };

    let mut per_track = play_tracks;
    for state in per_track.values_mut() {
        // Preserve RTP seq/rtptime continuity across PAUSE/PLAY, but restart
        // per-PLAY RTCP startup signaling (SDES + first SR) deterministically.
        state.packets_sent = 0;
        state.octets_sent = 0;
        state.sdes_sent = false;
    }
    let play_cancel = module_cancel.child_token();
    let play_cancel_child = play_cancel.child_token();
    let command_tx_clone = command_tx.clone();
    let runtime_api = engine.runtime_api.clone();
    let runtime_api_ref = runtime_api.clone();
    let runtime_api_for_task = runtime_api.clone();
    let multicast_for_task = multicast.clone();
    let sessions_for_task = sessions.clone();
    let (start_play_tx, mut start_play_rx) = runtime_api.oneshot();
    let configured_mtu = config.rtp_mtu;
    let startup_alert_threshold_ms = config.alert_thresholds.startup_timeout_ms;
    let timestamp_repair_alert_threshold = config.alert_thresholds.timestamp_repair_count;
    // Build per-track parameter set caches for cross-protocol keyframe repair.
    // When the source is RTMP, keyframe payloads don't contain SPS/PPS inline;
    // we must prepend them from TrackInfo.extradata before RTP packetization.
    let mut play_parameter_set_caches: HashMap<TrackId, cheetah_codec::ParameterSetCache> =
        HashMap::new();
    for (track_id, track) in &track_map {
        if matches!(
            track.codec,
            cheetah_codec::CodecId::H264
                | cheetah_codec::CodecId::H265
                | cheetah_codec::CodecId::H266
        ) {
            let mut cache = cheetah_codec::ParameterSetCache::default();
            cache.update_from_extradata(&track.extradata);
            play_parameter_set_caches.insert(*track_id, cache);
        }
    }

    let join = runtime_api.spawn(Box::pin(async move {
        let mut started_tracks = if wait_for_video_keyframe {
            HashSet::new()
        } else {
            per_track.keys().copied().collect()
        };
        let mut play_start_pacing = PlayStartPacingState::default();
        let mut av_sync_aligner = cheetah_codec::AvSyncAligner::new();
        let play_start_micros = runtime_api_for_task.now().as_micros();
        let mut startup_alert_emitted = false;
        let cancel_before_start = play_cancel_child.cancelled().fuse();
        let start_fut = start_play_rx.recv().fuse();
        pin_mut!(cancel_before_start, start_fut);
        select_biased! {
            _ = cancel_before_start => {
                return;
            }
            start = start_fut => {
                if start.is_err() {
                    return;
                }
            }
        }

        loop {
            let cancel_fut = play_cancel_child.cancelled().fuse();
            let recv_fut = subscriber.recv().fuse();
            pin_mut!(cancel_fut, recv_fut);
            select_biased! {
                _ = cancel_fut => {
                    break;
                }
                recv = recv_fut => {
                    match recv {
                        Ok(Some(frame)) => {
                            if !startup_alert_emitted
                                && has_pending_selected_video_gate(
                                    &track_map,
                                    &per_track,
                                    &started_tracks,
                                    wait_for_video_keyframe,
                                )
                            {
                                let elapsed_micros =
                                    runtime_api_for_task
                                        .now()
                                        .as_micros()
                                        .saturating_sub(play_start_micros);
                                let elapsed_ms = elapsed_micros / 1_000;
                                if elapsed_ms >= startup_alert_threshold_ms {
                                    startup_alert_emitted = true;
                                    warn!(
                                        connection_id,
                                        stream_key = %stream_key_for_logs,
                                        startup_elapsed_ms = elapsed_ms,
                                        startup_alert_threshold_ms,
                                        "rtsp play startup wait exceeded alert threshold"
                                    );
                                }
                            }
                            let Some(track) = track_map.get(&frame.track_id) else {
                                continue;
                            };
                            if !should_forward_play_frame(
                                &per_track,
                                &mut started_tracks,
                                wait_for_video_keyframe,
                                track,
                                frame.track_id,
                                frame.flags,
                            ) {
                                continue;
                            }
                            let Some(state) = per_track.get_mut(&frame.track_id) else {
                                continue;
                            };
                            let fields = frame_observability_fields(frame.as_ref());
                            let packet_mtu = play_packet_mtu(&state.transport, configured_mtu);
                            if let Some(media_timestamp_ms) = frame_media_timestamp_ms(
                                track.media_kind,
                                frame.pts,
                                frame.dts,
                                frame.timebase,
                            ) {
                                let pacing_delay = play_start_pacing.delay_for(
                                    media_timestamp_ms,
                                    runtime_now_micros(&runtime_api_for_task),
                                    frame.flags.contains(cheetah_codec::FrameFlags::DISCONTINUITY),
                                );
                                if !pacing_delay.is_zero()
                                    && wait_or_cancel(
                                        &runtime_api_for_task,
                                        &play_cancel_child,
                                        pacing_delay,
                                    )
                                    .await
                                {
                                    break;
                                }
                            }

                            let (primary_ts, secondary_ts) =
                                media_timestamp_priority(track.media_kind, frame.pts, frame.dts);
                            // A/V sync alignment: adjust timestamps so audio and video
                            // share a common epoch for cross-protocol egress.
                            av_sync_aligner.on_frame(track.media_kind, frame.dts_us);
                            let canonical_raw_timestamp = media_ts_to_rtp_ticks(
                                primary_ts,
                                secondary_ts,
                                frame.timebase,
                                track.clock_rate.max(1),
                            );
                            let raw_timestamp =
                                source_rtp_timestamp_for_egress(frame.as_ref(), track.codec)
                                    .unwrap_or(canonical_raw_timestamp);
                            let normalized_timestamp = if state.packets_sent == 0
                                || frame.flags.contains(cheetah_codec::FrameFlags::DISCONTINUITY)
                                || should_skip_monotonic_repair_for_b_frames(
                                    track.media_kind,
                                    track.codec,
                                )
                            {
                                raw_timestamp
                            } else {
                                let repaired = cheetah_codec::repair_monotonic_timestamp(
                                    raw_timestamp,
                                    Some(state.last_rtp_timestamp),
                                    RTP_TIMESTAMP_BACKWARD_REPAIR_THRESHOLD_TICKS,
                                );
                                if repaired.repaired {
                                    state.timestamp_repair_count =
                                        state.timestamp_repair_count.saturating_add(1);
                                    let repair_count = state.timestamp_repair_count;
                                    if cheetah_codec::should_sample_timestamp_repair(repair_count)
                                    {
                                        warn!(
                                            connection_id,
                                            stream_key = %stream_key_for_logs,
                                            track_id = fields.track_id,
                                            codec = ?fields.codec,
                                            protocol_ingress = "rtsp-play",
                                            pts = fields.pts,
                                            dts = fields.dts,
                                            canonical_raw_timestamp,
                                            raw_timestamp,
                                            repaired_timestamp = repaired.timestamp,
                                            repair_count,
                                            "rtsp play rtp timestamp repaired for monotonic egress"
                                        );
                                    }
                                    if cheetah_codec::should_emit_alert_threshold(
                                        repair_count,
                                        timestamp_repair_alert_threshold,
                                    ) {
                                        warn!(
                                            connection_id,
                                            stream_key = %stream_key_for_logs,
                                            track_id = fields.track_id,
                                            codec = ?fields.codec,
                                            protocol_ingress = "rtsp-play",
                                            pts = fields.pts,
                                            dts = fields.dts,
                                            canonical_raw_timestamp,
                                            raw_timestamp,
                                            repaired_timestamp = repaired.timestamp,
                                            repair_count,
                                            repair_alert_threshold = timestamp_repair_alert_threshold,
                                            "rtsp play timestamp disorder alert threshold reached"
                                        );
                                    }
                                    repaired.timestamp
                                } else {
                                    raw_timestamp
                                }
                            };
                            if let PlayTransport::UdpMulticast {
                                stream_key,
                                track_id,
                                ..
                            } = &state.transport
                            {
                                if !multicast_for_task
                                    .should_forward_rtp(connection_id, stream_key, *track_id)
                                {
                                    continue;
                                }
                            }
                            // For cross-protocol scenarios (e.g. RTMP source), keyframes may
                            // not contain inline parameter sets. Prepend SPS/PPS/VPS from cache
                            // so RTP subscribers can decode without relying solely on SDP.
                            let effective_payload;
                            let effective_frame;
                            let frame_ref = if frame.flags.contains(cheetah_codec::FrameFlags::KEY) {
                                if let Some(ps_cache) = play_parameter_set_caches.get_mut(&frame.track_id) {
                                    // Update cache from frame in case parameter sets changed in-band
                                    ps_cache.update_from_annexb(frame.codec, frame.payload.as_ref());
                                    if ps_cache.has_required_sets(frame.codec) && frame.format == cheetah_codec::FrameFormat::CanonicalH26x {
                                        effective_payload = ps_cache.prepend_to_annexb_access_unit(frame.codec, frame.payload.as_ref());
                                        effective_frame = cheetah_codec::AVFrame {
                                            payload: effective_payload,
                                            ..frame.as_ref().clone()
                                        };
                                        &effective_frame
                                    } else {
                                        frame.as_ref()
                                    }
                                } else {
                                    frame.as_ref()
                                }
                            } else {
                                // Non-keyframes: still update cache if parameter sets appear
                                if let Some(ps_cache) = play_parameter_set_caches.get_mut(&frame.track_id) {
                                    ps_cache.update_from_annexb(frame.codec, frame.payload.as_ref());
                                }
                                frame.as_ref()
                            };
                            let packets = packetize_frame_to_rtp_with_timestamp(
                                frame_ref,
                                track,
                                state.payload_type,
                                &mut state.seq,
                                state.ssrc,
                                packet_mtu,
                                normalized_timestamp,
                            );
                            if packets.is_empty() {
                                warn!(
                                    connection_id,
                                    stream_key = %stream_key_for_logs,
                                    track_id = fields.track_id,
                                    codec = ?fields.codec,
                                    pts = fields.pts,
                                    dts = fields.dts,
                                    packet_mtu,
                                    "packetize play frame produced no RTP packets; drop frame"
                                );
                                continue;
                            }
                            for packet in packets {
                                let payload = packet.encode();
                                state.packets_sent = state.packets_sent.wrapping_add(1);
                                state.octets_sent = state
                                    .octets_sent
                                    .wrapping_add(packet.payload.len().min(u32::MAX as usize) as u32);
                                state.last_rtp_timestamp = packet.header.timestamp;
                                let mut rtp_send_failed = false;
                                match &state.transport {
                                    PlayTransport::TcpInterleaved { rtp_channel, .. } => {
                                        if command_tx_clone
                                            .send_core(
                                                connection_id,
                                                RtspCommand::SendInterleaved {
                                                    channel: *rtp_channel,
                                                    payload,
                                                },
                                            )
                                            .await
                                            .is_err()
                                        {
                                            rtp_send_failed = true;
                                        }
                                    }
                                    PlayTransport::UdpUnicast {
                                        rtp_socket,
                                        target_rtp,
                                        ..
                                    } => {
                                        if rtp_socket.send_to(&payload, *target_rtp).await.is_err() {
                                            rtp_send_failed = true;
                                        }
                                    }
                                    PlayTransport::UdpMulticast {
                                        rtp_socket,
                                        target_rtp,
                                        ..
                                    } => {
                                        if rtp_socket.send_to(&payload, *target_rtp).await.is_err() {
                                            rtp_send_failed = true;
                                        }
                                    }
                                }
                                if rtp_send_failed {
                                    warn!(
                                        connection_id,
                                        stream_key = %stream_key_for_logs,
                                        track_id = fields.track_id,
                                        codec = ?fields.codec,
                                        pts = fields.pts,
                                        dts = fields.dts,
                                        "send play rtp packet failed"
                                    );
                                    return;
                                }

                                if !state.sdes_sent {
                                    let sdes_packet = PlayRtcpPacket::SdesCname {
                                        ssrc: state.ssrc,
                                        cname: format!("cheetah-play-{connection_id}"),
                                    };
                                    match send_play_rtcp_packet(
                                        &command_tx_clone,
                                        connection_id,
                                        &state.transport,
                                        sdes_packet,
                                    )
                                    .await
                                    {
                                        Ok(()) => {
                                            state.sdes_sent = true;
                                        }
                                        Err(PlayRtcpSendError::Build { packet, detail }) => {
                                            warn!(
                                                connection_id,
                                                stream_key = %stream_key_for_logs,
                                                track_id = fields.track_id,
                                                codec = ?fields.codec,
                                                pts = fields.pts,
                                                dts = fields.dts,
                                                "build play rtcp {packet} failed: {detail}"
                                            );
                                            continue;
                                        }
                                        Err(PlayRtcpSendError::Send { packet }) => {
                                            warn!(
                                                connection_id,
                                                stream_key = %stream_key_for_logs,
                                                track_id = fields.track_id,
                                                codec = ?fields.codec,
                                                pts = fields.pts,
                                                dts = fields.dts,
                                                "send play rtcp {packet} failed"
                                            );
                                            return;
                                        }
                                    }
                                }

                                if should_emit_sender_report(state.packets_sent) {
                                    let sr_packet = PlayRtcpPacket::SenderReport {
                                        ssrc: state.ssrc,
                                        rtp_timestamp: state.last_rtp_timestamp,
                                        packets_sent: state.packets_sent,
                                        octets_sent: state.octets_sent,
                                        unix_time_micros: runtime_unix_time_micros(&runtime_api_ref),
                                    };
                                    match send_play_rtcp_packet(
                                        &command_tx_clone,
                                        connection_id,
                                        &state.transport,
                                        sr_packet,
                                    )
                                    .await
                                    {
                                        Ok(()) => {}
                                        Err(PlayRtcpSendError::Build { packet, detail }) => {
                                            warn!(
                                                connection_id,
                                                stream_key = %stream_key_for_logs,
                                                track_id = fields.track_id,
                                                codec = ?fields.codec,
                                                pts = fields.pts,
                                                dts = fields.dts,
                                                "build play rtcp {packet} failed: {detail}"
                                            );
                                            continue;
                                        }
                                        Err(PlayRtcpSendError::Send { packet }) => {
                                            warn!(
                                                connection_id,
                                                stream_key = %stream_key_for_logs,
                                                track_id = fields.track_id,
                                                codec = ?fields.codec,
                                                pts = fields.pts,
                                                dts = fields.dts,
                                                "send play rtcp {packet} failed"
                                            );
                                            return;
                                        }
                                    }
                                }
                            }

                            let updated_state = state.clone();
                            if let Some(connection_state) =
                                sessions_for_task.lock().get_mut(&connection_id)
                            {
                                connection_state
                                    .play_tracks
                                    .insert(frame.track_id, updated_state);
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            }
        }
    }));

    let (session_id, replaced, response_range) = {
        let mut guard = sessions.lock();
        let Some(state) = guard.get_mut(&connection_id) else {
            join.abort();
            return;
        };
        let replaced = state.play.take();
        state.play = Some(PlaySession {
            cancel: play_cancel,
            join,
        });
        let response_range = apply_play_response_range(state, requested_range);
        (state.session_id.clone(), replaced, response_range)
    };
    if let Some(old_play) = replaced {
        old_play.cancel.cancel();
        old_play.join.abort();
    }

    send_response(
        command_tx,
        connection_id,
        cseq,
        200,
        "OK",
        build_play_response_headers(
            session_id,
            response_range,
            rtp_info,
            config.session_timeout_secs,
        ),
        Bytes::new(),
    )
    .await;
    let _ = start_play_tx.send();
}

pub(super) async fn handle_pause(
    connection_id: RtspConnectionId,
    request: PauseRequestMeta,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    session_timeout_secs: u32,
) {
    let PauseRequestMeta {
        cseq,
        requested_range,
    } = request;
    let pause_result: Result<PauseResponse, RtspErrorResponse> = {
        let mut guard = sessions.lock();
        if let Some(state) = guard.get_mut(&connection_id) {
            let has_play = state.play.is_some();
            let has_publish = state.publish.is_some();
            let record_started = state.publish.as_ref().is_some_and(|p| p.record_started);
            if let Err(err) =
                validate_pause_state(state.mode, has_play, has_publish, record_started)
            {
                Err(err)
            } else {
                match state.mode {
                    Some(SessionMode::Play) => {
                        if let Some(play) = state.play.take() {
                            play.cancel.cancel();
                            play.join.abort();
                        }
                        let range = apply_pause_response_range(state, requested_range.clone());
                        Ok((state.session_id.clone(), Some(range)))
                    }
                    Some(SessionMode::Publish) => {
                        if let Some(publish) = state.publish.as_mut() {
                            flush_publish_video_reorder(publish);
                            publish.record_started = false;
                        }
                        Ok((state.session_id.clone(), None))
                    }
                    None => Err((
                        455,
                        "Method Not Valid in This State",
                        b"PAUSE requires session",
                    )),
                }
            }
        } else {
            Err((454, "Session Not Found", b""))
        }
    };

    match pause_result {
        Ok((session_id, response_range)) => {
            send_response(
                command_tx,
                connection_id,
                cseq,
                200,
                "OK",
                build_pause_response_headers(session_id, response_range, session_timeout_secs),
                Bytes::new(),
            )
            .await;
        }
        Err((code, reason, body)) => {
            send_response(
                command_tx,
                connection_id,
                cseq,
                code,
                reason,
                Vec::new(),
                Bytes::from_static(body),
            )
            .await;
        }
    }
}

pub(super) async fn send_play_rtcp_payload(
    command_tx: &RtspCoreCommandSender,
    connection_id: RtspConnectionId,
    transport: &PlayTransport,
    payload: Bytes,
) -> Result<(), ()> {
    match transport {
        PlayTransport::TcpInterleaved { rtcp_channel, .. } => command_tx
            .send_core(
                connection_id,
                RtspCommand::SendInterleaved {
                    channel: *rtcp_channel,
                    payload,
                },
            )
            .await
            .map_err(|_| ()),
        PlayTransport::UdpUnicast {
            rtcp_socket,
            target_rtcp,
            ..
        } => rtcp_socket
            .send_to(&payload, *target_rtcp)
            .await
            .map(|_| ())
            .map_err(|_| ()),
        PlayTransport::UdpMulticast {
            rtcp_socket,
            target_rtcp,
            ..
        } => rtcp_socket
            .send_to(&payload, *target_rtcp)
            .await
            .map(|_| ())
            .map_err(|_| ()),
    }
}

#[derive(Debug, Clone)]
pub(super) enum PlayRtcpPacket {
    SdesCname {
        ssrc: u32,
        cname: String,
    },
    SenderReport {
        ssrc: u32,
        rtp_timestamp: u32,
        packets_sent: u32,
        octets_sent: u32,
        unix_time_micros: u64,
    },
    Bye {
        ssrc: u32,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlayRtcpSendError {
    Build {
        packet: &'static str,
        detail: String,
    },
    Send {
        packet: &'static str,
    },
}

pub(super) async fn send_play_rtcp_packet(
    command_tx: &RtspCoreCommandSender,
    connection_id: RtspConnectionId,
    transport: &PlayTransport,
    packet: PlayRtcpPacket,
) -> Result<(), PlayRtcpSendError> {
    let (packet_name, payload) = match packet {
        PlayRtcpPacket::SdesCname { ssrc, cname } => {
            let payload =
                build_rtcp_sdes_cname(ssrc, &cname).map_err(|err| PlayRtcpSendError::Build {
                    packet: "SDES",
                    detail: err.to_string(),
                })?;
            ("SDES", payload)
        }
        PlayRtcpPacket::SenderReport {
            ssrc,
            rtp_timestamp,
            packets_sent,
            octets_sent,
            unix_time_micros,
        } => {
            let payload = build_rtcp_sender_report(
                ssrc,
                rtp_timestamp,
                packets_sent,
                octets_sent,
                unix_time_micros,
            )
            .map_err(|err| PlayRtcpSendError::Build {
                packet: "SR",
                detail: err.to_string(),
            })?;
            ("SR", payload)
        }
        PlayRtcpPacket::Bye { ssrc, reason } => {
            let payload = build_rtcp_bye(ssrc, reason.as_deref()).map_err(|err| {
                PlayRtcpSendError::Build {
                    packet: "BYE",
                    detail: err.to_string(),
                }
            })?;
            ("BYE", payload)
        }
    };
    send_play_rtcp_payload(command_tx, connection_id, transport, payload)
        .await
        .map_err(|_| PlayRtcpSendError::Send {
            packet: packet_name,
        })
}

pub(super) fn should_emit_sender_report(packets_sent: u32) -> bool {
    packets_sent == 1 || packets_sent.is_multiple_of(200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{AVFrame, FrameFormat, Timebase};

    struct NullUdpSocket;

    #[async_trait::async_trait]
    impl cheetah_sdk::AsyncUdpSocket for NullUdpSocket {
        async fn recv_from(&self, _buf: &mut [u8]) -> std::io::Result<cheetah_sdk::UdpRecvMeta> {
            Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "null"))
        }

        async fn send_to(&self, _buf: &[u8], _target: SocketAddr) -> std::io::Result<usize> {
            Ok(0)
        }

        fn local_addr(&self) -> std::io::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 0)))
        }
    }

    fn tcp_play_track_state() -> PlayTrackState {
        PlayTrackState {
            transport: PlayTransport::TcpInterleaved {
                rtp_channel: 0,
                rtcp_channel: 1,
            },
            payload_type: 96,
            seq: 0,
            ssrc: 1,
            packets_sent: 0,
            octets_sent: 0,
            last_rtp_timestamp: 0,
            timestamp_repair_count: 0,
            sdes_sent: false,
        }
    }

    #[test]
    fn tcp_play_packet_mtu_allows_oversized_passthrough_payload() {
        let track = TrackInfo::new(
            TrackId(3),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::AV1,
            90_000,
        );
        let payload: Vec<u8> = (0..20).map(|v| v as u8).collect();
        let frame = AVFrame::new(
            TrackId(3),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::AV1,
            FrameFormat::DataPacket,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from(payload),
        );
        let state = tcp_play_track_state();
        let mut seq = 100u16;
        let configured_mtu = 23usize;
        let packet_mtu = play_packet_mtu(&state.transport, configured_mtu);

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x0102_0304,
            packet_mtu,
            900,
        );

        assert_eq!(
            packets.len(),
            1,
            "TCP interleaved PLAY must not be limited by configured UDP rtp_mtu"
        );
    }

    #[test]
    fn tcp_play_packet_mtu_is_bounded_by_interleaved_u16_length() {
        let track = TrackInfo::new(
            TrackId(5),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::AV1,
            90_000,
        );
        let payload = vec![0x7fu8; (u16::MAX as usize - 12) + 1];
        let frame = AVFrame::new(
            TrackId(5),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::AV1,
            FrameFormat::DataPacket,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from(payload),
        );
        let state = tcp_play_track_state();
        let mut seq = 33u16;
        let packet_mtu = play_packet_mtu(&state.transport, 1200);

        assert!(
            packet_mtu <= u16::MAX as usize,
            "TCP interleaved packet mtu must fit RTSP interleaved 16-bit length field"
        );

        let packets = packetize_frame_to_rtp_with_timestamp(
            &frame,
            &track,
            96,
            &mut seq,
            0x1111_2222,
            packet_mtu,
            1234,
        );

        assert!(
            packets.is_empty(),
            "payload larger than RTSP interleaved 16-bit frame length must not be packetized"
        );
    }

    #[test]
    fn udp_play_packet_mtu_uses_configured_mtu() {
        let configured_mtu = 1200usize;
        let transport = PlayTransport::UdpUnicast {
            rtp_socket: Arc::new(NullUdpSocket),
            rtcp_socket: Arc::new(NullUdpSocket),
            target_rtp: SocketAddr::from(([127, 0, 0, 1], 5000)),
            target_rtcp: SocketAddr::from(([127, 0, 0, 1], 5001)),
        };

        assert_eq!(play_packet_mtu(&transport, configured_mtu), configured_mtu);
    }

    #[test]
    fn multicast_play_packet_mtu_uses_configured_mtu() {
        let configured_mtu = 1300usize;
        let transport = PlayTransport::UdpMulticast {
            rtp_socket: Arc::new(NullUdpSocket),
            rtcp_socket: Arc::new(NullUdpSocket),
            target_rtp: SocketAddr::from(([239, 1, 0, 1], 63000)),
            target_rtcp: SocketAddr::from(([239, 1, 0, 1], 63001)),
            stream_key: cheetah_sdk::StreamKey::new("live", "cam01"),
            track_id: TrackId(1),
        };

        assert_eq!(play_packet_mtu(&transport, configured_mtu), configured_mtu);
    }

    #[test]
    fn selected_play_tracks_have_video_uses_only_setup_tracks() {
        let mut track_map = HashMap::new();
        track_map.insert(
            TrackId(1),
            TrackInfo::new(
                TrackId(1),
                cheetah_codec::MediaKind::Video,
                cheetah_codec::CodecId::H264,
                90_000,
            ),
        );
        track_map.insert(
            TrackId(2),
            TrackInfo::new(
                TrackId(2),
                cheetah_codec::MediaKind::Audio,
                cheetah_codec::CodecId::AAC,
                48_000,
            ),
        );

        let mut play_tracks = HashMap::new();
        play_tracks.insert(TrackId(2), tcp_play_track_state());
        assert!(!selected_play_tracks_have_video(&track_map, &play_tracks));

        play_tracks.insert(TrackId(1), tcp_play_track_state());
        assert!(selected_play_tracks_have_video(&track_map, &play_tracks));
    }

    #[test]
    fn play_subscription_settings_keep_configured_bootstrap_for_video_tracks() {
        let (bootstrap_policy, subscriber_queue_capacity, wait_for_video_keyframe) =
            resolve_play_subscription_settings(true, 1024, true, 150, 256);
        assert!(wait_for_video_keyframe);
        assert_eq!(bootstrap_policy.mode, BootstrapMode::LiveTail);
        assert_eq!(bootstrap_policy.max_bootstrap_frames, 1024);
        assert!(bootstrap_policy.wait_for_next_random_access_point);
        assert_eq!(subscriber_queue_capacity, 1024);

        let (bootstrap_policy, subscriber_queue_capacity, _) =
            resolve_play_subscription_settings(true, 1024, true, 1200, 200);
        assert_eq!(bootstrap_policy.max_bootstrap_frames, 1200);
        assert_eq!(subscriber_queue_capacity, 1200);
    }

    #[test]
    fn play_subscription_settings_expands_bootstrap_for_high_packet_video_codecs() {
        let (bootstrap_policy, subscriber_queue_capacity, wait_for_video_keyframe) =
            resolve_play_subscription_settings(true, 2048, true, 150, 512);
        assert!(wait_for_video_keyframe);
        assert_eq!(bootstrap_policy.max_bootstrap_frames, 2048);
        assert_eq!(subscriber_queue_capacity, 2048);
    }

    #[test]
    fn selected_play_tracks_video_bootstrap_floor_covers_all_rtsp_video_codecs() {
        let video_codecs = [
            (cheetah_codec::CodecId::H264, 1024usize),
            (cheetah_codec::CodecId::VP8, 1024usize),
            (cheetah_codec::CodecId::H265, 2048usize),
            (cheetah_codec::CodecId::H266, 2048usize),
            (cheetah_codec::CodecId::AV1, 2048usize),
            (cheetah_codec::CodecId::VP9, 2048usize),
        ];

        for (codec, expected_floor) in video_codecs {
            let track_id = TrackId(9);
            let mut track_map = HashMap::new();
            track_map.insert(
                track_id,
                TrackInfo::new(track_id, cheetah_codec::MediaKind::Video, codec, 90_000),
            );
            let mut play_tracks = HashMap::new();
            play_tracks.insert(track_id, tcp_play_track_state());

            assert_eq!(
                selected_play_tracks_video_bootstrap_floor(&track_map, &play_tracks),
                expected_floor,
                "unexpected floor for codec {:?}",
                codec
            );
        }
    }

    #[test]
    fn media_ts_to_rtp_ticks_preserves_native_clock_timebase() {
        let ticks =
            media_ts_to_rtp_ticks(817_500, 0, cheetah_codec::Timebase::new(1, 90_000), 90_000);
        assert_eq!(ticks, 817_500);
    }

    #[test]
    fn media_ts_to_rtp_ticks_wraps_at_u32_boundary() {
        let ticks = media_ts_to_rtp_ticks(
            i64::from(u32::MAX) + 1,
            0,
            cheetah_codec::Timebase::new(1, 90_000),
            90_000,
        );
        assert_eq!(ticks, 0);
    }

    #[test]
    fn media_ts_to_rtp_ticks_falls_back_when_primary_timestamp_is_unknown() {
        let ticks =
            media_ts_to_rtp_ticks(-1, 90_000, cheetah_codec::Timebase::new(1, 90_000), 90_000);
        assert_eq!(ticks, 90_000);
    }

    #[test]
    fn media_ts_to_rtp_ticks_keeps_primary_zero_timestamp() {
        let video_ticks =
            media_ts_to_rtp_ticks(0, 9_000, cheetah_codec::Timebase::new(1, 1_000), 90_000);
        assert_eq!(video_ticks, 0, "video must keep pts=0 as valid timestamp");

        let audio_ticks =
            media_ts_to_rtp_ticks(0, 1_024, cheetah_codec::Timebase::new(1, 1_000), 48_000);
        assert_eq!(audio_ticks, 0, "audio must keep dts=0 as valid timestamp");
    }

    #[test]
    fn media_timestamp_priority_prefers_pts_for_video_and_dts_for_audio() {
        let video = media_timestamp_priority(cheetah_codec::MediaKind::Video, 9_000, 3_000);
        assert_eq!(video, (9_000, 3_000));

        let audio = media_timestamp_priority(cheetah_codec::MediaKind::Audio, 9_000, 3_000);
        assert_eq!(audio, (3_000, 9_000));
    }

    #[test]
    fn play_start_gate_ignores_unselected_video_track_frames() {
        let selected_track = TrackId(1);
        let unselected_track = TrackId(2);
        let selected_video = TrackInfo::new(
            selected_track,
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );
        let unselected_video = TrackInfo::new(
            unselected_track,
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );

        let mut per_track = HashMap::new();
        per_track.insert(selected_track, tcp_play_track_state());

        let mut started_tracks = HashSet::new();
        let wait_for_video_keyframe = true;
        assert!(!should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            wait_for_video_keyframe,
            &unselected_video,
            unselected_track,
            cheetah_codec::FrameFlags::KEY,
        ));
        assert!(
            !started_tracks.contains(&selected_track),
            "unselected track must not unlock play start gate"
        );

        assert!(!should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            wait_for_video_keyframe,
            &selected_video,
            selected_track,
            cheetah_codec::FrameFlags::empty(),
        ));
    }

    #[test]
    fn play_start_gate_requires_keyframe_for_h265_h266() {
        for codec in [cheetah_codec::CodecId::H265, cheetah_codec::CodecId::H266] {
            let track_id = TrackId(1);
            let video_track =
                TrackInfo::new(track_id, cheetah_codec::MediaKind::Video, codec, 90_000);
            let mut per_track = HashMap::new();
            per_track.insert(track_id, tcp_play_track_state());

            let mut started_tracks = HashSet::new();
            let wait_for_video_keyframe = true;
            assert!(!should_forward_play_frame(
                &per_track,
                &mut started_tracks,
                wait_for_video_keyframe,
                &video_track,
                track_id,
                cheetah_codec::FrameFlags::empty(),
            ));
            assert!(!started_tracks.contains(&track_id));
        }
    }

    #[test]
    fn play_start_gate_requires_keyframe_for_av1_vp8_vp9() {
        for codec in [
            cheetah_codec::CodecId::AV1,
            cheetah_codec::CodecId::VP8,
            cheetah_codec::CodecId::VP9,
        ] {
            let track_id = TrackId(3);
            let video_track =
                TrackInfo::new(track_id, cheetah_codec::MediaKind::Video, codec, 90_000);
            let mut per_track = HashMap::new();
            per_track.insert(track_id, tcp_play_track_state());

            let mut started_tracks = HashSet::new();
            assert!(!should_forward_play_frame(
                &per_track,
                &mut started_tracks,
                true,
                &video_track,
                track_id,
                cheetah_codec::FrameFlags::empty(),
            ));
            assert!(
                !started_tracks.contains(&track_id),
                "{codec:?} must wait for keyframe"
            );
        }
    }

    #[test]
    fn play_start_gate_allows_audio_before_video_keyframe() {
        let video_track_id = TrackId(1);
        let audio_track_id = TrackId(2);
        let video_track = TrackInfo::new(
            video_track_id,
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );
        let audio_track = TrackInfo::new(
            audio_track_id,
            cheetah_codec::MediaKind::Audio,
            cheetah_codec::CodecId::AAC,
            48_000,
        );
        let mut per_track = HashMap::new();
        per_track.insert(video_track_id, tcp_play_track_state());
        per_track.insert(audio_track_id, tcp_play_track_state());

        let mut started_tracks = HashSet::new();
        assert!(should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            true,
            &audio_track,
            audio_track_id,
            cheetah_codec::FrameFlags::empty(),
        ));
        assert!(started_tracks.contains(&audio_track_id));
        assert!(!started_tracks.contains(&video_track_id));

        assert!(!should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            true,
            &video_track,
            video_track_id,
            cheetah_codec::FrameFlags::empty(),
        ));
        assert!(should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            true,
            &video_track,
            video_track_id,
            cheetah_codec::FrameFlags::KEY,
        ));
        assert!(started_tracks.contains(&video_track_id));
    }

    #[test]
    fn play_start_gate_is_independent_per_selected_video_track() {
        let track_a = TrackId(11);
        let track_b = TrackId(12);
        let video_a = TrackInfo::new(
            track_a,
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );
        let video_b = TrackInfo::new(
            track_b,
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::H264,
            90_000,
        );

        let mut per_track = HashMap::new();
        per_track.insert(track_a, tcp_play_track_state());
        per_track.insert(track_b, tcp_play_track_state());

        let mut started_tracks = HashSet::new();
        assert!(should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            true,
            &video_a,
            track_a,
            cheetah_codec::FrameFlags::KEY,
        ));
        assert!(started_tracks.contains(&track_a));
        assert!(!started_tracks.contains(&track_b));

        assert!(
            !should_forward_play_frame(
                &per_track,
                &mut started_tracks,
                true,
                &video_b,
                track_b,
                cheetah_codec::FrameFlags::empty(),
            ),
            "other selected video tracks must keep waiting until their own keyframe"
        );
        assert!(should_forward_play_frame(
            &per_track,
            &mut started_tracks,
            true,
            &video_b,
            track_b,
            cheetah_codec::FrameFlags::KEY,
        ));
        assert!(started_tracks.contains(&track_b));
    }

    #[test]
    fn raw_rtp_timestamp_preservation_codec_matrix() {
        for codec in [
            cheetah_codec::CodecId::H265,
            cheetah_codec::CodecId::AV1,
            cheetah_codec::CodecId::VP8,
            cheetah_codec::CodecId::VP9,
            cheetah_codec::CodecId::Opus,
            cheetah_codec::CodecId::ADPCM,
            cheetah_codec::CodecId::G711A,
            cheetah_codec::CodecId::G711U,
            cheetah_codec::CodecId::MP3,
        ] {
            assert!(
                preserve_raw_rtp_timestamps(codec),
                "{codec:?} should preserve raw RTP timestamps for bridge consistency"
            );
        }

        for codec in [cheetah_codec::CodecId::H264, cheetah_codec::CodecId::AAC] {
            assert!(
                !preserve_raw_rtp_timestamps(codec),
                "{codec:?} should keep monotonic normalization enabled"
            );
        }
    }

    #[test]
    fn source_rtp_timestamp_for_egress_uses_supported_codec_only() {
        let mut frame = AVFrame::new(
            TrackId(99),
            cheetah_codec::MediaKind::Video,
            cheetah_codec::CodecId::VP9,
            FrameFormat::CanonicalVp9Frame,
            9_000,
            9_000,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x90, 0x80]),
        );
        frame.set_source_timestamp(cheetah_codec::SourceTimestamp::Rtp(
            cheetah_codec::RtpTimestamp::new(3_000_001, 3_000_001),
        ));

        assert_eq!(
            source_rtp_timestamp_for_egress(&frame, cheetah_codec::CodecId::VP9),
            Some(3_000_001)
        );
        assert_eq!(
            source_rtp_timestamp_for_egress(&frame, cheetah_codec::CodecId::H264),
            None
        );
        assert_eq!(
            source_rtp_timestamp_for_egress(&frame, cheetah_codec::CodecId::AAC),
            None
        );
    }

    #[test]
    fn should_sample_timestamp_repair_uses_progressive_sampling() {
        let sampled = (1_u64..=20)
            .filter(|count| cheetah_codec::should_sample_timestamp_repair(*count))
            .collect::<Vec<_>>();
        assert_eq!(sampled, vec![1, 2, 3, 4, 8, 16]);
    }

    #[test]
    fn should_emit_alert_threshold_on_threshold_and_multiples() {
        assert!(!cheetah_codec::should_emit_alert_threshold(31, 32));
        assert!(cheetah_codec::should_emit_alert_threshold(32, 32));
        assert!(cheetah_codec::should_emit_alert_threshold(64, 32));
    }

    #[test]
    fn play_start_pacing_delays_followup_media_by_timestamp_delta() {
        let mut pacing = PlayStartPacingState::default();
        let first = pacing.delay_for(1_000, 1_000_000, false);
        assert_eq!(first, Duration::ZERO);

        let second = pacing.delay_for(1_040, 1_000_000, false);
        assert_eq!(second, Duration::from_millis(40));

        let third = pacing.delay_for(1_060, 1_030_000, false);
        assert_eq!(third, Duration::from_millis(30));
    }

    #[test]
    fn play_start_pacing_first_frame_is_immediate_even_with_large_epoch_timestamp() {
        let mut pacing = PlayStartPacingState::default();
        let first = pacing.delay_for(3_895_818, 9_000_000, false);
        assert_eq!(first, Duration::ZERO);
    }

    #[test]
    fn play_start_pacing_resets_on_discontinuity_and_large_backward_jump() {
        let mut pacing = PlayStartPacingState::default();
        assert_eq!(pacing.delay_for(5_000, 2_000_000, false), Duration::ZERO);
        assert_eq!(
            pacing.delay_for(5_200, 2_000_000, false),
            Duration::from_millis(200)
        );

        let discontinuity = pacing.delay_for(1_000, 2_300_000, true);
        assert_eq!(discontinuity, Duration::ZERO);

        let after_discontinuity = pacing.delay_for(1_100, 2_320_000, false);
        assert_eq!(after_discontinuity, Duration::from_millis(80));

        let backward_reset = pacing.delay_for(0, 2_500_000, false);
        assert_eq!(backward_reset, Duration::ZERO);
    }

    #[test]
    fn play_start_pacing_uses_single_timeline_for_audio_video_interleaving() {
        let mut pacing = PlayStartPacingState::default();
        assert_eq!(pacing.delay_for(10_000, 1_000_000, false), Duration::ZERO);

        let audio_delay = pacing.delay_for(10_020, 1_000_000, false);
        assert_eq!(audio_delay, Duration::from_millis(20));

        let video_delay = pacing.delay_for(10_033, 1_020_000, false);
        assert_eq!(video_delay, Duration::from_millis(13));
    }

    #[test]
    fn frame_media_timestamp_ms_uses_media_kind_priority_and_timebase() {
        let video = frame_media_timestamp_ms(
            cheetah_codec::MediaKind::Video,
            9_000,
            6_000,
            Timebase::new(1, 90_000),
        );
        assert_eq!(video, Some(100));

        let audio = frame_media_timestamp_ms(
            cheetah_codec::MediaKind::Audio,
            9_000,
            4_800,
            Timebase::new(1, 48_000),
        );
        assert_eq!(audio, Some(100));
    }
}
