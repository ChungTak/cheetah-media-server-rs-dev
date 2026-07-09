//! Classic MP4 sample entry decoders/encoders for the supported codec matrix.
//!
//! Maps `stsd` sample entry boxes to/from `(CodecId, CodecExtradata)`. Used by
//! both the writer (when emitting `stsd`) and the reader (when ingesting an
//! existing file's `stsd`).

use crate::prelude::*;
use bytes::Bytes;

use crate::track::{CodecExtradata, CodecId};

/// Map a sample entry 4cc to a `CodecId`. Returns `Unknown` for codecs that
/// the project knowingly cannot demux to canonical frames.
pub fn codec_id_from_sample_entry(fourcc: &[u8; 4]) -> CodecId {
    match fourcc {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => CodecId::H264,
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => CodecId::H265,
        b"vvc1" | b"vvi1" => CodecId::H266,
        b"vp08" => CodecId::VP8,
        b"vp09" => CodecId::VP9,
        b"av01" => CodecId::AV1,
        b"mp4v" | b"jpeg" | b"mjpa" | b"mjpb" => CodecId::MJPEG,
        b"mp4a" => CodecId::AAC,
        b"alaw" => CodecId::G711A,
        b"ulaw" => CodecId::G711U,
        b"Opus" | b"opus" => CodecId::Opus,
        b".mp3" => CodecId::MP3,
        _ => CodecId::Unknown,
    }
}

/// Build a `CodecExtradata` from a parsed sample entry's child config box, if
/// any.
pub fn extradata_from_sample_entry(
    codec: CodecId,
    config_box_fourcc: Option<[u8; 4]>,
    config_payload: &[u8],
) -> CodecExtradata {
    match (codec, config_box_fourcc) {
        (CodecId::H264, Some(fourcc)) if &fourcc == b"avcC" => CodecExtradata::H264 {
            sps: Vec::new(),
            pps: Vec::new(),
            avcc: Some(Bytes::copy_from_slice(config_payload)),
        },
        (CodecId::H265, Some(fourcc)) if &fourcc == b"hvcC" => CodecExtradata::H265 {
            vps: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
            hvcc: Some(Bytes::copy_from_slice(config_payload)),
        },
        (CodecId::H266, Some(fourcc)) if &fourcc == b"vvcC" => {
            CodecExtradata::Raw(Bytes::copy_from_slice(config_payload))
        }
        (CodecId::VP8, Some(fourcc)) | (CodecId::VP9, Some(fourcc)) if &fourcc == b"vpcC" => {
            if codec == CodecId::VP8 {
                CodecExtradata::VP8 {
                    config: Some(Bytes::copy_from_slice(config_payload)),
                }
            } else {
                CodecExtradata::VP9 {
                    config: Some(Bytes::copy_from_slice(config_payload)),
                }
            }
        }
        (CodecId::AV1, Some(fourcc)) if &fourcc == b"av1C" => CodecExtradata::AV1 {
            sequence_header: None,
            codec_config: Some(Bytes::copy_from_slice(config_payload)),
        },
        (CodecId::Opus, Some(fourcc)) if &fourcc == b"dOps" => CodecExtradata::Opus {
            fmtp: None,
            channel_mapping: Some(Bytes::copy_from_slice(config_payload)),
        },
        (CodecId::AAC, Some(fourcc)) if &fourcc == b"esds" => {
            let asc = parse_esds_decoder_specific(config_payload);
            CodecExtradata::AAC { asc }
        }
        _ => CodecExtradata::None,
    }
}

/// Extract the AAC AudioSpecificConfig (DSI) payload from an `esds` box body
/// (everything after the 4-byte FullBox version+flags). This is best-effort
/// and tolerates non-strict descriptor encodings emitted by various muxers.
fn parse_esds_decoder_specific(payload: &[u8]) -> Bytes {
    if payload.len() < 4 {
        return Bytes::new();
    }
    // Skip 4-byte version+flags
    let buf = &payload[4..];
    let mut pos = 0usize;
    while pos + 1 < buf.len() {
        let tag = buf[pos];
        pos += 1;
        let mut size = 0usize;
        for _ in 0..4 {
            if pos >= buf.len() {
                return Bytes::new();
            }
            let b = buf[pos];
            pos += 1;
            size = (size << 7) | ((b & 0x7F) as usize);
            if b & 0x80 == 0 {
                break;
            }
        }
        if pos + size > buf.len() {
            return Bytes::new();
        }
        match tag {
            0x03 => {
                // ES_Descriptor: skip ES_ID(2) + flags(1)
                if size < 3 {
                    return Bytes::new();
                }
                pos += 3;
                continue;
            }
            0x04 => {
                // DecoderConfigDescriptor: skip 13-byte fixed header
                if size < 13 {
                    return Bytes::new();
                }
                pos += 13;
                continue;
            }
            0x05 => {
                // DecoderSpecificInfo: payload is DSI
                let dsi = &buf[pos..pos + size];
                return Bytes::copy_from_slice(dsi);
            }
            _ => {
                pos += size;
            }
        }
    }
    Bytes::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_video_codecs() {
        assert_eq!(codec_id_from_sample_entry(b"avc1"), CodecId::H264);
        assert_eq!(codec_id_from_sample_entry(b"hev1"), CodecId::H265);
        assert_eq!(codec_id_from_sample_entry(b"dvh1"), CodecId::H265);
        assert_eq!(codec_id_from_sample_entry(b"av01"), CodecId::AV1);
        assert_eq!(codec_id_from_sample_entry(b"mp4v"), CodecId::MJPEG);
    }

    #[test]
    fn parses_aac_esds_dsi() {
        // Minimal esds: full_box(4) + ES desc tag 0x03 ... DSI 0x05 size=2 [0x12,0x10]
        let esds = vec![
            0x00, 0x00, 0x00, 0x00, // full box
            0x03, 0x80, 0x80, 0x80, 0x18, // ES_Descriptor, size=24
            0x00, 0x01, 0x00, // ES_ID + flags
            0x04, 0x80, 0x80, 0x80, 0x10, // DecoderConfigDescriptor, size=16
            0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05,
            0x80, 0x80, 0x80, 0x02, // DSI, size=2
            0x12, 0x10, 0x06, 0x80, 0x80, 0x80, 0x01, 0x02, // SLConfigDescriptor
        ];
        let dsi = parse_esds_decoder_specific(&esds);
        assert_eq!(dsi.as_ref(), &[0x12, 0x10]);
    }
}
