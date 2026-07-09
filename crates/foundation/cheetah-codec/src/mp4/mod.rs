//! Classic ISO Base Media File Format (MP4) support.
//!
//! Provides Sans-I/O classic MP4 reader (`Mp4Reader`) and writer (`Mp4Writer`)
//! for VOD playback and unified record output. The runtime layer (record module
//! / mp4 module) is responsible for actual disk I/O via `read_at` callbacks.
//!
//! Modules:
//! * [`box_parser`] — generic ISO BMFF box reader/writer helpers
//! * [`sample_table`] — `stbl` / `stsd` / `stts` / `stss` / `stsc` / `stsz` /
//!   `stco`/`co64` parsing and seek index construction
//! * [`writer`] — `Mp4Writer` builder for VOD/record files
//! * [`reader`] — `Mp4Reader` Sans-I/O reader producing `AVFrame`
//! * [`compat`] — quirks for non-standard MP4 inputs (faststart, missing
//!   `stss`, anomalous `ctts`, oversize boxes, etc.)

pub mod box_parser;
pub mod compat;
pub mod reader;
pub mod sample_entry;
pub mod sample_table;
pub mod writer;

use crate::prelude::*;
use bytes::Bytes;

use crate::track::{CodecExtradata, CodecId, TrackInfo};

pub use reader::{Mp4ReadEvent, Mp4ReadRequest, Mp4ReadResult, Mp4Reader, Mp4ReaderConfig};
pub use sample_entry::{codec_id_from_sample_entry, extradata_from_sample_entry};
pub use sample_table::{SampleIndex, SampleIndexEntry, SampleTable, TrackBuilder};
pub use writer::{Mp4WriteEvent, Mp4Writer, Mp4WriterConfig};

/// Logical sample passed in from upstream when writing a classic MP4 file.
#[derive(Debug, Clone)]
pub struct Mp4Sample {
    pub dts_us: i64,
    pub pts_us: i64,
    pub is_sync: bool,
    pub payload: Bytes,
}

/// Sample entry descriptor (for legacy compat helpers).
#[derive(Debug, Clone)]
pub struct Mp4SampleEntry {
    pub codec: String,
    pub extradata: Bytes,
}

impl Mp4SampleEntry {
    pub fn from_track(track: &TrackInfo) -> Option<Self> {
        let (codec, extradata) = match (&track.codec, &track.extradata) {
            (
                CodecId::H264,
                CodecExtradata::H264 {
                    avcc: Some(avcc), ..
                },
            ) => ("avc1".to_string(), avcc.clone()),
            (
                CodecId::H265,
                CodecExtradata::H265 {
                    hvcc: Some(hvcc), ..
                },
            ) => ("hvc1".to_string(), hvcc.clone()),
            (CodecId::AAC, CodecExtradata::AAC { asc }) => ("mp4a".to_string(), asc.clone()),
            (
                CodecId::Opus,
                CodecExtradata::Opus {
                    channel_mapping, ..
                },
            ) => (
                "Opus".to_string(),
                channel_mapping.clone().unwrap_or_else(Bytes::new),
            ),
            (CodecId::AV1, CodecExtradata::AV1 { codec_config, .. }) => (
                "av01".to_string(),
                codec_config.clone().unwrap_or_else(Bytes::new),
            ),
            (CodecId::VP8, CodecExtradata::VP8 { config }) => (
                "vp08".to_string(),
                config.clone().unwrap_or_else(Bytes::new),
            ),
            (CodecId::VP8, _) => ("vp08".to_string(), Bytes::new()),
            (CodecId::VP9, CodecExtradata::VP9 { config }) => (
                "vp09".to_string(),
                config.clone().unwrap_or_else(Bytes::new),
            ),
            (CodecId::VP9, _) => ("vp09".to_string(), Bytes::new()),
            (CodecId::G711A, _) => ("alaw".to_string(), Bytes::new()),
            (CodecId::G711U, _) => ("ulaw".to_string(), Bytes::new()),
            (CodecId::MP2, _) => ("mp4a".to_string(), Bytes::new()),
            (CodecId::MP3, _) => ("mp4a".to_string(), Bytes::new()),
            (CodecId::MJPEG, _) => ("mp4v".to_string(), Bytes::new()),
            _ => return None,
        };
        Some(Self { codec, extradata })
    }
}

/// Errors raised by classic MP4 reader/writer.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Mp4Error {
    #[error("invalid box at offset {offset}: {detail}")]
    InvalidBox { offset: u64, detail: &'static str },
    #[error("box {fourcc} truncated: need {need} bytes, have {have}")]
    BoxTruncated {
        fourcc: String,
        need: u64,
        have: u64,
    },
    #[error("unsupported track configuration: {0}")]
    UnsupportedTrack(&'static str),
    #[error("missing required box: {0}")]
    MissingBox(&'static str),
    #[error("oversize box: {fourcc} {size} > limit {limit}")]
    OversizeBox {
        fourcc: String,
        size: u64,
        limit: u64,
    },
    #[error("invalid sample table: {0}")]
    InvalidSampleTable(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{CodecId, MediaKind, TrackId};

    #[test]
    fn builds_h264_sample_entry() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(Bytes::from_static(&[1, 2, 3])),
        };
        let entry = Mp4SampleEntry::from_track(&track).expect("entry");
        assert_eq!(entry.codec, "avc1");
        assert_eq!(entry.extradata, Bytes::from_static(&[1, 2, 3]));
    }
}
