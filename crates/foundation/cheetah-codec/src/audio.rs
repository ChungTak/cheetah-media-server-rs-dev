use crate::prelude::*;
use bytes::Bytes;

/// Layout of audio samples in a multi-channel frame.
///
/// `Interleaved` stores samples from all channels in one contiguous stream, while
/// `Planar` keeps each channel in its own buffer. The codec layer uses this to
/// decide whether `payload` can be interpreted as a single slice or must be split.
///
/// 多声道帧中音频样本的排布方式。
///
/// `Interleaved` 将所有声道样本连续存放；`Planar` 每个声道独立存放。
/// codec 层据此判断 `payload` 能否作为单一切片处理还是必须拆分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleLayout {
    Interleaved,
    Planar,
}

/// Basic audio characteristics used by the codec layer and protocol modules.
///
/// `sample_rate` is the decoded sample rate in Hz, `channels` is the channel count,
/// and `samples_per_frame` is the expected number of samples per codec frame.
///
/// codec 层和协议模块使用的基本音频特征。
///
/// `sample_rate` 为解码后的采样率（Hz），`channels` 为声道数，
/// `samples_per_frame` 为每个编解码器帧的预期样本数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub channels: u8,
    pub samples_per_frame: u16,
}

/// MPEG-4 Audio Specific Config (ASC) for AAC.
///
/// Encapsulates the audio object type, sampling frequency index and channel
/// configuration, which are the minimum fields needed to parse or build an AAC
/// elementary stream.
///
/// AAC 的 MPEG-4 Audio Specific Config（ASC）。
///
/// 封装音频对象类型、采样频率索引和声道配置，是解析或构建 AAC 基本流所需的
/// 最小字段集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AacAudioSpecificConfig {
    pub audio_object_type: u8,
    pub sampling_frequency_index: u8,
    pub channel_configuration: u8,
}

impl AacAudioSpecificConfig {
    /// Parse the first 2 bytes of an AudioSpecificConfig.
    ///
    /// The 2-byte layout is the most common configuration header; it does not
    /// support extension sampling frequencies or `channel_configuration == 0`.
    ///
    /// 解析 AudioSpecificConfig 的前 2 字节。
    ///
    /// 2 字节布局是最常见的配置头；不支持扩展采样频率和 `channel_configuration == 0`。
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let b0 = bytes[0];
        let b1 = bytes[1];
        let audio_object_type = (b0 >> 3) & 0x1f;
        let sampling_frequency_index = ((b0 & 0x07) << 1) | (b1 >> 7);
        let channel_configuration = (b1 >> 3) & 0x0f;
        Some(Self {
            audio_object_type,
            sampling_frequency_index,
            channel_configuration,
        })
    }

    /// Serialize the ASC into the canonical 2-byte header.
    ///
    /// 将 ASC 序列化为标准的 2 字节头。
    pub fn to_bytes(self) -> [u8; 2] {
        let b0 = (self.audio_object_type << 3) | ((self.sampling_frequency_index >> 1) & 0x07);
        let b1 = ((self.sampling_frequency_index & 0x01) << 7)
            | ((self.channel_configuration & 0x0f) << 3);
        [b0, b1]
    }
}

/// Convert an AAC `channel_configuration` value to a channel count.
///
/// Values follow ISO/IEC 14496-3: 1=mono, 2=stereo, 3-7=3.0-7.1, 11=6.1,
/// 12/14=7.1 variants. Unsupported values return `None`.
///
/// 将 AAC `channel_configuration` 值转换为声道数。
///
/// 值遵循 ISO/IEC 14496-3：1=单声道、2=立体声、3-7=3.0-7.1、11=6.1、
/// 12/14=7.1 变体。不支持的值返回 `None`。
pub fn aac_channel_count_from_config(channel_configuration: u8) -> Option<u8> {
    match channel_configuration {
        1 => Some(1),
        2 => Some(2),
        3 => Some(3),
        4 => Some(4),
        5 => Some(5),
        6 => Some(6),
        7 => Some(8),
        11 => Some(7),
        12 | 14 => Some(8),
        _ => None,
    }
}

/// Extract channel count from a full MPEG-4 AudioSpecificConfig bitstream.
///
/// Reads the audio object type, sampling frequency, and channel configuration. If the
/// value is 0 or uses a `channel_configuration` not in the simple table, it falls back
/// to the Program Config Element (PCE) parsing for explicit speaker layouts.
///
/// 从完整 MPEG-4 AudioSpecificConfig 比特流中提取声道数。
///
/// 读取音频对象类型、采样频率和声道配置。若值为 0 或 `channel_configuration` 不在
/// 简单表中，则回退到 Program Config Element（PCE）解析显式扬声器布局。
pub fn aac_channel_count_from_asc(bytes: &[u8]) -> Option<u8> {
    let mut reader = BitReader::new(bytes);
    let mut audio_object_type = read_aac_audio_object_type(&mut reader)?;
    let sampling_frequency_index = reader.read_bits(4)? as u8;
    if sampling_frequency_index == 15 {
        reader.skip_bits(24)?;
    }
    let channel_configuration = reader.read_bits(4)? as u8;
    if let Some(channels) = aac_channel_count_from_config(channel_configuration) {
        return Some(channels);
    }

    if matches!(audio_object_type, 5 | 29) {
        let extension_sampling_frequency_index = reader.read_bits(4)? as u8;
        if extension_sampling_frequency_index == 15 {
            reader.skip_bits(24)?;
        }
        audio_object_type = read_aac_audio_object_type(&mut reader)?;
        if audio_object_type == 22 {
            let extension_channel_configuration = reader.read_bits(4)? as u8;
            if let Some(channels) = aac_channel_count_from_config(extension_channel_configuration) {
                return Some(channels);
            }
        }
    }

    if channel_configuration != 0 {
        return None;
    }

    parse_ga_specific_config_channel_count(&mut reader, audio_object_type)
}

/// Parsed fields of an AAC ADTS (Audio Data Transport Stream) fixed header.
///
/// ADTS encapsulates raw AAC frames with a 7-byte header that carries the profile,
/// sample rate index, channel configuration and frame length needed for demuxing.
///
/// AAC ADTS 固定头解析字段。
///
/// ADTS 用 7 字节头封装原始 AAC 帧，包含解复用所需的 profile、采样率索引、
/// 声道配置和帧长度。
///
/// Map an AAC `sampling_frequency_index` to a sample rate in Hz.
pub(crate) fn aac_sample_rate(index: u8) -> u32 {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    RATES.get(index as usize).copied().unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdtsHeader {
    pub profile: u8,
    pub sampling_frequency_index: u8,
    pub channel_configuration: u8,
    pub frame_length: u16,
}

impl AdtsHeader {
    /// Parse a 7-byte ADTS fixed header.
    ///
    /// Validates the sync word (`0xFFF`) and extracts the profile, sampling frequency
    /// index, channel configuration and full frame length.
    ///
    /// 解析 7 字节 ADTS 固定头。
    ///
    /// 校验同步字（`0xFFF`）并提取 profile、采样率索引、声道配置和完整帧长度。
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 7 {
            return None;
        }
        if data[0] != 0xff || (data[1] & 0xf0) != 0xf0 {
            return None;
        }
        let profile = (data[2] >> 6) & 0x03;
        let sampling_frequency_index = (data[2] >> 2) & 0x0f;
        let channel_configuration = ((data[2] & 0x01) << 2) | ((data[3] >> 6) & 0x03);
        let frame_length = (u16::from(data[3] & 0x03) << 11)
            | (u16::from(data[4]) << 3)
            | u16::from((data[5] >> 5) & 0x07);
        Some(Self {
            profile,
            sampling_frequency_index,
            channel_configuration,
            frame_length,
        })
    }

    /// Build the 7-byte ADTS fixed header for this frame.
    ///
    /// The channel configuration is split across two bytes and must be saturated to
    /// 3 bits because ADTS can only represent values 0..=7. The `frame_length` field
    /// includes the 7 header bytes plus the raw AAC payload.
    ///
    /// 为当前帧构建 7 字节 ADTS 固定头。
    ///
    /// 声道配置跨两个字节拆分，且必须饱和到 3 位，因为 ADTS 只能表示 0..=7。
    /// `frame_length` 字段包含 7 字节头加原始 AAC 负载。
    pub fn build(self) -> [u8; 7] {
        let mut out = [0u8; 7];
        out[0] = 0xff;
        out[1] = 0xf1;
        // ADTS layout per ISO/IEC 13818-7:
        //   byte 2 bits [7:6] profile, [5:2] sampling_frequency_index, [1] private_bit,
        //                [0] channel_configuration[2] (the MSB of the 3-bit field).
        //   byte 3 bits [7:6] channel_configuration[1:0], [5] original/copy, [4] home,
        //                [3:2] reserved/copyright bits, [1:0] aac_frame_length[12:11].
        // Splitting the ch_cfg field across bytes is the historical wart of ADTS; it's
        // also why the previous `(ch_cfg & 0x07) << 6` was wrong: it dropped the MSB of
        // ch_cfg whenever the value exceeded 3 (so 5.1 = 6 silently became chan_cfg=2).
        // ADTS only reserves 3 bits for ch_cfg (values 0..=7). MPEG-4 ASC values like
        // 11 cannot be expressed; we saturate to 7 (8-channel layout) instead of silently
        // truncating to 3 (`11 & 0x07`), which would have produced a layout mismatch.
        let ch_cfg = self.channel_configuration.min(7);
        out[2] = ((self.profile & 0x03) << 6)
            | ((self.sampling_frequency_index & 0x0f) << 2)
            | ((ch_cfg >> 2) & 0x01);
        out[3] =
            ((ch_cfg & 0x03) << 6) | u8::try_from((self.frame_length >> 11) & 0x03).unwrap_or(0);
        out[4] = u8::try_from((self.frame_length >> 3) & 0xff).unwrap_or(0);
        out[5] = (u8::try_from(self.frame_length & 0x07).unwrap_or(0) << 5) | 0x1f;
        out[6] = 0xfc;
        out
    }
}

/// Wrap a raw AAC frame with an ADTS header derived from an ASC.
///
/// Computes the ADTS profile from the ASC audio object type (`profile = AOT - 1`)
/// and uses the ASC sample rate and channel configuration. The frame length is capped
/// at `u16::MAX` to avoid malformed headers.
///
/// 用从 ASC 派生的 ADTS 头包装原始 AAC 帧。
///
/// ADTS profile 由 ASC 音频对象类型计算（`profile = AOT - 1`），
/// 并使用 ASC 采样率和声道配置。帧长度限制在 `u16::MAX` 以内以避免畸形头。
pub fn adts_wrap(raw_aac: &[u8], asc: AacAudioSpecificConfig) -> Bytes {
    let frame_len = raw_aac.len().saturating_add(7).min(usize::from(u16::MAX)) as u16;
    let header = AdtsHeader {
        profile: asc.audio_object_type.saturating_sub(1) & 0x03,
        sampling_frequency_index: asc.sampling_frequency_index,
        channel_configuration: asc.channel_configuration,
        frame_length: frame_len,
    }
    .build();
    let mut out = Vec::with_capacity(usize::from(frame_len));
    out.extend_from_slice(&header);
    out.extend_from_slice(raw_aac);
    Bytes::from(out)
}

/// Strip the ADTS header from a frame and return the header plus raw payload.
///
/// Verifies that the declared frame length is at least 7 bytes and fits within the
/// provided buffer before returning the payload slice.
///
/// 从帧中剥离 ADTS 头并返回头和原始负载。
///
/// 在返回负载切片前，校验声明的帧长度至少为 7 字节且不超过提供的缓冲区。
pub fn adts_strip(frame: &[u8]) -> Option<(AdtsHeader, &[u8])> {
    let header = AdtsHeader::parse(frame)?;
    if usize::from(header.frame_length) < 7 || frame.len() < usize::from(header.frame_length) {
        return None;
    }
    Some((header, &frame[7..usize::from(header.frame_length)]))
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        if count > 32 || self.bit_pos + count > self.data.len() * 8 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..count {
            let byte = self.data[self.bit_pos / 8];
            let shift = 7 - (self.bit_pos % 8);
            value = (value << 1) | u32::from((byte >> shift) & 1);
            self.bit_pos += 1;
        }
        Some(value)
    }

    fn skip_bits(&mut self, count: usize) -> Option<()> {
        if self.bit_pos + count > self.data.len() * 8 {
            return None;
        }
        self.bit_pos += count;
        Some(())
    }

    fn byte_align(&mut self) {
        self.bit_pos = (self.bit_pos + 7) & !7;
    }
}

fn read_aac_audio_object_type(reader: &mut BitReader<'_>) -> Option<u8> {
    let object_type = reader.read_bits(5)? as u8;
    if object_type == 31 {
        Some(32 + reader.read_bits(6)? as u8)
    } else {
        Some(object_type)
    }
}

fn parse_ga_specific_config_channel_count(
    reader: &mut BitReader<'_>,
    audio_object_type: u8,
) -> Option<u8> {
    match audio_object_type {
        1 | 2 | 3 | 4 | 6 | 7 | 17 | 19 | 20 | 21 | 22 | 23 => {}
        _ => return None,
    }

    reader.skip_bits(1)?; // frameLengthFlag
    let depends_on_core_coder = reader.read_bits(1)? != 0;
    if depends_on_core_coder {
        reader.skip_bits(14)?;
    }
    reader.skip_bits(1)?; // extensionFlag
    parse_program_config_element_channel_count(reader)
}

fn parse_program_config_element_channel_count(reader: &mut BitReader<'_>) -> Option<u8> {
    reader.skip_bits(4)?; // element_instance_tag
    reader.skip_bits(2)?; // object_type
    reader.skip_bits(4)?; // sampling_frequency_index
    let front = reader.read_bits(4)? as usize;
    let side = reader.read_bits(4)? as usize;
    let back = reader.read_bits(4)? as usize;
    let lfe = reader.read_bits(2)? as usize;
    let assoc_data = reader.read_bits(3)? as usize;
    let valid_cc = reader.read_bits(4)? as usize;

    if reader.read_bits(1)? != 0 {
        reader.skip_bits(4)?;
    }
    if reader.read_bits(1)? != 0 {
        reader.skip_bits(4)?;
    }
    if reader.read_bits(1)? != 0 {
        reader.skip_bits(3)?;
    }

    let mut channels = 0u8;
    for _ in 0..front {
        channels = channels.saturating_add(if reader.read_bits(1)? != 0 { 2 } else { 1 });
        reader.skip_bits(4)?;
    }
    for _ in 0..side {
        channels = channels.saturating_add(if reader.read_bits(1)? != 0 { 2 } else { 1 });
        reader.skip_bits(4)?;
    }
    for _ in 0..back {
        channels = channels.saturating_add(if reader.read_bits(1)? != 0 { 2 } else { 1 });
        reader.skip_bits(4)?;
    }
    for _ in 0..lfe {
        channels = channels.saturating_add(1);
        reader.skip_bits(4)?;
    }
    for _ in 0..assoc_data {
        reader.skip_bits(4)?;
    }
    for _ in 0..valid_cc {
        reader.skip_bits(5)?;
    }

    reader.byte_align();
    let comment_bytes = reader.read_bits(8)? as usize;
    reader.skip_bits(comment_bytes.saturating_mul(8))?;

    if channels > 0 {
        Some(channels)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asc_roundtrip() {
        let asc = AacAudioSpecificConfig {
            audio_object_type: 2,
            sampling_frequency_index: 4,
            channel_configuration: 2,
        };
        let bytes = asc.to_bytes();
        assert_eq!(AacAudioSpecificConfig::from_bytes(&bytes), Some(asc));
    }

    #[test]
    fn adts_wrap_and_strip() {
        let asc = AacAudioSpecificConfig {
            audio_object_type: 2,
            sampling_frequency_index: 4,
            channel_configuration: 2,
        };
        let wrapped = adts_wrap(&[1, 2, 3, 4], asc);
        let (header, payload) = adts_strip(&wrapped).expect("strip");
        assert_eq!(payload, &[1, 2, 3, 4]);
        assert_eq!(header.sampling_frequency_index, 4);
        assert_eq!(header.channel_configuration, 2);
    }

    /// 5.1 layouts (channel_configuration=6) span both bytes of the ADTS fixed header.
    /// The previous build() implementation dropped the MSB of the 3-bit field, so values
    /// greater than 3 silently round-tripped to garbage (e.g. 6 -> 2).
    #[test]
    fn adts_wrap_preserves_high_channel_configurations() {
        for ch_cfg in [4u8, 5, 6, 7] {
            let asc = AacAudioSpecificConfig {
                audio_object_type: 2,
                sampling_frequency_index: 3,
                channel_configuration: ch_cfg,
            };
            let wrapped = adts_wrap(&[0xAA, 0xBB], asc);
            let (header, _) = adts_strip(&wrapped).expect("strip");
            assert_eq!(header.channel_configuration, ch_cfg, "ch_cfg={ch_cfg}");
        }
    }

    /// MPEG-4 ASC permits channel_configuration values up to 14, but ADTS reserves
    /// only 3 bits for the field. Values > 7 must saturate to 7 (8-channel layout)
    /// instead of silently truncating to a smaller value (e.g., 11 & 0x07 = 3).
    #[test]
    fn adts_wrap_saturates_oversized_channel_configurations() {
        for ch_cfg in [11u8, 12, 14] {
            let asc = AacAudioSpecificConfig {
                audio_object_type: 2,
                sampling_frequency_index: 3,
                channel_configuration: ch_cfg,
            };
            let wrapped = adts_wrap(&[0xAA, 0xBB], asc);
            let (header, _) = adts_strip(&wrapped).expect("strip");
            assert_eq!(
                header.channel_configuration, 7,
                "out-of-range ch_cfg={ch_cfg} should saturate to 7"
            );
        }
    }

    #[test]
    fn parses_aac_pce_channel_count_from_asc() {
        let asc = [
            0x11, 0x80, 0x04, 0xc8, 0x44, 0x00, 0x20, 0x00, 0xc4, 0x0c, 0x4c, 0x61, 0x76, 0x63,
            0x36, 0x31, 0x2e, 0x33, 0x2e, 0x31, 0x30, 0x30, 0x56, 0xe5, 0x00,
        ];

        assert_eq!(aac_channel_count_from_asc(&asc), Some(6));
    }
}
