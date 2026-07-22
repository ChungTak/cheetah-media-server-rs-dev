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
