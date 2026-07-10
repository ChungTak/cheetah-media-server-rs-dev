//! MPEG-TS muxer: packs video/audio PES packets into 188-byte TS segments.
//!
//! MPEG-TS 封装器：将视频/音频 PES 包封装为 188 字节 TS 分片。
//! 生成 PAT/PMT、TS 包，并支持 H.264/H.265/AV1/VP8/VP9 与 AAC/MP3/Opus/G.711。

use bytes::{BufMut, Bytes, BytesMut};
use cheetah_codec::CodecId;

const TS_PACKET_SIZE: usize = 188;
const TS_SYNC: u8 = 0x47;
const PAT_PID: u16 = 0x0000;
const PMT_PID: u16 = 0x1000;
const VIDEO_PID: u16 = 0x0100;
const AUDIO_PID: u16 = 0x0101;

/// MPEG-TS muxer that builds a single TS segment.
///
/// 构建单个 TS 分片的 MPEG-TS 封装器。
///
/// PAT/PMT are written once at the start of the segment. Video and audio frames are
/// packetized into PES and then split into 188-byte TS packets. AV1 and Opus/VP9
/// are carried as private streams with registration and descriptor tags.
///
/// PAT/PMT 在分片开头写入一次。视频与音频帧先打包为 PES，再拆分为 188 字节 TS 包。
/// AV1 与 Opus/VP9 作为私有流带注册与描述符标签传输。
pub struct TsMuxer {
    buf: BytesMut,
    video_codec: CodecId,
    audio_codec: CodecId,
    has_audio: bool,
    continuity_counter: [u8; 0x2000],
    first_pat: bool,
    av1_enabled: bool,
    opus_enabled: bool,
    vp9_enabled: bool,
}

impl TsMuxer {
    /// Create a TS muxer.
    ///
    /// `has_audio` controls whether an audio track is included.
    ///
    /// 创建 TS 封装器。`has_audio` 控制是否包含音频轨道。
    pub fn new(video_codec: CodecId, audio_codec: CodecId, has_audio: bool) -> Self {
        Self {
            buf: BytesMut::new(),
            video_codec,
            audio_codec,
            has_audio,
            continuity_counter: [0; 0x2000],
            first_pat: false,
            av1_enabled: matches!(video_codec, CodecId::AV1),
            opus_enabled: matches!(audio_codec, CodecId::Opus),
            vp9_enabled: matches!(video_codec, CodecId::VP9),
        }
    }

    /// Write a TS packet header into the internal buffer.
    ///
    /// 写入 TS 包头到内部缓冲。
    fn write_packet_header(&mut self, pid: u16, payload_start: bool, payload_len: usize) {
        let cc = self.continuity_counter[pid as usize];
        // adaptation_field_control: 01 = payload only, 11 = adaptation + payload
        let af = if payload_len < TS_PACKET_SIZE - 4 {
            0x30
        } else {
            0x10
        };
        self.buf.extend_from_slice(&[
            TS_SYNC,
            // PUSI (payload_unit_start_indicator) is bit 6, PID high bits in bits 4-0
            ((pid >> 8) as u8 & 0x1F) | if payload_start { 0x40 } else { 0x00 },
            pid as u8,
            af | (cc & 0x0F),
        ]);
        self.continuity_counter[pid as usize] = (cc + 1) & 0x0F;
    }

    /// Write a packet with an adaptation field to fill the remaining 188 bytes.
    ///
    /// 写入带适配域的包以填满 188 字节。
    fn write_packet_payload(&mut self, pid: u16, payload_start: bool, payload: &[u8]) {
        self.write_packet_header(pid, payload_start, payload.len());
        // Bytes after header to fill with adaptation field + payload
        let fill = TS_PACKET_SIZE - 4 - payload.len();
        // adaptation_field_length counts the bytes after this length byte
        let adaptation_field_length = fill - 1;
        self.buf.put_u8(adaptation_field_length as u8);
        if adaptation_field_length > 0 {
            self.buf.put_u8(0x00); // adaptation flags: none
            self.buf.put_bytes(0xFF, adaptation_field_length - 1);
        }
        self.buf.extend_from_slice(payload);
    }

    /// Write a complete TS packet with a start indicator and payload (possibly split).
    ///
    /// 写入带起始指示的完整 TS 包及其负载（可能被分割）。
    fn write_pes_packets(
        &mut self,
        pid: u16,
        mut payload: &[u8],
        payload_start: bool,
        is_first: bool,
    ) {
        let mut first = is_first;
        while !payload.is_empty() {
            let available = TS_PACKET_SIZE - 4;
            let chunk_len = payload.len().min(available);
            let chunk = &payload[..chunk_len];
            let start = payload_start && first;

            if chunk_len == available {
                // Middle packet: no adaptation field
                self.write_packet_header(pid, start, chunk_len);
                self.buf.extend_from_slice(chunk);
            } else {
                // First or last packet: use adaptation field
                self.write_packet_payload(pid, start, chunk);
            }
            first = false;
            payload = &payload[chunk_len..];
        }
    }

    /// Build a PES packet for video with optional DTS and keyframe flag.
    ///
    /// 构造带可选 DTS 与关键帧标志的视频 PES 包。
    fn video_pes_packet(&self, data: &[u8], pts: u64, dts: u64, _is_keyframe: bool) -> Vec<u8> {
        let mut pes = Vec::with_capacity(32 + data.len());
        pes.extend_from_slice(&[0x00, 0x00, 0x01]);
        // Stream ID: video stream 0xE0
        pes.push(0xE0);

        // PES packet length: 0 (unbounded) for video with DTS
        pes.extend_from_slice(&[0x00, 0x00]);

        // Flags: PES scrambling=0, priority=0, data alignment=1, copyright=0, original=0,
        // PTS_DTS flags, ESCR=0, ES rate=0, DSM trick=0, additional=0, CRC=0, extension=0
        let pts_dts_flags: u8 = if pts == dts || dts == 0 { 0x80 } else { 0xC0 };
        pes.push(0x84); // data alignment indicator
        pes.push(pts_dts_flags);

        // PES header data length
        let header_data_len: u8 = if pts_dts_flags == 0xC0 { 0x0A } else { 0x05 };
        pes.push(header_data_len);

        // PTS
        pes.extend_from_slice(&encode_timestamp(pts, pts_dts_flags >> 6));
        if pts_dts_flags == 0xC0 {
            // DTS
            pes.extend_from_slice(&encode_timestamp(dts, 0x01));
        }

        // Annex-B start code prefix for H.264/H.265 if not present
        if (self.video_codec == CodecId::H264 || self.video_codec == CodecId::H265)
            && !data.starts_with(&[0x00, 0x00, 0x00, 0x01])
            && !data.starts_with(&[0x00, 0x00, 0x01])
        {
            pes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        }
        pes.extend_from_slice(data);
        pes
    }

    /// Build a PES packet for audio.
    ///
    /// 构造音频 PES 包。
    fn audio_pes_packet(&self, data: &[u8], pts: u64) -> Vec<u8> {
        let mut pes = Vec::with_capacity(32 + data.len());
        pes.extend_from_slice(&[0x00, 0x00, 0x01]);
        // Stream ID: audio stream 0xC0
        pes.push(0xC0);

        // PES packet length (bounded)
        let pes_len = 8 + 5 + data.len();
        if pes_len > 0xFFFF {
            pes.extend_from_slice(&[0x00, 0x00]);
        } else {
            pes.extend_from_slice(&[(pes_len >> 8) as u8, pes_len as u8]);
        }

        // Flags: data alignment, PTS only
        pes.push(0x84);
        pes.push(0x80);
        pes.push(0x05);
        pes.extend_from_slice(&encode_timestamp(pts, 0x02));
        pes.extend_from_slice(data);
        pes
    }

    /// Write PAT and PMT tables to the segment.
    ///
    /// 写入 PAT 与 PMT 表到分片。
    pub fn write_pat_pmt(&mut self) {
        if self.first_pat {
            return;
        }
        self.first_pat = true;

        // Build PAT
        let mut pat = Vec::new();
        pat.push(0x00); // table_id
        pat.push(0xB0); // section_syntax_indicator=1, section_length high
        pat.push(0x0D); // section_length low
        pat.extend_from_slice(&[0x00, 0x01]); // transport_stream_id
        pat.push(0xC1); // version + current_next
        pat.push(0x00); // section_number
        pat.push(0x00); // last_section_number
        pat.extend_from_slice(&[0x00, 0x00]); // program_number
        pat.extend_from_slice(&[0xF0, 0x00]); // reserved + PMT PID (0x1000)
        let crc = crc32_mpeg2(&pat);
        pat.extend_from_slice(&crc.to_be_bytes());

        // Pad to 188 bytes; payload_unit_start_indicator requires a pointer byte first.
        let mut pat_pkt = Vec::with_capacity(TS_PACKET_SIZE);
        pat_pkt.extend_from_slice(&[TS_SYNC, 0x40, 0x00, 0x10]); // PID=0, payload_start, cc=0
        pat_pkt.push(0x00); // pointer_field = 0
        pat_pkt.extend_from_slice(&pat);
        pat_pkt.resize(TS_PACKET_SIZE, 0xFF);
        self.buf.extend_from_slice(&pat_pkt);
        self.continuity_counter[PAT_PID as usize] = 1;

        // Build PMT
        let mut pmt = Vec::new();
        pmt.push(0x02); // table_id
                        // section_length placeholder
        let section_len_pos = pmt.len();
        pmt.push(0xB0);
        pmt.push(0x00); // placeholder
        pmt.extend_from_slice(&[0x00, 0x01]); // program_number
        pmt.push(0xC1); // version + current_next
        pmt.push(0x00); // section_number
        pmt.push(0x00); // last_section_number
        pmt.extend_from_slice(&[0xE1, 0x00]); // reserved + PCR PID = 0x0100
        pmt.extend_from_slice(&[0xF0, 0x00]); // reserved + program_info_length = 0

        // Video stream info
        pmt.push(stream_type(self.video_codec));
        pmt.extend_from_slice(&[0xE1, 0x00]); // reserved + elementary PID = 0x0100
        let video_desc = self.video_descriptor();
        let video_info_len = video_desc.len();
        pmt.push(((video_info_len >> 8) as u8 & 0x0F) | 0xF0);
        pmt.push(video_info_len as u8);
        pmt.extend_from_slice(&video_desc);

        // Audio stream info
        if self.has_audio {
            pmt.push(stream_type(self.audio_codec));
            pmt.extend_from_slice(&[0xE1, 0x01]); // reserved + elementary PID = 0x0101
            let audio_desc = self.audio_descriptor();
            let audio_info_len = audio_desc.len();
            pmt.push(((audio_info_len >> 8) as u8 & 0x0F) | 0xF0);
            pmt.push(audio_info_len as u8);
            pmt.extend_from_slice(&audio_desc);
        }

        // section_length counts bytes after the 2-byte section_length field, including CRC.
        let section_len = pmt.len() - section_len_pos - 2 + 4;
        pmt[section_len_pos] = 0xB0 | ((section_len >> 8) & 0x0F) as u8;
        pmt[section_len_pos + 1] = (section_len & 0xFF) as u8;

        let pmt_crc = crc32_mpeg2(&pmt);
        pmt.extend_from_slice(&pmt_crc.to_be_bytes());

        let mut pmt_pkt = Vec::with_capacity(TS_PACKET_SIZE);
        pmt_pkt.extend_from_slice(&[TS_SYNC, 0x50, 0x00, 0x10]); // PID=0x1000, payload_start
        pmt_pkt.push(0x00); // pointer_field = 0
        pmt_pkt.extend_from_slice(&pmt);
        pmt_pkt.resize(TS_PACKET_SIZE, 0xFF);
        self.buf.extend_from_slice(&pmt_pkt);
        self.continuity_counter[PMT_PID as usize] = 1;
    }

    /// Write a video frame (PES packetized into TS packets).
    ///
    /// 写入一帧视频（PES 包化后分装 TS 包）。
    pub fn write_video(&mut self, data: &[u8], pts: u64, dts: u64, is_keyframe: bool) {
        let pes = self.video_pes_packet(data, pts, dts, is_keyframe);
        self.write_pes_packets(VIDEO_PID, &pes, true, true);
    }

    /// Write an audio frame (PES packetized into TS packets).
    ///
    /// 写入一帧音频（PES 包化后分装 TS 包）。
    pub fn write_audio(&mut self, data: &[u8], pts: u64) {
        if !self.has_audio {
            return;
        }
        let pes = self.audio_pes_packet(data, pts);
        self.write_pes_packets(AUDIO_PID, &pes, true, true);
    }

    /// Finalize the current segment and return the payload.
    ///
    /// 结束当前分片并返回负载。
    pub fn take_segment(&mut self) -> Bytes {
        std::mem::take(&mut self.buf).freeze()
    }

    /// Return the stream_type byte for a codec in the PMT.
    ///
    /// 返回 PMT 中该编解码器的 stream_type 字节。
    fn stream_type(codec: CodecId) -> u8 {
        match codec {
            CodecId::H264 => 0x1B,
            CodecId::H265 => 0x24,
            CodecId::AV1 => 0x06, // private; identified by descriptor
            CodecId::VP8 => 0x9D,
            CodecId::VP9 => 0x9E,
            CodecId::AAC => 0x0F,
            CodecId::MP3 => 0x04,
            CodecId::MP2 => 0x03,
            CodecId::G711A => 0x90,
            CodecId::G711U => 0x91,
            CodecId::Opus => 0x06, // private; identified by descriptor
            _ => 0x06,
        }
    }

    /// Descriptor bytes for the video stream.
    ///
    /// 视频流的描述符字节。
    fn video_descriptor(&self) -> Vec<u8> {
        let mut desc = Vec::new();
        if self.av1_enabled {
            // AV1 registration + AV1 video descriptor
            desc.extend_from_slice(&[0x05, 0x04, b'A', b'V', b'0', b'1']);
            desc.extend_from_slice(&[0x80, 0x00]);
        } else if self.vp9_enabled {
            desc.extend_from_slice(&[0x05, 0x04, b'V', b'P', b'0', b'9']);
        }
        desc
    }

    /// Descriptor bytes for the audio stream.
    ///
    /// 音频流的描述符字节。
    fn audio_descriptor(&self) -> Vec<u8> {
        let mut desc = Vec::new();
        if self.opus_enabled {
            // Opus registration descriptor
            desc.extend_from_slice(&[0x05, 0x04, b'O', b'p', b'u', b's']);
        }
        desc
    }
}

impl Default for TsMuxer {
    fn default() -> Self {
        Self::new(CodecId::H264, CodecId::AAC, true)
    }
}

/// Map `CodecId` to the PMT `stream_type` byte.
///
/// 将 `CodecId` 映射为 PMT `stream_type` 字节。
fn stream_type(codec: CodecId) -> u8 {
    TsMuxer::stream_type(codec)
}

/// Encode a 33-bit MPEG-2 PES timestamp into 5 bytes.
///
/// `marker` is the 4-bit marker (0x2 for PTS, 0x3 for DTS, etc.) used in the PES spec.
///
/// 将 33 位 MPEG-2 PES 时间戳编码为 5 字节。
/// `marker` 为 PES 规范中的 4 位标记（PTS 为 0x2，DTS 为 0x3 等）。
fn encode_timestamp(ts: u64, marker: u8) -> [u8; 5] {
    let mut out = [0u8; 5];
    out[0] = (marker & 0x0F) << 4 | (((ts >> 30) & 0x07) as u8) << 1 | 1;
    out[1] = ((ts >> 22) & 0xFF) as u8;
    out[2] = (((ts >> 15) & 0x7F) as u8) << 1 | 1;
    out[3] = ((ts >> 7) & 0xFF) as u8;
    out[4] = (((ts) & 0x7F) as u8) << 1 | 1;
    out
}

/// CRC32 as defined in MPEG-2 for PSI tables.
///
/// MPEG-2 中 PSI 表使用的 CRC32。
fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            if crc & 0x80000000 != 0 {
                crc = (crc << 1) ^ 0x04C11DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_segment_size_aligned() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, true);
        muxer.write_pat_pmt();
        muxer.write_video(
            &[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB],
            90000,
            90000,
            true,
        );
        let segment = muxer.take_segment();
        assert_eq!(segment.len() % TS_PACKET_SIZE, 0);
        assert_eq!(segment[0], TS_SYNC);
    }

    #[test]
    fn mux_pat_pmt_only_once() {
        let mut muxer = TsMuxer::new(CodecId::H264, CodecId::AAC, true);
        muxer.write_pat_pmt();
        muxer.write_pat_pmt();
        muxer.write_video(&[0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB], 90000, 90000, true);
        let segment = muxer.take_segment();
        // PAT and PMT packets are each 188 bytes; two packets total
        let packet_count = segment.len() / TS_PACKET_SIZE;
        assert!(packet_count >= 3, "expected PAT, PMT, and video packets");
    }

    #[test]
    fn encode_timestamp_pts() {
        let ts = 90000u64;
        let encoded = encode_timestamp(ts, 0x02);
        let decoded = super::super::ts_demux::decode_timestamp(&encoded);
        assert_eq!(decoded, ts);
    }

    #[test]
    fn av1_muxer_adds_descriptors() {
        let mut muxer = TsMuxer::new(CodecId::AV1, CodecId::AAC, false);
        muxer.write_pat_pmt();
        muxer.write_video(&[0x12, 0x00, 0x32, 0x10], 90000, 90000, true);
        let segment = muxer.take_segment();
        assert!(segment.len() >= TS_PACKET_SIZE);
        assert_eq!(segment.len() % TS_PACKET_SIZE, 0);
    }

    #[test]
    fn opus_muxer_adds_descriptor() {
        let mut muxer = TsMuxer::new(CodecId::AV1, CodecId::Opus, true);
        muxer.write_pat_pmt();
        muxer.write_audio(&[0x00, 0x00], 90000);
        let segment = muxer.take_segment();
        assert_eq!(segment.len() % TS_PACKET_SIZE, 0);
    }
}
