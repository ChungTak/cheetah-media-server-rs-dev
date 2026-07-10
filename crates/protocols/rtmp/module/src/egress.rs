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

/// `frame_dts_to_rtmp_timestamp_ms` function.
/// `frame_dts_to_rtmp_timestamp_ms` 函数.
pub(crate) fn frame_dts_to_rtmp_timestamp_ms(frame: &AVFrame) -> u32 {
    core_frame_dts_to_rtmp_timestamp_ms(frame)
}

fn map_play_mode(mode: RtmpPlayMode) -> RtmpFlvPlayMode {
    match mode {
        RtmpPlayMode::Enhanced => RtmpFlvPlayMode::Enhanced,
        RtmpPlayMode::Normal | RtmpPlayMode::FastPts => RtmpFlvPlayMode::Normal,
    }
}

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

/// `map_frame_to_rtmp_with_tracks` function.
/// `map_frame_to_rtmp_with_tracks` 函数.
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

/// `map_non_h264_video` function.
/// `map_non_h264_video` 函数.
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

/// `send_track_bootstrap` function.
/// `send_track_bootstrap` 函数.
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

/// Builds `track_bootstrap_commands` output.
/// 构建 `track_bootstrap_commands` 输出.
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

/// Builds `video_config_payload` output.
/// 构建 `video_config_payload` 输出.
#[cfg(test)]
pub(crate) fn build_video_config_payload(
    codec: CodecId,
    config: &[u8],
    mode: RtmpPlayMode,
) -> Option<Bytes> {
    core_build_video_config_payload(codec, config, map_play_mode(mode))
}

/// `use_enhanced_video_mode` function.
/// `use_enhanced_video_mode` 函数.
#[cfg(test)]
pub(crate) fn use_enhanced_video_mode(mode: RtmpPlayMode, codec: CodecId) -> bool {
    core_use_enhanced_video_mode(map_play_mode(mode), codec)
}

/// Builds `h266_config` output.
/// 构建 `h266_config` 输出.
#[cfg(test)]
pub(crate) fn build_h266_config(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    core_build_h266_config(vps, sps, pps)
}

/// `track_list_has_audio` function.
/// `track_list_has_audio` 函数.
pub(crate) fn track_list_has_audio(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Audio)
}

/// `track_list_has_video` function.
/// `track_list_has_video` 函数.
pub(crate) fn track_list_has_video(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Video)
}

/// `track_list_has_supported_playback_codec` function.
/// `track_list_has_supported_playback_codec` 函数.
pub(crate) fn track_list_has_supported_playback_codec(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| rtmp_playback_codec_supported(track.media_kind, track.codec))
}

/// `rtmp_playback_codec_supported` function.
/// `rtmp_playback_codec_supported` 函数.
pub(crate) fn rtmp_playback_codec_supported(media_kind: MediaKind, codec: CodecId) -> bool {
    core_rtmp_playback_codec_supported(media_kind, codec)
}

/// `track_list_has_codec` function.
/// `track_list_has_codec` 函数.
pub(crate) fn track_list_has_codec(tracks: &[TrackInfo], codec: CodecId) -> bool {
    tracks.iter().any(|track| track.codec == codec)
}

/// `should_delay_publish_release_for_h264` function.
/// `should_delay_publish_release_for_h264` 函数.
pub(crate) fn should_delay_publish_release_for_h264(session: &PublishSession) -> bool {
    session
        .tracks
        .video
        .as_ref()
        .is_some_and(|track| track.codec == CodecId::H264)
}

/// `play_accept_flags` function.
/// `play_accept_flags` 函数.
pub(crate) fn play_accept_flags(tracks: &[TrackInfo]) -> (bool, bool) {
    let enabled = track_list_has_video(tracks) || track_list_has_audio(tracks);
    (enabled, enabled)
}

/// `should_force_close_play_on_source_end` function.
/// `should_force_close_play_on_source_end` 函数.
pub(crate) fn should_force_close_play_on_source_end(tracks: &[TrackInfo]) -> bool {
    track_list_has_codec(tracks, CodecId::H265)
}

/// `maybe_make_mute_audio` function.
/// `maybe_make_mute_audio` 函数.
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

fn mute_aac_frame_payload() -> Bytes {
    core_mute_aac_frame_payload()
}

/// Builds `metadata` output.
/// 构建 `metadata` 输出.
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
