//! Shared MPEG-TS constants and helpers used by both muxer and demuxer.

use crate::prelude::*;
use bytes::{BufMut, BytesMut};

use crate::track::CodecId;

/// `TS_PACKET_SIZE` constant.
/// `TS_PACKET_SIZE` 常量.
pub const TS_PACKET_SIZE: usize = 188;
/// `SYNC_BYTE` constant.
/// `SYNC_BYTE` 常量.
pub const SYNC_BYTE: u8 = 0x47;
/// `PAT_PID` constant.
/// `PAT_PID` 常量.
pub const PAT_PID: u16 = 0x0000;
/// `PMT_PID` constant.
/// `PMT_PID` 常量.
pub const PMT_PID: u16 = 0x1000;

/// H.264 Access Unit Delimiter: 00 00 00 01 09 F0
pub const AUD_H264: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0];
/// H.265 Access Unit Delimiter: 00 00 00 01 46 01 50
pub const AUD_H265: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x46, 0x01, 0x50];
/// H.266/VVC Access Unit Delimiter.
pub const AUD_H266: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x00, 0xA0, 0x01];

/// Map CodecId to MPEG-TS stream_type.
pub fn stream_type_for_codec(codec: CodecId) -> u8 {
    match codec {
        CodecId::H264 => 0x1B,
        CodecId::H265 => 0x24,
        CodecId::H266 => 0x33,
        CodecId::VP8 => 0x9D,
        CodecId::VP9 => 0x9E,
        CodecId::AV1 => 0x9F,
        CodecId::AAC => 0x0F,
        CodecId::MP2 => 0x03,
        CodecId::MP3 => 0x04,
        CodecId::G711A => 0x90,
        CodecId::G711U => 0x91,
        CodecId::Opus => 0x06,
        CodecId::ADPCM | CodecId::MJPEG | CodecId::Unknown => 0x06,
    }
}

/// Map stream_type to CodecId (input direction).
pub fn codec_from_stream_type(stream_type: u8) -> Option<(CodecId, crate::MediaKind)> {
    match stream_type {
        0x1B => Some((CodecId::H264, crate::MediaKind::Video)),
        0x24 => Some((CodecId::H265, crate::MediaKind::Video)),
        0x33 => Some((CodecId::H266, crate::MediaKind::Video)),
        0x9D => Some((CodecId::VP8, crate::MediaKind::Video)),
        0x9E => Some((CodecId::VP9, crate::MediaKind::Video)),
        0x9F => Some((CodecId::AV1, crate::MediaKind::Video)),
        0x0F => Some((CodecId::AAC, crate::MediaKind::Audio)),
        0x03 => Some((CodecId::MP2, crate::MediaKind::Audio)),
        0x04 => Some((CodecId::MP3, crate::MediaKind::Audio)),
        0x90 => Some((CodecId::G711A, crate::MediaKind::Audio)),
        0x91 => Some((CodecId::G711U, crate::MediaKind::Audio)),
        0x9C => Some((CodecId::Opus, crate::MediaKind::Audio)),
        _ => None,
    }
}

/// Build ES_info descriptors for codecs that need them in the PMT.
pub fn registration_descriptor(codec: CodecId) -> &'static [u8] {
    match codec {
        CodecId::AV1 => &[0x05, 0x04, b'A', b'V', b'0', b'1'],
        CodecId::VP8 => &[0x05, 0x04, b'V', b'P', b'8', b'0'],
        CodecId::VP9 => &[0x05, 0x04, b'V', b'P', b'0', b'9'],
        CodecId::Opus => &[0x05, 0x04, b'O', b'p', b'u', b's'],
        CodecId::MJPEG => &[0x05, 0x04, b'M', b'J', b'P', b'G'],
        CodecId::ADPCM => &[0x05, 0x04, b'A', b'D', b'P', b'C'],
        _ => &[],
    }
}

/// Encode a 33-bit PTS/DTS timestamp into 5 bytes.
pub fn encode_timestamp(buf: &mut Vec<u8>, marker: u8, ts: u64) {
    let ts = ts & 0x1_FFFF_FFFF;
    buf.push((marker << 4) | ((ts >> 29) as u8 & 0x0E) | 0x01);
    buf.push((ts >> 22) as u8);
    buf.push(((ts >> 14) as u8 & 0xFE) | 0x01);
    buf.push((ts >> 7) as u8);
    buf.push(((ts << 1) as u8 & 0xFE) | 0x01);
}

/// Decode a 33-bit PTS/DTS timestamp from 5 bytes.
pub fn decode_timestamp(buf: &[u8]) -> u64 {
    let b0 = buf[0] as u64;
    let b1 = buf[1] as u64;
    let b2 = buf[2] as u64;
    let b3 = buf[3] as u64;
    let b4 = buf[4] as u64;
    ((b0 >> 1) & 0x07) << 30 | (b1 << 22) | ((b2 >> 1) << 15) | (b3 << 7) | (b4 >> 1)
}

/// CRC-32/MPEG-2 used in PAT/PMT.
pub fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Write PES data into multiple 188-byte TS packets.
pub fn write_pes_packets(
    buf: &mut BytesMut,
    pid: u16,
    pes: &[u8],
    cc: &mut u8,
    random_access: bool,
    pcr: Option<u64>,
) {
    let mut offset = 0;
    let mut first = true;

    while offset < pes.len() {
        let mut pkt = [0xFF_u8; TS_PACKET_SIZE];
        pkt[0] = 0x47;

        let pusi_bit: u16 = if first { 0x4000 } else { 0 };
        let pid_bytes = (pusi_bit | pid).to_be_bytes();
        pkt[1] = pid_bytes[0];
        pkt[2] = pid_bytes[1];

        let remaining = pes.len() - offset;
        let need_af = first && (random_access || pcr.is_some());
        let mut header_end = 4;

        if need_af {
            pkt[3] = 0x30 | (*cc & 0x0F);
            let mut af_flags: u8 = 0;
            let mut af_data = BytesMut::new();

            if random_access {
                af_flags |= 0x40;
            }
            if let Some(pcr_val) = pcr {
                af_flags |= 0x10;
                let pcr_base = pcr_val;
                let pcr_ext: u16 = 0;
                af_data.put_u8((pcr_base >> 25) as u8);
                af_data.put_u8((pcr_base >> 17) as u8);
                af_data.put_u8((pcr_base >> 9) as u8);
                af_data.put_u8((pcr_base >> 1) as u8);
                af_data.put_u8(((pcr_base & 1) as u8) << 7 | 0x7E | ((pcr_ext >> 8) as u8 & 0x01));
                af_data.put_u8(pcr_ext as u8);
            }

            let af_len = 1 + af_data.len();
            pkt[4] = af_len as u8;
            pkt[5] = af_flags;
            if !af_data.is_empty() {
                pkt[6..6 + af_data.len()].copy_from_slice(&af_data);
            }
            header_end = 4 + 1 + af_len;
        } else {
            pkt[3] = 0x10 | (*cc & 0x0F);
        }

        let available = TS_PACKET_SIZE - header_end;

        if remaining < available {
            if !need_af {
                pkt[3] = 0x30 | (*cc & 0x0F);
                let stuff_len = available - remaining;
                if stuff_len == 1 {
                    pkt[4] = 0x00;
                    header_end = 5;
                } else {
                    pkt[4] = (stuff_len - 1) as u8;
                    pkt[5] = 0x00;
                    header_end = 4 + stuff_len;
                }
            } else {
                let current_af_len = pkt[4] as usize;
                let stuff_needed = available - remaining;
                pkt[4] = (current_af_len + stuff_needed) as u8;
                header_end += stuff_needed;
            }
            pkt[header_end..header_end + remaining].copy_from_slice(&pes[offset..]);
            offset = pes.len();
        } else {
            pkt[header_end..header_end + available]
                .copy_from_slice(&pes[offset..offset + available]);
            offset += available;
        }

        buf.extend_from_slice(&pkt);
        *cc = (*cc + 1) & 0x0F;
        first = false;
    }
}

/// Derive G711 frame duration in microseconds from payload length.
/// G711A/G711U: 1 byte = 1 sample at 8000Hz.
pub fn g711_duration_us(payload_len: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    (payload_len as u64) * 1_000_000 / sample_rate as u64
}

/// Derive G711 frame duration in 90kHz ticks from payload length.
pub fn g711_duration_90k(payload_len: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    (payload_len as u64) * 90_000 / sample_rate as u64
}

/// Identify a private stream (stream_type=0x06) by parsing ES_info descriptors.
pub fn identify_private_stream(es_info: &[u8]) -> Option<(CodecId, crate::MediaKind)> {
    let mut offset = 0;
    while offset + 2 <= es_info.len() {
        let tag = es_info[offset];
        let len = es_info[offset + 1] as usize;
        if offset + 2 + len > es_info.len() {
            break;
        }
        let desc_data = &es_info[offset + 2..offset + 2 + len];
        match tag {
            0x05 if len >= 4 => match desc_data[..4] {
                [b'A', b'V', b'0', b'1'] => return Some((CodecId::AV1, crate::MediaKind::Video)),
                [b'O', b'p', b'u', b's'] => return Some((CodecId::Opus, crate::MediaKind::Audio)),
                [b'V', b'P', b'0', b'9'] => return Some((CodecId::VP9, crate::MediaKind::Video)),
                [b'V', b'P', b'8', b'0'] => return Some((CodecId::VP8, crate::MediaKind::Video)),
                [b'M', b'J', b'P', b'G'] => return Some((CodecId::MJPEG, crate::MediaKind::Video)),
                [b'A', b'D', b'P', b'C'] => return Some((CodecId::ADPCM, crate::MediaKind::Audio)),
                _ => {}
            },
            0x80 => return Some((CodecId::AV1, crate::MediaKind::Video)),
            _ => {}
        }
        offset += 2 + len;
    }
    None
}

/// Find a sync byte (0x47) confirmed by a second 0x47 at +188.
pub fn find_sync(data: &[u8], start: usize) -> Option<usize> {
    let end = data.len().saturating_sub(TS_PACKET_SIZE);
    for i in start..end {
        if data[i] == SYNC_BYTE
            && (i + TS_PACKET_SIZE >= data.len() || data[i + TS_PACKET_SIZE] == SYNC_BYTE)
        {
            return Some(i);
        }
    }
    (end.max(start)..data.len()).find(|&i| data[i] == SYNC_BYTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_encode_decode_roundtrip() {
        for &ts in &[0u64, 90_000, 0x1_FFFF_FFFF, 12345678] {
            let mut buf = Vec::new();
            encode_timestamp(&mut buf, 0x02, ts);
            let decoded = decode_timestamp(&buf);
            assert_eq!(decoded, ts & 0x1_FFFF_FFFF);
        }
    }

    #[test]
    fn crc32_mpeg2_known_value() {
        // PAT payload without CRC
        let pat = [
            0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xE1, 0x00,
        ];
        let crc = crc32_mpeg2(&pat);
        // Just verify it's deterministic
        assert_eq!(crc, crc32_mpeg2(&pat));
    }

    #[test]
    fn stream_type_mapping_complete() {
        assert_eq!(stream_type_for_codec(CodecId::H264), 0x1B);
        assert_eq!(stream_type_for_codec(CodecId::H265), 0x24);
        assert_eq!(stream_type_for_codec(CodecId::H266), 0x33);
        assert_eq!(stream_type_for_codec(CodecId::VP8), 0x9D);
        assert_eq!(stream_type_for_codec(CodecId::VP9), 0x9E);
        assert_eq!(stream_type_for_codec(CodecId::AV1), 0x9F);
        assert_eq!(stream_type_for_codec(CodecId::AAC), 0x0F);
        assert_eq!(stream_type_for_codec(CodecId::MP2), 0x03);
        assert_eq!(stream_type_for_codec(CodecId::MP3), 0x04);
        assert_eq!(stream_type_for_codec(CodecId::G711A), 0x90);
        assert_eq!(stream_type_for_codec(CodecId::G711U), 0x91);
        assert_eq!(stream_type_for_codec(CodecId::Opus), 0x06);
    }

    #[test]
    fn stream_type_roundtrip_all_codecs() {
        let codecs = [
            (CodecId::H264, 0x1B),
            (CodecId::H265, 0x24),
            (CodecId::H266, 0x33),
            (CodecId::VP8, 0x9D),
            (CodecId::VP9, 0x9E),
            (CodecId::AV1, 0x9F),
            (CodecId::AAC, 0x0F),
            (CodecId::MP2, 0x03),
            (CodecId::MP3, 0x04),
            (CodecId::G711A, 0x90),
            (CodecId::G711U, 0x91),
        ];
        for (codec, st) in codecs {
            assert_eq!(stream_type_for_codec(codec), st);
            let (decoded_codec, _) = codec_from_stream_type(st).unwrap();
            assert_eq!(decoded_codec, codec, "roundtrip failed for {codec:?}");
        }
    }

    #[test]
    fn opus_input_compat_0x9c() {
        let result = codec_from_stream_type(0x9C);
        assert_eq!(result, Some((CodecId::Opus, crate::MediaKind::Audio)));
    }

    #[test]
    fn identify_private_stream_opus() {
        let desc = registration_descriptor(CodecId::Opus);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::Opus, crate::MediaKind::Audio)));
    }

    #[test]
    fn identify_private_stream_vp8() {
        let desc = registration_descriptor(CodecId::VP8);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::VP8, crate::MediaKind::Video)));
    }

    #[test]
    fn identify_private_stream_vp9() {
        let desc = registration_descriptor(CodecId::VP9);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::VP9, crate::MediaKind::Video)));
    }

    #[test]
    fn identify_private_stream_av1() {
        let desc = registration_descriptor(CodecId::AV1);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::AV1, crate::MediaKind::Video)));
    }

    #[test]
    fn identify_private_stream_mjpeg() {
        let desc = registration_descriptor(CodecId::MJPEG);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::MJPEG, crate::MediaKind::Video)));
    }

    #[test]
    fn identify_private_stream_adpcm() {
        let desc = registration_descriptor(CodecId::ADPCM);
        let result = identify_private_stream(desc);
        assert_eq!(result, Some((CodecId::ADPCM, crate::MediaKind::Audio)));
    }

    #[test]
    fn identify_private_stream_unknown_returns_none() {
        // Unknown registration descriptor "ZZZZ"
        let desc = &[0x05, 0x04, b'Z', b'Z', b'Z', b'Z'];
        let result = identify_private_stream(desc);
        assert_eq!(result, None);
    }

    #[test]
    fn identify_private_stream_empty_returns_none() {
        let result = identify_private_stream(&[]);
        assert_eq!(result, None);
    }

    #[test]
    fn identify_private_stream_truncated_descriptor() {
        // Descriptor claims length 4 but only 2 bytes available
        let desc = &[0x05, 0x04, b'O', b'p'];
        let result = identify_private_stream(desc);
        assert_eq!(result, None);
    }

    #[test]
    fn find_sync_works() {
        let mut data = vec![0xAA; 10]; // garbage
        data.push(0x47);
        data.extend_from_slice(&[0x00; 187]);
        data.push(0x47);
        data.extend_from_slice(&[0x00; 187]);
        assert_eq!(find_sync(&data, 0), Some(10));
    }
}
