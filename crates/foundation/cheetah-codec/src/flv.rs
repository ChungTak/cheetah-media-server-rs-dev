use crate::prelude::*;
use bytes::Bytes;

use crate::track::{CodecExtradata, CodecId, TrackInfo};

const FLV_FILE_HEADER_BYTES: usize = 9;
const FLV_PREVIOUS_TAG_SIZE_BYTES: usize = 4;
const FLV_FULL_HEADER_BYTES: usize = FLV_FILE_HEADER_BYTES + FLV_PREVIOUS_TAG_SIZE_BYTES;
const FLV_TAG_HEADER_BYTES: usize = 11;
const FLV_TAG_DATA_LEN_MAX: usize = 0xFF_FFFF;
const FLV_DEMUX_DEFAULT_MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// FLV tag stream type.
///
/// Maps to the one-byte tag type field used in the FLV file and chunk
/// formats: 8 for audio, 9 for video, 18 for script.
///
/// FLV 标签流类型。
///
/// 映射到 FLV 文件与分块格式中使用的单字节标签类型字段：
/// 8 表示音频，9 表示视频，18 表示脚本。
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

/// FLV file header parsed from the first 9+ bytes of a stream.
///
/// Carries the audio/video presence flags and the data offset (usually 9).
///
/// 从流的前 9+ 字节解析的 FLV 文件头。
///
/// 携带音频/视频存在标志与数据偏移（通常为 9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvHeader {
    pub has_audio: bool,
    pub has_video: bool,
}

impl FlvHeader {
    /// Encode the FLV file header including the leading previous tag size (0).
    ///
    /// 编码 FLV 文件头，包含前导的上一标签大小（0）。
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

    /// Parse the FLV file header from the start of a byte slice.
    ///
    /// 从字节切片开头解析 FLV 文件头。
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

/// A single FLV tag header plus payload.
///
/// `timestamp_ms` is stored as a 32-bit millisecond value (24 low bits plus
/// 8 high bits). The payload is the tag body after the 11-byte header.
///
/// 单个 FLV 标签头及其负载。
///
/// `timestamp_ms` 以 32 位毫秒值存储（24 低位 + 8 高位）。
/// payload 是 11 字节头部后的标签体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvTag {
    pub tag_type: FlvTagType,
    pub timestamp_ms: u32,
    pub payload: Bytes,
}

impl FlvTag {
    /// Encode this tag as an 11-byte header followed by the payload.
    ///
    /// Payloads larger than `FLV_TAG_DATA_LEN_MAX` are truncated when encoded.
    ///
    /// 将本标签编码为 11 字节头部加负载。
    ///
    /// 编码时超过 `FLV_TAG_DATA_LEN_MAX` 的负载会被截断。
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

    /// Encode this tag followed by the 4-byte previous-tag-size trailer.
    ///
    /// 将本标签编码后追加 4 字节上一标签大小尾部。
    pub fn encode_with_previous_tag_size(&self) -> Bytes {
        let tag = self.encode();
        let mut out = Vec::with_capacity(tag.len() + FLV_PREVIOUS_TAG_SIZE_BYTES);
        out.extend_from_slice(&tag);
        out.extend_from_slice(&(tag.len() as u32).to_be_bytes());
        Bytes::from(out)
    }

    /// Parse a tag from the start of `raw` if enough bytes are present.
    ///
    /// Returns `None` when the header or declared payload is not fully buffered.
    ///
    /// 当 `raw` 起始处有足够字节时解析标签。
    ///
    /// 头部或声明的负载未完全缓冲时返回 `None`。
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

/// Alias for `FlvTag` used by the higher-level encoder helpers.
///
/// 高级编码辅助函数使用的 `FlvTag` 别名。
pub type FlvTagBody = FlvTag;

/// Diagnostic emitted when the previous tag size trailer does not match.
///
/// FLV places the size of the previous tag after each tag; this records the
/// mismatch so the caller can decide whether to tolerate it.
///
/// 上一个标签大小尾部不匹配时发出的诊断。
///
/// FLV 在每个标签后放置上一个标签大小；此结构记录不匹配，
/// 供调用方决定是否容忍。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvPreviousTagSizeMismatch {
    pub expected: u32,
    pub actual: u32,
}

/// Events produced by the streaming FLV demuxer.
///
/// The caller receives a header once, then tags and optional mismatch
/// diagnostics as the parser advances through the byte stream.
///
/// 流式 FLV 解复用器产生的事件。
///
/// 调用方会收到一次头部，随后解析器在字节流中前进时产生标签
/// 与可选的不匹配诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlvDemuxEvent {
    Header(FlvHeader),
    Tag(FlvTag),
    PreviousTagSizeMismatch(FlvPreviousTagSizeMismatch),
}

/// Errors that can occur while parsing an FLV byte stream.
///
/// These are fatal to the current demuxer state; the parser resets after
/// reporting an error so the caller can feed a fresh stream.
///
/// 解析 FLV 字节流时可能发生的错误。
///
/// 这些错误对当前解复用器状态是致命的；报告错误后解析器会重置，
/// 以便调用方送入新流。
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

/// Streaming FLV demuxer with a reassembly buffer.
///
/// Accepts arbitrary byte chunks, parses the file header, then iteratively
/// extracts tags and validates the previous-tag-size trailer. The buffer is
/// bounded by `max_buffer_bytes` to prevent unbounded growth.
///
/// 带重组缓冲区的流式 FLV 解复用器。
///
/// 接受任意字节块，解析文件头，然后迭代提取标签并验证
/// 上一个标签大小尾部。缓冲区受 `max_buffer_bytes` 限制，防止无限增长。
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
    /// Create a new demuxer with the given per-buffer byte limit.
    ///
    /// The limit is clamped to at least the full header size so a single
    /// header can always be parsed.
    ///
    /// 使用给定的缓冲区字节上限创建新的解复用器。
    ///
    /// 下限会被限制为完整文件头大小，确保单个头部总能被解析。
    pub fn new(max_buffer_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            header_parsed: false,
            max_buffer_bytes: max_buffer_bytes.max(FLV_FULL_HEADER_BYTES),
        }
    }

    /// Reset the demuxer state so it can parse a new stream.
    ///
    /// 重置解复用器状态，以便解析新流。
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.header_parsed = false;
    }

    /// Push a chunk of bytes into the demuxer and parse as many complete
    /// events as possible.
    ///
    /// Overflows the configured buffer if the chunk would exceed the limit,
    /// returning `DemuxBufferTooLarge` and resetting state.
    ///
    /// 将字节块推入解复用器并解析尽可能多的完整事件。
    ///
    /// 若块会超出配置缓冲区限制，则返回 `DemuxBufferTooLarge` 并重置状态。
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

/// Build a FLV video sequence header tag for the track.
///
/// For H.264 this builds the AVCDecoderConfigurationRecord (avcC) and wraps
/// it in a 0x17/0x00 key-frame sequence header. For H.265 it emits the hvcc
/// configuration. Returns `None` when the codec is unsupported or parameters
/// are missing.
///
/// 为轨道构建 FLV 视频序列头标签。
///
/// 对 H.264 构建 AVCDecoderConfigurationRecord（avcC）并封装为
/// 0x17/0x00 关键帧序列头；对 H.265 输出 hvcc 配置。不支持该编解码器
/// 或参数缺失时返回 `None`。
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

/// Parse the SPS and PPS arrays from an H.264 AVCDecoderConfigurationRecord.
///
/// 从 H.264 AVCDecoderConfigurationRecord 中解析 SPS 与 PPS 数组。
pub fn parse_avcc_parameter_sets(avcc: &[u8]) -> (Vec<Bytes>, Vec<Bytes>) {
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

/// Build a FLV audio sequence header tag for AAC tracks.
///
/// Produces the 0xaf/0x00 AudioSpecificConfig (ASC) packet needed by FLV
/// players to configure the AAC decoder.
///
/// 为 AAC 轨道构建 FLV 音频序列头标签。
///
/// 生成 FLV 播放器配置 AAC 解码器所需的 0xaf/0x00 AudioSpecificConfig（ASC）包。
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
        let (parsed_sps, parsed_pps) = super::parse_avcc_parameter_sets(avcc);
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
}
