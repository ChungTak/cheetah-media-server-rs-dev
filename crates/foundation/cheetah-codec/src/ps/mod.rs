//! Program Stream (PS) parser, demuxer and muxer modules.
//!
//! PS 节目流解析、解复用与复用模块。

pub mod demuxer;
pub mod diagnostic;
pub mod muxer;
pub mod pes;

#[cfg(test)]
mod tests;

pub use demuxer::PsDemuxer;
pub use diagnostic::PsDemuxerConfig;
pub use diagnostic::{PsDemuxDiagnostic, PsDemuxEvent};
pub use muxer::PsMuxer;
pub use pes::{PesPacket, PsPacket, PsStreamKind};

use crate::frame::FrameFormat;
use crate::track::CodecId;

pub(crate) fn probe_video_codec(payload: &[u8]) -> Option<CodecId> {
    let mut offset = 0;
    while offset + 6 <= payload.len() {
        let Some(start) = payload[offset..]
            .windows(3)
            .position(|w| w == [0x00, 0x00, 0x01])
        else {
            break;
        };
        let start = offset + start;
        let header_offset = if payload.get(start + 3).copied() == Some(0x00) {
            start + 4
        } else {
            start + 3
        };
        if header_offset >= payload.len() {
            break;
        }

        let b0 = payload[header_offset];
        let b1 = payload.get(header_offset + 1).copied().unwrap_or(0);

        // H.265 NAL header: first byte contains forbidden zero bit, nal_unit_type (6 bits),
        // second byte contains nuh_temporal_id_plus1 (3 bits).
        let h265_nal_type = (b0 >> 1) & 0x3F;
        let h265_temporal_id = b1 & 0x07;
        if (b0 & 0x80) == 0
            && h265_temporal_id > 0
            && matches!(h265_nal_type, 1 | 2 | 19 | 20 | 32 | 33 | 34 | 35 | 36)
        {
            return Some(CodecId::H265);
        }

        // H.264 NAL header: 1 forbidden bit, 2 ref idc, 5 nal_unit_type.
        let h264_nal_type = b0 & 0x1F;
        if (b0 & 0x80) == 0 && matches!(h264_nal_type, 1 | 5 | 7 | 8 | 9) {
            return Some(CodecId::H264);
        }

        offset = header_offset;
    }
    None
}

pub(crate) fn probe_audio_codec(payload: &[u8], stream_id: u8) -> CodecId {
    // ADTS syncword is 0xFFF (12 bits).
    if payload.len() >= 2 && payload[0] == 0xFF && (payload[1] & 0xF0) == 0xF0 {
        return CodecId::AAC;
    }
    // Without a PSM, G.711 A-law and mu-law are indistinguishable from payload alone.
    // Use the stream id range as a best-effort hint: the lower half of audio stream ids
    // maps to A-law, the upper half to mu-law.
    if (0xD0..=0xDF).contains(&stream_id) {
        CodecId::G711U
    } else {
        CodecId::G711A
    }
}

pub(crate) fn default_frame_format(codec: CodecId) -> FrameFormat {
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => FrameFormat::CanonicalH26x,
        CodecId::AAC => FrameFormat::AacRaw,
        CodecId::G711A | CodecId::G711U => FrameFormat::G711Packet,
        CodecId::MP3 => FrameFormat::Mp3Frame,
        CodecId::Opus => FrameFormat::OpusPacket,
        _ => FrameFormat::Unknown,
    }
}

/// Stream IDs recognised at the Program Stream layer. Used to validate that a
/// 3-byte `00 00 01` prefix really starts a new PS packet rather than appearing
/// inside a video PES payload as part of an H.264 / H.265 NALU start code.
pub(crate) fn is_ps_stream_id(stream_id: u8) -> bool {
    matches!(
        stream_id,
        0xBA       // pack_start_code
        | 0xBB     // system_header
        | 0xBC     // program_stream_map
        | 0xBD     // private_stream_1
        | 0xBE     // padding_stream
        | 0xBF     // private_stream_2
        | 0xC0..=0xDF  // audio streams
        | 0xE0..=0xEF  // video streams
        | 0xF0..=0xF2  // ECM, EMM, DSMCC
        | 0xF8..=0xFF  // ITU-T Rec. H.222.1 type E, program_stream_directory, etc.
    )
}

pub(crate) fn stream_kind(stream_id: u8) -> PsStreamKind {
    if (0xE0..=0xEF).contains(&stream_id) {
        PsStreamKind::Video
    } else if (0xC0..=0xDF).contains(&stream_id) {
        PsStreamKind::Audio
    } else {
        PsStreamKind::Private
    }
}

pub(crate) fn find_start_code(data: &[u8]) -> Option<usize> {
    data.windows(3).position(|w| w == [0x00, 0x00, 0x01])
}

pub(crate) fn parse_pts_dts(raw: &[u8]) -> Option<i64> {
    if raw.len() < 5 {
        return None;
    }
    let v = (((raw[0] >> 1) as u64 & 0x07) << 30)
        | ((raw[1] as u64) << 22)
        | (((raw[2] >> 1) as u64) << 15)
        | ((raw[3] as u64) << 7)
        | ((raw[4] >> 1) as u64);
    Some(v as i64)
}

pub(crate) fn encode_pts_dts(value: i64, prefix: u8) -> [u8; 5] {
    let v = value.max(0) as u64;
    let b0 = (prefix << 4) | (((v >> 30) as u8 & 0x07) << 1) | 0x01;
    let b1 = (v >> 22) as u8;
    let b2 = (((v >> 15) as u8) << 1) | 0x01;
    let b3 = (v >> 7) as u8;
    let b4 = ((v as u8) << 1) | 0x01;
    [b0, b1, b2, b3, b4]
}
