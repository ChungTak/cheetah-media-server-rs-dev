//! FLV egress mapping: AVFrame -> FLV audio/video/script payloads.
//!
//! This is the shared FLV encapsulation layer consumed by RTMP and HTTP-FLV.
//! Script-data (`onMetaData`) is serialized with a minimal, self-contained
//! AMF0 encoder so the FLV container layer carries no protocol dependency.
//!
//! FLV 出口映射：AVFrame -> FLV 音频/视频/脚本负载。
//!
//! 这是 RTMP 与 HTTP-FLV 共享的 FLV 封装层。脚本数据（`onMetaData`）
//! 使用最小自包含的 AMF0 编码器序列化，使 FLV 容器层不依赖具体协议。

use crate::prelude::*;

use bytes::{Bytes, BytesMut};

use crate::{
    build_audio_sequence_header, build_video_sequence_header, frame_composition_time_ms,
    frame_dts_to_rtmp_timestamp_ms, h26x_length_prefixed_from_payload, rtmp_audio_flag,
    rtmp_codec_id_from_codec, rtmp_fourcc_from_codec, AVFrame, CodecExtradata, CodecId, MediaKind,
    Rational32, TrackInfo, RTMP_AUDIO_CODEC_ID_AAC,
};

const RTMP_AUDIO_FOURCC_OPUS: &[u8; 4] = b"Opus";
const ENHANCED_RTMP_AUDIO_SEQUENCE_START: u8 = 0x90;
const ENHANCED_RTMP_AUDIO_CODED_FRAMES: u8 = 0x91;
const OPUS_DEFAULT_PRE_SKIP: u16 = 312;

/// RTMP/FLV playback mode selector.
///
/// `Enhanced` uses the new FLV video message spec (FourCC + packet type) for
/// H.265/AV1/VP8/VP9, while `Normal` uses classic codec-id framing.
///
/// RTMP/FLV 播放模式选择器。
///
/// `Enhanced` 使用新的 FLV 视频消息规范（FourCC + 包类型）承载 H.265/AV1/VP8/VP9，
/// `Normal` 使用经典 codec-id 帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpFlvPlayMode {
    Normal,
    Enhanced,
}

/// Kind of an RTMP/FLV payload produced by the egress mapper.
///
/// 出口映射器产生的 RTMP/FLV 负载类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpFlvPayloadKind {
    Audio,
    Video,
    Data,
}

/// A fully framed RTMP/FLV payload ready for the wire.
///
/// `kind` selects the tag type, `timestamp_ms` is the millisecond timestamp,
/// and `payload` is the raw tag body.
///
/// 已完全成帧、可直接发送的 RTMP/FLV 负载。
///
/// `kind` 选择标签类型，`timestamp_ms` 为毫秒时间戳，`payload` 为原始标签体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpFlvPayload {
    pub kind: RtmpFlvPayloadKind,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

/// Build the initial sequence of payloads required to start RTMP/FLV playback.
///
/// Optionally emits an `onMetaData` script tag, then codec-specific sequence
/// headers for every track, and finally a mute AAC config when the track list
/// has no audio and the caller requests it.
///
/// 构建启动 RTMP/FLV 播放所需的初始负载序列。
///
/// 可选输出 `onMetaData` 脚本标签，随后为每个轨道输出编解码器特定序列头，
/// 若轨道列表无音频且调用方要求，则追加静音 AAC 配置。
pub fn build_track_bootstrap_payloads(
    tracks: &[TrackInfo],
    mode: RtmpFlvPlayMode,
    enable_add_mute: bool,
    emit_play_metadata: bool,
) -> Vec<RtmpFlvPayload> {
    let mut payloads = Vec::new();
    if emit_play_metadata {
        payloads.push(RtmpFlvPayload {
            kind: RtmpFlvPayloadKind::Data,
            timestamp_ms: 0,
            payload: build_metadata(tracks),
        });
    }
    for track in tracks {
        match (&track.codec, &track.extradata) {
            (CodecId::H264, CodecExtradata::H264 { .. }) => {
                if let Some(tag) = build_video_sequence_header(track) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload: tag.payload,
                    });
                }
            }
            (
                CodecId::H265,
                CodecExtradata::H265 {
                    vps,
                    sps,
                    pps,
                    hvcc,
                },
            ) => {
                let fallback_hvcc;
                let hvcc = if let Some(hvcc) = hvcc.as_ref() {
                    hvcc.as_ref()
                } else {
                    fallback_hvcc = build_h265_config(vps, sps, pps);
                    if fallback_hvcc.is_empty() {
                        continue;
                    }
                    fallback_hvcc.as_ref()
                };
                if let Some(payload) = build_video_config_payload(CodecId::H265, hvcc, mode) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload,
                    });
                }
            }
            (CodecId::H266, CodecExtradata::H266 { vps, sps, pps }) => {
                let hvcc = build_h266_config(vps, sps, pps);
                if let Some(payload) = build_video_config_payload(CodecId::H266, &hvcc, mode) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload,
                    });
                }
            }
            (
                CodecId::AV1,
                CodecExtradata::AV1 {
                    codec_config: Some(config),
                    ..
                },
            ) => {
                if let Some(payload) = build_video_config_payload(CodecId::AV1, config, mode) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload,
                    });
                }
            }
            (
                CodecId::VP9,
                CodecExtradata::VP9 {
                    config: Some(config),
                },
            ) => {
                if let Some(payload) = build_video_config_payload(CodecId::VP9, config, mode) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload,
                    });
                }
            }
            (
                CodecId::VP8,
                CodecExtradata::VP8 {
                    config: Some(config),
                },
            ) => {
                if let Some(payload) = build_video_config_payload(CodecId::VP8, config, mode) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: 0,
                        payload,
                    });
                }
            }
            (CodecId::AAC, CodecExtradata::AAC { .. }) => {
                if let Some(tag) = build_audio_sequence_header(track) {
                    payloads.push(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Audio,
                        timestamp_ms: 0,
                        payload: tag.payload,
                    });
                }
            }
            (CodecId::Opus, _) => {
                payloads.push(RtmpFlvPayload {
                    kind: RtmpFlvPayloadKind::Audio,
                    timestamp_ms: 0,
                    payload: build_opus_sequence_header(track),
                });
            }
            _ => {}
        }
    }

    if enable_add_mute && !track_list_has_audio(tracks) {
        payloads.push(RtmpFlvPayload {
            kind: RtmpFlvPayloadKind::Audio,
            timestamp_ms: 0,
            payload: mute_aac_config_payload(),
        });
    }

    payloads
}

/// Map a canonical `AVFrame` to an RTMP/FLV payload.
///
/// Rejects unsupported codec/kind combinations, normalizes the timestamp to
/// milliseconds, and wraps the payload in the correct FLV video/audio tag
/// for the requested playback mode.
///
/// 将标准 `AVFrame` 映射为 RTMP/FLV 负载。
///
/// 拒绝不支持的编解码器/类型组合，将时间戳归一化为毫秒，
/// 并按请求播放模式将负载封装到正确的 FLV 视频/音频标签中。
pub fn map_frame_to_rtmp_flv_payload(
    frame: &AVFrame,
    mode: RtmpFlvPlayMode,
    tracks: &[TrackInfo],
) -> Option<RtmpFlvPayload> {
    if !rtmp_playback_codec_supported(frame.media_kind, frame.codec) {
        return None;
    }
    let track = find_track_for_frame(frame, tracks);
    let dts_ms = frame_dts_to_rtmp_timestamp_ms(frame);
    match (frame.media_kind, frame.codec) {
        (MediaKind::Video, CodecId::H264) => {
            let payload = h26x_payload_for_track(frame, track);
            let (timestamp_ms, composition_time) = normalize_video_timestamp_for_legacy_playback(
                dts_ms,
                frame_composition_time_ms(frame),
                mode,
            );
            let first = if frame.is_key_frame() { 0x17 } else { 0x27 };
            let mut out = BytesMut::with_capacity(5 + payload.len());
            out.extend_from_slice(&[
                first,
                0x01,
                ((composition_time >> 16) & 0xff) as u8,
                ((composition_time >> 8) & 0xff) as u8,
                (composition_time & 0xff) as u8,
            ]);
            out.extend_from_slice(&payload);
            Some(RtmpFlvPayload {
                kind: RtmpFlvPayloadKind::Video,
                timestamp_ms,
                payload: out.freeze(),
            })
        }
        (MediaKind::Video, CodecId::H265)
        | (MediaKind::Video, CodecId::H266)
        | (MediaKind::Video, CodecId::AV1)
        | (MediaKind::Video, CodecId::VP8)
        | (MediaKind::Video, CodecId::VP9) => map_non_h264_video_with_track(frame, mode, track),
        (MediaKind::Audio, CodecId::AAC) => {
            let mut out = BytesMut::with_capacity(2 + frame.payload.len());
            out.extend_from_slice(&[0xaf, 0x01]);
            out.extend_from_slice(&frame.payload);
            Some(RtmpFlvPayload {
                kind: RtmpFlvPayloadKind::Audio,
                timestamp_ms: dts_ms,
                payload: out.freeze(),
            })
        }
        (MediaKind::Audio, CodecId::Opus) => {
            let payload =
                build_enhanced_opus_audio_payload(ENHANCED_RTMP_AUDIO_CODED_FRAMES, &frame.payload);
            Some(RtmpFlvPayload {
                kind: RtmpFlvPayloadKind::Audio,
                timestamp_ms: dts_ms,
                payload,
            })
        }
        (MediaKind::Audio, CodecId::G711A)
        | (MediaKind::Audio, CodecId::ADPCM)
        | (MediaKind::Audio, CodecId::G711U)
        | (MediaKind::Audio, CodecId::MP3) => {
            let payload = build_non_aac_audio_payload(frame.codec, &frame.payload, track)?;
            Some(RtmpFlvPayload {
                kind: RtmpFlvPayloadKind::Audio,
                timestamp_ms: dts_ms,
                payload,
            })
        }
        _ => None,
    }
}

/// Build a video sequence-header payload for `codec` and `config`.
///
/// Uses enhanced FourCC framing when `use_enhanced_video_mode` returns true,
/// otherwise falls back to legacy codec-id framing.
///
/// 为 `codec` 与 `config` 构建视频序列头负载。
///
/// 当 `use_enhanced_video_mode` 返回 true 时使用增强 FourCC 帧，
/// 否则回退到传统 codec-id 帧。
pub fn build_video_config_payload(
    codec: CodecId,
    config: &[u8],
    mode: RtmpFlvPlayMode,
) -> Option<Bytes> {
    if use_enhanced_video_mode(mode, codec) {
        let fourcc = rtmp_fourcc_from_codec(codec)?;
        let mut payload = BytesMut::with_capacity(5 + config.len());
        payload.extend_from_slice(&[0x90]);
        payload.extend_from_slice(&fourcc.to_be_bytes());
        payload.extend_from_slice(config);
        return Some(payload.freeze());
    }

    let codec_id = rtmp_codec_id_from_codec(codec)?;
    let mut payload = BytesMut::with_capacity(5 + config.len());
    payload.extend_from_slice(&[(0x10 | codec_id), 0x00, 0x00, 0x00, 0x00]);
    payload.extend_from_slice(config);
    Some(payload.freeze())
}

/// Determine whether enhanced FLV video framing should be used.
///
/// 判断是否应使用增强 FLV 视频帧。
pub fn use_enhanced_video_mode(mode: RtmpFlvPlayMode, codec: CodecId) -> bool {
    mode == RtmpFlvPlayMode::Enhanced
        || matches!(
            codec,
            CodecId::H265 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9
        )
}

/// Build a VVC (H.266) decoder configuration from parameter sets.
///
/// Emits a compact vvcc-style array with NAL unit type and count headers.
/// Returns an empty `Bytes` if no parameter sets are present.
///
/// 从参数集构建 VVC（H.266）解码器配置。
///
/// 输出紧凑的 vvcc 风格数组，包含 NAL 单元类型与计数头。
/// 若不存在参数集则返回空 `Bytes`。
pub fn build_h266_config(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    let mut array_count = 0u8;
    if !vps.is_empty() {
        array_count += 1;
    }
    if !sps.is_empty() {
        array_count += 1;
    }
    if !pps.is_empty() {
        array_count += 1;
    }
    if array_count == 0 {
        return Bytes::new();
    }

    let mut out = BytesMut::with_capacity(2 + vps.len() + sps.len() + pps.len());
    out.extend_from_slice(&[(0b1_1111 << 3) | (0b11 << 1), array_count]);
    append_h266_vvcc_array(&mut out, 14, vps);
    append_h266_vvcc_array(&mut out, 15, sps);
    append_h266_vvcc_array(&mut out, 16, pps);
    out.freeze()
}

/// Build an HEVC (H.265) decoder configuration record (hvcc) from parameter sets.
///
/// Extracts profile/level/constraint information from the first SPS and
/// packages VPS/SPS/PPS into the NAL array format used by `hvcC`.
///
/// 从参数集构建 HEVC（H.265）解码器配置记录（hvcc）。
///
/// 从首个 SPS 提取 profile/level/constraint 信息，并将 VPS/SPS/PPS
/// 打包为 `hvcC` 使用的 NAL 数组格式。
pub fn build_h265_config(vps: &[Bytes], sps: &[Bytes], pps: &[Bytes]) -> Bytes {
    let mut array_count = 0u8;
    if !vps.is_empty() {
        array_count += 1;
    }
    if !sps.is_empty() {
        array_count += 1;
    }
    if !pps.is_empty() {
        array_count += 1;
    }
    if array_count == 0 {
        return Bytes::new();
    }

    let mut out = BytesMut::new();
    out.extend_from_slice(&[
        1,
        0x01,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0xF0,
        0x00,
        0xFC,
        0xFC,
        0xF8,
        0xF8,
        0,
        0,
        0x0F,
        array_count,
    ]);
    append_h265_hvcc_array(&mut out, 32, vps);
    append_h265_hvcc_array(&mut out, 33, sps);
    append_h265_hvcc_array(&mut out, 34, pps);
    out.freeze()
}

/// Return true if any track in the list is an audio track.
///
/// 若列表中存在音频轨道则返回 true。
pub fn track_list_has_audio(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Audio)
}

/// Check whether the codec can be played back through RTMP/FLV paths.
///
/// 检查该编解码器是否可以通过 RTMP/FLV 路径播放。
pub fn rtmp_playback_codec_supported(media_kind: MediaKind, codec: CodecId) -> bool {
    matches!(
        (media_kind, codec),
        (
            MediaKind::Video,
            CodecId::H264
                | CodecId::H265
                | CodecId::H266
                | CodecId::AV1
                | CodecId::VP8
                | CodecId::VP9
        ) | (
            MediaKind::Audio,
            CodecId::AAC
                | CodecId::MP3
                | CodecId::G711A
                | CodecId::G711U
                | CodecId::ADPCM
                | CodecId::Opus,
        )
    )
}

/// Build the AAC AudioSpecificConfig payload for a mute audio track.
///
/// 构建静音音频轨道的 AAC AudioSpecificConfig 负载。
pub fn mute_aac_config_payload() -> Bytes {
    let asc = [0x12, 0x10];
    let mut payload = BytesMut::with_capacity(2 + asc.len());
    payload.extend_from_slice(&[0xaf, 0x00]);
    payload.extend_from_slice(&asc);
    payload.freeze()
}

/// Build a single mute AAC audio frame payload.
///
/// 构建单个静音 AAC 音频帧负载。
pub fn mute_aac_frame_payload() -> Bytes {
    let mut payload = BytesMut::with_capacity(3);
    payload.extend_from_slice(&[0xaf, 0x01, 0x00]);
    payload.freeze()
}

/// Build an AMF0 `onMetaData` script payload from the track list.
///
/// Fills video width/height/framerate and audio sample-rate/stereo/codec
/// fields as ECMA array entries, matching the shape expected by generic
/// FLV/RTMP players.
///
/// 根据轨道列表构建 AMF0 `onMetaData` 脚本负载。
///
/// 将视频宽度/高度/帧率以及音频采样率/立体声/编解码器字段
/// 填充为 ECMA 数组条目，与普通 FLV/RTMP 播放器期望的形状一致。
pub fn build_metadata(tracks: &[TrackInfo]) -> Bytes {
    let mut entries: Vec<(&str, Amf0Meta)> = Vec::new();
    entries.push(("duration", Amf0Meta::Number(0.0)));
    entries.push(("fileSize", Amf0Meta::Number(0.0)));

    for track in tracks {
        match track.media_kind {
            MediaKind::Video => {
                entries.push(("videodatarate", Amf0Meta::Number(5000.0)));
                if let Some(width) = track.width {
                    entries.push(("width", Amf0Meta::Number(width as f64)));
                }
                if let Some(height) = track.height {
                    entries.push(("height", Amf0Meta::Number(height as f64)));
                }
                if let Some(Rational32 { num, den }) = track.fps {
                    if num > 0 && den > 0 {
                        entries.push(("framerate", Amf0Meta::Number(num as f64 / den as f64)));
                    }
                }
                if let Some(codec_id) = rtmp_metadata_video_codec_id(track.codec) {
                    entries.push(("videocodecid", Amf0Meta::Number(codec_id as f64)));
                }
            }
            MediaKind::Audio => {
                entries.push(("audiodatarate", Amf0Meta::Number(160.0)));
                if let Some(sample_rate) = track.sample_rate {
                    entries.push(("audiosamplerate", Amf0Meta::Number(sample_rate as f64)));
                }
                if let Some(channels) = track.channels {
                    entries.push(("stereo", Amf0Meta::Boolean(channels > 1)));
                }
                if let Some(codec_id) = rtmp_codec_id_from_codec(track.codec) {
                    let value = if codec_id == RTMP_AUDIO_CODEC_ID_AAC {
                        10.0
                    } else {
                        codec_id as f64
                    };
                    entries.push(("audiocodecid", Amf0Meta::Number(value)));
                }
            }
            _ => {}
        }
    }

    encode_on_metadata(&entries)
}

fn rtmp_metadata_video_codec_id(codec: CodecId) -> Option<u32> {
    match codec {
        CodecId::H265 | CodecId::H266 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9 => {
            rtmp_fourcc_from_codec(codec)
        }
        _ => rtmp_codec_id_from_codec(codec).map(u32::from),
    }
}

fn normalize_video_timestamp_for_legacy_playback(
    timestamp_ms: u32,
    composition_time_ms: i32,
    mode: RtmpFlvPlayMode,
) -> (u32, i32) {
    if mode == RtmpFlvPlayMode::Enhanced || composition_time_ms >= 0 {
        return (timestamp_ms, composition_time_ms);
    }

    let shift_ms = composition_time_ms.unsigned_abs();
    (
        timestamp_ms.saturating_sub(shift_ms),
        composition_time_ms.saturating_add_unsigned(shift_ms),
    )
}

fn map_non_h264_video_with_track(
    frame: &AVFrame,
    mode: RtmpFlvPlayMode,
    track: Option<&TrackInfo>,
) -> Option<RtmpFlvPayload> {
    let (dts_ms, composition_time) = normalize_video_timestamp_for_legacy_playback(
        frame_dts_to_rtmp_timestamp_ms(frame),
        frame_composition_time_ms(frame),
        mode,
    );
    let key = if frame.is_key_frame() { 1u8 } else { 2u8 };

    if use_enhanced_video_mode(mode, frame.codec) {
        let mut out = BytesMut::new();
        let fourcc = rtmp_fourcc_from_codec(frame.codec)?;
        match frame.codec {
            CodecId::H265 | CodecId::H266 => {
                let payload = h26x_payload_for_track(frame, track);
                if payload.is_empty() {
                    out.extend_from_slice(&[(1 << 7) | (key << 4) | 3]);
                    out.extend_from_slice(&fourcc.to_be_bytes());
                    return Some(RtmpFlvPayload {
                        kind: RtmpFlvPayloadKind::Video,
                        timestamp_ms: dts_ms,
                        payload: out.freeze(),
                    });
                }
                out.extend_from_slice(&[(1 << 7) | (key << 4) | 1]);
                out.extend_from_slice(&fourcc.to_be_bytes());
                out.extend_from_slice(&[
                    ((composition_time >> 16) & 0xff) as u8,
                    ((composition_time >> 8) & 0xff) as u8,
                    (composition_time & 0xff) as u8,
                ]);
                out.extend_from_slice(&payload);
            }
            CodecId::AV1 | CodecId::VP8 | CodecId::VP9 => {
                out.extend_from_slice(&[(1 << 7) | (key << 4) | 1]);
                out.extend_from_slice(&fourcc.to_be_bytes());
                out.extend_from_slice(&frame.payload);
            }
            _ => return None,
        }
        return Some(RtmpFlvPayload {
            kind: RtmpFlvPayloadKind::Video,
            timestamp_ms: dts_ms,
            payload: out.freeze(),
        });
    }

    let codec_id = rtmp_codec_id_from_codec(frame.codec)?;
    let first = if frame.is_key_frame() {
        0x10 | codec_id
    } else {
        0x20 | codec_id
    };
    let mut out = BytesMut::new();
    out.extend_from_slice(&[
        first,
        0x01,
        ((composition_time >> 16) & 0xff) as u8,
        ((composition_time >> 8) & 0xff) as u8,
        (composition_time & 0xff) as u8,
    ]);
    match frame.codec {
        CodecId::H265 | CodecId::H266 => {
            out.extend_from_slice(&h26x_payload_for_track(frame, track))
        }
        CodecId::AV1 | CodecId::VP9 => out.extend_from_slice(&frame.payload),
        _ => return None,
    }
    Some(RtmpFlvPayload {
        kind: RtmpFlvPayloadKind::Video,
        timestamp_ms: dts_ms,
        payload: out.freeze(),
    })
}

fn find_track_for_frame<'a>(frame: &AVFrame, tracks: &'a [TrackInfo]) -> Option<&'a TrackInfo> {
    tracks
        .iter()
        .find(|track| track.track_id == frame.track_id && track.media_kind == frame.media_kind)
        .or_else(|| {
            tracks
                .iter()
                .find(|track| track.media_kind == frame.media_kind && track.codec == frame.codec)
        })
}

fn h26x_payload_for_track(frame: &AVFrame, track: Option<&TrackInfo>) -> Bytes {
    let target_len_size = track
        .and_then(h26x_nal_length_size_from_track)
        .map(normalize_nal_length_size)
        .unwrap_or(4);
    h26x_payload_with_nal_length_size(frame, target_len_size)
}

fn h26x_nal_length_size_from_track(track: &TrackInfo) -> Option<usize> {
    match (&track.codec, &track.extradata) {
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
    }
}

fn h26x_payload_with_nal_length_size(frame: &AVFrame, target_len_size: usize) -> Bytes {
    let target_len_size = normalize_nal_length_size(target_len_size);
    let canonical = h26x_length_prefixed_from_payload(frame.payload.clone());
    if target_len_size == 4 || canonical.is_empty() {
        return canonical;
    }

    for source_len_size in [4usize, 2, 1] {
        if let Some(units) = split_length_prefixed_units(canonical.as_ref(), source_len_size) {
            if let Some(encoded) = encode_length_prefixed_units(&units, target_len_size) {
                return encoded;
            }
        }
    }

    canonical
}

fn split_length_prefixed_units(payload: &[u8], nal_length_size: usize) -> Option<Vec<&[u8]>> {
    let nal_length_size = normalize_nal_length_size(nal_length_size);
    let mut units = Vec::new();
    let mut pos = 0usize;

    while payload.len() >= pos + nal_length_size {
        let nal_len = read_nal_length(payload, pos, nal_length_size);
        pos += nal_length_size;
        if nal_len == 0 || payload.len() < pos + nal_len {
            return None;
        }
        units.push(&payload[pos..pos + nal_len]);
        pos += nal_len;
    }

    if units.is_empty() || pos != payload.len() {
        return None;
    }

    Some(units)
}

fn encode_length_prefixed_units(units: &[&[u8]], nal_length_size: usize) -> Option<Bytes> {
    let nal_length_size = normalize_nal_length_size(nal_length_size);
    let mut out = BytesMut::new();

    for unit in units {
        match nal_length_size {
            1 => {
                let Ok(len) = u8::try_from(unit.len()) else {
                    return None;
                };
                out.extend_from_slice(&[len]);
            }
            2 => {
                let Ok(len) = u16::try_from(unit.len()) else {
                    return None;
                };
                out.extend_from_slice(&len.to_be_bytes());
            }
            _ => {
                if unit.len() > u32::MAX as usize {
                    return None;
                }
                out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
            }
        }
        out.extend_from_slice(unit);
    }

    Some(out.freeze())
}

fn build_non_aac_audio_payload(
    codec: CodecId,
    raw: &[u8],
    track: Option<&TrackInfo>,
) -> Option<Bytes> {
    let default = match codec {
        CodecId::ADPCM => (8_000, 16, 1),
        CodecId::G711A | CodecId::G711U => (8_000, 16, 1),
        CodecId::MP3 => (44_100, 16, 2),
        CodecId::Opus => (48_000, 16, 2),
        _ => return None,
    };

    let (sample_rate, bits, channels) = if codec == CodecId::MP3 {
        let sample_rate = track.and_then(|v| v.sample_rate).unwrap_or(default.0);
        let channels = track.and_then(|v| v.channels).unwrap_or(default.2);
        (sample_rate, default.1, channels)
    } else {
        default
    };

    let flag = rtmp_audio_flag(codec, sample_rate, bits, channels)
        .or_else(|| rtmp_audio_flag(codec, default.0, default.1, default.2))?;
    let mut out = BytesMut::with_capacity(1 + raw.len());
    out.extend_from_slice(&[flag]);
    out.extend_from_slice(raw);
    Some(out.freeze())
}

fn append_h265_hvcc_array(out: &mut BytesMut, nalu_type: u8, units: &[Bytes]) {
    if units.is_empty() {
        return;
    }
    out.extend_from_slice(&[(1 << 7) | (nalu_type & 0x3f)]);
    out.extend_from_slice(&(units.len() as u16).to_be_bytes());
    for unit in units {
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
}

fn append_h266_vvcc_array(out: &mut BytesMut, nalu_type: u8, units: &[Bytes]) {
    if units.is_empty() {
        return;
    }
    out.extend_from_slice(&[(1 << 7) | (nalu_type & 0x1f)]);
    out.extend_from_slice(&(units.len() as u16).to_be_bytes());
    for unit in units {
        out.extend_from_slice(&(unit.len() as u16).to_be_bytes());
        out.extend_from_slice(unit);
    }
}

fn build_enhanced_opus_audio_payload(packet_header: u8, body: &[u8]) -> Bytes {
    let mut payload = BytesMut::with_capacity(1 + RTMP_AUDIO_FOURCC_OPUS.len() + body.len());
    payload.extend_from_slice(&[packet_header]);
    payload.extend_from_slice(RTMP_AUDIO_FOURCC_OPUS);
    payload.extend_from_slice(body);
    payload.freeze()
}

fn build_opus_sequence_header(track: &TrackInfo) -> Bytes {
    build_enhanced_opus_audio_payload(ENHANCED_RTMP_AUDIO_SEQUENCE_START, &build_opus_head(track))
}

fn build_opus_head(track: &TrackInfo) -> Bytes {
    let channels = track.channels.unwrap_or(2).clamp(1, u8::MAX);
    let sample_rate = track.sample_rate.unwrap_or(track.clock_rate).max(1);

    let mut payload = BytesMut::with_capacity(19);
    payload.extend_from_slice(b"OpusHead");
    payload.extend_from_slice(&[1, channels]);
    payload.extend_from_slice(&OPUS_DEFAULT_PRE_SKIP.to_le_bytes());
    payload.extend_from_slice(&sample_rate.to_le_bytes());
    payload.extend_from_slice(&0i16.to_le_bytes());
    payload.extend_from_slice(&[0]);
    payload.freeze()
}

fn avcc_nal_length_size(avcc: &[u8]) -> Option<usize> {
    if avcc.len() < 5 {
        return None;
    }
    Some(((avcc[4] & 0x03) + 1) as usize)
}

fn hvcc_nal_length_size(hvcc: &[u8]) -> Option<usize> {
    if hvcc.len() < 22 {
        return None;
    }
    Some(((hvcc[21] & 0x03) + 1) as usize)
}

fn normalize_nal_length_size(length_size: usize) -> usize {
    match length_size {
        1 | 2 | 4 => length_size,
        _ => 4,
    }
}

fn read_nal_length(payload: &[u8], pos: usize, nal_length_size: usize) -> usize {
    match nal_length_size {
        1 => payload[pos] as usize,
        2 => u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize,
        _ => u32::from_be_bytes([
            payload[pos],
            payload[pos + 1],
            payload[pos + 2],
            payload[pos + 3],
        ]) as usize,
    }
}

/// Minimal AMF0 value model for the FLV `onMetaData` script tag.
enum Amf0Meta {
    Number(f64),
    Boolean(bool),
}

const AMF0_MARKER_NUMBER: u8 = 0x00;
const AMF0_MARKER_BOOLEAN: u8 = 0x01;
const AMF0_MARKER_STRING: u8 = 0x02;
const AMF0_MARKER_ECMA_ARRAY: u8 = 0x08;
const AMF0_MARKER_OBJECT_END: u8 = 0x09;

/// Encode `["onMetaData", ECMA-array(entries)]` as AMF0, byte-identical to a
/// generic AMF0 encoder for this value shape.
fn encode_on_metadata(entries: &[(&str, Amf0Meta)]) -> Bytes {
    let mut out = BytesMut::new();
    amf0_write_string(&mut out, "onMetaData");
    out.extend_from_slice(&[AMF0_MARKER_ECMA_ARRAY]);
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (key, value) in entries {
        amf0_write_key(&mut out, key);
        match value {
            Amf0Meta::Number(n) => {
                out.extend_from_slice(&[AMF0_MARKER_NUMBER]);
                out.extend_from_slice(&n.to_be_bytes());
            }
            Amf0Meta::Boolean(b) => {
                out.extend_from_slice(&[AMF0_MARKER_BOOLEAN, if *b { 1 } else { 0 }]);
            }
        }
    }
    // ECMA-array terminator: empty key + object-end marker.
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[AMF0_MARKER_OBJECT_END]);
    out.freeze()
}

fn amf0_write_string(out: &mut BytesMut, value: &str) {
    out.extend_from_slice(&[AMF0_MARKER_STRING]);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn amf0_write_key(out: &mut BytesMut, key: &str) {
    out.extend_from_slice(&(key.len() as u16).to_be_bytes());
    out.extend_from_slice(key.as_bytes());
}
