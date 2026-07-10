use bytes::Bytes;
#[cfg(test)]
use bytes::BytesMut;
use cheetah_codec::{
    aac_channel_count_from_asc, aac_channel_count_from_config, codec_from_rtmp_codec_id,
    video_payload_is_random_access, AVFrame, AacAudioSpecificConfig, CodecExtradata, CodecId,
    FrameFlags, FrameFormat, MediaKind, SourceTimestamp, Timebase, TimestampNormalizeInput,
    TimestampNormalizeMode, TimestampNormalizeOutput, TimestampValue, TrackId, TrackInfo,
};
use cheetah_rtmp_core::{
    apply_flv_metadata_to_tracks as core_apply_flv_metadata_to_tracks,
    apply_flv_video_config as core_apply_flv_video_config,
    attach_raw_rtmp_audio_payload as core_attach_raw_rtmp_audio_payload,
    attach_raw_rtmp_video_payload as core_attach_raw_rtmp_video_payload,
    length_prefixed_to_annexb_with_size as core_length_prefixed_to_annexb_with_size,
    maybe_reset_rtmp_timestamp_normalizer as core_maybe_reset_rtmp_timestamp_normalizer,
    parse_video_ingress_header,
    source_timestamp_from_rtmp_ms as core_source_timestamp_from_rtmp_ms,
    update_timestamp_repair_counter as core_update_timestamp_repair_counter, AmfValue,
};
#[cfg(test)]
use cheetah_rtmp_core::{
    parse_flv_avcc_parameter_sets as core_parse_flv_avcc_parameter_sets,
    parse_flv_hvcc_parameter_sets as core_parse_flv_hvcc_parameter_sets,
};

use crate::nal::{avcc_nal_length_size, hvcc_nal_length_size, normalize_nal_length_size};
use crate::session::{PublishSession, PublishTrackTimestampState};

pub(crate) const RTMP_VIDEO_RAW_SIDEDATA_MAGIC: &[u8] = b"rtmp-video-raw:";
pub(crate) const RTMP_AUDIO_RAW_SIDEDATA_MAGIC: &[u8] = b"rtmp-audio-raw:";
const RTMP_AUDIO_FOURCC_OPUS: &[u8; 4] = b"Opus";
const ENHANCED_RTMP_AUDIO_SEQUENCE_START: u8 = 0;
const ENHANCED_RTMP_AUDIO_CODED_FRAMES: u8 = 1;
const ENHANCED_RTMP_AUDIO_METADATA: u8 = 4;

/// Handles an RTMP data tag (currently a no-op placeholder).
///
/// 处理 RTMP 数据标签（当前为占位空实现）。
pub(crate) fn handle_data_ingest(_session: &mut PublishSession, _payload: &[u8]) {}

/// Applies `onMetaData` values to the publish session's audio and video tracks.
///
/// Delegates to the core FLV metadata parser to extract codec hints, dimensions,
/// and frame-rate fields.
///
/// 将 `onMetaData` 值应用到发布会话的音频与视频轨道。
///
/// 委托给核心 FLV 元数据解析器，提取编解码器提示、分辨率与帧率字段。
pub(crate) fn apply_metadata_to_tracks(session: &mut PublishSession, values: &[AmfValue]) {
    core_apply_flv_metadata_to_tracks(&mut session.tracks.video, &mut session.tracks.audio, values);
}

#[cfg(test)]
const DEFAULT_TIMESTAMP_REPAIR_ALERT_THRESHOLD: u64 = 32;

#[cfg(test)]
pub(crate) fn handle_video_ingest(
    session: &mut PublishSession,
    timestamp_ms: u32,
    payload: &[u8],
) -> Option<AVFrame> {
    handle_video_ingest_with_alert_threshold(
        session,
        timestamp_ms,
        payload,
        DEFAULT_TIMESTAMP_REPAIR_ALERT_THRESHOLD,
    )
}

/// Parses an RTMP video tag into an internal `AVFrame`.
///
/// Handles sequence headers, normalizes DTS/PTS with composition offsets, converts
/// length-prefixed NALUs to AnnexB, and sets key-frame/discontinuity flags.
///
/// 将 RTMP 视频标签解析为内部 `AVFrame`。
///
/// 处理序列头、归一化含合成偏移的 DTS/PTS、将长度前缀 NALU 转换为 AnnexB，
/// 并设置关键帧/不连续标记。
pub(crate) fn handle_video_ingest_with_alert_threshold(
    session: &mut PublishSession,
    timestamp_ms: u32,
    payload: &[u8],
    repair_alert_threshold: u64,
) -> Option<AVFrame> {
    let header = parse_video_ingress_header(payload)?;

    if header.packet_type == 0 {
        apply_video_config(session, header.codec, &payload[header.payload_offset..]);
        return None;
    }

    // RTMP video ingest keeps DTS sourced from tag timestamp and derives
    // PTS via CTS. When source timestamps regress, enforce monotonic DTS and
    // carry the same CTS offset forward.
    let normalized_ts = {
        let state = &mut session.timestamp_states.video;
        maybe_reset_rtmp_timestamp_normalizer(state, timestamp_ms);
        let normalized = match state.normalizer.normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                dts: TimestampValue::Wrapped(u64::from(timestamp_ms)),
                composition_offset: Some(i64::from(header.cts)),
            },
            frame_duration: None,
            fallback_step: Some(1),
            is_video: true,
            force_discontinuity: false,
        }) {
            Ok(value) => value,
            Err(err) => {
                state.last_raw_timestamp_ms = Some(timestamp_ms);
                tracing::warn!(
                    stream_key = %session.lease.stream_key,
                    track_id = TrackId(1).0,
                    codec = ?header.codec,
                    protocol_ingress = "rtmp-publish",
                    raw_timestamp_ms = timestamp_ms,
                    cts = header.cts,
                    "rtmp ingest video timestamp normalize failed: {err}"
                );
                return None;
            }
        };
        state.last_raw_timestamp_ms = Some(timestamp_ms);
        let (repair_count, sampled_repair_log) =
            update_repair_counter(&normalized, &mut state.repair_count);
        (normalized, repair_count, sampled_repair_log)
    };
    log_timestamp_repair_sample(
        session,
        TimestampRepairLogContext {
            track_id: TrackId(1),
            codec: header.codec,
            raw_timestamp_ms: timestamp_ms,
            normalized_ts: &normalized_ts.0,
            repair_count: normalized_ts.1,
            sampled_repair_log: normalized_ts.2,
            message: "rtmp ingest video timestamp repaired for monotonic dts",
        },
        repair_alert_threshold,
    );
    let frame_payload = &payload[header.payload_offset..];
    let nal_length_size = configured_h26x_nal_length_size(session, header.codec);

    let (format, data) = match header.codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => (
            FrameFormat::CanonicalH26x,
            length_prefixed_to_annexb_with_size(frame_payload, nal_length_size),
        ),
        CodecId::AV1 => (
            FrameFormat::CanonicalAv1Obu,
            Bytes::copy_from_slice(frame_payload),
        ),
        CodecId::VP8 => (
            FrameFormat::CanonicalVp8Frame,
            Bytes::copy_from_slice(frame_payload),
        ),
        CodecId::VP9 => (
            FrameFormat::CanonicalVp9Frame,
            Bytes::copy_from_slice(frame_payload),
        ),
        CodecId::Unknown => (FrameFormat::Unknown, Bytes::copy_from_slice(frame_payload)),
        _ => return None,
    };

    ensure_video_track(session, header.codec);
    let mut frame = AVFrame::new(
        TrackId(1),
        MediaKind::Video,
        header.codec,
        format,
        normalized_ts.0.pts,
        normalized_ts.0.dts,
        Timebase::new(1, 1000),
        data,
    );
    if header.frame_type == 1
        && video_payload_is_random_access(header.codec, format, frame.payload.as_ref())
    {
        frame.flags.insert(FrameFlags::KEY);
    }
    if frame.pts < frame.dts {
        frame.flags.insert(FrameFlags::B_FRAME);
    }
    if normalized_ts.0.discontinuity {
        frame.flags.insert(FrameFlags::DISCONTINUITY);
    }
    frame.set_source_timestamp(source_timestamp_from_rtmp_ms(timestamp_ms));
    if matches!(
        header.codec,
        CodecId::H264
            | CodecId::H265
            | CodecId::H266
            | CodecId::AV1
            | CodecId::VP8
            | CodecId::VP9
            | CodecId::Unknown
    ) {
        attach_raw_rtmp_video_payload(&mut frame, payload);
    }
    Some(frame)
}

/// Applies a video sequence header (AVCC/HVCC/VPCC/AV1 config) to the track.
///
/// 将视频序列头（AVCC/HVCC/VPCC/AV1 配置）应用到轨道。
pub(crate) fn apply_video_config(
    session: &mut PublishSession,
    codec: CodecId,
    config_payload: &[u8],
) {
    core_apply_flv_video_config(&mut session.tracks.video, codec, config_payload);
}

/// Determines the NAL length size to use for H.264/H.265/H.266 ingress.
///
/// Falls back to 4 bytes when no track config is available or the codec has changed.
///
/// 确定 H.264/H.265/H.266 输入使用的 NAL 长度大小。
///
/// 没有可用轨道配置或编解码器改变时回退为 4 字节。
fn configured_h26x_nal_length_size(session: &PublishSession, codec: CodecId) -> usize {
    let Some(track) = session.tracks.video.as_ref() else {
        return 4;
    };
    if track.codec != codec {
        return 4;
    }
    let parsed = match (&track.codec, &track.extradata) {
        (
            CodecId::H264,
            CodecExtradata::H264 {
                avcc: Some(avcc), ..
            },
        ) => avcc_nal_length_size(avcc),
        (
            CodecId::H265,
            CodecExtradata::H265 {
                hvcc: Some(hvcc), ..
            },
        ) => hvcc_nal_length_size(hvcc),
        _ => None,
    };
    normalize_nal_length_size(parsed.unwrap_or(4))
}

/// Ensures a video track exists for the session, creating one with default timing if needed.
///
/// 确保会话存在视频轨道，需要时使用默认时序创建。
fn ensure_video_track(session: &mut PublishSession, codec: CodecId) {
    let mut track = session
        .tracks
        .video
        .clone()
        .unwrap_or_else(|| TrackInfo::new(TrackId(1), MediaKind::Video, codec, 90_000));
    track.codec = codec;
    session.tracks.video = Some(track);
}

#[cfg(test)]
pub(crate) fn parse_hvcc_parameter_sets(
    payload: &[u8],
    codec: CodecId,
) -> (Vec<Bytes>, Vec<Bytes>, Vec<Bytes>) {
    core_parse_flv_hvcc_parameter_sets(payload, codec)
}

#[cfg(test)]
pub(crate) fn handle_audio_ingest(
    session: &mut PublishSession,
    timestamp_ms: u32,
    payload: &[u8],
) -> Option<AVFrame> {
    handle_audio_ingest_with_alert_threshold(
        session,
        timestamp_ms,
        payload,
        DEFAULT_TIMESTAMP_REPAIR_ALERT_THRESHOLD,
    )
}

/// Handles an enhanced RTMP audio tag (fourcc header), currently supporting Opus.
///
/// Sequence start packets update the Opus track; coded frames produce an audio frame.
///
/// 处理增强 RTMP 音频标签（fourcc 头），当前支持 Opus。
///
/// 序列开始包更新 Opus 轨道；编码帧生成音频帧。
fn handle_enhanced_audio_ingest(
    session: &mut PublishSession,
    timestamp_ms: u32,
    payload: &[u8],
    repair_alert_threshold: u64,
) -> Option<AVFrame> {
    if payload.len() < 5 || &payload[1..5] != RTMP_AUDIO_FOURCC_OPUS {
        return None;
    }

    match payload[0] & 0x0f {
        ENHANCED_RTMP_AUDIO_SEQUENCE_START => {
            update_opus_track_from_sequence_header(session, &payload[5..]);
            None
        }
        ENHANCED_RTMP_AUDIO_CODED_FRAMES => {
            ensure_opus_audio_track(session);
            build_normalized_audio_frame(
                session,
                timestamp_ms,
                CodecId::Opus,
                FrameFormat::OpusPacket,
                &payload[5..],
                repair_alert_threshold,
            )
        }
        ENHANCED_RTMP_AUDIO_METADATA => None,
        _ => None,
    }
}

/// Checks whether an audio payload is an enhanced RTMP Opus packet.
///
/// 判断音频负载是否为增强 RTMP Opus 包。
fn is_enhanced_opus_audio_payload(payload: &[u8]) -> bool {
    payload.len() >= 5 && payload[0] & 0x80 != 0 && &payload[1..5] == RTMP_AUDIO_FOURCC_OPUS
}

/// Updates the audio track from an enhanced Opus `OpusHead` sequence header.
///
/// Parses channel count, sample rate, and channel mapping from the header.
///
/// 从增强 Opus 的 `OpusHead` 序列头更新音频轨道。
///
/// 解析头部中的声道数、采样率与声道映射。
fn update_opus_track_from_sequence_header(session: &mut PublishSession, opus_head: &[u8]) {
    let mut track = session.tracks.audio.clone().unwrap_or_else(|| {
        TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::Opus,
            default_audio_clock(CodecId::Opus),
        )
    });
    track.codec = CodecId::Opus;
    track.extradata = CodecExtradata::Opus {
        fmtp: None,
        channel_mapping: None,
    };

    if opus_head.len() >= 19 && &opus_head[..8] == b"OpusHead" {
        let channels = opus_head[9].max(1);
        let sample_rate =
            u32::from_le_bytes([opus_head[12], opus_head[13], opus_head[14], opus_head[15]]);
        track.channels = Some(channels);
        track.sample_rate = Some(sample_rate.max(1));
        if opus_head[18] != 0 && opus_head.len() > 19 {
            track.extradata = CodecExtradata::Opus {
                fmtp: None,
                channel_mapping: Some(Bytes::copy_from_slice(&opus_head[18..])),
            };
        }
    } else {
        track.channels.get_or_insert(2);
        track.sample_rate.get_or_insert(48_000);
    }

    track.clock_rate = track.sample_rate.unwrap_or(48_000);
    track.refresh_readiness();
    session.tracks.audio = Some(track);
}

/// Ensures a default Opus audio track exists in the session.
///
/// 确保会话中存在默认 Opus 音频轨道。
fn ensure_opus_audio_track(session: &mut PublishSession) {
    if session
        .tracks
        .audio
        .as_ref()
        .is_some_and(|track| track.codec == CodecId::Opus)
    {
        return;
    }
    let mut track = TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::Opus, 48_000);
    track.sample_rate = Some(48_000);
    track.channels = Some(2);
    track.extradata = CodecExtradata::Opus {
        fmtp: None,
        channel_mapping: None,
    };
    track.refresh_readiness();
    session.tracks.audio = Some(track);
}

/// Builds a normalized audio `AVFrame` from the parsed payload and timestamp.
///
/// Infers a fallback duration from the codec and track metadata, normalizes the
/// timestamp, and attaches the source timestamp.
///
/// 根据解析后的负载与时间戳构建归一化的音频 `AVFrame`。
///
/// 从编解码器与轨道元数据推断回退时长，归一化时间戳，并附加源时间戳。
fn build_normalized_audio_frame(
    session: &mut PublishSession,
    timestamp_ms: u32,
    codec: CodecId,
    format: FrameFormat,
    frame_payload: &[u8],
    repair_alert_threshold: u64,
) -> Option<AVFrame> {
    let track = session
        .tracks
        .audio
        .as_ref()
        .cloned()
        .unwrap_or_else(|| TrackInfo::new(TrackId(2), MediaKind::Audio, codec, 48_000));
    let fallback_step = infer_audio_duration_ms(codec, &track, frame_payload);
    let normalized_ts = {
        let state = &mut session.timestamp_states.audio;
        maybe_reset_rtmp_timestamp_normalizer(state, timestamp_ms);
        let normalized = match state.normalizer.normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                dts: TimestampValue::Wrapped(u64::from(timestamp_ms)),
                composition_offset: None,
            },
            frame_duration: Some(fallback_step),
            fallback_step: Some(fallback_step),
            is_video: false,
            force_discontinuity: false,
        }) {
            Ok(value) => value,
            Err(err) => {
                state.last_raw_timestamp_ms = Some(timestamp_ms);
                tracing::warn!(
                    stream_key = %session.lease.stream_key,
                    track_id = TrackId(2).0,
                    codec = ?codec,
                    protocol_ingress = "rtmp-publish",
                    raw_timestamp_ms = timestamp_ms,
                    "rtmp ingest audio timestamp normalize failed: {err}"
                );
                return None;
            }
        };
        state.last_raw_timestamp_ms = Some(timestamp_ms);
        let (repair_count, sampled_repair_log) =
            update_repair_counter(&normalized, &mut state.repair_count);
        (normalized, repair_count, sampled_repair_log)
    };
    log_timestamp_repair_sample(
        session,
        TimestampRepairLogContext {
            track_id: TrackId(2),
            codec,
            raw_timestamp_ms: timestamp_ms,
            normalized_ts: &normalized_ts.0,
            repair_count: normalized_ts.1,
            sampled_repair_log: normalized_ts.2,
            message: "rtmp ingest audio timestamp repaired for monotonic dts",
        },
        repair_alert_threshold,
    );
    let mut frame = AVFrame::new(
        TrackId(2),
        MediaKind::Audio,
        codec,
        format,
        normalized_ts.0.pts,
        normalized_ts.0.dts,
        Timebase::new(1, 1000),
        Bytes::copy_from_slice(frame_payload),
    );
    let _ = frame.set_duration(fallback_step);
    if normalized_ts.0.discontinuity {
        frame.flags.insert(FrameFlags::DISCONTINUITY);
    }
    frame.set_source_timestamp(source_timestamp_from_rtmp_ms(timestamp_ms));
    Some(frame)
}

/// Parses an RTMP audio tag into an internal `AVFrame`.
///
/// Handles AAC sequence headers, Opus enhanced audio, and legacy FLV sound formats.
/// Normalizes timestamps and preserves source timestamps.
///
/// 将 RTMP 音频标签解析为内部 `AVFrame`。
///
/// 处理 AAC 序列头、Opus 增强音频与旧版 FLV 声音格式；归一化时间戳并保留源时间戳。
pub(crate) fn handle_audio_ingest_with_alert_threshold(
    session: &mut PublishSession,
    timestamp_ms: u32,
    payload: &[u8],
    repair_alert_threshold: u64,
) -> Option<AVFrame> {
    if payload.len() < 2 {
        return None;
    }

    if is_enhanced_opus_audio_payload(payload) {
        return handle_enhanced_audio_ingest(
            session,
            timestamp_ms,
            payload,
            repair_alert_threshold,
        );
    }

    let sound_format = payload[0] >> 4;
    let sound_rate = (payload[0] >> 2) & 0x03;
    let sound_size = (payload[0] >> 1) & 0x01;
    let sound_channel = payload[0] & 0x01;
    let codec =
        codec_from_rtmp_codec_id(MediaKind::Audio, sound_format).unwrap_or(CodecId::Unknown);

    if codec == CodecId::AAC {
        let packet_type = payload[1];
        if packet_type == 0 {
            if payload.len() < 3 {
                return None;
            }
            let asc = Bytes::copy_from_slice(&payload[2..]);
            let asc_parsed = AacAudioSpecificConfig::from_bytes(&asc);
            let mut track = session.tracks.audio.clone().unwrap_or_else(|| {
                TrackInfo::new(TrackId(2), MediaKind::Audio, CodecId::AAC, 48_000)
            });
            if let Some(asc_cfg) = asc_parsed {
                track.sample_rate = sample_rate_from_index(asc_cfg.sampling_frequency_index);
                track.channels = aac_channel_count_from_asc(&asc).or_else(|| {
                    if asc_cfg.channel_configuration > 0 {
                        aac_channel_count_from_config(asc_cfg.channel_configuration)
                    } else {
                        // ch_cfg=0 without a parseable PCE: use FLV sound_channel fallback.
                        Some(if sound_channel == 1 { 2 } else { 1 })
                    }
                });
                track.clock_rate = track.sample_rate.unwrap_or(48_000);
            }
            track.codec = CodecId::AAC;
            track.extradata = CodecExtradata::AAC { asc };
            track.refresh_readiness();
            session.tracks.audio = Some(track);
            return None;
        }
        if packet_type != 1 {
            return None;
        }
        ensure_audio_track(session, codec, sound_rate, sound_channel);
        let track = session
            .tracks
            .audio
            .as_ref()
            .cloned()
            .unwrap_or_else(|| TrackInfo::new(TrackId(2), MediaKind::Audio, codec, 48_000));
        let fallback_step = infer_audio_duration_ms(codec, &track, &payload[2..]);
        let normalized_ts = {
            let state = &mut session.timestamp_states.audio;
            maybe_reset_rtmp_timestamp_normalizer(state, timestamp_ms);
            let normalized = match state.normalizer.normalize(TimestampNormalizeInput {
                mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                    dts: TimestampValue::Wrapped(u64::from(timestamp_ms)),
                    composition_offset: None,
                },
                frame_duration: Some(fallback_step),
                fallback_step: Some(fallback_step),
                is_video: false,
                force_discontinuity: false,
            }) {
                Ok(value) => value,
                Err(err) => {
                    state.last_raw_timestamp_ms = Some(timestamp_ms);
                    tracing::warn!(
                        stream_key = %session.lease.stream_key,
                        track_id = TrackId(2).0,
                        codec = ?CodecId::AAC,
                        protocol_ingress = "rtmp-publish",
                        raw_timestamp_ms = timestamp_ms,
                        "rtmp ingest audio timestamp normalize failed: {err}"
                    );
                    return None;
                }
            };
            state.last_raw_timestamp_ms = Some(timestamp_ms);
            let (repair_count, sampled_repair_log) =
                update_repair_counter(&normalized, &mut state.repair_count);
            (normalized, repair_count, sampled_repair_log)
        };
        log_timestamp_repair_sample(
            session,
            TimestampRepairLogContext {
                track_id: TrackId(2),
                codec: CodecId::AAC,
                raw_timestamp_ms: timestamp_ms,
                normalized_ts: &normalized_ts.0,
                repair_count: normalized_ts.1,
                sampled_repair_log: normalized_ts.2,
                message: "rtmp ingest audio timestamp repaired for monotonic dts",
            },
            repair_alert_threshold,
        );
        let mut frame = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            normalized_ts.0.pts,
            normalized_ts.0.dts,
            Timebase::new(1, 1000),
            Bytes::copy_from_slice(&payload[2..]),
        );
        let _ = frame.set_duration(fallback_step);
        if normalized_ts.0.discontinuity {
            frame.flags.insert(FrameFlags::DISCONTINUITY);
        }
        frame.set_source_timestamp(source_timestamp_from_rtmp_ms(timestamp_ms));
        return Some(frame);
    }

    ensure_audio_track(session, codec, sound_rate, sound_channel);
    let format = match codec {
        CodecId::ADPCM => FrameFormat::AdpcmPacket,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::Opus => FrameFormat::OpusPacket,
        CodecId::Unknown => FrameFormat::Unknown,
        _ => return None,
    };
    let track = session
        .tracks
        .audio
        .as_ref()
        .cloned()
        .unwrap_or_else(|| TrackInfo::new(TrackId(2), MediaKind::Audio, codec, 48_000));
    let fallback_step = infer_audio_duration_ms(codec, &track, &payload[1..]);
    let normalized_ts = {
        let state = &mut session.timestamp_states.audio;
        maybe_reset_rtmp_timestamp_normalizer(state, timestamp_ms);
        let normalized = match state.normalizer.normalize(TimestampNormalizeInput {
            mode: TimestampNormalizeMode::DtsWithCompositionOffset {
                dts: TimestampValue::Wrapped(u64::from(timestamp_ms)),
                composition_offset: None,
            },
            frame_duration: Some(fallback_step),
            fallback_step: Some(fallback_step),
            is_video: false,
            force_discontinuity: false,
        }) {
            Ok(value) => value,
            Err(err) => {
                state.last_raw_timestamp_ms = Some(timestamp_ms);
                tracing::warn!(
                    stream_key = %session.lease.stream_key,
                    track_id = TrackId(2).0,
                    codec = ?codec,
                    protocol_ingress = "rtmp-publish",
                    raw_timestamp_ms = timestamp_ms,
                    "rtmp ingest audio timestamp normalize failed: {err}"
                );
                return None;
            }
        };
        state.last_raw_timestamp_ms = Some(timestamp_ms);
        let (repair_count, sampled_repair_log) =
            update_repair_counter(&normalized, &mut state.repair_count);
        (normalized, repair_count, sampled_repair_log)
    };
    log_timestamp_repair_sample(
        session,
        TimestampRepairLogContext {
            track_id: TrackId(2),
            codec,
            raw_timestamp_ms: timestamp_ms,
            normalized_ts: &normalized_ts.0,
            repair_count: normalized_ts.1,
            sampled_repair_log: normalized_ts.2,
            message: "rtmp ingest audio timestamp repaired for monotonic dts",
        },
        repair_alert_threshold,
    );
    let mut frame = AVFrame::new(
        TrackId(2),
        MediaKind::Audio,
        codec,
        format,
        normalized_ts.0.pts,
        normalized_ts.0.dts,
        Timebase::new(1, 1000),
        Bytes::copy_from_slice(&payload[1..]),
    );
    let _ = frame.set_duration(fallback_step);
    if normalized_ts.0.discontinuity {
        frame.flags.insert(FrameFlags::DISCONTINUITY);
    }
    frame.set_source_timestamp(source_timestamp_from_rtmp_ms(timestamp_ms));
    if sound_size == 1 {
        frame.flags.insert(FrameFlags::END_OF_AU);
    }
    if codec == CodecId::Unknown {
        attach_raw_rtmp_audio_payload(&mut frame, payload);
    }
    Some(frame)
}

fn source_timestamp_from_rtmp_ms(raw_timestamp_ms: u32) -> SourceTimestamp {
    core_source_timestamp_from_rtmp_ms(raw_timestamp_ms)
}

/// Logs a timestamp repair event, throttling to avoid log spam.
///
/// Emits once when the repair count reaches the alert threshold and multiples.
///
/// 记录时间戳修复事件，并进行限速以避免日志风暴。
///
/// 在修复计数达到告警阈值及倍数时触发一次。
fn log_timestamp_repair_sample(
    session: &PublishSession,
    context: TimestampRepairLogContext<'_>,
    repair_alert_threshold: u64,
) {
    let threshold_alert = should_emit_alert_threshold(context.repair_count, repair_alert_threshold);
    if !context.sampled_repair_log && !threshold_alert {
        return;
    }
    tracing::warn!(
        stream_key = %session.lease.stream_key,
        track_id = context.track_id.0,
        codec = ?context.codec,
        protocol_ingress = "rtmp-publish",
        pts = context.normalized_ts.pts,
        dts = context.normalized_ts.dts,
        raw_timestamp_ms = context.raw_timestamp_ms,
        repair_count = context.repair_count,
        repair_alert_threshold,
        threshold_alert,
        alerts = ?context.normalized_ts.alerts,
        "{}",
        context.message
    );
}

/// Context captured for a timestamp repair log sample.
///
/// 时间戳修复日志采样捕获的上下文。
struct TimestampRepairLogContext<'a> {
    track_id: TrackId,
    codec: CodecId,
    raw_timestamp_ms: u32,
    normalized_ts: &'a TimestampNormalizeOutput,
    repair_count: u64,
    sampled_repair_log: bool,
    message: &'static str,
}

/// Resets the timestamp normalizer when the raw timestamp wraps or jumps backward.
///
/// 当 raw 时间戳回绕或大幅向后跳变时重置时间戳归一器。
fn maybe_reset_rtmp_timestamp_normalizer(
    state: &mut PublishTrackTimestampState,
    raw_timestamp_ms: u32,
) {
    core_maybe_reset_rtmp_timestamp_normalizer(
        &mut state.normalizer,
        &mut state.repair_count,
        &mut state.last_raw_timestamp_ms,
        raw_timestamp_ms,
    );
}

/// Updates the running timestamp repair counter and returns a sampling flag.
///
/// 更新累计的时间戳修复计数并返回采样标志。
fn update_repair_counter(
    normalized: &TimestampNormalizeOutput,
    repair_count: &mut u64,
) -> (u64, bool) {
    core_update_timestamp_repair_counter(normalized, repair_count)
}

/// Returns true when a counter crosses or repeats an alert threshold.
///
/// 当计数器跨越或重复告警阈值时返回 true。
pub(crate) fn should_emit_alert_threshold(count: u64, threshold: u64) -> bool {
    cheetah_codec::should_emit_alert_threshold(count, threshold)
}

/// Ensures an audio track exists for the session, deriving sample rate and channels.
///
/// For non-AAC codecs, the FLV sound rate bits are preferred because metadata may be stale.
///
/// 确保会话存在音频轨道，并推导采样率与声道数。
///
/// 对于非 AAC 编解码器，优先使用 FLV 声音速率位，因为元数据可能过时。
fn ensure_audio_track(session: &mut PublishSession, codec: CodecId, rate: u8, channels_flag: u8) {
    let mut track = session.tracks.audio.clone().unwrap_or_else(|| {
        TrackInfo::new(
            TrackId(2),
            MediaKind::Audio,
            codec,
            default_audio_clock(codec),
        )
    });
    track.codec = codec;
    let derived_sample_rate =
        audio_sample_rate_from_payload(codec, rate).or_else(|| default_audio_sample_rate(codec));
    if codec == CodecId::AAC {
        if track.sample_rate.is_none() {
            track.sample_rate = derived_sample_rate;
        }
    } else {
        // Non-AAC stream headers commonly carry stale metadata.
        // Keep track timing aligned with actual codec/rate bits from media tags.
        track.sample_rate = derived_sample_rate;
    }
    if codec == CodecId::AAC {
        if track.channels.is_none() {
            track.channels = Some(if channels_flag == 0 { 1 } else { 2 });
        }
    } else {
        // Non-AAC stream headers can be stale; keep channels aligned with actual media tags.
        track.channels = Some(if channels_flag == 0 { 1 } else { 2 });
    }
    track.clock_rate = track
        .sample_rate
        .unwrap_or_else(|| default_audio_clock(codec));
    track.refresh_readiness();
    session.tracks.audio = Some(track);
}

/// Derives the audio sample rate from the payload/header for codec-specific cases.
///
/// 从负载/头部推导特定编解码器的音频采样率。
fn audio_sample_rate_from_payload(codec: CodecId, rate: u8) -> Option<u32> {
    match codec {
        CodecId::Opus | CodecId::G711A | CodecId::G711U | CodecId::ADPCM => {
            default_audio_sample_rate(codec)
        }
        _ => flv_sample_rate(rate),
    }
}

/// Returns the default clock rate for an audio codec.
///
/// 返回音频编解码器的默认时钟率。
fn default_audio_clock(codec: CodecId) -> u32 {
    match codec {
        CodecId::ADPCM => 8_000,
        CodecId::G711A | CodecId::G711U => 8_000,
        CodecId::Opus => 48_000,
        CodecId::MP3 => 44_100,
        _ => 48_000,
    }
}

/// Returns the default sample rate for an audio codec, if known.
///
/// 返回音频编解码器的默认采样率（如果已知）。
fn default_audio_sample_rate(codec: CodecId) -> Option<u32> {
    match codec {
        CodecId::ADPCM => Some(8_000),
        CodecId::G711A | CodecId::G711U => Some(8_000),
        CodecId::Opus => Some(48_000),
        CodecId::MP3 => Some(44_100),
        CodecId::AAC => Some(48_000),
        _ => None,
    }
}

/// Infers the audio frame duration in milliseconds from codec and payload.
///
/// Uses codec-specific samples-per-frame and the track sample rate. PCM-like codecs
/// derive duration from payload length and channel count.
///
/// 根据编解码器与负载推断音频帧时长（毫秒）。
///
/// 使用编解码器特定的每帧采样数与轨道采样率；类 PCM 编解码器按负载长度与声道数推导。
fn infer_audio_duration_ms(codec: CodecId, track: &TrackInfo, payload: &[u8]) -> i64 {
    let sample_rate = track
        .sample_rate
        .unwrap_or_else(|| default_audio_clock(codec))
        .max(1);
    let channels = u32::from(track.channels.unwrap_or(1).max(1));
    let samples_per_frame = match codec {
        CodecId::AAC => 1024,
        CodecId::Opus => 960,
        CodecId::MP3 => infer_mp3_samples_per_frame(payload).unwrap_or(1152),
        CodecId::G711A | CodecId::G711U | CodecId::ADPCM => {
            let bytes = u32::try_from(payload.len()).unwrap_or(u32::MAX);
            (bytes / channels).max(1)
        }
        _ => 1,
    };
    let duration = ((u128::from(samples_per_frame) * 1000u128) / u128::from(sample_rate))
        .min(u128::from(i64::MAX as u64)) as i64;
    duration.max(1)
}

/// Infers the number of samples per MP3 frame from the frame header.
///
/// 从 MP3 帧头推断每帧采样数。
fn infer_mp3_samples_per_frame(payload: &[u8]) -> Option<u32> {
    if payload.len() < 4 {
        return None;
    }
    if payload[0] != 0xFF || (payload[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version_id = (payload[1] >> 3) & 0x03;
    let layer = (payload[1] >> 1) & 0x03;
    let mpeg1 = version_id == 0x03;
    let samples = match layer {
        0x03 => 384,
        0x02 => 1152,
        0x01 => {
            if mpeg1 {
                1152
            } else {
                576
            }
        }
        _ => return None,
    };
    Some(samples)
}

/// Maps an AAC sampling frequency index to the sample rate in Hz.
///
/// 将 AAC 采样频率索引映射为 Hz 采样率。
fn sample_rate_from_index(idx: u8) -> Option<u32> {
    const TABLE: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    TABLE.get(idx as usize).copied()
}

/// Maps an FLV sound rate code to the sample rate in Hz.
///
/// 将 FLV 声音速率码映射为 Hz 采样率。
fn flv_sample_rate(code: u8) -> Option<u32> {
    match code {
        0 => Some(5_512),
        1 => Some(11_025),
        2 => Some(22_050),
        3 => Some(44_100),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn parse_avcc_parameter_sets(avcc: &[u8]) -> (Vec<Bytes>, Vec<Bytes>) {
    core_parse_flv_avcc_parameter_sets(avcc)
}

/// Attaches the original RTMP video payload as opaque side data for direct proxy.
///
/// 将原始 RTMP 视频负载作为 opaque side data 附加到帧，用于直接代理。
pub(crate) fn attach_raw_rtmp_video_payload(frame: &mut AVFrame, payload: &[u8]) {
    core_attach_raw_rtmp_video_payload(frame, payload);
}

/// Attaches the original RTMP audio payload as opaque side data for direct proxy.
///
/// 将原始 RTMP 音频负载作为 opaque side data 附加到帧，用于直接代理。
pub(crate) fn attach_raw_rtmp_audio_payload(frame: &mut AVFrame, payload: &[u8]) {
    core_attach_raw_rtmp_audio_payload(frame, payload);
}

#[cfg(test)]
pub(crate) fn length_prefixed_to_annexb(payload: &[u8]) -> Bytes {
    length_prefixed_to_annexb_with_size(payload, 4)
}

/// Converts a length-prefixed NALU payload into AnnexB start-code format.
///
/// 将长度前缀的 NALU 负载转换为 AnnexB 起始码格式。
pub(crate) fn length_prefixed_to_annexb_with_size(payload: &[u8], nal_length_size: usize) -> Bytes {
    core_length_prefixed_to_annexb_with_size(payload, nal_length_size)
}

#[cfg(test)]
pub(crate) fn annexb_to_length_prefixed(payload: &[u8]) -> Bytes {
    annexb_to_length_prefixed_with_size(payload, 4)
}

#[cfg(test)]
pub(crate) fn annexb_to_length_prefixed_with_size(payload: &[u8], nal_length_size: usize) -> Bytes {
    let nal_length_size = normalize_nal_length_size(nal_length_size);
    let units = split_annexb_units(payload);
    let mut out = BytesMut::with_capacity(payload.len() + 16);
    for unit in units {
        match nal_length_size {
            1 => {
                let Ok(len) = u8::try_from(unit.len()) else {
                    continue;
                };
                out.extend_from_slice(&[len]);
            }
            2 => {
                let Ok(len) = u16::try_from(unit.len()) else {
                    continue;
                };
                out.extend_from_slice(&len.to_be_bytes());
            }
            _ => {
                if unit.len() > u32::MAX as usize {
                    continue;
                }
                out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
            }
        }
        out.extend_from_slice(unit);
    }
    if out.is_empty() {
        Bytes::copy_from_slice(payload)
    } else {
        out.freeze()
    }
}

#[cfg(test)]
/// Splits an AnnexB payload into individual NAL units.
///
/// 将 AnnexB 负载拆分为独立 NAL 单元。
fn split_annexb_units(payload: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut data = payload;

    while let Some((start, code_len)) = find_start_code(data) {
        data = &data[start + code_len..];
        let next_start = find_start_code(data)
            .map(|(idx, _)| idx)
            .unwrap_or(data.len());
        if next_start > 0 {
            out.push(&data[..next_start]);
        }
        data = &data[next_start..];
    }

    out
}

#[cfg(test)]
/// Finds the next H.264/H.265-style start code (`0x000001` or `0x00000001`).
///
/// Returns the start index and the length of the start code (3 or 4 bytes).
///
/// 查找下一个 H.264/H.265 风格起始码（`0x000001` 或 `0x00000001`）。
///
/// 返回起始索引与起始码长度（3 或 4 字节）。
fn find_start_code(data: &[u8]) -> Option<(usize, usize)> {
    if data.len() < 3 {
        return None;
    }

    for i in 0..(data.len() - 2) {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
    }

    None
}
