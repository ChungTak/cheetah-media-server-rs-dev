//! Minimal JPEG header parser for content validation.
//!
//! 用于内容校验的最小化 JPEG 头部解析器。

use super::ImageDimensions;

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOF_MARKERS: &[u8] = &[
    0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
];

/// Parse width and height from a JPEG bytestream.
///
/// Returns `None` if the data is not a valid JPEG or contains no SOF marker.
///
/// 从 JPEG 字节流中解析宽高。若不是合法 JPEG 或无 SOF 标记则返回 `None`。
pub fn parse_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != SOI {
        return None;
    }

    let mut i = 2;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }

        let marker = data[i + 1];
        if marker == 0x00 || (0xD0..=0xD9).contains(&marker) {
            // Stuffing byte, restart marker, or standalone marker with no payload.
            i += 2;
            continue;
        }

        if marker == EOI {
            return None;
        }

        // Read segment length (including the length field itself).
        if i + 3 >= data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return None;
        }

        if SOF_MARKERS.contains(&marker) && len >= 7 {
            // SOF segment: precision (1) + height (2) + width (2).
            let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            if width == 0 || height == 0 {
                return None;
            }
            return Some(ImageDimensions { width, height });
        }

        i += 2 + len;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sof0(width: u16, height: u16) -> Vec<u8> {
        // Minimal SOF0 segment: length (7), precision (8), height, width, components (1).
        let mut seg = vec![0xFF, 0xC0, 0x00, 0x07, 0x08];
        seg.extend_from_slice(&height.to_be_bytes());
        seg.extend_from_slice(&width.to_be_bytes());
        seg.push(0x01);
        seg
    }

    fn minimal_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8]; // SOI
        data.extend_from_slice(&sof0(width, height));
        data.extend_from_slice(&[0xFF, 0xD9]); // EOI
        data
    }

    #[test]
    fn parse_minimal_jpeg() {
        let data = minimal_jpeg(640, 480);
        let dims = parse_dimensions(&data).unwrap();
        assert_eq!(dims.width, 640);
        assert_eq!(dims.height, 480);
    }

    #[test]
    fn reject_non_jpeg() {
        assert!(parse_dimensions(b"not a jpeg").is_none());
    }

    #[test]
    fn reject_zero_dimensions() {
        let data = minimal_jpeg(0, 480);
        assert!(parse_dimensions(&data).is_none());
    }
}
