//! Minimal PNG header parser for content validation.
//!
//! 用于内容校验的最小化 PNG 头部解析器。

use super::ImageDimensions;

const PNG_SIGNATURE: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const IHDR_TYPE: &[u8] = b"IHDR";

/// Parse width and height from a PNG bytestream.
///
/// Returns `None` if the data is not a valid PNG or contains no IHDR chunk.
///
/// 从 PNG 字节流中解析宽高。若不是合法 PNG 或无 IHDR chunk 则返回 `None`。
pub fn parse_dimensions(data: &[u8]) -> Option<ImageDimensions> {
    if !data.starts_with(PNG_SIGNATURE) {
        return None;
    }

    let mut i = PNG_SIGNATURE.len();
    while i + 12 <= data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let chunk_type = &data[i + 4..i + 8];
        if chunk_type == IHDR_TYPE && i + 24 <= data.len() {
            let width = u32::from_be_bytes([data[i + 8], data[i + 9], data[i + 10], data[i + 11]]);
            let height =
                u32::from_be_bytes([data[i + 12], data[i + 13], data[i + 14], data[i + 15]]);
            if width == 0 || height == 0 {
                return None;
            }
            return Some(ImageDimensions { width, height });
        }

        // Skip chunk: length field (4) + type (4) + data + CRC (4).
        let skip = 12 + len;
        if skip == 0 || i + skip > data.len() {
            return None;
        }
        i += skip;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_png(width: u32, height: u32) -> Vec<u8> {
        let mut data = PNG_SIGNATURE.to_vec();
        // IHDR length (13)
        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(IHDR_TYPE);
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        // bit depth, color type, compression, filter, interlace
        data.extend_from_slice(&[8, 2, 0, 0, 0]);
        // CRC placeholder (4 bytes, ignored by this minimal parser)
        data.extend_from_slice(&[0, 0, 0, 0]);
        data
    }

    #[test]
    fn parse_minimal_png() {
        let data = minimal_png(800, 600);
        let dims = parse_dimensions(&data).unwrap();
        assert_eq!(dims.width, 800);
        assert_eq!(dims.height, 600);
    }

    #[test]
    fn reject_non_png() {
        assert!(parse_dimensions(b"not a png").is_none());
    }

    #[test]
    fn reject_zero_dimensions() {
        let data = minimal_png(0, 600);
        assert!(parse_dimensions(&data).is_none());
    }
}
