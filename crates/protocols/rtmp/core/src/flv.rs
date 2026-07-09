use alloc::{string::ToString, vec::Vec};

use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    build_audio_sequence_header, build_video_sequence_header,
    frame_composition_time_ms as codec_frame_composition_time_ms,
    frame_dts_to_rtmp_timestamp_ms as codec_frame_dts_to_rtmp_timestamp_ms,
    h26x_length_prefixed_from_payload, rtmp_audio_flag, rtmp_codec_id_from_codec,
    rtmp_fourcc_from_codec, AVFrame, CodecExtradata, CodecId, MediaKind, Rational32, TrackInfo,
    RTMP_AUDIO_CODEC_ID_AAC,
};

use crate::{encode_all, Amf0Value};

const RTMP_AUDIO_FOURCC_OPUS: &[u8; 4] = b"Opus";
const ENHANCED_RTMP_AUDIO_SEQUENCE_START: u8 = 0x90;
const ENHANCED_RTMP_AUDIO_CODED_FRAMES: u8 = 0x91;
const OPUS_DEFAULT_PRE_SKIP: u16 = 312;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpFlvPlayMode {
    Normal,
    Enhanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtmpFlvPayloadKind {
    Audio,
    Video,
    Data,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpFlvPayload {
    pub kind: RtmpFlvPayloadKind,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

pub fn frame_dts_to_rtmp_timestamp_ms(frame: &AVFrame) -> u32 {
    codec_frame_dts_to_rtmp_timestamp_ms(frame)
}

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

pub fn use_enhanced_video_mode(mode: RtmpFlvPlayMode, codec: CodecId) -> bool {
    mode == RtmpFlvPlayMode::Enhanced
        || matches!(
            codec,
            CodecId::H265 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9
        )
}

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

pub fn track_list_has_audio(tracks: &[TrackInfo]) -> bool {
    tracks
        .iter()
        .any(|track| track.media_kind == MediaKind::Audio)
}

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

pub fn mute_aac_config_payload() -> Bytes {
    let asc = [0x12, 0x10];
    let mut payload = BytesMut::with_capacity(2 + asc.len());
    payload.extend_from_slice(&[0xaf, 0x00]);
    payload.extend_from_slice(&asc);
    payload.freeze()
}

pub fn mute_aac_frame_payload() -> Bytes {
    let mut payload = BytesMut::with_capacity(3);
    payload.extend_from_slice(&[0xaf, 0x01, 0x00]);
    payload.freeze()
}

pub fn build_metadata(tracks: &[TrackInfo]) -> Bytes {
    let mut entries = Vec::new();
    entries.push(("duration".to_string(), Amf0Value::Number(0.0)));
    entries.push(("fileSize".to_string(), Amf0Value::Number(0.0)));

    for track in tracks {
        match track.media_kind {
            MediaKind::Video => {
                entries.push(("videodatarate".to_string(), Amf0Value::Number(5000.0)));
                if let Some(width) = track.width {
                    entries.push(("width".to_string(), Amf0Value::Number(width as f64)));
                }
                if let Some(height) = track.height {
                    entries.push(("height".to_string(), Amf0Value::Number(height as f64)));
                }
                if let Some(Rational32 { num, den }) = track.fps {
                    if num > 0 && den > 0 {
                        entries.push((
                            "framerate".to_string(),
                            Amf0Value::Number(num as f64 / den as f64),
                        ));
                    }
                }
                if let Some(codec_id) = rtmp_metadata_video_codec_id(track.codec) {
                    entries.push((
                        "videocodecid".to_string(),
                        Amf0Value::Number(codec_id as f64),
                    ));
                }
            }
            MediaKind::Audio => {
                entries.push(("audiodatarate".to_string(), Amf0Value::Number(160.0)));
                if let Some(sample_rate) = track.sample_rate {
                    entries.push((
                        "audiosamplerate".to_string(),
                        Amf0Value::Number(sample_rate as f64),
                    ));
                }
                if let Some(channels) = track.channels {
                    entries.push(("stereo".to_string(), Amf0Value::Boolean(channels > 1)));
                }
                if let Some(codec_id) = rtmp_codec_id_from_codec(track.codec) {
                    let value = if codec_id == RTMP_AUDIO_CODEC_ID_AAC {
                        10.0
                    } else {
                        codec_id as f64
                    };
                    entries.push(("audiocodecid".to_string(), Amf0Value::Number(value)));
                }
            }
            _ => {}
        }
    }

    encode_all(&[
        Amf0Value::String("onMetaData".to_string()),
        Amf0Value::ecma_array(entries),
    ])
}

fn rtmp_metadata_video_codec_id(codec: CodecId) -> Option<u32> {
    match codec {
        CodecId::H265 | CodecId::H266 | CodecId::AV1 | CodecId::VP8 | CodecId::VP9 => {
            rtmp_fourcc_from_codec(codec)
        }
        _ => rtmp_codec_id_from_codec(codec).map(u32::from),
    }
}

fn frame_composition_time_ms(frame: &AVFrame) -> i32 {
    codec_frame_composition_time_ms(frame)
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

pub use crate::media::{
    decode_audio_frame, decode_video_frame, encode_audio_frame, encode_video_frame,
};

#[cfg(test)]
mod tests {
    use cheetah_codec::{FrameFlags, FrameFormat, Timebase, TrackId};

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

        let payload = map_frame_to_rtmp_flv_payload(&frame, RtmpFlvPlayMode::Normal, &[])
            .expect("av1 payload");
        let fourcc = rtmp_fourcc_from_codec(CodecId::AV1)
            .expect("av1 fourcc")
            .to_be_bytes();

        assert_eq!(payload.kind, RtmpFlvPayloadKind::Video);
        assert_eq!(payload.payload[0], 0x91);
        assert_eq!(&payload.payload[1..5], &fourcc);
        assert_eq!(
            &payload.payload[5..],
            &[0x0a, 0x0e, 0x4a],
            "AV1 enhanced RTMP coded frame must start with AV1 OBU bytes, not CTS"
        );
    }

    #[test]
    fn metadata_uses_enhanced_video_fourcc_for_av1() {
        let track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::AV1, 90_000);

        let metadata = build_metadata(&[track]);
        let values = crate::decode_all(&metadata).expect("decode metadata");
        let Some(Amf0Value::EcmaArray { entries }) = values.get(1) else {
            panic!("metadata must contain an ECMA array");
        };

        let video_codec_id = entries
            .iter()
            .find(|entry| entry.key == "videocodecid")
            .and_then(|entry| entry.value.as_f64())
            .expect("videocodecid");
        let av1_fourcc = rtmp_fourcc_from_codec(CodecId::AV1).expect("av1 fourcc") as f64;
        assert_eq!(video_codec_id, av1_fourcc);
    }
}
