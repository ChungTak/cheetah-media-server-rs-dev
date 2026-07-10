use super::*;
use crate::media::ps_compat::{is_mp2p_probe_track, probe_mp2p_ps_payload, record_mp2p_probe_drop};
use cheetah_codec::{
    AacAudioSpecificConfig, CodecExtradata, CodecId, FrameFlags, FrameFormat, ParameterSetCache,
    RtpReorderBuffer, RtpReorderSettings, Timebase, TimestampAlert, TimestampNormalizeInput,
    TimestampNormalizeMode, TimestampNormalizer, TimestampNormalizerConfig, TimestampValue,
};
use cheetah_sdk::{PublishLease, PublisherSink};

const UDP_INGEST_REORDER_MAX_PENDING_PACKETS: usize = 2;
const UDP_INGEST_REORDER_MAX_DELAY_MS: u64 = 40;
const UNSUPPORTED_CODEC_DROP_ALERT_THRESHOLD: u64 = 256;

/// `handle_announce` function.
/// `handle_announce` 函数.
pub(super) async fn handle_announce(
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
            Bytes::from_static(b"invalid announce uri"),
        )
        .await;
        return;
    };

    let body_text = match std::str::from_utf8(&req.body) {
        Ok(value) => value,
        Err(_) => {
            send_response(
                command_tx,
                connection_id,
                req.cseq,
                400,
                "Bad Request",
                Vec::new(),
                Bytes::from_static(b"announce body is not utf-8"),
            )
            .await;
            return;
        }
    };
    let (tracks, control_map) = match parse_announce_sdp(body_text) {
        Ok(value) => value,
        Err(err) => {
            send_response(
                command_tx,
                connection_id,
                req.cseq,
                400,
                "Bad Request",
                Vec::new(),
                Bytes::from(err),
            )
            .await;
            return;
        }
    };

    let (lease, sink) = match engine
        .publisher_api
        .acquire_publisher(stream_key.clone(), PublisherOptions::default())
        .await
    {
        Ok(value) => value,
        Err(err) => {
            send_response(
                command_tx,
                connection_id,
                req.cseq,
                403,
                "Forbidden",
                Vec::new(),
                Bytes::from(err.to_string()),
            )
            .await;
            return;
        }
    };

    if let Err(err) = sink.update_tracks(tracks.clone()) {
        let _ = engine.publisher_api.release_publisher(&lease).await;
        send_response(
            command_tx,
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

    let mut track_map = HashMap::<TrackId, TrackInfo>::new();
    for track in tracks {
        track_map.insert(track.track_id, track);
    }
    let video_parameter_sets = initial_video_parameter_set_caches(&track_map);

    let (session_id, previous_publish) = {
        let mut guard = sessions.lock();
        let state = guard
            .entry(connection_id)
            .or_insert_with(|| RtspConnectionState::new(connection_id));
        let previous_publish = state.publish.take();
        state.stream_key = Some(stream_key);
        state.mode = Some(SessionMode::Publish);
        state.play_response_range = None;
        state.announced_tracks = track_map.clone();
        state.announced_control_to_track = control_map
            .iter()
            .map(|(k, v)| (normalize_control(k), *v))
            .collect();
        state.publish = Some(PublishSession {
            cancel: CancellationToken::new(),
            lease,
            sink,
            record_started: false,
            pre_record_rtp_drop_count: 0,
            timestamp_repair_alert_threshold: config.alert_thresholds.timestamp_repair_count,
            queue_drop_alert_threshold: config.alert_thresholds.queue_drop_count,
            queue_drop_counts: HashMap::new(),
            unsupported_codec_drop_counts: HashMap::new(),
            compat_probe_drop_counts: HashMap::new(),
            tracks: track_map,
            track_channels: HashMap::new(),
            rtcp_channels: HashMap::new(),
            clocks: HashMap::new(),
            h264_depacketizers: HashMap::new(),
            h265_depacketizers: HashMap::new(),
            av1_depacketizers: HashMap::new(),
            vp9_depacketizers: HashMap::new(),
            vp8_depacketizers: HashMap::new(),
            track_last_frame_timestamps: HashMap::new(),
            timestamp_normalizers: HashMap::new(),
            video_parameter_sets,
            udp_tracks: HashMap::new(),
            udp_task_handles: Vec::new(),
            mute_audio_maker: None,
            codec_probed: HashSet::new(),
        });
        (state.session_id.clone(), previous_publish)
    };

    if let Some(mut prev) = previous_publish {
        flush_publish_video_reorder(&mut prev);
        prev.cancel.cancel();
        for join in prev.udp_task_handles.drain(..) {
            join.abort();
        }
        let _ = prev.sink.close();
        let _ = engine.publisher_api.release_publisher(&prev.lease).await;
    }

    send_response(
        command_tx,
        connection_id,
        req.cseq,
        200,
        "OK",
        vec![(
            "Session".to_string(),
            session_header_value(&session_id, config.session_timeout_secs),
        )],
        Bytes::new(),
    )
    .await;
}

fn initial_video_parameter_set_caches(
    tracks: &HashMap<TrackId, TrackInfo>,
) -> HashMap<TrackId, ParameterSetCache> {
    let mut caches = HashMap::new();
    for (track_id, track) in tracks {
        if !matches!(track.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
            continue;
        }
        let mut cache = ParameterSetCache::default();
        cache.update_from_extradata(&track.extradata);
        caches.insert(*track_id, cache);
    }
    caches
}

/// Builds `pull_publish_session` output.
/// 构建 `pull_publish_session` 输出.
pub(super) fn build_pull_publish_session(
    config: &RtspModuleConfig,
    cancel: CancellationToken,
    lease: PublishLease,
    sink: Box<dyn PublisherSink>,
    tracks: HashMap<TrackId, TrackInfo>,
) -> PublishSession {
    let video_parameter_sets = initial_video_parameter_set_caches(&tracks);
    PublishSession {
        cancel,
        lease,
        sink,
        record_started: true,
        pre_record_rtp_drop_count: 0,
        timestamp_repair_alert_threshold: config.alert_thresholds.timestamp_repair_count,
        queue_drop_alert_threshold: config.alert_thresholds.queue_drop_count,
        queue_drop_counts: HashMap::new(),
        unsupported_codec_drop_counts: HashMap::new(),
        compat_probe_drop_counts: HashMap::new(),
        tracks,
        track_channels: HashMap::new(),
        rtcp_channels: HashMap::new(),
        clocks: HashMap::new(),
        h264_depacketizers: HashMap::new(),
        h265_depacketizers: HashMap::new(),
        av1_depacketizers: HashMap::new(),
        vp9_depacketizers: HashMap::new(),
        vp8_depacketizers: HashMap::new(),
        track_last_frame_timestamps: HashMap::new(),
        timestamp_normalizers: HashMap::new(),
        video_parameter_sets,
        udp_tracks: HashMap::new(),
        udp_task_handles: Vec::new(),
        mute_audio_maker: None,
        codec_probed: HashSet::new(),
    }
}

/// `handle_record` function.
/// `handle_record` 函数.
pub(super) async fn handle_record(
    connection_id: RtspConnectionId,
    cseq: Option<u32>,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    session_timeout_secs: u32,
    enable_mute_audio: bool,
) {
    let record_result: Result<String, (u16, &'static str, &'static [u8])> = {
        let mut guard = sessions.lock();
        if let Some(state) = guard.get_mut(&connection_id) {
            let has_publish = state.publish.is_some();
            let configured_tracks = state
                .publish
                .as_ref()
                .map_or(0usize, publish_configured_track_count);
            match validate_record_state(state.mode, has_publish, configured_tracks) {
                Ok(()) => {
                    if let Some(publish) = state.publish.as_mut() {
                        if !publish.record_started && publish.pre_record_rtp_drop_count > 0 {
                            warn!(
                                connection_id,
                                stream_key = %publish.lease.stream_key,
                                dropped_packets = publish.pre_record_rtp_drop_count,
                                "rtsp publish dropped rtp packets before RECORD"
                            );
                        }
                        publish.record_started = true;
                        // Activate mute audio if stream has video but no audio
                        if enable_mute_audio {
                            activate_mute_audio_if_video_only(publish);
                        }
                    }
                    Ok(state.session_id.clone())
                }
                Err(err) => Err(err),
            }
        } else {
            Err((454, "Session Not Found", b""))
        }
    };

    match record_result {
        Ok(session_id) => {
            send_response(
                command_tx,
                connection_id,
                cseq,
                200,
                "OK",
                vec![(
                    "Session".to_string(),
                    session_header_value(&session_id, session_timeout_secs),
                )],
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

/// `handle_interleaved_frame` function.
/// `handle_interleaved_frame` 函数.
pub(super) async fn handle_interleaved_frame(
    connection_id: RtspConnectionId,
    channel: u8,
    payload: Bytes,
    command_tx: &RtspCoreCommandSender,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    runtime_api: &Arc<dyn RuntimeApi>,
) {
    if channel.is_multiple_of(2) {
        let track_id = {
            let mut guard = sessions.lock();
            guard
                .get_mut(&connection_id)
                .and_then(|state| state.publish.as_mut())
                .and_then(|publish| publish.track_channels.get(&channel).copied())
        };
        if let Some(track_id) = track_id {
            ingest_publish_rtp_payload(connection_id, track_id, &payload, sessions, runtime_api);
        }
        return;
    }

    let track_id = {
        let mut guard = sessions.lock();
        guard
            .get_mut(&connection_id)
            .and_then(|state| state.publish.as_mut())
            .and_then(|publish| publish.rtcp_channels.get(&channel).copied())
    };

    if let Some(track_id) = track_id {
        let Some(rr_payload) =
            build_publish_receiver_report(connection_id, track_id, &payload, sessions, runtime_api)
        else {
            return;
        };
        if command_tx
            .send_core(
                connection_id,
                RtspCommand::SendInterleaved {
                    channel,
                    payload: rr_payload,
                },
            )
            .await
            .is_err()
        {
            warn!(
                connection_id,
                track_id = track_id.0,
                "send rtcp receiver report failed"
            );
        }
    }
}

/// `ingest_publish_rtp_payload` function.
/// `ingest_publish_rtp_payload` 函数.
pub(super) fn ingest_publish_rtp_payload(
    connection_id: RtspConnectionId,
    track_id: TrackId,
    payload: &[u8],
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    runtime_api: &Arc<dyn RuntimeApi>,
) {
    let Some(packet) = RtpPacket::parse(payload) else {
        return;
    };

    let mut guard = sessions.lock();
    let Some(state) = guard.get_mut(&connection_id) else {
        return;
    };
    let Some(publish) = state.publish.as_mut() else {
        return;
    };
    ingest_publish_rtp_packet(connection_id, track_id, &packet, publish, runtime_api);
}

/// `ingest_publish_rtp_packet` function.
/// `ingest_publish_rtp_packet` 函数.
pub(super) fn ingest_publish_rtp_packet(
    connection_id: RtspConnectionId,
    track_id: TrackId,
    packet: &RtpPacket,
    publish: &mut PublishSession,
    runtime_api: &Arc<dyn RuntimeApi>,
) {
    if !publish.record_started {
        publish.pre_record_rtp_drop_count = publish.pre_record_rtp_drop_count.saturating_add(1);
        return;
    }
    let Some(track) = publish.tracks.get(&track_id).cloned() else {
        return;
    };
    if is_mp2p_probe_track(&track) {
        let probe_outcome = probe_mp2p_ps_payload(packet.payload.as_ref());
        record_mp2p_probe_drop(publish, connection_id, &track, probe_outcome);
        return;
    }
    if !is_publish_ingest_supported_codec(track.codec) {
        record_unsupported_codec_drop(publish, connection_id, &track);
        return;
    }

    let clock = publish.clocks.entry(track_id).or_default();
    clock.on_rtp_packet(
        packet.header.sequence_number,
        packet.header.timestamp,
        track.clock_rate.max(1),
        runtime_unix_time_micros(runtime_api),
    );
    let built_frames = if track.codec == CodecId::H264 {
        let h264_state = publish.h264_depacketizers.entry(track_id).or_default();
        build_frames_from_rtp(&track, packet, clock, Some(h264_state), None, None)
    } else if matches!(track.codec, CodecId::H265 | CodecId::H266) {
        let h265_state = publish.h265_depacketizers.entry(track_id).or_default();
        build_frames_from_rtp(&track, packet, clock, None, Some(h265_state), None)
    } else if track.codec == CodecId::AV1 {
        let av1_state = publish.av1_depacketizers.entry(track_id).or_default();
        build_frames_from_rtp(&track, packet, clock, None, None, Some(av1_state))
    } else if track.codec == CodecId::VP9 {
        let vp9_state = publish.vp9_depacketizers.entry(track_id).or_default();
        build_vp9_frame_from_rtp(&track, packet, clock, vp9_state)
            .into_iter()
            .collect()
    } else if track.codec == CodecId::VP8 {
        let vp8_state = publish.vp8_depacketizers.entry(track_id).or_default();
        build_vp8_frame_from_rtp(&track, packet, clock, vp8_state)
            .into_iter()
            .collect()
    } else {
        build_frames_from_rtp(&track, packet, clock, None, None, None)
    };
    for mut built in built_frames {
        // Codec probe: on first video keyframe, verify NALU type matches SDP-declared codec.
        if built.frame.media_kind == cheetah_codec::MediaKind::Video
            && built.frame.flags.contains(cheetah_codec::FrameFlags::KEY)
            && !publish.codec_probed.contains(&track_id)
        {
            publish.codec_probed.insert(track_id);
            let probed =
                probe_video_codec_from_payload(built.frame.codec, built.frame.payload.as_ref());
            if let Some(actual_codec) = probed {
                if actual_codec != built.frame.codec {
                    tracing::warn!(
                        connection_id,
                        track_id = track_id.0,
                        declared = ?built.frame.codec,
                        actual = ?actual_codec,
                        "SDP codec mismatch detected on first keyframe (vendor quirk)"
                    );
                }
            }
        }
        if let Some(asc) = built.discovered_audio_asc {
            update_publish_audio_track_config(publish, connection_id, track_id, asc);
        }
        if let (Some(sequence_header), Some(codec_config)) = (
            built.discovered_av1_sequence_header.take(),
            built.discovered_av1_codec_config.take(),
        ) {
            update_publish_av1_track_config(
                publish,
                connection_id,
                track_id,
                sequence_header,
                codec_config,
                built.discovered_video_dimensions,
            );
        }
        update_publish_track_fps_from_frame(publish, connection_id, track_id, &built.frame);
        repair_video_keyframe_parameter_sets(publish, connection_id, track_id, &mut built.frame);
        if normalize_publish_frame_timestamps(publish, connection_id, &track, &mut built.frame) {
            let is_video = built.frame.media_kind == cheetah_codec::MediaKind::Video;
            let video_pts_us = built.frame.pts_us;
            push_publish_frame(publish, connection_id, built.frame);
            if is_video {
                inject_mute_audio_frames(publish, connection_id, video_pts_us);
            }
        }
    }
}

fn inject_mute_audio_frames(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    video_pts_us: i64,
) {
    let Some(maker) = publish.mute_audio_maker.as_mut() else {
        return;
    };
    let frames = maker.fill_until(video_pts_us);
    for frame in frames {
        push_publish_frame(publish, connection_id, frame);
    }
}

fn activate_mute_audio_if_video_only(publish: &mut PublishSession) {
    use cheetah_codec::{MediaKind, MuteAudioMaker, TrackId};
    const MUTE_AUDIO_TRACK_ID: TrackId = TrackId(99);
    let has_video = publish
        .tracks
        .values()
        .any(|t| t.media_kind == MediaKind::Video);
    let has_audio = publish
        .tracks
        .values()
        .any(|t| t.media_kind == MediaKind::Audio);
    if has_video && !has_audio {
        publish.mute_audio_maker = Some(MuteAudioMaker::new(MUTE_AUDIO_TRACK_ID));
    }
}

fn is_video_codec(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::H264 | CodecId::H265 | CodecId::H266 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9
    )
}

fn is_publish_ingest_supported_codec(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::H264
            | CodecId::H265
            | CodecId::H266
            | CodecId::AAC
            | CodecId::AV1
            | CodecId::VP8
            | CodecId::VP9
            | CodecId::Opus
            | CodecId::ADPCM
            | CodecId::G711A
            | CodecId::G711U
            | CodecId::MP3
    )
}

fn record_unsupported_codec_drop(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track: &TrackInfo,
) {
    let drop_count = publish
        .unsupported_codec_drop_counts
        .entry(track.track_id)
        .and_modify(|count| *count = count.saturating_add(1))
        .or_insert(1);
    let should_sample = cheetah_codec::should_sample_timestamp_repair(*drop_count);
    let should_threshold = cheetah_codec::should_emit_alert_threshold(
        *drop_count,
        UNSUPPORTED_CODEC_DROP_ALERT_THRESHOLD,
    );
    if should_sample || should_threshold {
        warn!(
            connection_id,
            stream_key = %publish.lease.stream_key,
            track_id = track.track_id.0,
            codec = ?track.codec,
            payload_type = ?track.payload_type,
            drop_count = *drop_count,
            "rtsp publish dropped rtp packet for unsupported codec track"
        );
    }
}

fn is_video_frame(frame: &cheetah_codec::AVFrame) -> bool {
    frame.media_kind == cheetah_codec::MediaKind::Video && is_video_codec(frame.codec)
}

fn source_timeline_mode_for_rtsp_ingress(frame: &cheetah_codec::AVFrame) -> TimestampNormalizeMode {
    cheetah_codec::source_timeline_mode_for_rtp_ingress(frame)
}

fn source_timestamp_value_to_i64(value: TimestampValue) -> i64 {
    match value {
        TimestampValue::Unwrapped(v) => v,
        TimestampValue::Wrapped(v) => v as i64,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RtspPublishAlertClass {
    SourceDisorder,
    CanonicalRepair,
    Discontinuity,
}

fn classify_rtsp_publish_alert_class(
    alerts: &[TimestampAlert],
    discontinuity: bool,
) -> Option<RtspPublishAlertClass> {
    if discontinuity || alerts.contains(&TimestampAlert::TimelineDiscontinuityDetected) {
        return Some(RtspPublishAlertClass::Discontinuity);
    }
    if alerts.contains(&TimestampAlert::NonMonotonicDtsRepaired)
        || alerts.contains(&TimestampAlert::NegativeCompositionClamped)
    {
        return Some(RtspPublishAlertClass::CanonicalRepair);
    }
    if alerts.contains(&TimestampAlert::PtsReorderObserved) {
        return Some(RtspPublishAlertClass::SourceDisorder);
    }
    None
}

fn should_warn_canonical_repair(alert_count: u64) -> bool {
    alert_count == 1 || alert_count.is_power_of_two()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameObservabilityFields {
    track_id: u32,
    codec: CodecId,
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

fn push_publish_frame(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    frame: cheetah_codec::AVFrame,
) {
    let is_key = frame.flags.contains(cheetah_codec::FrameFlags::KEY);
    let fields = frame_observability_fields(&frame);
    tracing::info!(
        connection_id,
        stream_key = %publish.lease.stream_key,
        track_id = fields.track_id,
        pts = fields.pts,
        dts = fields.dts,
        key = is_key,
        "rtsp publish push frame"
    );
    match publish.sink.push_frame(Arc::new(frame)) {
        Ok(cheetah_sdk::DispatchResult::Accepted) => {
            publish.queue_drop_counts.remove(&TrackId(fields.track_id));
        }
        Ok(cheetah_sdk::DispatchResult::DroppedByPolicy) => {
            let drop_count = publish
                .queue_drop_counts
                .entry(TrackId(fields.track_id))
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
            warn!(
                connection_id,
                stream_key = %publish.lease.stream_key,
                track_id = fields.track_id,
                codec = ?fields.codec,
                pts = fields.pts,
                dts = fields.dts,
                "push frame to engine dropped by backpressure policy"
            );
            if cheetah_codec::should_emit_alert_threshold(
                *drop_count,
                publish.queue_drop_alert_threshold,
            ) {
                warn!(
                    connection_id,
                    stream_key = %publish.lease.stream_key,
                    track_id = fields.track_id,
                    codec = ?fields.codec,
                    pts = fields.pts,
                    dts = fields.dts,
                    queue_drop_count = *drop_count,
                    queue_drop_alert_threshold = publish.queue_drop_alert_threshold,
                    "rtsp publish queue buildup alert threshold reached"
                );
            }
        }
        Ok(cheetah_sdk::DispatchResult::RejectedClosed) => {}
        Err(_) => {
            warn!(
                connection_id,
                stream_key = %publish.lease.stream_key,
                track_id = fields.track_id,
                codec = ?fields.codec,
                pts = fields.pts,
                dts = fields.dts,
                "push frame to engine failed"
            );
        }
    }
}

fn fallback_step_for_publish_frame(
    track: &TrackInfo,
    frame: &cheetah_codec::AVFrame,
    timebase: Timebase,
) -> i64 {
    cheetah_codec::fallback_step_for_rtp_ingress(track, frame, timebase)
}

fn normalize_publish_frame_timestamps(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track: &TrackInfo,
    frame: &mut cheetah_codec::AVFrame,
) -> bool {
    let stream_key = publish.lease.stream_key.clone();
    let repair_alert_threshold = publish.timestamp_repair_alert_threshold;
    let source_pts = frame.pts;
    let source_timeline_mode = source_timeline_mode_for_rtsp_ingress(frame);
    let source_dts_for_log = match source_timeline_mode {
        TimestampNormalizeMode::DtsPts { dts, .. }
        | TimestampNormalizeMode::DtsWithCompositionOffset { dts, .. } => {
            Some(source_timestamp_value_to_i64(dts))
        }
        TimestampNormalizeMode::PtsOnly { .. } | TimestampNormalizeMode::NoTimestamp => None,
    };
    let timebase = match track.media_timebase() {
        Ok(value) => value,
        Err(err) => {
            warn!(
                connection_id,
                stream_key = %stream_key,
                track_id = track.track_id.0,
                codec = ?track.codec,
                "rtsp publish track media timebase invalid: {err}"
            );
            return false;
        }
    };
    let (normalized, alert_class, alert_count) = {
        let state = match ensure_track_timestamp_normalizer(publish, connection_id, track, timebase)
        {
            Some(value) => value,
            None => return false,
        };
        let normalized = match state.normalizer.normalize(TimestampNormalizeInput {
            mode: source_timeline_mode,
            frame_duration: (frame.duration > 0).then_some(frame.duration),
            fallback_step: Some(fallback_step_for_publish_frame(track, frame, timebase)),
            is_video: is_video_frame(frame),
            force_discontinuity: frame.flags.contains(FrameFlags::DISCONTINUITY),
        }) {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    connection_id,
                    stream_key = %stream_key,
                    track_id = frame.track_id.0,
                    codec = ?frame.codec,
                    source_pts,
                    source_dts = ?source_dts_for_log,
                    pts = frame.pts,
                    dts = frame.dts,
                    "rtsp publish timestamp normalize failed: {err}"
                );
                return false;
            }
        };
        let alert_class = classify_rtsp_publish_alert_class(
            normalized.alerts.as_slice(),
            normalized.discontinuity,
        );
        let alert_count = match alert_class {
            Some(RtspPublishAlertClass::CanonicalRepair) => {
                state.repair_count = state.repair_count.saturating_add(1);
                Some(state.repair_count)
            }
            Some(RtspPublishAlertClass::SourceDisorder) => {
                state.source_disorder_count = state.source_disorder_count.saturating_add(1);
                Some(state.source_disorder_count)
            }
            Some(RtspPublishAlertClass::Discontinuity) => {
                state.discontinuity_count = state.discontinuity_count.saturating_add(1);
                Some(state.discontinuity_count)
            }
            None => None,
        };
        (normalized, alert_class, alert_count)
    };
    frame.pts = normalized.pts;
    frame.dts = normalized.dts;
    frame.pts_us = normalized.pts_us;
    frame.dts_us = normalized.dts_us;
    if normalized.discontinuity {
        frame.flags.insert(FrameFlags::DISCONTINUITY);
    }
    if frame.pts < frame.dts {
        frame.flags.insert(FrameFlags::B_FRAME);
    } else {
        frame.flags.remove(FrameFlags::B_FRAME);
    }

    if let (Some(alert_class), Some(alert_count)) = (alert_class, alert_count) {
        let should_sample = cheetah_codec::should_sample_timestamp_repair(alert_count);
        let should_threshold =
            cheetah_codec::should_emit_alert_threshold(alert_count, repair_alert_threshold);
        match alert_class {
            RtspPublishAlertClass::SourceDisorder => {
                if should_sample {
                    tracing::debug!(
                        connection_id,
                        stream_key = %stream_key,
                        track_id = frame.track_id.0,
                        codec = ?frame.codec,
                        protocol_ingress = "rtsp-publish",
                        alert_class = "source_disorder",
                        source_pts,
                        source_dts = ?source_dts_for_log,
                        pts = frame.pts,
                        dts = frame.dts,
                        alert_count,
                        alerts = ?normalized.alerts,
                        "rtsp publish source timeline disorder observed"
                    );
                }
                if should_threshold {
                    warn!(
                        connection_id,
                        stream_key = %stream_key,
                        track_id = frame.track_id.0,
                        codec = ?frame.codec,
                        protocol_ingress = "rtsp-publish",
                        alert_class = "source_disorder",
                        source_pts,
                        source_dts = ?source_dts_for_log,
                        pts = frame.pts,
                        dts = frame.dts,
                        alert_count,
                        alert_threshold = repair_alert_threshold,
                        alerts = ?normalized.alerts,
                        "rtsp publish source timeline disorder alert threshold reached"
                    );
                }
            }
            RtspPublishAlertClass::CanonicalRepair => {
                if should_warn_canonical_repair(alert_count) {
                    warn!(
                        connection_id,
                        stream_key = %stream_key,
                        track_id = frame.track_id.0,
                        codec = ?frame.codec,
                        protocol_ingress = "rtsp-publish",
                        alert_class = "canonical_repair",
                        source_pts,
                        source_dts = ?source_dts_for_log,
                        pts = frame.pts,
                        dts = frame.dts,
                        alert_count,
                        alerts = ?normalized.alerts,
                        "rtsp publish canonical timeline repaired by codec normalizer"
                    );
                }
                if should_threshold {
                    tracing::debug!(
                        connection_id,
                        stream_key = %stream_key,
                        track_id = frame.track_id.0,
                        codec = ?frame.codec,
                        protocol_ingress = "rtsp-publish",
                        alert_class = "canonical_repair",
                        source_pts,
                        source_dts = ?source_dts_for_log,
                        pts = frame.pts,
                        dts = frame.dts,
                        alert_count,
                        alert_threshold = repair_alert_threshold,
                        alerts = ?normalized.alerts,
                        "rtsp publish canonical timeline repair alert threshold reached"
                    );
                }
            }
            RtspPublishAlertClass::Discontinuity => {
                if should_sample || should_threshold {
                    warn!(
                        connection_id,
                        stream_key = %stream_key,
                        track_id = frame.track_id.0,
                        codec = ?frame.codec,
                        protocol_ingress = "rtsp-publish",
                        alert_class = "discontinuity",
                        source_pts,
                        source_dts = ?source_dts_for_log,
                        pts = frame.pts,
                        dts = frame.dts,
                        alert_count,
                        alert_threshold = repair_alert_threshold,
                        alerts = ?normalized.alerts,
                        "rtsp publish timeline discontinuity detected"
                    );
                }
            }
        }
    }
    true
}

fn ensure_track_timestamp_normalizer<'a>(
    publish: &'a mut PublishSession,
    connection_id: RtspConnectionId,
    track: &TrackInfo,
    timebase: Timebase,
) -> Option<&'a mut crate::session::PublishTrackTimestampState> {
    if publish.timestamp_normalizers.contains_key(&track.track_id) {
        return publish.timestamp_normalizers.get_mut(&track.track_id);
    }
    let config = match TimestampNormalizerConfig::new(timebase, timebase, Some(32)) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                connection_id,
                stream_key = %publish.lease.stream_key,
                track_id = track.track_id.0,
                codec = ?track.codec,
                "rtsp publish create timestamp normalizer config failed: {err}"
            );
            return None;
        }
    };
    publish.timestamp_normalizers.insert(
        track.track_id,
        crate::session::PublishTrackTimestampState {
            normalizer: TimestampNormalizer::new(config),
            repair_count: 0,
            source_disorder_count: 0,
            discontinuity_count: 0,
        },
    );
    publish.timestamp_normalizers.get_mut(&track.track_id)
}

/// `flush_publish_video_reorder` function.
/// `flush_publish_video_reorder` 函数.
pub(super) fn flush_publish_video_reorder(publish: &mut PublishSession) {
    publish.timestamp_normalizers.clear();
}

fn repair_video_keyframe_parameter_sets(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    frame: &mut cheetah_codec::AVFrame,
) {
    if frame.format != FrameFormat::CanonicalH26x {
        return;
    }
    if !matches!(frame.codec, CodecId::H264 | CodecId::H265 | CodecId::H266) {
        return;
    }

    let discovered_extradata = {
        let cache = publish.video_parameter_sets.entry(track_id).or_default();
        cache.repair_h26x_keyframe_frame(frame)
    };
    if let Some(extradata) = discovered_extradata {
        update_publish_h26x_track_config(publish, connection_id, track_id, frame.codec, extradata);
    }
}

fn update_publish_h26x_track_config(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    codec: CodecId,
    extradata: CodecExtradata,
) {
    let Some(track) = publish.tracks.get_mut(&track_id) else {
        return;
    };
    if track.codec != codec || track.extradata == extradata {
        return;
    }

    track.extradata = extradata;
    track.refresh_readiness();
    publish_track_updates_with_error_handling(
        publish,
        connection_id,
        "h26x parameter set discovery",
    );
}

fn update_publish_audio_track_config(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    asc: Bytes,
) {
    let mut should_publish_tracks = false;
    if let Some(track) = publish.tracks.get_mut(&track_id) {
        let already_set = matches!(
            &track.extradata,
            CodecExtradata::AAC { asc: current } if current == &asc
        );
        if !already_set {
            if let Some(parsed) = AacAudioSpecificConfig::from_bytes(&asc) {
                track.sample_rate = sampling_frequency_from_index(parsed.sampling_frequency_index);
                if parsed.channel_configuration > 0 {
                    track.channels = Some(parsed.channel_configuration);
                }
            }
            track.extradata = CodecExtradata::AAC { asc };
            track.refresh_readiness();
            should_publish_tracks = true;
        }
    }

    if should_publish_tracks {
        publish_track_updates_with_error_handling(publish, connection_id, "aac asc discovery");
    }
}

fn update_publish_av1_track_config(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    sequence_header: Bytes,
    codec_config: Bytes,
    dimensions: Option<(u32, u32)>,
) {
    let mut should_publish_tracks = false;
    if let Some(track) = publish.tracks.get_mut(&track_id) {
        let already_set = matches!(
            &track.extradata,
            CodecExtradata::AV1 {
                sequence_header: current_sequence,
                codec_config: current_config,
            } if current_sequence.as_ref() == Some(&sequence_header)
                && current_config.as_ref() == Some(&codec_config)
        );
        if !already_set {
            track.extradata = CodecExtradata::AV1 {
                sequence_header: Some(sequence_header),
                codec_config: Some(codec_config),
            };
            if let Some((width, height)) = dimensions {
                track.width = Some(width);
                track.height = Some(height);
            }
            track.refresh_readiness();
            should_publish_tracks = true;
        } else if let Some((width, height)) = dimensions {
            if track.width != Some(width) || track.height != Some(height) {
                track.width = Some(width);
                track.height = Some(height);
                should_publish_tracks = true;
            }
        }
    }

    if should_publish_tracks {
        publish_track_updates_with_error_handling(publish, connection_id, "av1 sequence discovery");
    }
}

fn update_publish_track_fps_from_frame(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    frame: &cheetah_codec::AVFrame,
) {
    if !is_video_frame(frame) {
        return;
    }

    let previous = publish
        .track_last_frame_timestamps
        .insert(track_id, frame.dts);
    let Some(previous) = previous else {
        return;
    };
    let delta = frame.dts.saturating_sub(previous);
    if delta <= 0 {
        return;
    }

    let Some(track) = publish.tracks.get_mut(&track_id) else {
        return;
    };
    if track.fps.is_some() {
        return;
    }

    let Ok(delta) = u32::try_from(delta) else {
        return;
    };
    if delta == 0 || track.clock_rate == 0 {
        return;
    }
    let fps = track.clock_rate as f64 / delta as f64;
    if !(1.0..=240.0).contains(&fps) {
        return;
    }

    let divisor = gcd_u32(track.clock_rate, delta);
    track.fps = Some(cheetah_codec::Rational32::new(
        track.clock_rate / divisor,
        delta / divisor,
    ));
    publish_track_updates_with_error_handling(publish, connection_id, "video fps discovery");
}

fn publish_track_updates_with_error_handling(
    publish: &mut PublishSession,
    connection_id: RtspConnectionId,
    reason: &'static str,
) {
    let tracks = publish.tracks.values().cloned().collect::<Vec<_>>();
    if let Err(err) = publish.sink.update_tracks(tracks) {
        warn!(
            connection_id,
            stream_key = %publish.lease.stream_key,
            reason,
            "publish track info update failed: {err}"
        );
    }
}

fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

fn sampling_frequency_from_index(index: u8) -> Option<u32> {
    match index {
        0 => Some(96_000),
        1 => Some(88_200),
        2 => Some(64_000),
        3 => Some(48_000),
        4 => Some(44_100),
        5 => Some(32_000),
        6 => Some(24_000),
        7 => Some(22_050),
        8 => Some(16_000),
        9 => Some(12_000),
        10 => Some(11_025),
        11 => Some(8_000),
        12 => Some(7_350),
        _ => None,
    }
}

/// Builds `publish_receiver_report` output.
/// 构建 `publish_receiver_report` 输出.
pub(super) fn build_publish_receiver_report(
    connection_id: RtspConnectionId,
    track_id: TrackId,
    payload: &[u8],
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    runtime_api: &Arc<dyn RuntimeApi>,
) -> Option<Bytes> {
    let sender_report = match parse_rtcp_sender_report(payload) {
        Ok(Some(report)) => report,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                connection_id,
                track_id = track_id.0,
                "parse incoming rtcp sender report failed: {err}"
            );
            return None;
        }
    };
    let now_unix_micros = runtime_unix_time_micros(runtime_api);

    let mut guard = sessions.lock();
    let state = guard.get_mut(&connection_id)?;
    let publish = state.publish.as_mut()?;
    if !publish.record_started {
        return None;
    }
    let clock = publish.clocks.entry(track_id).or_default();
    clock.note_sender_report(sender_report.lsr, now_unix_micros);
    let metrics: RtcpReceiverMetrics = clock.build_receiver_metrics(now_unix_micros);
    let receiver_ssrc = sender_report.sender_ssrc ^ connection_id as u32;
    match build_rtcp_receiver_report(
        receiver_ssrc,
        RtcpReceiverReportBlock {
            sender_ssrc: sender_report.sender_ssrc,
            fraction_lost: metrics.fraction_lost,
            cumulative_lost: metrics.cumulative_lost,
            extended_highest_seq: metrics.extended_highest_seq,
            jitter: metrics.jitter,
            lsr: metrics.lsr,
            dlsr: metrics.dlsr,
        },
    ) {
        Ok(payload) => Some(payload),
        Err(err) => {
            warn!(
                connection_id,
                track_id = track_id.0,
                "build rtcp receiver report failed: {err}"
            );
            None
        }
    }
}

/// `spawn_publish_rtp_udp_task` function.
/// `spawn_publish_rtp_udp_task` 函数.
pub(super) fn spawn_publish_rtp_udp_task(
    ctx: PublishUdpRtpTaskContext,
) -> Box<dyn cheetah_sdk::JoinHandle> {
    let PublishUdpRtpTaskContext {
        runtime_api,
        sessions,
        connection_id,
        track_id,
        rtp_socket,
        expected_remote,
        enable_reorder,
        cancel,
    } = ctx;
    let runtime_api_ref = runtime_api.clone();
    runtime_api.spawn(Box::pin(async move {
        let mut buf = vec![0u8; 64 * 1024];
        let mut reorder = enable_reorder.then(|| {
            RtpReorderBuffer::new(RtpReorderSettings {
                max_packets: UDP_INGEST_REORDER_MAX_PENDING_PACKETS,
                max_delay_ms: UDP_INGEST_REORDER_MAX_DELAY_MS,
            })
        });
        loop {
            let recv = {
                let cancel_fut = cancel.cancelled().fuse();
                let recv_fut = rtp_socket.recv_from(&mut buf).fuse();
                pin_mut!(cancel_fut, recv_fut);
                select_biased! {
                    _ = cancel_fut => {
                        break;
                    }
                    recv = recv_fut => recv,
                }
            };
            let Ok(meta) = recv else {
                break;
            };
            if meta.len == 0 || meta.len > buf.len() {
                continue;
            }
            if meta.from != expected_remote {
                continue;
            }
            if let Some(reorder) = reorder.as_mut() {
                if !publish_record_started(connection_id, &sessions) {
                    reorder.reset();
                    ingest_publish_rtp_payload(
                        connection_id,
                        track_id,
                        &buf[..meta.len],
                        &sessions,
                        &runtime_api_ref,
                    );
                    continue;
                }
                let now_ms = runtime_unix_time_micros(&runtime_api_ref) / 1_000;
                let Some(packet) = RtpPacket::parse(&buf[..meta.len]) else {
                    continue;
                };
                let ordered_packets = reorder.push(
                    packet.header.sequence_number,
                    now_ms,
                    Bytes::copy_from_slice(&buf[..meta.len]),
                );
                for ordered in ordered_packets {
                    ingest_publish_rtp_payload(
                        connection_id,
                        track_id,
                        ordered.as_ref(),
                        &sessions,
                        &runtime_api_ref,
                    );
                }
            } else {
                ingest_publish_rtp_payload(
                    connection_id,
                    track_id,
                    &buf[..meta.len],
                    &sessions,
                    &runtime_api_ref,
                );
            }
        }
    }))
}

fn publish_record_started(
    connection_id: RtspConnectionId,
    sessions: &Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
) -> bool {
    let guard = sessions.lock();
    guard
        .get(&connection_id)
        .and_then(|state| state.publish.as_ref())
        .is_some_and(|publish| publish.record_started)
}

/// `PublishUdpRtpTaskContext` data structure.
/// `PublishUdpRtpTaskContext` 数据结构.
pub(super) struct PublishUdpRtpTaskContext {
    /// `runtime_api` field.
    /// `runtime_api` 字段.
    pub runtime_api: Arc<dyn RuntimeApi>,
    /// `sessions` field.
    /// `sessions` 字段.
    pub sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    /// `connection_id` field of type `RtspConnectionId`.
    /// `connection_id` 字段，类型为 `RtspConnectionId`.
    pub connection_id: RtspConnectionId,
    /// `track_id` field of type `TrackId`.
    /// `track_id` 字段，类型为 `TrackId`.
    pub track_id: TrackId,
    /// `rtp_socket` field.
    /// `rtp_socket` 字段.
    pub rtp_socket: Arc<dyn cheetah_sdk::AsyncUdpSocket>,
    /// `expected_remote` field of type `SocketAddr`.
    /// `expected_remote` 字段，类型为 `SocketAddr`.
    pub expected_remote: SocketAddr,
    /// `enable_reorder` field of type `bool`.
    /// `enable_reorder` 字段，类型为 `bool`.
    pub enable_reorder: bool,
    /// `cancel` field of type `CancellationToken`.
    /// `cancel` 字段，类型为 `CancellationToken`.
    pub cancel: CancellationToken,
}

/// `spawn_publish_rtcp_udp_task` function.
/// `spawn_publish_rtcp_udp_task` 函数.
pub(super) fn spawn_publish_rtcp_udp_task(
    runtime_api: Arc<dyn RuntimeApi>,
    sessions: Arc<Mutex<HashMap<RtspConnectionId, RtspConnectionState>>>,
    connection_id: RtspConnectionId,
    track_id: TrackId,
    rtcp_socket: Arc<dyn cheetah_sdk::AsyncUdpSocket>,
    target_rtcp: SocketAddr,
    cancel: CancellationToken,
) -> Box<dyn cheetah_sdk::JoinHandle> {
    let runtime_api_ref = runtime_api.clone();
    runtime_api.spawn(Box::pin(async move {
        let mut buf = vec![0u8; 2048];
        loop {
            let recv = {
                let cancel_fut = cancel.cancelled().fuse();
                let recv_fut = rtcp_socket.recv_from(&mut buf).fuse();
                pin_mut!(cancel_fut, recv_fut);
                select_biased! {
                    _ = cancel_fut => {
                        break;
                    }
                    recv = recv_fut => recv,
                }
            };
            let Ok(meta) = recv else {
                break;
            };
            if meta.len == 0 || meta.len > buf.len() {
                continue;
            }
            if meta.from != target_rtcp {
                continue;
            }
            if let Some(rr_payload) = build_publish_receiver_report(
                connection_id,
                track_id,
                &buf[..meta.len],
                &sessions,
                &runtime_api_ref,
            ) {
                if rtcp_socket.send_to(&rr_payload, target_rtcp).await.is_err() {
                    break;
                }
            }
        }
    }))
}

/// Probe actual video codec from Annex-B payload by inspecting NALU type.
/// Returns `Some(codec)` if determinable, `None` if payload is too short or ambiguous.
fn probe_video_codec_from_payload(declared: CodecId, payload: &[u8]) -> Option<CodecId> {
    // Find first NALU start code
    let nalu_start = find_nalu_start(payload)?;
    let nalu_header = *payload.get(nalu_start)?;
    match declared {
        CodecId::H264 | CodecId::H265 => {
            // H.264 NALU type is in bits [4:0] of first byte, forbidden_zero_bit=0
            // H.265 NALU type is in bits [6:1] of first byte (shifted), forbidden_zero_bit=0
            let h264_type = nalu_header & 0x1F;
            let h265_type = (nalu_header >> 1) & 0x3F;
            // H.264 IDR = 5, SPS = 7, PPS = 8
            // H.265 IDR_W_RADL = 19, IDR_N_LP = 20, VPS = 32, SPS = 33, PPS = 34
            if (32..=34).contains(&h265_type) {
                Some(CodecId::H265)
            } else if h264_type == 5 || h264_type == 7 || h264_type == 8 {
                Some(CodecId::H264)
            } else if h265_type == 19 || h265_type == 20 {
                Some(CodecId::H265)
            } else {
                None // ambiguous
            }
        }
        _ => None,
    }
}

fn find_nalu_start(payload: &[u8]) -> Option<usize> {
    for i in 0..payload.len().saturating_sub(3) {
        if payload[i] == 0 && payload[i + 1] == 0 {
            if payload[i + 2] == 1 {
                return Some(i + 3);
            }
            if i + 3 < payload.len() && payload[i + 2] == 0 && payload[i + 3] == 1 {
                return Some(i + 4);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::{AVFrame, MediaKind, ParameterSetCache, Timebase, TrackInfo};
    use cheetah_rtsp_core::{RtcpPacket, RtcpSenderReport};
    use cheetah_sdk::{DispatchResult, PublishLease, PublisherSink, SdkError, StreamId, StreamKey};
    use parking_lot::Mutex as ParkingMutex;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};
    use std::time::Duration;

    struct DummySink;

    impl PublisherSink for DummySink {
        fn update_tracks(&self, _tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            Ok(DispatchResult::Accepted)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    struct RecordingSink {
        updates: StdArc<StdMutex<Vec<Vec<TrackInfo>>>>,
    }

    impl PublisherSink for RecordingSink {
        fn update_tracks(&self, tracks: Vec<TrackInfo>) -> Result<(), SdkError> {
            self.updates.lock().expect("updates lock").push(tracks);
            Ok(())
        }

        fn push_frame(&self, _frame: Arc<AVFrame>) -> Result<DispatchResult, SdkError> {
            Ok(DispatchResult::Accepted)
        }

        fn close(&self) -> Result<(), SdkError> {
            Ok(())
        }

        fn take_keyframe_requests(&self) -> u64 {
            0
        }
    }

    fn publish_session_with_video_parameter_cache(
        track_id: TrackId,
        cache: ParameterSetCache,
    ) -> PublishSession {
        let mut video_parameter_sets = HashMap::new();
        video_parameter_sets.insert(track_id, cache);
        PublishSession {
            cancel: CancellationToken::new(),
            lease: PublishLease {
                stream_id: StreamId(1),
                stream_key: StreamKey::new("live", "test"),
                lease_id: 1,
            },
            sink: Box::new(DummySink),
            record_started: true,
            pre_record_rtp_drop_count: 0,
            timestamp_repair_alert_threshold: 32,
            queue_drop_alert_threshold: 64,
            queue_drop_counts: HashMap::new(),
            unsupported_codec_drop_counts: HashMap::new(),
            compat_probe_drop_counts: HashMap::new(),
            tracks: HashMap::new(),
            track_channels: HashMap::new(),
            rtcp_channels: HashMap::new(),
            clocks: HashMap::new(),
            h264_depacketizers: HashMap::new(),
            h265_depacketizers: HashMap::new(),
            av1_depacketizers: HashMap::new(),
            vp9_depacketizers: HashMap::new(),
            vp8_depacketizers: HashMap::new(),
            track_last_frame_timestamps: HashMap::new(),
            timestamp_normalizers: HashMap::new(),
            video_parameter_sets,
            udp_tracks: HashMap::new(),
            udp_task_handles: Vec::new(),
            mute_audio_maker: None,
            codec_probed: HashSet::new(),
        }
    }

    fn publish_session_with_recording_sink(
        track: TrackInfo,
    ) -> (PublishSession, StdArc<StdMutex<Vec<Vec<TrackInfo>>>>) {
        let updates = StdArc::new(StdMutex::new(Vec::new()));
        let mut tracks = HashMap::new();
        tracks.insert(track.track_id, track);
        let publish = PublishSession {
            cancel: CancellationToken::new(),
            lease: PublishLease {
                stream_id: StreamId(1),
                stream_key: StreamKey::new("live", "test"),
                lease_id: 1,
            },
            sink: Box::new(RecordingSink {
                updates: updates.clone(),
            }),
            record_started: true,
            pre_record_rtp_drop_count: 0,
            timestamp_repair_alert_threshold: 32,
            queue_drop_alert_threshold: 64,
            queue_drop_counts: HashMap::new(),
            unsupported_codec_drop_counts: HashMap::new(),
            compat_probe_drop_counts: HashMap::new(),
            tracks,
            track_channels: HashMap::new(),
            rtcp_channels: HashMap::new(),
            clocks: HashMap::new(),
            h264_depacketizers: HashMap::new(),
            h265_depacketizers: HashMap::new(),
            av1_depacketizers: HashMap::new(),
            vp9_depacketizers: HashMap::new(),
            vp8_depacketizers: HashMap::new(),
            track_last_frame_timestamps: HashMap::new(),
            timestamp_normalizers: HashMap::new(),
            video_parameter_sets: HashMap::new(),
            udp_tracks: HashMap::new(),
            udp_task_handles: Vec::new(),
            mute_audio_maker: None,
            codec_probed: HashSet::new(),
        };
        (publish, updates)
    }

    fn build_test_rtp_payload(sequence_number: u16) -> Bytes {
        let mut payload = Vec::with_capacity(13);
        payload.push(0x80);
        payload.push(96);
        payload.extend_from_slice(&sequence_number.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.push(0xAB);
        Bytes::from(payload)
    }

    fn build_test_rtcp_sender_report() -> Vec<u8> {
        RtcpPacket::build(&[RtcpPacket::SenderReport(RtcpSenderReport {
            ssrc: 0x1122_3344,
            ntp_timestamp: 0x0102_0304_0506_0708,
            rtp_timestamp: 90_000,
            packet_count: 7,
            octet_count: 1024,
            reports: Vec::new(),
        })])
        .expect("build sender report")
    }

    fn build_udp_ingest_reorder() -> RtpReorderBuffer<Bytes> {
        RtpReorderBuffer::new(RtpReorderSettings {
            max_packets: UDP_INGEST_REORDER_MAX_PENDING_PACKETS,
            max_delay_ms: UDP_INGEST_REORDER_MAX_DELAY_MS,
        })
    }

    #[test]
    fn udp_ingest_reorder_orders_small_out_of_order_packets() {
        let mut reorder = build_udp_ingest_reorder();

        let first = reorder.push(100, 100, build_test_rtp_payload(100));
        assert_eq!(first.len(), 1);
        assert_eq!(
            RtpPacket::parse(first[0].as_ref())
                .expect("rtp")
                .header
                .sequence_number,
            100
        );

        let second = reorder.push(102, 102, build_test_rtp_payload(102));
        assert!(second.is_empty());

        let third = reorder.push(101, 101, build_test_rtp_payload(101));
        assert_eq!(third.len(), 2);
        let seqs: Vec<u16> = third
            .iter()
            .map(|packet| {
                RtpPacket::parse(packet.as_ref())
                    .expect("rtp")
                    .header
                    .sequence_number
            })
            .collect();
        assert_eq!(seqs, vec![101, 102]);
    }

    #[test]
    fn udp_ingest_reorder_releases_when_gap_exceeds_threshold() {
        let mut reorder = build_udp_ingest_reorder();

        let first = reorder.push(100, 100, build_test_rtp_payload(100));
        assert_eq!(first.len(), 1);

        for seq in 102..130 {
            let _ = reorder.push(seq, u64::from(seq), build_test_rtp_payload(seq));
        }
        let released = reorder.push(130, 130, build_test_rtp_payload(130));
        assert!(
            !released.is_empty(),
            "gap-based release should prevent indefinite head-of-line blocking"
        );
        let first_seq = RtpPacket::parse(released[0].as_ref())
            .expect("rtp")
            .header
            .sequence_number;
        assert!(first_seq >= 102);
    }

    #[test]
    fn udp_ingest_reorder_reset_clears_pending_gap_state() {
        let mut reorder = build_udp_ingest_reorder();
        let first = reorder.push(100, 100, build_test_rtp_payload(100));
        assert_eq!(first.len(), 1);

        let pending = reorder.push(102, 102, build_test_rtp_payload(102));
        assert!(
            pending.is_empty(),
            "seq=102 should be buffered while waiting 101"
        );

        reorder.reset();

        let after_reset = reorder.push(101, 101, build_test_rtp_payload(101));
        assert_eq!(after_reset.len(), 1);
        let seq = RtpPacket::parse(after_reset[0].as_ref())
            .expect("rtp")
            .header
            .sequence_number;
        assert_eq!(seq, 101);

        let next = reorder.push(102, 102, build_test_rtp_payload(102));
        assert_eq!(next.len(), 1);
        let next_seq = RtpPacket::parse(next[0].as_ref())
            .expect("rtp")
            .header
            .sequence_number;
        assert_eq!(next_seq, 102);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_rtcp_udp_task_filters_unexpected_peer() {
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(cheetah_runtime_tokio::TokioRuntime::new());
        let connection_id = 91u64;
        let track_id = TrackId(1);
        let sessions: Arc<ParkingMutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
            Arc::new(ParkingMutex::new(HashMap::new()));
        {
            let mut guard = sessions.lock();
            let mut state = RtspConnectionState::new(connection_id);
            state.publish = Some(publish_session_with_video_parameter_cache(
                track_id,
                ParameterSetCache::default(),
            ));
            guard.insert(connection_id, state);
        }

        let rtcp_socket: Arc<dyn cheetah_sdk::AsyncUdpSocket> = Arc::from(
            runtime_api
                .bind_udp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .expect("bind rtcp socket"),
        );
        let rtcp_addr = rtcp_socket.local_addr().expect("rtcp local addr");
        let expected_peer = UdpSocket::bind("127.0.0.1:0").expect("bind expected peer");
        expected_peer
            .set_read_timeout(Some(Duration::from_millis(300)))
            .expect("set read timeout");
        let unexpected_peer = UdpSocket::bind("127.0.0.1:0").expect("bind unexpected peer");
        let cancel = CancellationToken::new();
        let join = spawn_publish_rtcp_udp_task(
            runtime_api,
            sessions,
            connection_id,
            track_id,
            rtcp_socket,
            expected_peer.local_addr().expect("expected peer addr"),
            cancel.clone(),
        );

        unexpected_peer
            .send_to(&build_test_rtcp_sender_report(), rtcp_addr)
            .expect("send unexpected sender report");

        let mut buf = [0u8; 1500];
        let recv = expected_peer.recv_from(&mut buf);
        assert!(
            recv.is_err(),
            "unexpected RTCP peer must not trigger a receiver report"
        );

        cancel.cancel();
        let _ = join.wait().await;
    }

    #[test]
    fn ingest_publish_rtp_payload_counts_drops_before_record() {
        let connection_id = 88u64;
        let track_id = TrackId(1);
        let sessions: Arc<ParkingMutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
            Arc::new(ParkingMutex::new(HashMap::new()));
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(cheetah_runtime_tokio::TokioRuntime::new());

        {
            let mut guard = sessions.lock();
            let mut state = RtspConnectionState::new(connection_id);
            state.publish = Some(PublishSession {
                cancel: CancellationToken::new(),
                lease: PublishLease {
                    stream_id: StreamId(1),
                    stream_key: StreamKey::new("live", "drop-before-record"),
                    lease_id: 1,
                },
                sink: Box::new(DummySink),
                record_started: false,
                pre_record_rtp_drop_count: 0,
                timestamp_repair_alert_threshold: 32,
                queue_drop_alert_threshold: 64,
                queue_drop_counts: HashMap::new(),
                unsupported_codec_drop_counts: HashMap::new(),
                compat_probe_drop_counts: HashMap::new(),
                tracks: HashMap::new(),
                track_channels: HashMap::new(),
                rtcp_channels: HashMap::new(),
                clocks: HashMap::new(),
                h264_depacketizers: HashMap::new(),
                h265_depacketizers: HashMap::new(),
                av1_depacketizers: HashMap::new(),
                vp9_depacketizers: HashMap::new(),
                vp8_depacketizers: HashMap::new(),
                track_last_frame_timestamps: HashMap::new(),
                timestamp_normalizers: HashMap::new(),
                video_parameter_sets: HashMap::new(),
                udp_tracks: HashMap::new(),
                udp_task_handles: Vec::new(),
                mute_audio_maker: None,
                codec_probed: HashSet::new(),
            });
            guard.insert(connection_id, state);
        }

        let payload = build_test_rtp_payload(1);
        ingest_publish_rtp_payload(
            connection_id,
            track_id,
            payload.as_ref(),
            &sessions,
            &runtime_api,
        );
        ingest_publish_rtp_payload(
            connection_id,
            track_id,
            payload.as_ref(),
            &sessions,
            &runtime_api,
        );

        let guard = sessions.lock();
        let state = guard.get(&connection_id).expect("state");
        let publish = state.publish.as_ref().expect("publish");
        assert_eq!(publish.pre_record_rtp_drop_count, 2);
    }

    #[test]
    fn ingest_publish_rtp_payload_mp2p_probe_track_skips_clock_path() {
        let connection_id = 89u64;
        let track_id = TrackId(2);
        let sessions: Arc<ParkingMutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
            Arc::new(ParkingMutex::new(HashMap::new()));
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(cheetah_runtime_tokio::TokioRuntime::new());

        let mut track = TrackInfo::new(track_id, MediaKind::Video, CodecId::Unknown, 90_000);
        track.payload_type = Some(96);
        track.extradata = CodecExtradata::Raw(Bytes::from_static(b"rtsp-compat/mp2p-probe/v1"));

        {
            let mut guard = sessions.lock();
            let mut state = RtspConnectionState::new(connection_id);
            let mut tracks = HashMap::new();
            tracks.insert(track_id, track);
            state.publish = Some(PublishSession {
                cancel: CancellationToken::new(),
                lease: PublishLease {
                    stream_id: StreamId(1),
                    stream_key: StreamKey::new("live", "mp2p-probe"),
                    lease_id: 1,
                },
                sink: Box::new(DummySink),
                record_started: true,
                pre_record_rtp_drop_count: 0,
                timestamp_repair_alert_threshold: 32,
                queue_drop_alert_threshold: 64,
                queue_drop_counts: HashMap::new(),
                unsupported_codec_drop_counts: HashMap::new(),
                compat_probe_drop_counts: HashMap::new(),
                tracks,
                track_channels: HashMap::new(),
                rtcp_channels: HashMap::new(),
                clocks: HashMap::new(),
                h264_depacketizers: HashMap::new(),
                h265_depacketizers: HashMap::new(),
                av1_depacketizers: HashMap::new(),
                vp9_depacketizers: HashMap::new(),
                vp8_depacketizers: HashMap::new(),
                track_last_frame_timestamps: HashMap::new(),
                timestamp_normalizers: HashMap::new(),
                video_parameter_sets: HashMap::new(),
                udp_tracks: HashMap::new(),
                udp_task_handles: Vec::new(),
                mute_audio_maker: None,
                codec_probed: HashSet::new(),
            });
            guard.insert(connection_id, state);
        }

        let payload = build_test_rtp_payload(10);
        ingest_publish_rtp_payload(
            connection_id,
            track_id,
            payload.as_ref(),
            &sessions,
            &runtime_api,
        );

        let guard = sessions.lock();
        let state = guard.get(&connection_id).expect("state");
        let publish = state.publish.as_ref().expect("publish");
        assert!(
            publish.clocks.is_empty(),
            "mp2p probe track should not enter normal per-track clock path"
        );
    }

    #[test]
    fn ingest_publish_rtp_payload_unsupported_codec_track_skips_ingest_with_count() {
        let connection_id = 90u64;
        let track_id = TrackId(3);
        let sessions: Arc<ParkingMutex<HashMap<RtspConnectionId, RtspConnectionState>>> =
            Arc::new(ParkingMutex::new(HashMap::new()));
        let runtime_api: Arc<dyn RuntimeApi> = Arc::new(cheetah_runtime_tokio::TokioRuntime::new());

        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::Unknown, 90_000);

        {
            let mut guard = sessions.lock();
            let mut state = RtspConnectionState::new(connection_id);
            let mut tracks = HashMap::new();
            tracks.insert(track_id, track);
            state.publish = Some(PublishSession {
                cancel: CancellationToken::new(),
                lease: PublishLease {
                    stream_id: StreamId(1),
                    stream_key: StreamKey::new("live", "unsupported-codec"),
                    lease_id: 1,
                },
                sink: Box::new(DummySink),
                record_started: true,
                pre_record_rtp_drop_count: 0,
                timestamp_repair_alert_threshold: 32,
                queue_drop_alert_threshold: 64,
                queue_drop_counts: HashMap::new(),
                unsupported_codec_drop_counts: HashMap::new(),
                compat_probe_drop_counts: HashMap::new(),
                tracks,
                track_channels: HashMap::new(),
                rtcp_channels: HashMap::new(),
                clocks: HashMap::new(),
                h264_depacketizers: HashMap::new(),
                h265_depacketizers: HashMap::new(),
                av1_depacketizers: HashMap::new(),
                vp9_depacketizers: HashMap::new(),
                vp8_depacketizers: HashMap::new(),
                track_last_frame_timestamps: HashMap::new(),
                timestamp_normalizers: HashMap::new(),
                video_parameter_sets: HashMap::new(),
                udp_tracks: HashMap::new(),
                udp_task_handles: Vec::new(),
                mute_audio_maker: None,
                codec_probed: HashSet::new(),
            });
            guard.insert(connection_id, state);
        }

        let payload = build_test_rtp_payload(11);
        ingest_publish_rtp_payload(
            connection_id,
            track_id,
            payload.as_ref(),
            &sessions,
            &runtime_api,
        );
        ingest_publish_rtp_payload(
            connection_id,
            track_id,
            payload.as_ref(),
            &sessions,
            &runtime_api,
        );

        let guard = sessions.lock();
        let state = guard.get(&connection_id).expect("state");
        let publish = state.publish.as_ref().expect("publish");
        assert!(
            publish.clocks.is_empty(),
            "unsupported codec track should not enter per-track clock path"
        );
        assert_eq!(
            publish
                .unsupported_codec_drop_counts
                .get(&track_id)
                .copied(),
            Some(2)
        );
    }

    #[test]
    fn repair_video_keyframe_parameter_sets_prepends_cached_h264_sps_pps() {
        let track_id = TrackId(1);
        let cache = ParameterSetCache {
            sps: Some(Bytes::from_static(&[0x67, 1])),
            pps: Some(Bytes::from_static(&[0x68, 2])),
            ..Default::default()
        };
        let mut publish = publish_session_with_video_parameter_cache(track_id, cache);
        let mut frame = AVFrame::new(
            track_id,
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0, 0, 1, 0x65, 9]),
        );
        frame.flags.insert(FrameFlags::KEY);

        repair_video_keyframe_parameter_sets(&mut publish, 1, track_id, &mut frame);

        assert_eq!(
            frame.payload.as_ref(),
            &[0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9]
        );
    }

    #[test]
    fn repair_video_keyframe_parameter_sets_prepends_cached_h265_vps_sps_pps() {
        let track_id = TrackId(2);
        let cache = ParameterSetCache {
            vps: Some(Bytes::from_static(&[0x40, 1])),
            sps: Some(Bytes::from_static(&[0x42, 2])),
            pps: Some(Bytes::from_static(&[0x44, 3])),
        };
        let mut publish = publish_session_with_video_parameter_cache(track_id, cache);
        let mut frame = AVFrame::new(
            track_id,
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0, 0, 1, 0x26, 9]),
        );
        frame.flags.insert(FrameFlags::KEY);

        repair_video_keyframe_parameter_sets(&mut publish, 1, track_id, &mut frame);

        assert_eq!(
            frame.payload.as_ref(),
            &[0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 2, 0, 0, 0, 1, 0x44, 3, 0, 0, 0, 1, 0x26, 9]
        );
    }

    #[test]
    fn repair_video_keyframe_parameter_sets_publishes_discovered_h265_extradata() {
        let track_id = TrackId(3);
        let mut track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H265, 90_000);
        track.refresh_readiness();
        let (mut publish, updates) = publish_session_with_recording_sink(track);
        let mut frame = AVFrame::new(
            track_id,
            MediaKind::Video,
            CodecId::H265,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 2, 0, 0, 0, 1, 0x44, 3, 0, 0, 0, 1, 0x26, 9,
            ]),
        );
        frame.flags.insert(FrameFlags::KEY);

        repair_video_keyframe_parameter_sets(&mut publish, 1, track_id, &mut frame);

        let updates = updates.lock().expect("updates lock");
        let updated_track = updates
            .last()
            .and_then(|tracks| tracks.iter().find(|track| track.track_id == track_id))
            .expect("updated h265 track");
        let CodecExtradata::H265 {
            vps,
            sps,
            pps,
            hvcc,
        } = &updated_track.extradata
        else {
            panic!("expected h265 extradata");
        };
        assert_eq!(vps, &[Bytes::from_static(&[0x40, 1])]);
        assert_eq!(sps, &[Bytes::from_static(&[0x42, 2])]);
        assert_eq!(pps, &[Bytes::from_static(&[0x44, 3])]);
        assert!(hvcc.is_none());
        assert!(updated_track.is_ready());
    }

    #[test]
    fn repair_video_keyframe_parameter_sets_publishes_discovered_h264_extradata() {
        let track_id = TrackId(13);
        let mut track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
        track.refresh_readiness();
        let (mut publish, updates) = publish_session_with_recording_sink(track);
        let mut frame = AVFrame::new(
            track_id,
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[
                0, 0, 0, 1, 0x67, 1, 2, 3, // SPS
                0, 0, 0, 1, 0x68, 4, 5, // PPS
                0, 0, 0, 1, 0x65, 6, 7, 8, // IDR
            ]),
        );
        frame.flags.insert(FrameFlags::KEY);

        repair_video_keyframe_parameter_sets(&mut publish, 1, track_id, &mut frame);

        let updates = updates.lock().expect("updates lock");
        let updated_track = updates
            .last()
            .and_then(|tracks| tracks.iter().find(|track| track.track_id == track_id))
            .expect("updated h264 track");
        let CodecExtradata::H264 { sps, pps, avcc } = &updated_track.extradata else {
            panic!("expected h264 extradata");
        };
        assert_eq!(sps, &[Bytes::from_static(&[0x67, 1, 2, 3])]);
        assert_eq!(pps, &[Bytes::from_static(&[0x68, 4, 5])]);
        assert!(avcc.is_none());
        assert!(updated_track.is_ready());
    }

    #[test]
    fn update_publish_audio_track_config_sets_aac_asc_and_audio_meta() {
        let track_id = TrackId(21);
        let track = TrackInfo::new(track_id, MediaKind::Audio, CodecId::AAC, 48_000);
        let (mut publish, updates) = publish_session_with_recording_sink(track);

        update_publish_audio_track_config(
            &mut publish,
            1,
            track_id,
            Bytes::from_static(&[0x11, 0x90]),
        );

        let updates = updates.lock().expect("updates lock");
        let updated_track = updates
            .last()
            .and_then(|tracks| tracks.iter().find(|track| track.track_id == track_id))
            .expect("updated aac track");
        let CodecExtradata::AAC { asc } = &updated_track.extradata else {
            panic!("expected aac extradata");
        };
        assert_eq!(asc.as_ref(), &[0x11, 0x90]);
        assert_eq!(updated_track.sample_rate, Some(48_000));
        assert_eq!(updated_track.channels, Some(2));
        assert!(updated_track.is_ready());
    }

    #[test]
    fn update_publish_av1_track_config_sets_sequence_and_dimensions() {
        let track_id = TrackId(22);
        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::AV1, 90_000);
        let (mut publish, updates) = publish_session_with_recording_sink(track);

        update_publish_av1_track_config(
            &mut publish,
            1,
            track_id,
            Bytes::from_static(&[0x81, 0x02, 0x03]),
            Bytes::from_static(&[0x81, 0x20]),
            Some((1920, 1080)),
        );

        let updates = updates.lock().expect("updates lock");
        let updated_track = updates
            .last()
            .and_then(|tracks| tracks.iter().find(|track| track.track_id == track_id))
            .expect("updated av1 track");
        let CodecExtradata::AV1 {
            sequence_header,
            codec_config,
        } = &updated_track.extradata
        else {
            panic!("expected av1 extradata");
        };
        assert_eq!(
            sequence_header.as_ref().expect("sequence header").as_ref(),
            &[0x81, 0x02, 0x03]
        );
        assert_eq!(
            codec_config.as_ref().expect("codec config").as_ref(),
            &[0x81, 0x20]
        );
        assert_eq!(updated_track.width, Some(1920));
        assert_eq!(updated_track.height, Some(1080));
        assert!(updated_track.is_ready());
    }

    #[test]
    fn initial_video_parameter_set_caches_uses_announce_sdp_extradata() {
        let track_id = TrackId(1);
        let mut track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[0x67, 1])],
            pps: vec![Bytes::from_static(&[0x68, 2])],
            avcc: None,
        };
        let mut tracks = HashMap::new();
        tracks.insert(track_id, track);

        let caches = initial_video_parameter_set_caches(&tracks);
        let cache = caches.get(&track_id).expect("h264 cache");

        assert_eq!(cache.sps.as_deref(), Some(&[0x67, 1][..]));
        assert_eq!(cache.pps.as_deref(), Some(&[0x68, 2][..]));
    }

    #[test]
    fn normalize_publish_frame_timestamps_repairs_non_monotonic_video_dts() {
        let track_id = TrackId(1);
        let cache = ParameterSetCache::default();
        let mut publish = publish_session_with_video_parameter_cache(track_id, cache);
        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);

        let input_pts = [0_i64, 9_000, 3_000, 6_000, 12_000];
        let mut out = Vec::new();
        for pts in input_pts {
            let mut frame = AVFrame::new(
                track_id,
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                pts,
                pts,
                Timebase::new(1, 90_000),
                Bytes::from_static(&[0, 0, 0, 1, 0x61, 0x00]),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish,
                1,
                &track,
                &mut frame
            ));
            out.push(frame);
        }
        assert_eq!(out.len(), input_pts.len());

        let mut prev_dts = i64::MIN;
        let mut saw_b_frame = false;
        for frame in out {
            assert!(frame.dts > prev_dts, "dts must be strictly monotonic");
            if frame.pts < frame.dts {
                assert!(frame.flags.contains(FrameFlags::B_FRAME));
                saw_b_frame = true;
            }
            prev_dts = frame.dts;
        }
        assert!(saw_b_frame, "reordered pattern should produce B-frame flag");
    }

    #[test]
    fn normalize_publish_frame_timestamps_repairs_non_h264_video_dts() {
        let track_id = TrackId(2);
        let cache = ParameterSetCache::default();
        let mut publish = publish_session_with_video_parameter_cache(track_id, cache);
        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::VP8, 90_000);

        let input_pts = [0_i64, 9_000, 3_000, 6_000];
        let mut prev_dts = i64::MIN;
        for pts in input_pts {
            let mut frame = AVFrame::new(
                track_id,
                MediaKind::Video,
                CodecId::VP8,
                FrameFormat::DataPacket,
                pts,
                pts,
                Timebase::new(1, 90_000),
                Bytes::from_static(&[0x10, 0x00, 0x00]),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish,
                1,
                &track,
                &mut frame
            ));
            assert!(frame.dts > prev_dts, "dts must be strictly monotonic");
            prev_dts = frame.dts;
        }
    }

    #[test]
    fn normalize_publish_frame_timestamps_ignores_raw_video_dts_input_for_all_video_codecs() {
        let codecs = [
            CodecId::H264,
            CodecId::H265,
            CodecId::H266,
            CodecId::AV1,
            CodecId::VP8,
            CodecId::VP9,
        ];

        for (idx, codec) in codecs.iter().copied().enumerate() {
            let track_id = TrackId(90 + idx as u32);
            let track = TrackInfo::new(track_id, MediaKind::Video, codec, 90_000);
            let (format, payload) = match codec {
                CodecId::H264 | CodecId::H265 | CodecId::H266 => (
                    FrameFormat::CanonicalH26x,
                    Bytes::from_static(&[0, 0, 0, 1, 0x61, 0x00]),
                ),
                CodecId::AV1 => (
                    FrameFormat::CanonicalAv1Obu,
                    Bytes::from_static(&[0x12, 0x34]),
                ),
                CodecId::VP8 | CodecId::VP9 => (
                    FrameFormat::DataPacket,
                    Bytes::from_static(&[0x10, 0x00, 0x00]),
                ),
                _ => unreachable!(),
            };

            let mut publish_a =
                publish_session_with_video_parameter_cache(track_id, ParameterSetCache::default());
            let mut frame_a = AVFrame::new(
                track_id,
                MediaKind::Video,
                codec,
                format,
                9_000,
                0,
                Timebase::new(1, 90_000),
                payload.clone(),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish_a,
                1,
                &track,
                &mut frame_a
            ));

            let mut publish_b =
                publish_session_with_video_parameter_cache(track_id, ParameterSetCache::default());
            let mut frame_b = AVFrame::new(
                track_id,
                MediaKind::Video,
                codec,
                format,
                9_000,
                54_000,
                Timebase::new(1, 90_000),
                payload,
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish_b,
                1,
                &track,
                &mut frame_b
            ));

            assert_eq!(frame_a.pts, frame_b.pts, "codec={codec:?}");
            assert_eq!(frame_a.dts, frame_b.dts, "codec={codec:?}");
        }
    }

    #[test]
    fn normalize_publish_frame_timestamps_preserves_audio_dts_input_for_all_audio_codecs() {
        let codecs = [
            (CodecId::AAC, FrameFormat::AacRaw, 48_000, &[0x11, 0x90][..]),
            (
                CodecId::Opus,
                FrameFormat::OpusPacket,
                48_000,
                &[0xF8, 0xFF][..],
            ),
            (
                CodecId::ADPCM,
                FrameFormat::AdpcmPacket,
                8_000,
                &[0x11, 0x22][..],
            ),
            (
                CodecId::G711A,
                FrameFormat::G711Packet,
                8_000,
                &[0x11, 0x22][..],
            ),
            (
                CodecId::G711U,
                FrameFormat::G711Packet,
                8_000,
                &[0x33, 0x44][..],
            ),
            (
                CodecId::MP3,
                FrameFormat::Mp3Frame,
                90_000,
                &[0xFF, 0xFB][..],
            ),
        ];

        for (idx, (codec, format, clock_rate, payload)) in codecs.iter().copied().enumerate() {
            let track_id = TrackId(120 + idx as u32);
            let mut publish =
                publish_session_with_video_parameter_cache(track_id, ParameterSetCache::default());
            let track = TrackInfo::new(track_id, MediaKind::Audio, codec, clock_rate);
            let timebase = Timebase::new(1, clock_rate);
            let first_source_dts = i64::from(clock_rate * 2);
            let second_source_dts = i64::from(clock_rate * 2 + clock_rate / 4);
            let expected_delta = second_source_dts - first_source_dts;

            let mut first = AVFrame::new(
                track_id,
                MediaKind::Audio,
                codec,
                format,
                first_source_dts + i64::from(clock_rate),
                first_source_dts,
                timebase,
                Bytes::copy_from_slice(payload),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish,
                1,
                &track,
                &mut first
            ));

            let mut second = AVFrame::new(
                track_id,
                MediaKind::Audio,
                codec,
                format,
                second_source_dts + i64::from(clock_rate),
                second_source_dts,
                timebase,
                Bytes::copy_from_slice(payload),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish,
                1,
                &track,
                &mut second
            ));

            assert_eq!(first.dts, 0, "codec={codec:?}");
            assert_eq!(second.dts, expected_delta, "codec={codec:?}");
        }
    }

    #[test]
    fn frame_observability_fields_cover_required_keys() {
        let frame = AVFrame::new(
            TrackId(8),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            12_345,
            12_300,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x12, 0x34]),
        );

        let fields = frame_observability_fields(&frame);
        assert_eq!(fields.track_id, 8);
        assert_eq!(fields.codec, CodecId::AV1);
        assert_eq!(fields.pts, 12_345);
        assert_eq!(fields.dts, 12_300);
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
        assert!(!cheetah_codec::should_emit_alert_threshold(63, 64));
        assert!(cheetah_codec::should_emit_alert_threshold(64, 64));
        assert!(cheetah_codec::should_emit_alert_threshold(128, 64));
    }

    #[test]
    fn canonical_repair_warn_sampling_uses_first_and_powers_of_two_only() {
        let sampled = (1_u64..=20)
            .filter(|count| should_warn_canonical_repair(*count))
            .collect::<Vec<_>>();
        assert_eq!(sampled, vec![1, 2, 4, 8, 16]);
        assert!(should_warn_canonical_repair(32));
        assert!(should_warn_canonical_repair(64));
        assert!(should_warn_canonical_repair(1024));
        assert!(!should_warn_canonical_repair(48));
        assert!(!should_warn_canonical_repair(96));
        assert!(!should_warn_canonical_repair(1023));
    }

    #[test]
    fn classify_rtsp_publish_alert_class_prioritizes_discontinuity_then_repair_then_disorder() {
        assert_eq!(
            classify_rtsp_publish_alert_class(&[TimestampAlert::PtsReorderObserved], false),
            Some(RtspPublishAlertClass::SourceDisorder)
        );
        assert_eq!(
            classify_rtsp_publish_alert_class(
                &[
                    TimestampAlert::PtsReorderObserved,
                    TimestampAlert::NonMonotonicDtsRepaired
                ],
                false
            ),
            Some(RtspPublishAlertClass::CanonicalRepair)
        );
        assert_eq!(
            classify_rtsp_publish_alert_class(
                &[TimestampAlert::TimelineDiscontinuityDetected],
                false
            ),
            Some(RtspPublishAlertClass::Discontinuity)
        );
        assert_eq!(
            classify_rtsp_publish_alert_class(&[TimestampAlert::NonMonotonicDtsRepaired], true),
            Some(RtspPublishAlertClass::Discontinuity)
        );
    }

    #[test]
    fn normalize_publish_frame_timestamps_tracks_bframe_reorder_with_bounded_repair() {
        let track_id = TrackId(30);
        let mut publish =
            publish_session_with_video_parameter_cache(track_id, ParameterSetCache::default());
        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
        let input_pts = [0_i64, 9_000, 3_000];
        for pts in input_pts {
            let mut frame = AVFrame::new(
                track_id,
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                pts,
                pts,
                Timebase::new(1, 90_000),
                Bytes::from_static(&[0, 0, 0, 1, 0x61, 0x00]),
            );
            assert!(normalize_publish_frame_timestamps(
                &mut publish,
                1,
                &track,
                &mut frame
            ));
        }
        let state = publish
            .timestamp_normalizers
            .get(&track_id)
            .expect("timestamp normalizer state");
        assert!(
            state.repair_count >= 1,
            "reordered pts should trigger bounded canonical repair for monotonic dts"
        );
        assert_eq!(
            state.source_disorder_count, 0,
            "with dts+pts ingress mode, reorder should be repaired in canonical lane"
        );
    }

    #[test]
    fn normalize_publish_frame_timestamps_video_reorder_keeps_dts_drift_bounded() {
        let track_id = TrackId(31);
        let mut publish =
            publish_session_with_video_parameter_cache(track_id, ParameterSetCache::default());
        let track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
        let mut max_dts_pts_diff = 0_i64;
        for _ in 0..40 {
            for pts in [0_i64, 9_000, 3_000, 12_000] {
                let mut frame = AVFrame::new(
                    track_id,
                    MediaKind::Video,
                    CodecId::H264,
                    FrameFormat::CanonicalH26x,
                    pts,
                    pts,
                    Timebase::new(1, 90_000),
                    Bytes::from_static(&[0, 0, 0, 1, 0x61, 0x00]),
                );
                assert!(normalize_publish_frame_timestamps(
                    &mut publish,
                    1,
                    &track,
                    &mut frame
                ));
                max_dts_pts_diff = max_dts_pts_diff.max(frame.dts.saturating_sub(frame.pts));
            }
        }
        assert!(
            max_dts_pts_diff <= 90_000,
            "video dts drift too large under reorder: {max_dts_pts_diff}"
        );
    }

    #[test]
    fn monotonic_dts_min_step_matches_timebase_millisecond_tick() {
        assert_eq!(
            cheetah_codec::monotonic_dts_min_step(Timebase::new(1, 90_000)),
            90
        );
        assert_eq!(
            cheetah_codec::monotonic_dts_min_step(Timebase::new(1, 1_000)),
            1
        );
        assert_eq!(
            cheetah_codec::monotonic_dts_min_step(Timebase::new(1, 48_000)),
            48
        );
    }

    #[test]
    fn fallback_step_for_video_prefers_duration_then_fps_then_clock_hint() {
        let track_id = TrackId(20);
        let timebase = Timebase::new(1, 90_000);
        let mut track = TrackInfo::new(track_id, MediaKind::Video, CodecId::H264, 90_000);
        track.fps = Some(cheetah_codec::Rational32::new(30, 1));
        let mut frame = AVFrame::new(
            track_id,
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            timebase,
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        assert_eq!(
            fallback_step_for_publish_frame(&track, &frame, timebase),
            3_000
        );

        frame.duration = 3_600;
        assert_eq!(
            fallback_step_for_publish_frame(&track, &frame, timebase),
            3_600
        );

        track.fps = None;
        frame.duration = 0;
        assert_eq!(
            fallback_step_for_publish_frame(&track, &frame, timebase),
            3_000
        );
    }
}
