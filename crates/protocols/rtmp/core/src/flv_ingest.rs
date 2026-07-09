use alloc::vec::Vec;

use bytes::{Bytes, BytesMut};
use cheetah_codec::{
    codec_from_rtmp_metadata, AVFrame, CodecExtradata, CodecId, FrameSideData, MediaKind,
    Rational32, RtmpTimestamp, SourceTimestamp, TimestampAlert, TimestampNormalizeOutput,
    TimestampNormalizer, TrackId, TrackInfo,
};

use crate::{Amf0Value as WireAmf0Value, Amf3Value, AmfValue, AmfValueRef};

pub const RTMP_VIDEO_RAW_SIDEDATA_MAGIC: &[u8] = b"rtmp-video-raw:";
pub const RTMP_AUDIO_RAW_SIDEDATA_MAGIC: &[u8] = b"rtmp-audio-raw:";

const RTMP_TIMESTAMP_BACKWARD_JITTER_MS: u32 = 3_000;
const RTMP_TIMESTAMP_WRAP_FORWARD_MAX_MS: u32 = 300_000;

pub fn apply_flv_metadata_to_tracks(
    video_track: &mut Option<TrackInfo>,
    audio_track: &mut Option<TrackInfo>,
    values: &[AmfValue],
) {
    let Some(meta) = extract_metadata_object(values) else {
        return;
    };

    if let Some(codec) = codec_from_rtmp_metadata(
        MediaKind::Video,
        metadata_member_f64(meta, "videocodecid"),
        metadata_member_str(meta, "videocodecid"),
    ) {
        let mut track = video_track
            .clone()
            .unwrap_or_else(|| TrackInfo::new(TrackId(1), MediaKind::Video, codec, 90_000));
        track.codec = codec;
        if let Some(width) = metadata_member_f64(meta, "width") {
            track.width = Some(width.max(0.0) as u32);
        }
        if let Some(height) = metadata_member_f64(meta, "height") {
            track.height = Some(height.max(0.0) as u32);
        }
        if let Some(fps) = metadata_member_f64(meta, "framerate") {
            let scaled = if fps > 0.0 {
                ((fps * 1000.0) + 0.5) as u32
            } else {
                1
            };
            track.fps = Some(Rational32::new(scaled, 1000));
        }
        track.refresh_readiness();
        *video_track = Some(track);
    }

    if let Some(codec) = codec_from_rtmp_metadata(
        MediaKind::Audio,
        metadata_member_f64(meta, "audiocodecid"),
        metadata_member_str(meta, "audiocodecid"),
    ) {
        let mut track = audio_track.clone().unwrap_or_else(|| {
            TrackInfo::new(
                TrackId(2),
                MediaKind::Audio,
                codec,
                default_audio_clock(codec),
            )
        });
        track.codec = codec;
        if let Some(sample_rate) = metadata_member_f64(meta, "audiosamplerate") {
            let sample_rate = sample_rate.max(0.0) as u32;
            if sample_rate > 0 {
                track.sample_rate = Some(sample_rate);
                track.clock_rate = sample_rate;
            }
        }
        if let Some(stereo) = metadata_member_bool(meta, "stereo") {
            track.channels = Some(if stereo { 2 } else { 1 });
        }
        track.refresh_readiness();
        *audio_track = Some(track);
    }
}

pub fn apply_flv_video_config(
    video_track: &mut Option<TrackInfo>,
    codec: CodecId,
    config_payload: &[u8],
) {
    let mut track = video_track
        .clone()
        .unwrap_or_else(|| TrackInfo::new(TrackId(1), MediaKind::Video, codec, 90_000));
    track.codec = codec;

    match codec {
        CodecId::H264 => {
            let avcc = Bytes::copy_from_slice(config_payload);
            let (sps, pps) = parse_flv_avcc_parameter_sets(&avcc);
            track.extradata = CodecExtradata::H264 {
                sps,
                pps,
                avcc: Some(avcc),
            };
        }
        CodecId::H265 => {
            let hvcc = Bytes::copy_from_slice(config_payload);
            let (vps, sps, pps) = parse_flv_hvcc_parameter_sets(config_payload, codec);
            track.extradata = CodecExtradata::H265 {
                vps,
                sps,
                pps,
                hvcc: Some(hvcc),
            };
        }
        CodecId::H266 => {
            let (vps, sps, pps) = parse_flv_hvcc_parameter_sets(config_payload, codec);
            track.extradata = CodecExtradata::H266 { vps, sps, pps };
        }
        CodecId::AV1 => {
            let config = Bytes::copy_from_slice(config_payload);
            track.extradata = CodecExtradata::AV1 {
                sequence_header: Some(config.clone()),
                codec_config: Some(config),
            };
        }
        CodecId::VP9 => {
            let config = Bytes::copy_from_slice(config_payload);
            track.extradata = CodecExtradata::VP9 {
                config: Some(config),
            };
        }
        CodecId::VP8 => {
            track.extradata = CodecExtradata::VP8 {
                config: Some(Bytes::copy_from_slice(config_payload)),
            };
        }
        _ => {}
    }
    track.refresh_readiness();
    *video_track = Some(track);
}

pub fn parse_flv_hvcc_parameter_sets(
    payload: &[u8],
    codec: CodecId,
) -> (Vec<Bytes>, Vec<Bytes>, Vec<Bytes>) {
    match codec {
        CodecId::H265 => parse_h265_hvcc_parameter_sets(payload),
        CodecId::H266 => parse_h266_vvcc_parameter_sets(payload),
        _ => (Vec::new(), Vec::new(), Vec::new()),
    }
}

pub fn parse_flv_avcc_parameter_sets(avcc: &[u8]) -> (Vec<Bytes>, Vec<Bytes>) {
    if avcc.len() < 7 {
        return (Vec::new(), Vec::new());
    }

    let mut pos = 5usize;
    let sps_count = (avcc[pos] & 0x1f) as usize;
    pos += 1;

    let mut sps = Vec::new();
    for _ in 0..sps_count {
        if avcc.len() < pos + 2 {
            return (Vec::new(), Vec::new());
        }
        let len = u16::from_be_bytes([avcc[pos], avcc[pos + 1]]) as usize;
        pos += 2;
        if avcc.len() < pos + len {
            return (Vec::new(), Vec::new());
        }
        sps.push(Bytes::copy_from_slice(&avcc[pos..pos + len]));
        pos += len;
    }

    if avcc.len() <= pos {
        return (sps, Vec::new());
    }
    let pps_count = avcc[pos] as usize;
    pos += 1;

    let mut pps = Vec::new();
    for _ in 0..pps_count {
        if avcc.len() < pos + 2 {
            return (sps, Vec::new());
        }
        let len = u16::from_be_bytes([avcc[pos], avcc[pos + 1]]) as usize;
        pos += 2;
        if avcc.len() < pos + len {
            return (sps, Vec::new());
        }
        pps.push(Bytes::copy_from_slice(&avcc[pos..pos + len]));
        pos += len;
    }

    (sps, pps)
}

pub fn attach_raw_rtmp_video_payload(frame: &mut AVFrame, payload: &[u8]) {
    let mut tagged = BytesMut::with_capacity(RTMP_VIDEO_RAW_SIDEDATA_MAGIC.len() + payload.len());
    tagged.extend_from_slice(RTMP_VIDEO_RAW_SIDEDATA_MAGIC);
    tagged.extend_from_slice(payload);
    frame.side_data.push(FrameSideData::Opaque(tagged.freeze()));
}

pub fn attach_raw_rtmp_audio_payload(frame: &mut AVFrame, payload: &[u8]) {
    let mut tagged = BytesMut::with_capacity(RTMP_AUDIO_RAW_SIDEDATA_MAGIC.len() + payload.len());
    tagged.extend_from_slice(RTMP_AUDIO_RAW_SIDEDATA_MAGIC);
    tagged.extend_from_slice(payload);
    frame.side_data.push(FrameSideData::Opaque(tagged.freeze()));
}

pub fn length_prefixed_to_annexb_with_size(payload: &[u8], nal_length_size: usize) -> Bytes {
    let nal_length_size = normalize_nal_length_size(nal_length_size);
    let mut out = BytesMut::with_capacity(payload.len() + 16);
    let mut pos = 0usize;
    while payload.len() >= pos + nal_length_size {
        let len = read_nal_length(payload, pos, nal_length_size);
        pos += nal_length_size;
        if len == 0 || payload.len() < pos + len {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&payload[pos..pos + len]);
        pos += len;
    }
    if out.is_empty() {
        Bytes::copy_from_slice(payload)
    } else {
        out.freeze()
    }
}

pub fn source_timestamp_from_rtmp_ms(raw_timestamp_ms: u32) -> SourceTimestamp {
    SourceTimestamp::Rtmp(RtmpTimestamp::new(
        raw_timestamp_ms,
        u64::from(raw_timestamp_ms),
    ))
}

pub fn maybe_reset_rtmp_timestamp_normalizer(
    normalizer: &mut TimestampNormalizer,
    repair_count: &mut u64,
    last_raw_timestamp_ms: &mut Option<u32>,
    raw_timestamp_ms: u32,
) {
    let Some(last_raw) = *last_raw_timestamp_ms else {
        return;
    };
    if raw_timestamp_ms >= last_raw {
        return;
    }

    let backward = last_raw.saturating_sub(raw_timestamp_ms);
    let wrapped_forward = raw_timestamp_ms.wrapping_sub(last_raw);
    if backward > RTMP_TIMESTAMP_BACKWARD_JITTER_MS
        && wrapped_forward > RTMP_TIMESTAMP_WRAP_FORWARD_MAX_MS
    {
        normalizer.reset();
        *repair_count = 0;
    }
}

pub fn update_timestamp_repair_counter(
    normalized: &TimestampNormalizeOutput,
    repair_count: &mut u64,
) -> (u64, bool) {
    let repaired = normalized
        .alerts
        .iter()
        .any(|alert| matches!(alert, TimestampAlert::NonMonotonicDtsRepaired));
    if !repaired {
        return (0, false);
    }
    *repair_count = repair_count.saturating_add(1);
    let current = *repair_count;
    (
        current,
        cheetah_codec::should_sample_timestamp_repair(current),
    )
}

fn extract_metadata_object(values: &[AmfValue]) -> Option<&AmfValue> {
    if values.is_empty() {
        return None;
    }
    let first = values[0].expect_str().ok().unwrap_or_default();
    if first.eq_ignore_ascii_case("@setDataFrame") {
        return values.get(2);
    }
    if first.eq_ignore_ascii_case("onMetaData") {
        return values.get(1);
    }
    None
}

fn metadata_member<'a>(meta: &'a AmfValue, key: &str) -> Option<AmfValueRef<'a>> {
    meta.expect_object_member(key).ok()
}

fn metadata_member_f64(meta: &AmfValue, key: &str) -> Option<f64> {
    metadata_member(meta, key).and_then(|value| value.expect_number().ok())
}

fn metadata_member_str<'a>(meta: &'a AmfValue, key: &str) -> Option<&'a str> {
    metadata_member(meta, key).and_then(|value| value.expect_str().ok())
}

fn metadata_member_bool(meta: &AmfValue, key: &str) -> Option<bool> {
    match metadata_member(meta, key)? {
        AmfValueRef::Amf0(value) => match value {
            WireAmf0Value::Boolean(v) => Some(*v),
            WireAmf0Value::Number(v) => Some(*v > 0.0),
            _ => None,
        },
        AmfValueRef::Amf3(value) => match value {
            Amf3Value::Boolean(v) => Some(*v),
            Amf3Value::Integer(v) => Some(*v > 0),
            Amf3Value::Double(v) => Some(*v > 0.0),
            _ => None,
        },
    }
}

fn parse_h265_hvcc_parameter_sets(payload: &[u8]) -> (Vec<Bytes>, Vec<Bytes>, Vec<Bytes>) {
    if payload.len() < 23 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let mut vps = Vec::new();
    let mut sps = Vec::new();
    let mut pps = Vec::new();

    let mut index = 23usize;
    let num_arrays = payload[22] as usize;
    for _ in 0..num_arrays {
        if payload.len() < index + 3 {
            break;
        }
        let nalu_type = payload[index] & 0x3f;
        let num_nalus = u16::from_be_bytes([payload[index + 1], payload[index + 2]]) as usize;
        index += 3;

        for _ in 0..num_nalus {
            if payload.len() < index + 2 {
                break;
            }
            let nalu_len = u16::from_be_bytes([payload[index], payload[index + 1]]) as usize;
            index += 2;
            if payload.len() < index + nalu_len {
                break;
            }
            let nalu = Bytes::copy_from_slice(&payload[index..index + nalu_len]);
            match nalu_type {
                32 => vps.push(nalu),
                33 => sps.push(nalu),
                34 => pps.push(nalu),
                _ => {}
            };
            index += nalu_len;
        }
    }

    (vps, sps, pps)
}

fn parse_h266_vvcc_parameter_sets(payload: &[u8]) -> (Vec<Bytes>, Vec<Bytes>, Vec<Bytes>) {
    if payload.len() < 2 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let mut index = 1usize;
    let ptl_present = (payload[0] & 0x01) != 0;
    if ptl_present {
        index = match skip_h266_ptl(payload, index) {
            Some(value) => value,
            None => return (Vec::new(), Vec::new(), Vec::new()),
        };
    }
    if payload.len() < index + 1 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let mut vps = Vec::new();
    let mut sps = Vec::new();
    let mut pps = Vec::new();

    let num_arrays = payload[index] as usize;
    index += 1;

    for _ in 0..num_arrays {
        if payload.len() < index + 3 {
            break;
        }
        let nalu_type = payload[index] & 0x1f;
        let num_nalus = u16::from_be_bytes([payload[index + 1], payload[index + 2]]) as usize;
        index += 3;

        for _ in 0..num_nalus {
            if payload.len() < index + 2 {
                break;
            }
            let nalu_len = u16::from_be_bytes([payload[index], payload[index + 1]]) as usize;
            index += 2;
            if payload.len() < index + nalu_len {
                break;
            }
            let nalu = Bytes::copy_from_slice(&payload[index..index + nalu_len]);
            match nalu_type {
                14 => vps.push(nalu),
                15 => sps.push(nalu),
                16 => pps.push(nalu),
                _ => {}
            }
            index += nalu_len;
        }
    }

    (vps, sps, pps)
}

fn skip_h266_ptl(payload: &[u8], mut index: usize) -> Option<usize> {
    if payload.len() < index + 2 {
        return None;
    }
    let num_sublayers = (payload[index + 1] >> 4) & 0b111;
    index += 2;
    if payload.len() < index + 1 {
        return None;
    }
    index += 1;

    if payload.len() < index + 1 {
        return None;
    }
    let num_bytes_constraint_info = (payload[index] & 0x3f) as usize;
    index += 1;

    if payload.len() < index + 2 + num_bytes_constraint_info {
        return None;
    }
    index += 2 + num_bytes_constraint_info;

    if num_sublayers > 1 {
        if payload.len() < index + 1 {
            return None;
        }
        let flags = payload[index];
        index += 1;
        for layer in (0..=(num_sublayers - 2)).rev() {
            if (flags & (1 << layer)) != 0 {
                if payload.len() < index + 1 {
                    return None;
                }
                index += 1;
            }
        }
    }

    if payload.len() < index + 1 {
        return None;
    }
    let num_sub_profiles = payload[index] as usize;
    index += 1;
    if payload.len() < index + (num_sub_profiles * 4) {
        return None;
    }
    index += num_sub_profiles * 4;

    if payload.len() < index + 6 {
        return None;
    }
    index += 6;
    Some(index)
}

fn default_audio_clock(codec: CodecId) -> u32 {
    match codec {
        CodecId::ADPCM => 8_000,
        CodecId::G711A | CodecId::G711U => 8_000,
        CodecId::Opus => 48_000,
        CodecId::MP3 => 44_100,
        _ => 48_000,
    }
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

#[cfg(test)]
mod tests {
    use cheetah_codec::CodecId;

    use super::*;
    use crate::{Amf0Value, AmfVersion};

    #[test]
    fn metadata_updates_video_and_audio_tracks() {
        let values = [
            AmfValue::from((AmfVersion::Amf0, "onMetaData")),
            AmfValue::amf0_object([
                ("videocodecid", Amf0Value::String("av01".into())),
                ("audiocodecid", Amf0Value::Number(10.0)),
                ("audiosamplerate", Amf0Value::Number(44_100.0)),
                ("stereo", Amf0Value::Boolean(true)),
            ]),
        ];
        let mut video = None;
        let mut audio = None;
        apply_flv_metadata_to_tracks(&mut video, &mut audio, &values);
        assert_eq!(video.as_ref().map(|t| t.codec), Some(CodecId::AV1));
        assert_eq!(audio.as_ref().map(|t| t.codec), Some(CodecId::AAC));
        assert_eq!(audio.as_ref().and_then(|t| t.sample_rate), Some(44_100));
        assert_eq!(audio.as_ref().and_then(|t| t.channels), Some(2));
    }
}
