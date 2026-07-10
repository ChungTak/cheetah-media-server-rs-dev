use alloc::vec::Vec;
use core::time::Duration;

use cheetah_codec::{
    codec_from_rtmp_codec_id_with_mode, codec_from_rtmp_fourcc, CodecId, DomesticCodecMode,
    MediaKind,
};

use crate::error::Error;

/// `RtmpTimestamp` data structure.
/// `RtmpTimestamp` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RtmpTimestamp(u32);

impl RtmpTimestamp {
    pub const ZERO: Self = Self(0);

    /// Creates `millis` from input.
    /// 从输入创建 `millis`。
    pub const fn from_millis(t: u32) -> Self {
        Self(t)
    }

    /// `as_millis` function of `RtmpTimestamp`.
    /// `RtmpTimestamp` 的 `as_millis` 函数。
    pub const fn as_millis(self) -> u32 {
        self.0
    }

    /// `as_duration` function of `RtmpTimestamp`.
    /// `RtmpTimestamp` 的 `as_duration` 函数。
    pub const fn as_duration(self) -> Duration {
        Duration::from_millis(self.0 as u64)
    }

    /// `wrapping_add` function of `RtmpTimestamp`.
    /// `RtmpTimestamp` 的 `wrapping_add` 函数。
    pub fn wrapping_add(self, other: Self) -> Self {
        Self(self.0.wrapping_add(other.0))
    }

    /// `checked_sub` function of `RtmpTimestamp`.
    /// `RtmpTimestamp` 的 `checked_sub` 函数。
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }
}

/// `RtmpTimestampDelta` data structure.
/// `RtmpTimestampDelta` 数据结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RtmpTimestampDelta(i32);

impl RtmpTimestampDelta {
    pub const ZERO: Self = Self(0);

    /// Creates `millis` from input.
    /// 从输入创建 `millis`。
    pub const fn from_millis(t: i32) -> Self {
        Self(t)
    }

    /// `as_millis` function of `RtmpTimestampDelta`.
    /// `RtmpTimestampDelta` 的 `as_millis` 函数。
    pub const fn as_millis(self) -> i32 {
        self.0
    }
}

/// Frame for `Media`.
/// `Media` 的帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaFrame {
    Audio(AudioFrame),
    Video(VideoFrame),
}

impl MediaFrame {
    /// `unwrap_audio` function of `MediaFrame`.
    /// `MediaFrame` 的 `unwrap_audio` 函数。
    pub fn unwrap_audio(self) -> AudioFrame {
        match self {
            MediaFrame::Audio(f) => f,
            _ => panic!("expected Audio frame"),
        }
    }

    /// `unwrap_video` function of `MediaFrame`.
    /// `MediaFrame` 的 `unwrap_video` 函数。
    pub fn unwrap_video(self) -> VideoFrame {
        match self {
            MediaFrame::Video(f) => f,
            _ => panic!("expected Video frame"),
        }
    }
}

/// Frame for `Audio`.
/// `Audio` 的帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub timestamp: RtmpTimestamp,
    pub format: AudioFormat,
    pub sample_rate: AudioSampleRate,
    pub is_8bit_sample: bool,
    pub is_stereo: bool,
    pub is_aac_sequence_header: bool,
    pub data: Vec<u8>,
}

impl AudioFrame {
    pub const AAC_SAMPLE_RATE: AudioSampleRate = AudioSampleRate::Khz44;
    pub const AAC_STEREO: bool = true;
}

/// Frame for `Video`.
/// `Video` 的帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub timestamp: RtmpTimestamp,
    pub composition_timestamp_offset: RtmpTimestampDelta,
    pub frame_type: VideoFrameType,
    pub codec: VideoCodec,
    pub avc_packet_type: Option<AvcPacketType>,
    pub data: Vec<u8>,
}

impl VideoFrame {
    /// Returns `true` if `keyframe` is true.
    /// 当 `keyframe` 为真时返回 `true`。
    pub fn is_keyframe(&self) -> bool {
        self.frame_type == VideoFrameType::KeyFrame
    }
}

/// Type of `Avc Packet`.
/// `Avc Packet` 的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AvcPacketType {
    SequenceHeader = 0,
    NalUnit = 1,
    EndOfSequence = 2,
}

/// Type of `Video Frame`.
/// `Video Frame` 的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VideoFrameType {
    KeyFrame = 1,
    InterFrame = 2,
    DisposableInterFrame = 3,
    GeneratedKeyFrame = 4,
    VideoInfoOrCommandFrame = 5,
}

/// `VideoCodec` enumeration.
/// `VideoCodec` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VideoCodec {
    Jpeg,
    H263,
    ScreenVideo,
    Vp6,
    Vp6WithAlpha,
    ScreenVideoV2,
    Avc,
    Unknown(u8),
}

impl VideoCodec {
    /// `raw_id` function of `VideoCodec`.
    /// `VideoCodec` 的 `raw_id` 函数。
    pub fn raw_id(self) -> u8 {
        match self {
            VideoCodec::Jpeg => 1,
            VideoCodec::H263 => 2,
            VideoCodec::ScreenVideo => 3,
            VideoCodec::Vp6 => 4,
            VideoCodec::Vp6WithAlpha => 5,
            VideoCodec::ScreenVideoV2 => 6,
            VideoCodec::Avc => 7,
            VideoCodec::Unknown(id) => id,
        }
    }
}

/// `AudioFormat` enumeration.
/// `AudioFormat` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AudioFormat {
    Adpcm,
    Mp3,
    LinearPcmLittleEndian,
    Nellymoser16khzMono,
    Nellymoser8KhzMono,
    Nellymoser,
    G711AlawLogarithmicPcm,
    G711MuLawLogarithmicPcm,
    Aac,
    Speex,
    Mp3_8khz,
    DeviceSpecificSound,
    Unknown(u8),
}

impl AudioFormat {
    /// `raw_id` function of `AudioFormat`.
    /// `AudioFormat` 的 `raw_id` 函数。
    pub fn raw_id(self) -> u8 {
        match self {
            AudioFormat::Adpcm => 1,
            AudioFormat::Mp3 => 2,
            AudioFormat::LinearPcmLittleEndian => 3,
            AudioFormat::Nellymoser16khzMono => 4,
            AudioFormat::Nellymoser8KhzMono => 5,
            AudioFormat::Nellymoser => 6,
            AudioFormat::G711AlawLogarithmicPcm => 7,
            AudioFormat::G711MuLawLogarithmicPcm => 8,
            AudioFormat::Aac => 10,
            AudioFormat::Speex => 11,
            AudioFormat::Mp3_8khz => 14,
            AudioFormat::DeviceSpecificSound => 15,
            AudioFormat::Unknown(id) => id,
        }
    }
}

/// `AudioSampleRate` enumeration.
/// `AudioSampleRate` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AudioSampleRate {
    Khz5 = 0,
    Khz11 = 1,
    Khz22 = 2,
    Khz44 = 3,
}

/// Header for `Avc Sequence`.
/// `Avc Sequence` 的头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcSequenceHeader {
    pub avc_profile_indication: u8,
    pub profile_compatibility: u8,
    pub avc_level_indication: u8,
    pub length_size_minus_one: u8,
    pub sps_list: Vec<Vec<u8>>,
    pub pps_list: Vec<Vec<u8>>,
}

impl AvcSequenceHeader {
    const CONFIGURATION_VERSION: u8 = 1;
    const MAX_SPS_COUNT: usize = 31;
    const MAX_PPS_COUNT: usize = 255;
    const MAX_SPS_SIZE: usize = 4096;
    const MAX_PPS_SIZE: usize = 4096;

    /// Creates `bytes` from input.
    /// 从输入创建 `bytes`。
    pub fn from_bytes(data: &[u8]) -> Result<Self, Error> {
        if data.len() < 7 {
            return Err(Error::invalid_data(
                "AVCDecoderConfigurationRecord too short (expected at least 7 bytes)",
            ));
        }

        let configuration_version = data[0];
        if configuration_version != Self::CONFIGURATION_VERSION {
            return Err(Error::unsupported(format!(
                "unsupported AVC configuration version: {} (expected {})",
                configuration_version,
                Self::CONFIGURATION_VERSION
            )));
        }

        let avc_profile_indication = data[1];
        let profile_compatibility = data[2];
        let avc_level_indication = data[3];
        let length_size_minus_one = data[4] & 0x03;

        let mut offset = 5;
        let mut sps_list = Vec::new();
        let mut pps_list = Vec::new();

        if offset >= data.len() {
            return Err(Error::invalid_data("incomplete SPS count field (offset 5)"));
        }

        let num_sps = (data[offset] & 0x1F) as usize;
        if num_sps > Self::MAX_SPS_COUNT {
            return Err(Error::invalid_data(format!(
                "SPS count exceeds maximum ({} > {})",
                num_sps,
                Self::MAX_SPS_COUNT
            )));
        }
        offset += 1;

        if num_sps == 0 {
            return Err(Error::invalid_data("SPS list must not be empty"));
        }

        for i in 0..num_sps {
            if offset + 2 > data.len() {
                return Err(Error::invalid_data(format!(
                    "incomplete SPS length field at index {}: need 2 bytes, have {}",
                    i,
                    data.len() - offset
                )));
            }

            let sps_length = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;

            if sps_length > Self::MAX_SPS_SIZE {
                return Err(Error::invalid_data(format!(
                    "SPS size exceeds maximum at index {}: {} > {}",
                    i,
                    sps_length,
                    Self::MAX_SPS_SIZE
                )));
            }

            if offset + sps_length > data.len() {
                return Err(Error::invalid_data(format!(
                    "incomplete SPS data at index {}: need {} bytes, have {}",
                    i,
                    sps_length,
                    data.len() - offset
                )));
            }

            sps_list.push(data[offset..offset + sps_length].to_vec());
            offset += sps_length;
        }

        if offset >= data.len() {
            return Err(Error::invalid_data("incomplete PPS count field"));
        }

        let num_pps = data[offset] as usize;
        if num_pps > Self::MAX_PPS_COUNT {
            return Err(Error::invalid_data(format!(
                "PPS count exceeds maximum ({} > {})",
                num_pps,
                Self::MAX_PPS_COUNT
            )));
        }
        offset += 1;

        if num_pps == 0 {
            return Err(Error::invalid_data("PPS list must not be empty"));
        }

        for i in 0..num_pps {
            if offset + 2 > data.len() {
                return Err(Error::invalid_data(format!(
                    "incomplete PPS length field at index {}: need 2 bytes, have {}",
                    i,
                    data.len() - offset
                )));
            }

            let pps_length = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;

            if pps_length > Self::MAX_PPS_SIZE {
                return Err(Error::invalid_data(format!(
                    "PPS size exceeds maximum at index {}: {} > {}",
                    i,
                    pps_length,
                    Self::MAX_PPS_SIZE
                )));
            }

            if offset + pps_length > data.len() {
                return Err(Error::invalid_data(format!(
                    "incomplete PPS data at index {}: need {} bytes, have {}",
                    i,
                    pps_length,
                    data.len() - offset
                )));
            }

            pps_list.push(data[offset..offset + pps_length].to_vec());
            offset += pps_length;
        }

        Ok(Self {
            avc_profile_indication,
            profile_compatibility,
            avc_level_indication,
            length_size_minus_one,
            sps_list,
            pps_list,
        })
    }

    /// Converts to `bytes` representation.
    /// 转换为 `bytes` 表示。
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        if self.sps_list.is_empty() {
            return Err(Error::invalid_data("SPS list must not be empty"));
        }

        if self.pps_list.is_empty() {
            return Err(Error::invalid_data("PPS list must not be empty"));
        }

        if self.sps_list.len() > Self::MAX_SPS_COUNT {
            return Err(Error::invalid_data(format!(
                "too many SPS entries: {} (max {})",
                self.sps_list.len(),
                Self::MAX_SPS_COUNT
            )));
        }

        if self.pps_list.len() > Self::MAX_PPS_COUNT {
            return Err(Error::invalid_data(format!(
                "too many PPS entries: {} (max {})",
                self.pps_list.len(),
                Self::MAX_PPS_COUNT
            )));
        }

        for (i, sps) in self.sps_list.iter().enumerate() {
            if sps.len() > Self::MAX_SPS_SIZE {
                return Err(Error::invalid_data(format!(
                    "SPS size exceeds maximum at index {}: {} > {}",
                    i,
                    sps.len(),
                    Self::MAX_SPS_SIZE
                )));
            }
        }

        for (i, pps) in self.pps_list.iter().enumerate() {
            if pps.len() > Self::MAX_PPS_SIZE {
                return Err(Error::invalid_data(format!(
                    "PPS size exceeds maximum at index {}: {} > {}",
                    i,
                    pps.len(),
                    Self::MAX_PPS_SIZE
                )));
            }
        }

        let mut result = vec![
            Self::CONFIGURATION_VERSION,
            self.avc_profile_indication,
            self.profile_compatibility,
            self.avc_level_indication,
            0xFC | (self.length_size_minus_one & 0x03),
        ];

        result.push(0xE0 | (self.sps_list.len() as u8));
        for sps in &self.sps_list {
            result.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            result.extend_from_slice(sps);
        }

        result.push(self.pps_list.len() as u8);
        for pps in &self.pps_list {
            result.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            result.extend_from_slice(pps);
        }

        Ok(result)
    }
}

/// Encodes `audio frame` into the output buffer.
/// 将 `audio frame` 编码到输出缓冲区。
pub fn encode_audio_frame(buf: &mut Vec<u8>, frame: &AudioFrame) {
    let header = ((frame.format.raw_id()) << 4)
        | ((frame.sample_rate as u8) << 2)
        | ((frame.is_8bit_sample as u8) << 1)
        | (frame.is_stereo as u8);

    buf.push(header);

    if frame.format == AudioFormat::Aac {
        buf.push(if frame.is_aac_sequence_header { 0 } else { 1 });
    }

    buf.extend_from_slice(&frame.data);
}

/// Decodes `audio frame` from the input buffer.
/// 从输入缓冲区解码 `audio frame`。
pub fn decode_audio_frame(buf: &[u8], timestamp: RtmpTimestamp) -> Result<AudioFrame, Error> {
    let mut reader = buf;
    let header = read_u8(&mut reader)?;

    let format_bits = (header >> 4) & 0x0F;
    let format = match format_bits {
        1 => AudioFormat::Adpcm,
        2 => AudioFormat::Mp3,
        3 => AudioFormat::LinearPcmLittleEndian,
        4 => AudioFormat::Nellymoser16khzMono,
        5 => AudioFormat::Nellymoser8KhzMono,
        6 => AudioFormat::Nellymoser,
        7 => AudioFormat::G711AlawLogarithmicPcm,
        8 => AudioFormat::G711MuLawLogarithmicPcm,
        10 => AudioFormat::Aac,
        11 => AudioFormat::Speex,
        14 => AudioFormat::Mp3_8khz,
        15 => AudioFormat::DeviceSpecificSound,
        _ => AudioFormat::Unknown(format_bits),
    };

    let sample_rate_bits = (header >> 2) & 0x03;
    let sample_rate = match sample_rate_bits {
        0 => AudioSampleRate::Khz5,
        1 => AudioSampleRate::Khz11,
        2 => AudioSampleRate::Khz22,
        3 => AudioSampleRate::Khz44,
        _ => unreachable!(),
    };

    let is_8bit_sample = (header & 0x02) != 0;
    let is_stereo = (header & 0x01) != 0;
    let is_aac_sequence_header = if format == AudioFormat::Aac {
        let aac_packet_type = read_u8(&mut reader)?;
        aac_packet_type == 0
    } else {
        false
    };

    Ok(AudioFrame {
        timestamp,
        format,
        sample_rate,
        is_8bit_sample,
        is_stereo,
        is_aac_sequence_header,
        data: reader.to_vec(),
    })
}

/// Encodes `video frame` into the output buffer.
/// 将 `video frame` 编码到输出缓冲区。
pub fn encode_video_frame(buf: &mut Vec<u8>, frame: &VideoFrame) {
    let frame_type_codec = ((frame.frame_type as u8) << 4) | (frame.codec.raw_id());
    buf.push(frame_type_codec);

    if let Some(avc_packet_type) = frame.avc_packet_type {
        buf.push(avc_packet_type as u8);
        write_i24(buf, frame.composition_timestamp_offset.as_millis());
    }

    buf.extend_from_slice(&frame.data);
}

/// Decodes `video frame` from the input buffer.
/// 从输入缓冲区解码 `video frame`。
pub fn decode_video_frame(buf: &[u8], timestamp: RtmpTimestamp) -> Result<VideoFrame, Error> {
    let mut reader = buf;
    let frame_type_codec = read_u8(&mut reader)?;

    let frame_type_bits = (frame_type_codec >> 4) & 0x0F;
    let frame_type = match frame_type_bits {
        1 => VideoFrameType::KeyFrame,
        2 => VideoFrameType::InterFrame,
        3 => VideoFrameType::DisposableInterFrame,
        4 => VideoFrameType::GeneratedKeyFrame,
        5 => VideoFrameType::VideoInfoOrCommandFrame,
        _ => {
            return Err(Error::invalid_data(format!(
                "Invalid video frame type: {}",
                frame_type_bits
            )));
        }
    };

    let codec_bits = frame_type_codec & 0x0F;
    let codec = match codec_bits {
        1 => VideoCodec::Jpeg,
        2 => VideoCodec::H263,
        3 => VideoCodec::ScreenVideo,
        4 => VideoCodec::Vp6,
        5 => VideoCodec::Vp6WithAlpha,
        6 => VideoCodec::ScreenVideoV2,
        7 => VideoCodec::Avc,
        _ => VideoCodec::Unknown(codec_bits),
    };

    let (avc_packet_type, composition_timestamp_offset) = if codec == VideoCodec::Avc
        && frame_type != VideoFrameType::VideoInfoOrCommandFrame
    {
        let avc_packet_type_byte = read_u8(&mut reader)?;
        let avc_packet_type = match avc_packet_type_byte {
            0 => AvcPacketType::SequenceHeader,
            1 => AvcPacketType::NalUnit,
            2 => AvcPacketType::EndOfSequence,
            _ => {
                return Err(Error::invalid_data(format!(
                    "Invalid AVC packet type: {}",
                    avc_packet_type_byte
                )));
            }
        };
        let composition_timestamp_offset = RtmpTimestampDelta::from_millis(read_i24(&mut reader)?);
        (Some(avc_packet_type), composition_timestamp_offset)
    } else {
        (None, RtmpTimestampDelta::ZERO)
    };

    Ok(VideoFrame {
        timestamp,
        composition_timestamp_offset,
        frame_type,
        codec,
        avc_packet_type,
        data: reader.to_vec(),
    })
}

/// Header for `Video Ingress`.
/// `Video Ingress` 的头。
#[derive(Debug, Clone, Copy)]
pub struct VideoIngressHeader {
    pub frame_type: u8,
    pub codec: CodecId,
    pub packet_type: u8,
    pub cts: i32,
    pub payload_offset: usize,
}

/// Enhanced RTMP multi-track packet type (packet_type = 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiTrackType {
    OneTrack,
    ManyTracks,
    ManyTracksManyCodecs,
}

/// A single track entry within a multi-track packet.
#[derive(Debug, Clone)]
pub struct MultiTrackEntry {
    pub track_id: u8,
    pub codec: CodecId,
    pub packet_type: u8,
    pub data_offset: usize,
    pub data_len: usize,
}

/// Parsed multi-track video header.
#[derive(Debug, Clone)]
pub struct VideoMultiTrackHeader {
    pub frame_type: u8,
    pub multi_track_type: MultiTrackType,
    pub tracks: Vec<MultiTrackEntry>,
}

/// Attempts to parse an Enhanced RTMP multi-track video packet (packet_type = 5).
/// Returns `None` if the payload is not a multi-track packet.
pub fn parse_video_multi_track(payload: &[u8]) -> Option<VideoMultiTrackHeader> {
    if payload.len() < 6 {
        return None;
    }
    let is_enhanced = ((payload[0] >> 4) & 0b1000) != 0;
    if !is_enhanced {
        return None;
    }
    let frame_type = (payload[0] >> 4) & 0b0111;
    let packet_type = payload[0] & 0x0f;
    if packet_type != 5 {
        return None;
    }

    let fourcc = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
    let base_codec = codec_from_rtmp_fourcc(fourcc).unwrap_or(CodecId::Unknown);
    let multi_track_type_byte = payload[5];
    let multi_track_type = match multi_track_type_byte {
        0 => MultiTrackType::OneTrack,
        1 => MultiTrackType::ManyTracks,
        2 => MultiTrackType::ManyTracksManyCodecs,
        _ => return None,
    };

    let mut tracks = Vec::new();
    let mut pos = 6;

    match multi_track_type {
        MultiTrackType::OneTrack => {
            // Single track: rest of payload is the track data
            tracks.push(MultiTrackEntry {
                track_id: 0,
                codec: base_codec,
                packet_type: 1, // assume CodedFrames
                data_offset: pos,
                data_len: payload.len().saturating_sub(pos),
            });
        }
        MultiTrackType::ManyTracks => {
            // Multiple tracks, same codec (from FourCC in header)
            while pos + 5 <= payload.len() {
                let track_id = payload[pos];
                let track_pkt_type = payload[pos + 1];
                let track_len =
                    u32::from_be_bytes([0, payload[pos + 2], payload[pos + 3], payload[pos + 4]])
                        as usize;
                pos += 5;
                if pos + track_len > payload.len() {
                    break;
                }
                tracks.push(MultiTrackEntry {
                    track_id,
                    codec: base_codec,
                    packet_type: track_pkt_type,
                    data_offset: pos,
                    data_len: track_len,
                });
                pos += track_len;
            }
        }
        MultiTrackType::ManyTracksManyCodecs => {
            // Multiple tracks, each with its own FourCC
            while pos + 9 <= payload.len() {
                let track_id = payload[pos];
                let track_fourcc = u32::from_be_bytes([
                    payload[pos + 1],
                    payload[pos + 2],
                    payload[pos + 3],
                    payload[pos + 4],
                ]);
                let track_codec = codec_from_rtmp_fourcc(track_fourcc).unwrap_or(CodecId::Unknown);
                let track_pkt_type = payload[pos + 5];
                let track_len =
                    u32::from_be_bytes([0, payload[pos + 6], payload[pos + 7], payload[pos + 8]])
                        as usize;
                pos += 9;
                if pos + track_len > payload.len() {
                    break;
                }
                tracks.push(MultiTrackEntry {
                    track_id,
                    codec: track_codec,
                    packet_type: track_pkt_type,
                    data_offset: pos,
                    data_len: track_len,
                });
                pos += track_len;
            }
        }
    }

    Some(VideoMultiTrackHeader {
        frame_type,
        multi_track_type,
        tracks,
    })
}

/// Parses `video ingress header` from input.
/// 从输入解析 `video ingress header`。
pub fn parse_video_ingress_header(payload: &[u8]) -> Option<VideoIngressHeader> {
    parse_video_ingress_header_with_mode(payload, DomesticCodecMode::Standard)
}

/// Parses a video ingress header with domestic codec ID mode awareness.
///
/// In `Domestic` or `Auto` mode, legacy codec ID 14 is interpreted as VP8
/// and 15 as VP9 (ZLMediaKit convention). In `Standard` mode, 14 is H266.
pub fn parse_video_ingress_header_with_mode(
    payload: &[u8],
    mode: DomesticCodecMode,
) -> Option<VideoIngressHeader> {
    if payload.len() < 2 {
        return None;
    }

    let is_enhanced = ((payload[0] >> 4) & 0b1000) != 0;
    if is_enhanced {
        if payload.len() < 5 {
            return None;
        }
        let frame_type = (payload[0] >> 4) & 0b0111;
        let packet_type = payload[0] & 0x0f;
        let fourcc = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
        let codec = codec_from_rtmp_fourcc(fourcc).unwrap_or(CodecId::Unknown);
        let (cts, payload_offset) = match packet_type {
            0 => (0, 5),
            1 => {
                if should_parse_enhanced_cts(codec, payload) {
                    (signed_i24(&payload[5..8]), 8)
                } else {
                    (0, 5)
                }
            }
            3 => (0, 5),
            5 => {
                // Multi-track packet — use parse_video_multi_track() for full parsing.
                // Return a minimal header so callers can detect it.
                return Some(VideoIngressHeader {
                    frame_type,
                    codec,
                    packet_type: 5,
                    cts: 0,
                    payload_offset: 5,
                });
            }
            _ if matches!(
                codec,
                CodecId::AV1 | CodecId::VP8 | CodecId::VP9 | CodecId::Unknown
            ) =>
            {
                (0, 5)
            }
            _ => return None,
        };
        return Some(VideoIngressHeader {
            frame_type,
            codec,
            packet_type,
            cts,
            payload_offset,
        });
    }

    let frame_type = payload[0] >> 4;
    let codec_id = payload[0] & 0x0f;
    let codec = codec_from_rtmp_codec_id_with_mode(MediaKind::Video, codec_id, mode)
        .unwrap_or(CodecId::Unknown);
    let packet_type = payload[1];
    let (cts, payload_offset) = match packet_type {
        0 => {
            if payload.len() < 5 {
                return None;
            }
            (0, 5)
        }
        1 => {
            if payload.len() < 5 {
                return None;
            }
            (signed_i24(&payload[2..5]), 5)
        }
        3 => (0, 2),
        _ if codec == CodecId::Unknown => (0, 2),
        _ => return None,
    };
    Some(VideoIngressHeader {
        frame_type,
        codec,
        packet_type,
        cts,
        payload_offset,
    })
}

fn should_parse_enhanced_cts(codec: CodecId, payload: &[u8]) -> bool {
    if payload.len() < 8 {
        return false;
    }
    match codec {
        CodecId::H264 | CodecId::H265 | CodecId::H266 => true,
        CodecId::VP9 => {
            let payload_with_cts = &payload[8..];
            let payload_without_cts = &payload[5..];
            let with_cts_looks_valid = looks_like_video_payload(codec, payload_with_cts);
            let without_cts_looks_valid = looks_like_video_payload(codec, payload_without_cts);
            with_cts_looks_valid && !without_cts_looks_valid
        }
        CodecId::AV1 => {
            let payload_with_cts = &payload[8..];
            let payload_without_cts = &payload[5..];
            let with_cts_looks_valid = looks_like_video_payload(codec, payload_with_cts);
            let without_cts_looks_valid = looks_like_video_payload(codec, payload_without_cts);
            with_cts_looks_valid && !without_cts_looks_valid
        }
        CodecId::VP8 => {
            let payload_with_cts = &payload[8..];
            let payload_without_cts = &payload[5..];
            let with_cts_looks_valid = looks_like_video_payload(codec, payload_with_cts);
            let without_cts_looks_valid = looks_like_video_payload(codec, payload_without_cts);
            with_cts_looks_valid || !without_cts_looks_valid
        }
        _ => false,
    }
}

fn looks_like_video_payload(codec: CodecId, payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    match codec {
        CodecId::AV1 => looks_like_av1_payload(payload),
        CodecId::VP8 => looks_like_vp8_payload(payload),
        CodecId::VP9 => looks_like_vp9_payload(payload),
        CodecId::H265 | CodecId::H266 => true,
        _ => false,
    }
}

fn looks_like_av1_payload(payload: &[u8]) -> bool {
    let header = payload[0];
    if (header & 0x80) != 0 || (header & 0x01) != 0 {
        return false;
    }
    let obu_type = (header >> 3) & 0x0f;
    if obu_type == 0 {
        return false;
    }

    let has_extension = (header & 0x04) != 0;
    let has_size_field = (header & 0x02) != 0;
    let mut pos = 1usize;
    if has_extension {
        if payload.len() <= pos {
            return false;
        }
        pos += 1;
    }
    if !has_size_field {
        return true;
    }
    let Some((size, size_len)) = parse_leb128(&payload[pos..]) else {
        return false;
    };
    let remaining = payload.len().saturating_sub(pos + size_len);
    size <= remaining
}

fn looks_like_vp8_payload(payload: &[u8]) -> bool {
    if payload.len() < 3 {
        return false;
    }
    let frame_tag = payload[0];
    let frame_type = frame_tag & 0x01;
    let version = (frame_tag >> 1) & 0x07;
    if version > 3 {
        return false;
    }
    if frame_type == 0 {
        if payload.len() < 6 {
            return false;
        }
        return payload[3] == 0x9d && payload[4] == 0x01 && payload[5] == 0x2a;
    }
    true
}

fn looks_like_vp9_payload(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    let header = payload[0];
    if (header >> 6) != 0b10 {
        return false;
    }
    let profile_low = (header >> 5) & 1;
    if profile_low == 0 {
        if (header & 0x10) != 0 {
            return false;
        }
    } else {
        let profile_high = (header >> 4) & 1;
        if profile_high == 1 && (header & 0x08) != 0 {
            return false;
        }
    }
    true
}

fn parse_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    let mut shift = 0usize;
    for (idx, byte) in data.iter().copied().take(8).enumerate() {
        value |= ((byte & 0x7f) as usize) << shift;
        if (byte & 0x80) == 0 {
            return Some((value, idx + 1));
        }
        shift += 7;
    }
    None
}

fn signed_i24(v: &[u8]) -> i32 {
    let raw = ((v[0] as i32) << 16) | ((v[1] as i32) << 8) | v[2] as i32;
    if (raw & 0x80_0000) != 0 {
        raw | !0xFF_FFFF
    } else {
        raw
    }
}

fn read_u8(reader: &mut &[u8]) -> Result<u8, Error> {
    check_len(1, reader.len())?;
    let v = reader[0];
    *reader = &reader[1..];
    Ok(v)
}

fn read_i24(reader: &mut &[u8]) -> Result<i32, Error> {
    check_len(3, reader.len())?;
    let bytes = [
        if reader[0] & 0x80 != 0 { 0xFF } else { 0x00 },
        reader[0],
        reader[1],
        reader[2],
    ];
    *reader = &reader[3..];
    Ok(i32::from_be_bytes(bytes))
}

fn write_i24(buf: &mut Vec<u8>, v: i32) {
    debug_assert!(
        (-0x80_0000..0x80_0000).contains(&v),
        "i24 value out of range: {v}"
    );
    let bytes = v.to_be_bytes();
    buf.extend_from_slice(&bytes[1..4]);
}

fn check_len(required: usize, actual: usize) -> Result<(), Error> {
    if actual < required {
        Err(Error::invalid_data(format!(
            "insufficient buffer: require {}, got {}",
            required, actual
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avc_sequence_header_roundtrip() {
        let header = AvcSequenceHeader {
            avc_profile_indication: 0x42,
            profile_compatibility: 0xC0,
            avc_level_indication: 0x1F,
            length_size_minus_one: 3,
            sps_list: vec![vec![0x01, 0x02, 0x03]],
            pps_list: vec![vec![0x04, 0x05]],
        };

        let bytes = header.to_bytes().expect("to_bytes failed");
        let parsed = AvcSequenceHeader::from_bytes(&bytes).expect("from_bytes failed");
        assert_eq!(header, parsed);
    }

    #[test]
    fn audio_roundtrip() {
        let frame = AudioFrame {
            timestamp: RtmpTimestamp::from_millis(10),
            format: AudioFormat::Aac,
            sample_rate: AudioSampleRate::Khz44,
            is_8bit_sample: false,
            is_stereo: true,
            is_aac_sequence_header: false,
            data: vec![0x11, 0x22, 0x33],
        };
        let mut out = Vec::new();
        encode_audio_frame(&mut out, &frame);
        let decoded = decode_audio_frame(&out, frame.timestamp).expect("decode");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn video_roundtrip() {
        let frame = VideoFrame {
            timestamp: RtmpTimestamp::from_millis(42),
            composition_timestamp_offset: RtmpTimestampDelta::from_millis(-2),
            frame_type: VideoFrameType::KeyFrame,
            codec: VideoCodec::Avc,
            avc_packet_type: Some(AvcPacketType::NalUnit),
            data: vec![1, 2, 3, 4],
        };
        let mut out = Vec::new();
        encode_video_frame(&mut out, &frame);
        let decoded = decode_video_frame(&out, frame.timestamp).expect("decode");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn enhanced_av1_ambiguous_coded_frame_prefers_no_cts() {
        let fourcc = u32::from_be_bytes(*b"av01").to_be_bytes();
        let payload = [
            0x91, fourcc[0], fourcc[1], fourcc[2], fourcc[3], 0x12,
            0x00, // no-CTS offset: temporal delimiter OBU.
            0x32, 0x0a, // no-CTS offset: frame OBU declaring 10 bytes.
            0x01, 0x00, 0x12, 0x00, 0x32, 0x01, 0x00, 0x12, 0x00, 0x12,
        ];

        let header = parse_video_ingress_header(&payload).expect("av1 ingress header");

        assert_eq!(header.codec, CodecId::AV1);
        assert_eq!(header.packet_type, 1);
        assert_eq!(header.payload_offset, 5);
        assert_eq!(header.cts, 0);
    }

    #[test]
    fn unknown_legacy_codec_id_produces_unknown_codec() {
        // codec_id=9 is not assigned to any known codec
        // frame_type=1, codec_id=9, packet_type=1, cts=0, then payload
        let payload = [0x19, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB];
        let header = parse_video_ingress_header(&payload).expect("unknown codec header");
        assert_eq!(header.codec, CodecId::Unknown);
        assert_eq!(header.frame_type, 1);
        assert_eq!(header.packet_type, 1);
        assert_eq!(header.payload_offset, 5);
    }

    #[test]
    fn domestic_mode_resolves_codec_id_14_as_vp8() {
        // Legacy codec_id=14, frame_type=1 (keyframe), packet_type=1
        let payload = [0x1E, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB];
        let header = parse_video_ingress_header_with_mode(&payload, DomesticCodecMode::Domestic)
            .expect("domestic vp8 header");
        assert_eq!(header.codec, CodecId::VP8);
    }

    #[test]
    fn standard_mode_resolves_codec_id_14_as_h266() {
        let payload = [0x1E, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB];
        let header = parse_video_ingress_header_with_mode(&payload, DomesticCodecMode::Standard)
            .expect("standard h266 header");
        assert_eq!(header.codec, CodecId::H266);
    }

    #[test]
    fn domestic_mode_resolves_codec_id_15_as_vp9() {
        let payload = [0x1F, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB];
        let header = parse_video_ingress_header_with_mode(&payload, DomesticCodecMode::Domestic)
            .expect("domestic vp9 header");
        assert_eq!(header.codec, CodecId::VP9);
    }

    #[test]
    fn multi_track_packet_type_5_detected_by_ingress_header() {
        let fourcc = u32::from_be_bytes(*b"avc1").to_be_bytes();
        // Enhanced: frame_type=1, packet_type=5
        let payload = [
            0x95, fourcc[0], fourcc[1], fourcc[2], fourcc[3], 0x00, // multiTrackType=OneTrack
            0xAA, 0xBB,
        ];
        let header = parse_video_ingress_header(&payload).expect("multi-track header");
        assert_eq!(header.packet_type, 5);
        assert_eq!(header.codec, CodecId::H264);
    }

    #[test]
    fn parse_multi_track_one_track() {
        let fourcc = u32::from_be_bytes(*b"hvc1").to_be_bytes();
        let payload = [
            0x95, fourcc[0], fourcc[1], fourcc[2], fourcc[3], 0x00, // OneTrack
            0x01, 0x02, 0x03, // track data
        ];
        let mt = parse_video_multi_track(&payload).expect("multi-track");
        assert_eq!(mt.multi_track_type, MultiTrackType::OneTrack);
        assert_eq!(mt.tracks.len(), 1);
        assert_eq!(mt.tracks[0].codec, CodecId::H265);
        assert_eq!(mt.tracks[0].data_len, 3);
    }

    #[test]
    fn non_multi_track_returns_none() {
        let fourcc = u32::from_be_bytes(*b"avc1").to_be_bytes();
        // packet_type=1 (CodedFrames), not multi-track
        let payload = [0x91, fourcc[0], fourcc[1], fourcc[2], fourcc[3], 0xAA];
        assert!(parse_video_multi_track(&payload).is_none());
    }
}
