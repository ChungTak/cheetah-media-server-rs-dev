use std::sync::Arc;

use bytes::Bytes;
use cheetah_codec::{AVFrame, CodecId, MediaKind, TrackInfo};
#[cfg(test)]
use cheetah_rtmp_core::{
    build_h266_config as core_build_h266_config, build_metadata as core_build_metadata,
    build_video_config_payload as core_build_video_config_payload,
    use_enhanced_video_mode as core_use_enhanced_video_mode,
};
use cheetah_rtmp_core::{
    build_track_bootstrap_payloads as core_build_track_bootstrap_payloads,
    frame_dts_to_rtmp_timestamp_ms as core_frame_dts_to_rtmp_timestamp_ms,
    map_frame_to_rtmp_flv_payload as core_map_frame_to_rtmp_flv_payload,
    mute_aac_frame_payload as core_mute_aac_frame_payload,
    rtmp_playback_codec_supported as core_rtmp_playback_codec_supported, RtmpFlvPayloadKind,
    RtmpFlvPlayMode,
};
use cheetah_rtmp_driver_tokio::{
    DriverSendError, RtmpConnectionId, RtmpCoreCommand, RtmpCoreCommandSender,
};

use crate::ingest::{RTMP_AUDIO_RAW_SIDEDATA_MAGIC, RTMP_VIDEO_RAW_SIDEDATA_MAGIC};
use crate::route::RtmpPlayMode;
use crate::session::PublishSession;

/// Extracts the original RTMP video payload from a frame's opaque side data.
///
/// Used in direct-proxy mode to bypass re-encoding when forwarding RTMP-to-RTMP.
///
/// 从帧的 opaque side data 中提取原始 RTMP 视频负载。
///
/// 用于直接代理模式，在 RTMP 到 RTMP 转发时跳过重新编码。
fn extract_raw_rtmp_video_payload(frame: &AVFrame) -> Option<Bytes> {
    frame.side_data.iter().find_map(|entry| {
        let cheetah_codec::FrameSideData::Opaque(raw) = entry else {
            return None;
        };
        if raw.starts_with(RTMP_VIDEO_RAW_SIDEDATA_MAGIC) {
            Some(raw.slice(RTMP_VIDEO_RAW_SIDEDATA_MAGIC.len()..))
        } else {
            None
        }
    })
}

/// Extracts the original RTMP audio payload from a frame's opaque side data.
///
/// Used in direct-proxy mode to bypass re-encoding when forwarding RTMP-to-RTMP.
///
/// 从帧的 opaque side data 中提取原始 RTMP 音频负载。
///
/// 用于直接代理模式，在 RTMP 到 RTMP 转发时跳过重新编码。
fn extract_raw_rtmp_audio_payload(frame: &AVFrame) -> Option<Bytes> {
    frame.side_data.iter().find_map(|entry| {
        let cheetah_codec::FrameSideData::Opaque(raw) = entry else {
            return None;
        };
        if raw.starts_with(RTMP_AUDIO_RAW_SIDEDATA_MAGIC) {
            Some(raw.slice(RTMP_AUDIO_RAW_SIDEDATA_MAGIC.len()..))
        } else {
            None
        }
    })
}

/// Converts an internal frame's DTS to the RTMP 32-bit millisecond timestamp.
///
/// 将内部帧的 DTS 转换为 RTMP 32 位毫秒时间戳。
pub(crate) fn frame_dts_to_rtmp_timestamp_ms(frame: &AVFrame) -> u32 {
    core_frame_dts_to_rtmp_timestamp_ms(frame)
}

/// Maps the module-level `RtmpPlayMode` to the core's `RtmpFlvPlayMode`.
///
/// `FastPts` is mapped to `Normal` because the core handles fast-PTS by choosing
/// the timestamp source separately.
///
/// 将模块层 `RtmpPlayMode` 映射为核心层 `RtmpFlvPlayMode`。
///
/// `FastPts` 映射为 `Normal`，因为核心层会单独选择时间戳来源。
fn map_play_mode(mode: RtmpPlayMode) -> RtmpFlvPlayMode {
    match mode {
        RtmpPlayMode::Enhanced => RtmpFlvPlayMode::Enhanced,
        RtmpPlayMode::Normal | RtmpPlayMode::FastPts => RtmpFlvPlayMode::Normal,
    }
}

/// Converts a core FLV payload into a driver-level `RtmpCoreCommand`.
///
/// Dispatches by payload kind to `SendAudio`, `SendVideo`, or `SendMetadata`.
///
/// 将核心 FLV 负载转换为驱动层 `RtmpCoreCommand`。
///
/// 按负载类型分发为 `SendAudio`、`SendVideo` 或 `SendMetadata`。
fn payload_to_core_command(
    stream_id: u32,
    payload: cheetah_rtmp_core::RtmpFlvPayload,
) -> RtmpCoreCommand {
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

/// Maps an internal `AVFrame` to an RTMP command for a specific stream.
///
/// If the frame carries raw RTMP side data (direct-proxy mode), that payload is
/// sent directly; otherwise the core FLV payload mapper is used.
///
/// 将内部 `AVFrame` 映射为指定流的 RTMP 命令。
///
/// 若帧携带 raw RTMP side data（直接代理模式），直接发送该负载；否则调用核心 FLV 映射。
pub(crate) fn map_frame_to_rtmp_with_tracks(
    stream_id: u32,
    frame: Arc<AVFrame>,
    mode: RtmpPlayMode,
    tracks: &[TrackInfo],
) -> Option<RtmpCoreCommand> {
    if frame.media_kind == MediaKind::Video {
        if let Some(raw_payload) = extract_raw_rtmp_video_payload(&frame) {
            return Some(RtmpCoreCommand::SendVideo {
                stream_id,
                timestamp_ms: frame_dts_to_rtmp_timestamp_ms(&frame),
                payload: raw_payload,
            });
        }
    }
    if frame.media_kind == MediaKind::Audio {
        if let Some(raw_payload) = extract_raw_rtmp_audio_payload(&frame) {
            return Some(RtmpCoreCommand::SendAudio {
                stream_id,
                timestamp_ms: frame_dts_to_rtmp_timestamp_ms(&frame),
                payload: raw_payload,
            });
        }
    }

    let payload = core_map_frame_to_rtmp_flv_payload(&frame, map_play_mode(mode), tracks)?;
    Some(payload_to_core_command(stream_id, payload))
}

#[cfg(test)]
pub(crate) fn map_non_h264_video(
    stream_id: u32,
    frame: &AVFrame,
    mode: RtmpPlayMode,
) -> Option<RtmpCoreCommand> {
    if let Some(raw_payload) = extract_raw_rtmp_video_payload(frame) {
        return Some(RtmpCoreCommand::SendVideo {
            stream_id,
            timestamp_ms: frame_dts_to_rtmp_timestamp_ms(frame),
            payload: raw_payload,
        });
    }
    let payload = core_map_frame_to_rtmp_flv_payload(frame, map_play_mode(mode), &[])?;
    if payload.kind != RtmpFlvPayloadKind::Video {
        return None;
    }
    Some(payload_to_core_command(stream_id, payload))
}

/// Sends bootstrap sequence headers and metadata for a play stream.
///
/// Bootstrap commands include video/audio sequence headers and, optionally,
/// `onMetaData` and a silent AAC audio packet.
///
/// 向播放流发送引导序列头与元数据。
///
/// 引导命令包含视频/音频序列头，以及可选的 `onMetaData` 与静音 AAC 音频包。
pub(crate) async fn send_track_bootstrap(
    connection_id: RtmpConnectionId,
    stream_id: u32,
    tracks: &[TrackInfo],
    mode: RtmpPlayMode,
    enable_add_mute: bool,
    emit_play_metadata: bool,
    command_tx: &RtmpCoreCommandSender,
) -> Result<(), DriverSendError> {
    let commands = build_track_bootstrap_commands(
        stream_id,
        tracks,
        mode,
        enable_add_mute,
        emit_play_metadata,
    );
    for command in commands {
        command_tx.send_core(connection_id, command).await?;
    }
    Ok(())
}

/// Builds the list of RTMP bootstrap commands for the given tracks and play mode.
///
/// 为给定轨道与播放模式构建 RTMP 引导命令列表。
pub(crate) fn build_track_bootstrap_commands(
    stream_id: u32,
    tracks: &[TrackInfo],
    mode: RtmpPlayMode,
    enable_add_mute: bool,
    emit_play_metadata: bool,
) -> Vec<RtmpCoreCommand> {
    core_build_track_bootstrap_payloads(
        tracks,
        map_play_mode(mode),
        enable_add_mute,
        emit_play_metadata,
    )
    .into_iter()
    .map(|payload| payload_to_core_command(stream_id, payload))
    .collect()
}

#[cfg(test)]
pub(crate) fn build_video_config_payload(
    codec: CodecId,
    config: &[u8],
    mode: RtmpPlayMode,
) -> Option<Bytes> {
    core_build_video_config_payload(codec, config, map_play_mode(mode))
}

#[cfg(test)]
pub(crate) fn use_enhanced_video_mode(mode: RtmpPlayMode, codec: CodecId) -> bool {
    core_use_enhanced_video_mode(map_play_mode(mode), codec)
}

#[cfg(test)]
pub(crate) fn build_h266_config(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    core_build_h266_config(vps, sps, pps)
}

/// Returns true if the track list contains an audio track.
///
/// 判断轨道列表中是否包含音频轨道。
pub(crate) fn track_list_has_audio(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Audio)
}

/// Returns true if the track list contains a video track.
///
/// 判断轨道列表中是否包含视频轨道。
pub(crate) fn track_list_has_video(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Video)
}

/// Returns true if the track list contains at least one RTMP/FLV-playable codec.
///
/// 判断轨道列表中是否至少包含一种 RTMP/FLV 可播放的编解码器。
pub(crate) fn track_list_has_supported_playback_codec(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| rtmp_playback_codec_supported(track.media_kind, track.codec))
}

/// Checks whether a media kind/codec pair can be played back over RTMP/FLV.
///
/// 检查媒体类型/编解码器组合是否可通过 RTMP/FLV 播放。
pub(crate) fn rtmp_playback_codec_supported(media_kind: MediaKind, codec: CodecId) -> bool {
    core_rtmp_playback_codec_supported(media_kind, codec)
}

/// Returns true if the track list contains the given codec.
///
/// 判断轨道列表中是否包含指定编解码器。
pub(crate) fn track_list_has_codec(tracks: &[TrackInfo], codec: CodecId) -> bool {
    tracks.iter().any(|track| track.codec == codec)
}

/// Returns true if the publish session should be released with a short delay.
///
/// H264 publishers are delayed briefly so that pending play requests can discover
/// the stream before the lease is released.
///
/// 判断是否需要延迟释放发布会话。
///
/// H264 发布者短暂延迟释放，使待起播请求能在租约释放前发现该流。
pub(crate) fn should_delay_publish_release_for_h264(session: &PublishSession) -> bool {
    session
        .tracks
        .video
        .as_ref()
        .is_some_and(|track| track.codec == CodecId::H264)
}

/// Returns `(emit_play_status, emit_sample_access)` for a play accept response.
///
/// Both flags are enabled when at least one track exists.
///
/// 返回播放接受响应的 `(emit_play_status, emit_sample_access)`。
///
/// 当至少存在一条轨道时两者均启用。
pub(crate) fn play_accept_flags(tracks: &[TrackInfo]) -> (bool, bool) {
    let enabled = track_list_has_video(tracks) || track_list_has_audio(tracks);
    (enabled, enabled)
}

/// Returns true if a play session should be closed immediately when the source ends.
///
/// HEVC streams may carry stale backlog after the source is gone; closing right away
/// avoids playing stale frames.
///
/// 判断播放会话是否应在源流结束时立即关闭。
///
/// HEVC 流在源断开后可能残留过期积压，立即关闭可避免播放过期帧。
pub(crate) fn should_force_close_play_on_source_end(tracks: &[TrackInfo]) -> bool {
    track_list_has_codec(tracks, CodecId::H265)
}

/// Generates a silent AAC audio packet at the given timestamp.
///
/// Rate-limited to one packet per 128 ms so players keep the audio pipeline alive
/// when the source has no audio track.
///
/// 在给定时间戳生成一个静音 AAC 音频包。
///
/// 限速为每 128 ms 一个包，用于源没有音频轨道时保持播放器音频管线活跃。
pub(crate) fn maybe_make_mute_audio(
    stream_id: u32,
    timestamp_ms: u32,
    last_mute_ts: &mut Option<u32>,
) -> Option<RtmpCoreCommand> {
    if let Some(previous) = *last_mute_ts {
        if timestamp_ms.saturating_sub(previous) < 128 {
            return None;
        }
    }
    *last_mute_ts = Some(timestamp_ms);
    Some(RtmpCoreCommand::SendAudio {
        stream_id,
        timestamp_ms,
        payload: mute_aac_frame_payload(),
    })
}

/// Returns the payload bytes for a silent AAC audio frame.
///
/// 返回静音 AAC 音频帧的负载字节。
fn mute_aac_frame_payload() -> Bytes {
    core_mute_aac_frame_payload()
}

#[cfg(test)]
pub(crate) fn build_metadata(tracks: &[TrackInfo]) -> Bytes {
    core_build_metadata(tracks)
}

#[cfg(test)]
mod tests {
    use cheetah_codec::{rtmp_fourcc_from_codec, FrameFlags, FrameFormat, Timebase, TrackId};
    use cheetah_rtmp_core::{decode_all, Amf0Value};

    use super::*;

    #[test]
    fn av1_enhanced_video_frame_does_not_insert_composition_time_prefix() {
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::AV1,
            FrameFormat::CanonicalAv1Obu,
            3_000,
            0,
            Timebase::new(1, 90_000),
            Bytes::from_static(&[0x0a, 0x0e, 0x4a]),
        );
        frame.flags.insert(FrameFlags::KEY);

        let command = map_non_h264_video(1, &frame, RtmpPlayMode::Normal).expect("av1 command");
        let RtmpCoreCommand::SendVideo { payload, .. } = command else {
            panic!("expected video command");
        };
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1)
            .expect("av1 fourcc")
            .to_be_bytes();

        assert_eq!(payload[0], 0x91);
        assert_eq!(&payload[1..5], &fourcc);
        assert_eq!(
            &payload[5..],
            &[0x0a, 0x0e, 0x4a],
            "AV1 enhanced RTMP coded frame must start with AV1 OBU bytes, not CTS"
        );
    }

    #[test]
    fn metadata_uses_enhanced_video_fourcc_for_av1() {
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);

        let metadata = build_metadata(&[track]);
        let values = decode_all(&metadata).expect("decode metadata");
        let Some(Amf0Value::EcmaArray { entries }) = values.get(1) else {
            panic!("metadata must contain an ECMA array");
        };

        let video_codec_id = entries
            .iter()
            .find(|entry| entry.key == "videocodecid")
            .and_then(|entry| entry.value.as_f64())
            .expect("videocodecid");
        let av1_fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc") as f64;

        assert_eq!(
            video_codec_id, av1_fourcc,
            "enhanced FLV metadata should advertise AV1 as the av01 fourcc"
        );
    }
}
