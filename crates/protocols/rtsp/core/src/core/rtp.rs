const RTP_FIXED_HEADER_SIZE: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtpError {
    #[error("unsupported rtp version: {actual}")]
    UnsupportedVersion { actual: u8 },
    #[error("insufficient data for {context}: need at least {needed} bytes, got {actual}")]
    InsufficientData {
        context: &'static str,
        needed: usize,
        actual: usize,
    },
    #[error(
        "invalid rtp padding size: {padding_size}, available payload+padding bytes: {available}"
    )]
    InvalidPadding { padding_size: u8, available: usize },
    #[error("rtp csrc count exceeds 4-bit field: {count}")]
    TooManyCsrc { count: usize },
    #[error("rtp extension data too large: {bytes} bytes")]
    ExtensionTooLarge { bytes: usize },
}

/// RTP 头（RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpHeader {
    /// 版本（2 bit，当前仅支持 2）。
    pub version: u8,
    /// padding 标志位。
    pub padding: bool,
    /// extension 标志位。
    pub extension: bool,
    /// CSRC 数量（4 bit）。
    pub csrc_count: u8,
    /// marker 标志位。
    pub marker: bool,
    /// payload type（7 bit）。
    pub payload_type: u8,
    /// sequence number（16 bit）。
    pub sequence_number: u16,
    /// timestamp（32 bit）。
    pub timestamp: u32,
    /// SSRC（32 bit）。
    pub ssrc: u32,
    /// CSRC 列表。
    pub csrc: Vec<u32>,
}

impl Default for RtpHeader {
    fn default() -> Self {
        Self {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 0,
            sequence_number: 0,
            timestamp: 0,
            ssrc: 0,
            csrc: Vec::new(),
        }
    }
}

impl RtpHeader {
    pub fn new(payload_type: u8, sequence_number: u16, timestamp: u32, ssrc: u32) -> Self {
        Self {
            payload_type,
            sequence_number,
            timestamp,
            ssrc,
            ..Self::default()
        }
    }

    pub fn size(&self) -> usize {
        RTP_FIXED_HEADER_SIZE + self.csrc.len() * 4
    }
}

/// RTP 扩展头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpExtension {
    /// profile-specific 标识。
    pub profile: u16,
    /// 扩展数据（原始字节）。
    pub data: Vec<u8>,
}

/// RTP 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub extension: Option<RtpExtension>,
    pub payload: Vec<u8>,
    pub padding_size: u8,
}

impl RtpPacket {
    pub fn new(header: RtpHeader, payload: Vec<u8>) -> Self {
        Self {
            header,
            extension: None,
            payload,
            padding_size: 0,
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self, RtpError> {
        if data.len() < RTP_FIXED_HEADER_SIZE {
            return Err(RtpError::InsufficientData {
                context: "rtp fixed header",
                needed: RTP_FIXED_HEADER_SIZE,
                actual: data.len(),
            });
        }

        let first = data[0];
        let version = first >> 6;
        if version != 2 {
            return Err(RtpError::UnsupportedVersion { actual: version });
        }
        let padding = first & 0b0010_0000 != 0;
        let has_extension = first & 0b0001_0000 != 0;
        let csrc_count = (first & 0b0000_1111) as usize;

        let second = data[1];
        let marker = second & 0b1000_0000 != 0;
        let payload_type = second & 0b0111_1111;

        let sequence_number = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let csrc_bytes = csrc_count * 4;
        let mut offset = RTP_FIXED_HEADER_SIZE;
        if data.len() < offset + csrc_bytes {
            return Err(RtpError::InsufficientData {
                context: "csrc list",
                needed: offset + csrc_bytes,
                actual: data.len(),
            });
        }

        let mut csrc = Vec::with_capacity(csrc_count);
        for chunk in data[offset..offset + csrc_bytes].chunks_exact(4) {
            csrc.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        offset += csrc_bytes;

        let extension = if has_extension {
            if data.len() < offset + 4 {
                return Err(RtpError::InsufficientData {
                    context: "rtp extension header",
                    needed: offset + 4,
                    actual: data.len(),
                });
            }
            let profile = u16::from_be_bytes([data[offset], data[offset + 1]]);
            let length_words = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
            offset += 4;

            let extension_size = length_words * 4;
            if data.len() < offset + extension_size {
                return Err(RtpError::InsufficientData {
                    context: "rtp extension payload",
                    needed: offset + extension_size,
                    actual: data.len(),
                });
            }

            let ext_data = data[offset..offset + extension_size].to_vec();
            offset += extension_size;
            Some(RtpExtension {
                profile,
                data: ext_data,
            })
        } else {
            None
        };

        let remaining = &data[offset..];
        let (payload, padding_size) = if padding {
            if remaining.is_empty() {
                return Err(RtpError::InvalidPadding {
                    padding_size: 0,
                    available: 0,
                });
            }

            let padding_size = *remaining.last().unwrap();
            if padding_size == 0 || padding_size as usize > remaining.len() {
                return Err(RtpError::InvalidPadding {
                    padding_size,
                    available: remaining.len(),
                });
            }

            let payload_len = remaining.len() - padding_size as usize;
            (remaining[..payload_len].to_vec(), padding_size)
        } else {
            (remaining.to_vec(), 0)
        };

        Ok(Self {
            header: RtpHeader {
                version,
                padding,
                extension: has_extension,
                csrc_count: csrc_count as u8,
                marker,
                payload_type,
                sequence_number,
                timestamp,
                ssrc,
                csrc,
            },
            extension,
            payload,
            padding_size,
        })
    }

    pub fn build(&self) -> Result<Vec<u8>, RtpError> {
        if self.header.version != 2 {
            return Err(RtpError::UnsupportedVersion {
                actual: self.header.version,
            });
        }

        let csrc_count = self.header.csrc.len();
        if csrc_count > 0x0f {
            return Err(RtpError::TooManyCsrc { count: csrc_count });
        }

        if self.header.padding && self.padding_size == 0 {
            return Err(RtpError::InvalidPadding {
                padding_size: 0,
                available: self.payload.len(),
            });
        }

        let extension_size = self
            .extension
            .as_ref()
            .map(|ext| ext.data.len())
            .unwrap_or(0);
        let extension_words = extension_size.div_ceil(4);
        if extension_words > u16::MAX as usize {
            return Err(RtpError::ExtensionTooLarge {
                bytes: extension_size,
            });
        }

        let has_padding = self.padding_size > 0;
        let has_extension = self.extension.is_some();

        let mut out = Vec::with_capacity(self.size());
        let first = ((self.header.version & 0b0000_0011) << 6)
            | (u8::from(has_padding) << 5)
            | (u8::from(has_extension) << 4)
            | csrc_count as u8;
        out.push(first);

        let second = (u8::from(self.header.marker) << 7) | (self.header.payload_type & 0b0111_1111);
        out.push(second);
        out.extend_from_slice(&self.header.sequence_number.to_be_bytes());
        out.extend_from_slice(&self.header.timestamp.to_be_bytes());
        out.extend_from_slice(&self.header.ssrc.to_be_bytes());

        for csrc in &self.header.csrc {
            out.extend_from_slice(&csrc.to_be_bytes());
        }

        if let Some(extension) = &self.extension {
            out.extend_from_slice(&extension.profile.to_be_bytes());
            out.extend_from_slice(&(extension_words as u16).to_be_bytes());
            out.extend_from_slice(&extension.data);
            let pad_len = extension_words * 4 - extension.data.len();
            if pad_len > 0 {
                out.resize(out.len() + pad_len, 0);
            }
        }

        out.extend_from_slice(&self.payload);

        if has_padding {
            out.resize(out.len() + self.padding_size as usize, 0);
            let last = out.len() - 1;
            out[last] = self.padding_size;
        }

        Ok(out)
    }

    pub fn size(&self) -> usize {
        let mut size = self.header.size();
        if let Some(extension) = &self.extension {
            size += 4 + extension.data.len().div_ceil(4) * 4;
        }
        size += self.payload.len();
        size += self.padding_size as usize;
        size
    }
}

#[cfg(test)]
mod tests {
    use super::{RtpError, RtpHeader, RtpPacket, RTP_FIXED_HEADER_SIZE};

    #[test]
    fn rtp_packet_parse_build_roundtrip_basic() {
        let header = RtpHeader::new(96, 1234, 90_000, 0x1234_5678);
        let packet = RtpPacket::new(header, vec![0x01, 0x02, 0x03, 0x04]);
        let encoded = packet.build().expect("build basic rtp packet");
        let decoded = RtpPacket::parse(&encoded).expect("parse basic rtp packet");

        assert_eq!(decoded.header.version, 2);
        assert_eq!(decoded.header.payload_type, 96);
        assert_eq!(decoded.header.sequence_number, 1234);
        assert_eq!(decoded.header.timestamp, 90_000);
        assert_eq!(decoded.header.ssrc, 0x1234_5678);
        assert_eq!(decoded.payload, vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn rtp_packet_parse_rejects_invalid_version() {
        let mut raw = [0_u8; RTP_FIXED_HEADER_SIZE];
        raw[0] = 0b0100_0000;
        let err = RtpPacket::parse(&raw).expect_err("invalid version must fail");
        assert!(matches!(err, RtpError::UnsupportedVersion { actual: 1 }));
    }

    #[test]
    fn rtp_packet_build_rejects_too_many_csrc() {
        let mut header = RtpHeader::new(96, 1, 90_000, 0x1234_5678);
        header.csrc = (0..16).collect();
        let packet = RtpPacket::new(header, vec![0x01]);

        let err = packet.build().expect_err("too many csrc must fail");
        assert!(matches!(err, RtpError::TooManyCsrc { count: 16 }));
    }

    #[test]
    fn rtp_packet_build_rejects_invalid_version() {
        let mut header = RtpHeader::new(96, 1, 90_000, 0x1234_5678);
        header.version = 3;
        let packet = RtpPacket::new(header, vec![0x01]);

        let err = packet
            .build()
            .expect_err("build must reject unsupported rtp version");
        assert!(matches!(err, RtpError::UnsupportedVersion { actual: 3 }));
    }

    #[test]
    fn rtp_packet_parse_rejects_truncated_extension_payload() {
        let mut raw = vec![0_u8; RTP_FIXED_HEADER_SIZE];
        raw[0] = 0b1001_0000; // version=2 + extension=1
        raw[1] = 96;
        raw.extend_from_slice(&0xBEDE_u16.to_be_bytes());
        raw.extend_from_slice(&1_u16.to_be_bytes()); // length=1 word => 4 bytes

        let err = RtpPacket::parse(&raw).expect_err("truncated extension payload must fail");
        assert!(matches!(
            err,
            RtpError::InsufficientData {
                context: "rtp extension payload",
                ..
            }
        ));
    }

    #[test]
    fn rtp_packet_parse_rejects_invalid_padding_size() {
        let mut raw = vec![0_u8; RTP_FIXED_HEADER_SIZE];
        raw[0] = 0b1010_0000; // version=2 + padding=1
        raw[1] = 96;
        raw.extend_from_slice(&[0x11, 0x22]); // payload
        raw.push(4); // padding_size > available(3) => invalid

        let err = RtpPacket::parse(&raw).expect_err("invalid padding size must fail");
        assert!(matches!(
            err,
            RtpError::InvalidPadding {
                padding_size: 4,
                available: 3
            }
        ));
    }

    #[test]
    fn rtp_packet_build_rejects_padding_flag_without_padding_bytes() {
        let mut header = RtpHeader::new(96, 1, 90_000, 0x1234_5678);
        header.padding = true;
        let packet = RtpPacket::new(header, vec![0x01, 0x02]);

        let err = packet
            .build()
            .expect_err("padding flag without padding bytes must fail");
        assert!(matches!(
            err,
            RtpError::InvalidPadding {
                padding_size: 0,
                available: 2
            }
        ));
    }
}
