use crate::prelude::*;
use bytes::Bytes;

use crate::track::{CodecExtradata, CodecId, TrackInfo};

const FLV_FILE_HEADER_BYTES: usize = 9;
const FLV_PREVIOUS_TAG_SIZE_BYTES: usize = 4;
const FLV_FULL_HEADER_BYTES: usize = FLV_FILE_HEADER_BYTES + FLV_PREVIOUS_TAG_SIZE_BYTES;
const FLV_TAG_HEADER_BYTES: usize = 11;
const FLV_TAG_DATA_LEN_MAX: usize = 0xFF_FFFF;
const FLV_DEMUX_DEFAULT_MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvTagType {
    Audio,
    Video,
    Script,
}

impl FlvTagType {
    fn to_u8(self) -> u8 {
        match self {
            Self::Audio => 8,
            Self::Video => 9,
            Self::Script => 18,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            8 => Some(Self::Audio),
            9 => Some(Self::Video),
            18 => Some(Self::Script),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvHeader {
    pub has_audio: bool,
    pub has_video: bool,
}

impl FlvHeader {
    pub fn encode(&self) -> Bytes {
        let mut out = Vec::with_capacity(FLV_FULL_HEADER_BYTES);
        out.extend_from_slice(b"FLV");
        out.push(1);
        let mut flags = 0u8;
        if self.has_audio {
            flags |= 0x04;
        }
        if self.has_video {
            flags |= 0x01;
        }
        out.push(flags);
        out.extend_from_slice(&(FLV_FILE_HEADER_BYTES as u32).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        Bytes::from(out)
    }

    pub fn parse(raw: &[u8]) -> Result<Self, FlvStreamError> {
        let (header, _) = Self::parse_prefix(raw)?;
        Ok(header)
    }

    fn parse_prefix(raw: &[u8]) -> Result<(Self, usize), FlvStreamError> {
        if raw.len() < FLV_FILE_HEADER_BYTES {
            return Err(FlvStreamError::Truncated {
                context: "flv file header",
                expected_at_least: FLV_FILE_HEADER_BYTES,
                actual: raw.len(),
            });
        }
        if &raw[..3] != b"FLV" {
            return Err(FlvStreamError::InvalidHeaderSignature);
        }
        if raw[3] != 1 {
            return Err(FlvStreamError::UnsupportedHeaderVersion { version: raw[3] });
        }

        let flags = raw[4];
        let has_audio = (flags & 0x04) != 0;
        let has_video = (flags & 0x01) != 0;
        let data_offset = u32::from_be_bytes([raw[5], raw[6], raw[7], raw[8]]) as usize;
        if data_offset < FLV_FILE_HEADER_BYTES {
            return Err(FlvStreamError::InvalidHeaderDataOffset { data_offset });
        }
        let needed = data_offset.saturating_add(FLV_PREVIOUS_TAG_SIZE_BYTES);
        if raw.len() < needed {
            return Err(FlvStreamError::Truncated {
                context: "flv full header",
                expected_at_least: needed,
                actual: raw.len(),
            });
        }

        Ok((
            Self {
                has_audio,
                has_video,
            },
            needed,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvTag {
    pub tag_type: FlvTagType,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

impl FlvTag {
    pub fn encode(&self) -> Bytes {
        let data_len = self.payload.len().min(FLV_TAG_DATA_LEN_MAX);
        let mut out = Vec::with_capacity(FLV_TAG_HEADER_BYTES + data_len);
        out.push(self.tag_type.to_u8());
        out.extend_from_slice(&[
            ((data_len >> 16) & 0xff) as u8,
            ((data_len >> 8) & 0xff) as u8,
            (data_len & 0xff) as u8,
        ]);
        out.extend_from_slice(&[
            ((self.timestamp_ms >> 16) & 0xff) as u8,
            ((self.timestamp_ms >> 8) & 0xff) as u8,
            (self.timestamp_ms & 0xff) as u8,
            ((self.timestamp_ms >> 24) & 0xff) as u8,
        ]);
        out.extend_from_slice(&[0, 0, 0]);
        out.extend_from_slice(&self.payload[..data_len]);
        Bytes::from(out)
    }

    pub fn encode_with_previous_tag_size(&self) -> Bytes {
        let tag = self.encode();
        let mut out = Vec::with_capacity(tag.len() + FLV_PREVIOUS_TAG_SIZE_BYTES);
        out.extend_from_slice(&tag);
        out.extend_from_slice(&(tag.len() as u32).to_be_bytes());
        Bytes::from(out)
    }

    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < FLV_TAG_HEADER_BYTES {
            return None;
        }
        let tag_type = FlvTagType::from_u8(raw[0])?;
        let data_len = ((raw[1] as usize) << 16) | ((raw[2] as usize) << 8) | raw[3] as usize;
        if raw.len() < FLV_TAG_HEADER_BYTES + data_len {
            return None;
        }
        let timestamp_ms = ((raw[7] as u32) << 24)
            | ((raw[4] as u32) << 16)
            | ((raw[5] as u32) << 8)
            | raw[6] as u32;
        Some(Self {
            tag_type,
            timestamp_ms,
            payload: Bytes::copy_from_slice(
                &raw[FLV_TAG_HEADER_BYTES..FLV_TAG_HEADER_BYTES + data_len],
            ),
        })
    }
}

pub type FlvTagBody = FlvTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvPreviousTagSizeMismatch {
    pub expected: u32,
    pub actual: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlvDemuxEvent {
    Header(FlvHeader),
    Tag(FlvTag),
    PreviousTagSizeMismatch(FlvPreviousTagSizeMismatch),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FlvStreamError {
    #[error("invalid FLV header signature: expected \"FLV\"")]
    InvalidHeaderSignature,
    #[error("unsupported FLV header version: {version}")]
    UnsupportedHeaderVersion { version: u8 },
    #[error("invalid FLV header data offset: {data_offset}")]
    InvalidHeaderDataOffset { data_offset: usize },
    #[error("truncated {context}: expected at least {expected_at_least} bytes, got {actual}")]
    Truncated {
        context: &'static str,
        expected_at_least: usize,
        actual: usize,
    },
    #[error("invalid FLV tag type: {raw}")]
    InvalidTagType { raw: u8 },
    #[error("declared FLV tag payload is too large: {declared} > {max_allowed}")]
    TagPayloadTooLarge { declared: usize, max_allowed: usize },
    #[error("FLV demux buffer exceeded max size: {buffered} > {max_allowed}")]
    DemuxBufferTooLarge { buffered: usize, max_allowed: usize },
}

#[derive(Debug, Clone)]
pub struct FlvDemuxer {
    buffer: Vec<u8>,
    header_parsed: bool,
    max_buffer_bytes: usize,
}

impl Default for FlvDemuxer {
    fn default() -> Self {
        Self::new(FLV_DEMUX_DEFAULT_MAX_BUFFER_BYTES)
    }
}

impl FlvDemuxer {
    pub fn new(max_buffer_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            header_parsed: false,
            max_buffer_bytes: max_buffer_bytes.max(FLV_FULL_HEADER_BYTES),
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.header_parsed = false;
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<FlvDemuxEvent>, FlvStreamError> {
        let next_len = self.buffer.len().saturating_add(chunk.len());
        if next_len > self.max_buffer_bytes {
            let buffered = next_len;
            self.reset();
            return Err(FlvStreamError::DemuxBufferTooLarge {
                buffered,
                max_allowed: self.max_buffer_bytes,
            });
        }
        self.buffer.extend_from_slice(chunk);
        self.parse_buffer()
    }

    fn parse_buffer(&mut self) -> Result<Vec<FlvDemuxEvent>, FlvStreamError> {
        let mut events = Vec::new();
        let mut offset = 0usize;

        if !self.header_parsed {
            if self.buffer.len() < FLV_FULL_HEADER_BYTES {
                return Ok(events);
            }
            let (header, consumed) = FlvHeader::parse_prefix(&self.buffer)?;
            self.header_parsed = true;
            offset = consumed;
            events.push(FlvDemuxEvent::Header(header));
        }

        loop {
            let remaining = self.buffer.len().saturating_sub(offset);
            if remaining < FLV_TAG_HEADER_BYTES + FLV_PREVIOUS_TAG_SIZE_BYTES {
                break;
            }
            let start = offset;
            let raw = &self.buffer[start..];
            let Some(tag_type) = FlvTagType::from_u8(raw[0]) else {
                let bad = raw[0];
                self.reset();
                return Err(FlvStreamError::InvalidTagType { raw: bad });
            };
            let payload_len =
                ((raw[1] as usize) << 16) | ((raw[2] as usize) << 8) | raw[3] as usize;
            if payload_len > self.max_buffer_bytes {
                self.reset();
                return Err(FlvStreamError::TagPayloadTooLarge {
                    declared: payload_len,
                    max_allowed: self.max_buffer_bytes,
                });
            }
            let tag_total = FLV_TAG_HEADER_BYTES
                .saturating_add(payload_len)
                .saturating_add(FLV_PREVIOUS_TAG_SIZE_BYTES);
            if remaining < tag_total {
                break;
            }

            let timestamp_ms = ((raw[7] as u32) << 24)
                | ((raw[4] as u32) << 16)
                | ((raw[5] as u32) << 8)
                | raw[6] as u32;
            let payload_start = FLV_TAG_HEADER_BYTES;
            let payload_end = payload_start + payload_len;
            let payload = Bytes::copy_from_slice(&raw[payload_start..payload_end]);
            events.push(FlvDemuxEvent::Tag(FlvTag {
                tag_type,
                timestamp_ms,
                payload,
            }));

            let prev = u32::from_be_bytes([
                raw[payload_end],
                raw[payload_end + 1],
                raw[payload_end + 2],
                raw[payload_end + 3],
            ]);
            let expected = (FLV_TAG_HEADER_BYTES + payload_len) as u32;
            if prev != expected {
                events.push(FlvDemuxEvent::PreviousTagSizeMismatch(
                    FlvPreviousTagSizeMismatch {
                        expected,
                        actual: prev,
                    },
                ));
            }

            offset += tag_total;
        }

        if offset > 0 {
            self.buffer.drain(..offset);
        }
        Ok(events)
    }
}

pub fn build_video_sequence_header(track: &TrackInfo) -> Option<FlvTagBody> {
    let payload = match (&track.codec, &track.extradata) {
        (CodecId::H264, CodecExtradata::H264 { avcc, sps, pps }) => {
            let avcc = avcc
                .clone()
                .or_else(|| build_h264_avcc_from_parameter_sets(sps, pps))?;
            let mut p = Vec::with_capacity(5 + avcc.len());
            p.push(0x17);
            p.push(0x00);
            p.extend_from_slice(&[0, 0, 0]);
            p.extend_from_slice(&avcc);
            Bytes::from(p)
        }
        (
            CodecId::H265,
            CodecExtradata::H265 {
                hvcc: Some(hvcc), ..
            },
        ) => {
            let mut p = Vec::with_capacity(5 + hvcc.len());
            p.push(0x1c);
            p.push(0x00);
            p.extend_from_slice(&[0, 0, 0]);
            p.extend_from_slice(hvcc);
            Bytes::from(p)
        }
        _ => return None,
    };

    Some(FlvTagBody {
        tag_type: FlvTagType::Video,
        timestamp_ms: 0,
        payload,
    })
}

fn build_h264_avcc_from_parameter_sets(sps: &[Bytes], pps: &[Bytes]) -> Option<Bytes> {
    let first_sps = sps.first()?;
    if first_sps.is_empty() || pps.is_empty() || pps.iter().any(Bytes::is_empty) {
        return None;
    }
    if sps.iter().any(Bytes::is_empty) {
        return None;
    }

    let profile = *first_sps.get(1).unwrap_or(&0);
    let compatibility = *first_sps.get(2).unwrap_or(&0);
    let level = *first_sps.get(3).unwrap_or(&0);

    let mut out = Vec::with_capacity(
        6 + sps.iter().map(|set| set.len() + 2).sum::<usize>()
            + 1
            + pps.iter().map(|set| set.len() + 2).sum::<usize>(),
    );
    out.push(1); // configurationVersion
    out.push(profile); // AVCProfileIndication
    out.push(compatibility); // profile_compatibility
    out.push(level); // AVCLevelIndication
    out.push(0xFF); // reserved + lengthSizeMinusOne(3 -> 4-byte length)
    out.push(0xE0 | (sps.len().min(0x1F) as u8));
    for set in sps.iter().take(0x1F) {
        out.extend_from_slice(&(set.len() as u16).to_be_bytes());
        out.extend_from_slice(set);
    }
    out.push(pps.len().min(u8::MAX as usize) as u8);
    for set in pps.iter().take(u8::MAX as usize) {
        out.extend_from_slice(&(set.len() as u16).to_be_bytes());
        out.extend_from_slice(set);
    }

    if let Some(extension) = build_h264_avcc_extension_fields(first_sps) {
        out.extend_from_slice(&extension);
    }
    Some(Bytes::from(out))
}

fn build_h264_avcc_extension_fields(sps: &[u8]) -> Option<[u8; 4]> {
    let profile = *sps.get(1)?;
    if !matches!(
        profile,
        100 | 110 | 122 | 144 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        return None;
    }

    let rbsp = remove_h264_emulation_prevention(sps.get(1..)?);
    let mut bits = H264BitReader::new(&rbsp);
    let _profile_idc = bits.read_bits(8)?;
    let _constraint_flags = bits.read_bits(8)?;
    let _level_idc = bits.read_bits(8)?;
    let _seq_parameter_set_id = bits.read_ue()?;
    let chroma_format_idc = bits.read_ue()? as u8;
    if chroma_format_idc == 3 {
        let _separate_colour_plane_flag = bits.read_bits(1)?;
    }
    let bit_depth_luma_minus8 = bits.read_ue()? as u8;
    let bit_depth_chroma_minus8 = bits.read_ue()? as u8;

    Some([
        0xFC | (chroma_format_idc & 0x03),
        0xF8 | (bit_depth_luma_minus8 & 0x07),
        0xF8 | (bit_depth_chroma_minus8 & 0x07),
        0,
    ])
}

fn remove_h264_emulation_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zero_run = 0usize;
    for &byte in payload {
        if zero_run >= 2 && byte == 0x03 {
            zero_run = 0;
            continue;
        }
        out.push(byte);
        if byte == 0 {
            zero_run += 1;
        } else {
            zero_run = 0;
        }
    }
    out
}

struct H264BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> H264BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte_index = self.bit_offset / 8;
        let bit_in_byte = 7usize.saturating_sub(self.bit_offset % 8);
        let byte = *self.data.get(byte_index)?;
        self.bit_offset += 1;
        Some((byte >> bit_in_byte) & 1)
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut out = 0u32;
        for _ in 0..n {
            out = (out << 1) | u32::from(self.read_bit()?);
        }
        Some(out)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0usize;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return None;
            }
        }
        if leading_zeros == 0 {
            return Some(0);
        }
        let suffix = self.read_bits(leading_zeros)?;
        Some((1u32 << leading_zeros) - 1 + suffix)
    }
}

pub fn build_audio_sequence_header(track: &TrackInfo) -> Option<FlvTagBody> {
    let payload = match (&track.codec, &track.extradata) {
        (CodecId::AAC, CodecExtradata::AAC { asc }) => {
            let mut p = Vec::with_capacity(2 + asc.len());
            p.push(0xaf);
            p.push(0x00);
            p.extend_from_slice(asc);
            Bytes::from(p)
        }
        _ => return None,
    };

    Some(FlvTagBody {
        tag_type: FlvTagType::Audio,
        timestamp_ms: 0,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{CodecId, MediaKind, TrackId};

    #[test]
    fn flv_tag_roundtrip() {
        let tag = FlvTagBody {
            tag_type: FlvTagType::Video,
            timestamp_ms: 33,
            payload: Bytes::from_static(b"payload"),
        };
        let encoded = tag.encode();
        let decoded = FlvTagBody::parse(&encoded).expect("parse");
        assert_eq!(decoded.tag_type, FlvTagType::Video);
        assert_eq!(decoded.timestamp_ms, 33);
        assert_eq!(decoded.payload, Bytes::from_static(b"payload"));
    }

    #[test]
    fn flv_tag_encode_truncates_oversized_payload_consistently() {
        let payload = vec![0xAB; FLV_TAG_DATA_LEN_MAX + 32];
        let tag = FlvTagBody {
            tag_type: FlvTagType::Video,
            timestamp_ms: 90,
            payload: Bytes::from(payload),
        };

        let encoded = tag.encode();
        assert_eq!(encoded.len(), FLV_TAG_HEADER_BYTES + FLV_TAG_DATA_LEN_MAX);

        let parsed = FlvTagBody::parse(&encoded).expect("parse oversized-encoded tag");
        assert_eq!(parsed.payload.len(), FLV_TAG_DATA_LEN_MAX);
        assert_eq!(parsed.payload[0], 0xAB);
        assert_eq!(parsed.payload[FLV_TAG_DATA_LEN_MAX - 1], 0xAB);

        let with_prev = tag.encode_with_previous_tag_size();
        let prev = u32::from_be_bytes([
            with_prev[with_prev.len() - 4],
            with_prev[with_prev.len() - 3],
            with_prev[with_prev.len() - 2],
            with_prev[with_prev.len() - 1],
        ]);
        assert_eq!(prev as usize, FLV_TAG_HEADER_BYTES + FLV_TAG_DATA_LEN_MAX);
    }

    #[test]
    fn flv_header_roundtrip() {
        let header = FlvHeader {
            has_audio: true,
            has_video: true,
        };
        let encoded = header.encode();
        let parsed = FlvHeader::parse(&encoded).expect("parse header");
        assert_eq!(parsed, header);
    }

    #[test]
    fn flv_demuxer_parses_chunked_header_and_tag() {
        let mut demuxer = FlvDemuxer::default();
        let header = FlvHeader {
            has_audio: false,
            has_video: true,
        }
        .encode();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 42,
            payload: Bytes::from_static(b"abc"),
        }
        .encode_with_previous_tag_size();
        let stream = [header.as_ref(), tag.as_ref()].concat();

        let first = demuxer.push(&stream[..6]).expect("first chunk");
        assert!(first.is_empty());

        let second = demuxer.push(&stream[6..]).expect("second chunk");
        assert_eq!(second.len(), 2);
        assert_eq!(
            second[0],
            FlvDemuxEvent::Header(FlvHeader {
                has_audio: false,
                has_video: true,
            })
        );
        assert_eq!(
            second[1],
            FlvDemuxEvent::Tag(FlvTag {
                tag_type: FlvTagType::Video,
                timestamp_ms: 42,
                payload: Bytes::from_static(b"abc"),
            })
        );
    }

    #[test]
    fn flv_demuxer_reports_previous_tag_size_mismatch() {
        let mut demuxer = FlvDemuxer::default();
        let header = FlvHeader {
            has_audio: false,
            has_video: true,
        }
        .encode();
        let tag = FlvTag {
            tag_type: FlvTagType::Video,
            timestamp_ms: 1,
            payload: Bytes::from_static(b"abcd"),
        }
        .encode();
        let mut stream = Vec::with_capacity(header.len() + tag.len() + 4);
        stream.extend_from_slice(&header);
        stream.extend_from_slice(&tag);
        stream.extend_from_slice(&999u32.to_be_bytes());

        let events = demuxer.push(&stream).expect("demux");
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[2],
            FlvDemuxEvent::PreviousTagSizeMismatch(FlvPreviousTagSizeMismatch {
                expected: tag.len() as u32,
                actual: 999,
            })
        );
    }

    #[test]
    fn flv_demuxer_rejects_invalid_tag_type() {
        let mut demuxer = FlvDemuxer::default();
        let header = FlvHeader {
            has_audio: true,
            has_video: true,
        }
        .encode();
        let mut bad_tag = Vec::new();
        bad_tag.push(7); // invalid tag type
        bad_tag.extend_from_slice(&[0, 0, 0]);
        bad_tag.extend_from_slice(&[0, 0, 0, 0]);
        bad_tag.extend_from_slice(&[0, 0, 0]);
        bad_tag.extend_from_slice(&0u32.to_be_bytes());
        let stream = [header.as_ref(), bad_tag.as_slice()].concat();

        let err = demuxer.push(&stream).expect_err("invalid tag type");
        assert_eq!(err, FlvStreamError::InvalidTagType { raw: 7 });
    }

    #[test]
    fn builds_aac_sequence_header() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Audio, CodecId::AAC, 48_000);
        track.extradata = CodecExtradata::AAC {
            asc: Bytes::from_static(&[0x12, 0x10]),
        };
        let seq = build_audio_sequence_header(&track).expect("sequence header");
        assert_eq!(seq.tag_type, FlvTagType::Audio);
        assert_eq!(&seq.payload[..2], &[0xaf, 0x00]);
    }

    #[test]
    fn builds_h264_sequence_header_from_sps_pps_without_avcc() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[
                0x67, 0x42, 0x00, 0x1f, 0x96, 0x54, 0x05, 0x01, 0xed, 0x00, 0xf0, 0x88, 0x45, 0x80,
            ])],
            pps: vec![Bytes::from_static(&[0x68, 0xce, 0x06, 0xe2])],
            avcc: None,
        };

        let seq = build_video_sequence_header(&track).expect("h264 sequence header");
        assert_eq!(seq.tag_type, FlvTagType::Video);
        assert_eq!(&seq.payload[..2], &[0x17, 0x00]);
    }

    #[test]
    fn builds_h264_sequence_header_with_all_parameter_sets_without_avcc() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![
                Bytes::from_static(&[0x67, 0x42, 0x00, 0x1f]),
                Bytes::from_static(&[0x67, 0x4d, 0x00, 0x1f, 0xaa]),
            ],
            pps: vec![
                Bytes::from_static(&[0x68, 0xce, 0x06, 0xe2]),
                Bytes::from_static(&[0x68, 0xde, 0xad]),
            ],
            avcc: None,
        };

        let seq = build_video_sequence_header(&track).expect("h264 sequence header");
        let avcc = &seq.payload[5..];
        let (parsed_sps, parsed_pps) = parse_avcc_parameter_sets(avcc);
        assert_eq!(parsed_sps, track_sps(&track));
        assert_eq!(parsed_pps, track_pps(&track));
    }

    #[test]
    fn builds_h264_high_profile_sequence_header_with_avcc_extensions() {
        let mut track = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        track.extradata = CodecExtradata::H264 {
            sps: vec![Bytes::from_static(&[
                0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x1e, 0xd0, 0x08, 0x9f, 0x97, 0x01,
                0x6e, 0x40,
            ])],
            pps: vec![Bytes::from_static(&[0x68, 0xee, 0x3c, 0x80])],
            avcc: None,
        };

        let seq = build_video_sequence_header(&track).expect("h264 sequence header");
        let avcc = &seq.payload[5..];
        assert!(avcc.len() >= 4);
        assert_eq!(&avcc[avcc.len() - 4..], &[0xFD, 0xF8, 0xF8, 0x00]);
    }

    fn track_sps(track: &TrackInfo) -> Vec<Bytes> {
        match &track.extradata {
            CodecExtradata::H264 { sps, .. } => sps.clone(),
            _ => Vec::new(),
        }
    }

    fn track_pps(track: &TrackInfo) -> Vec<Bytes> {
        match &track.extradata {
            CodecExtradata::H264 { pps, .. } => pps.clone(),
            _ => Vec::new(),
        }
    }

    fn parse_avcc_parameter_sets(avcc: &[u8]) -> (Vec<Bytes>, Vec<Bytes>) {
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
                return (Vec::new(), Vec::new());
            }
            let len = u16::from_be_bytes([avcc[pos], avcc[pos + 1]]) as usize;
            pos += 2;
            if avcc.len() < pos + len {
                return (Vec::new(), Vec::new());
            }
            pps.push(Bytes::copy_from_slice(&avcc[pos..pos + len]));
            pos += len;
        }

        (sps, pps)
    }
}
