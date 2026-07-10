//! Generic ISO BMFF box parser/serializer helpers.

use crate::prelude::*;
use bytes::{BufMut, BytesMut};

use super::Mp4Error;

/// Maximum default box size enforced when parsing untrusted content.
pub const DEFAULT_MAX_BOX_SIZE: u64 = 256 * 1024 * 1024;

/// A parsed box header (size + 4cc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxHeader {
    /// `fourcc` field of type `[u8; 4]`.
    /// `fourcc` 字段，类型为 `[u8; 4]`.
    pub fourcc: [u8; 4],
    /// `size` field of type `u64`.
    /// `size` 字段，类型为 `u64`.
    pub size: u64,
    /// `header_size` field of type `u8`.
    /// `header_size` 字段，类型为 `u8`.
    pub header_size: u8,
    /// `is_extends_to_eof` field of type `bool`.
    /// `is_extends_to_eof` 字段，类型为 `bool`.
    pub is_extends_to_eof: bool,
}

impl BoxHeader {
    /// `type_str` function.
    /// `type_str` 函数.
    pub fn type_str(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }

    /// `payload_size` function.
    /// `payload_size` 函数.
    pub fn payload_size(&self) -> u64 {
        self.size.saturating_sub(self.header_size as u64)
    }
}

/// Read a single box header from `buf` starting at `offset`. Returns the parsed
/// header and the offset of the payload (i.e. `offset + header_size`).
///
/// Allowed encodings:
///   * `size32(4) + type(4)` (compact)
///   * `size32 == 1 -> size64(4) + type(4) + largesize(8)` (extended)
///   * `size32 == 0 -> extends to end of file/parent` (`is_extends_to_eof`)
///
/// `uuid` boxes carry an additional 16-byte usertype that callers must skip
/// based on `header_size`. The caller must validate the parent boundary and
/// use `payload_size()` rather than treating the header as opaque.
pub fn read_box_header(
    buf: &[u8],
    offset: usize,
    parent_end: usize,
    max_box_size: u64,
) -> Result<BoxHeader, Mp4Error> {
    if offset + 8 > parent_end {
        return Err(Mp4Error::InvalidBox {
            offset: offset as u64,
            detail: "header truncated",
        });
    }
    let size32 = u32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]);
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&buf[offset + 4..offset + 8]);

    let (size, header_size, is_extends_to_eof) = match size32 {
        0 => {
            let size = (parent_end - offset) as u64;
            (size, 8u8, true)
        }
        1 => {
            if offset + 16 > parent_end {
                return Err(Mp4Error::InvalidBox {
                    offset: offset as u64,
                    detail: "largesize truncated",
                });
            }
            let large = u64::from_be_bytes([
                buf[offset + 8],
                buf[offset + 9],
                buf[offset + 10],
                buf[offset + 11],
                buf[offset + 12],
                buf[offset + 13],
                buf[offset + 14],
                buf[offset + 15],
            ]);
            (large, 16u8, false)
        }
        n => (n as u64, 8u8, false),
    };

    if size < header_size as u64 {
        return Err(Mp4Error::InvalidBox {
            offset: offset as u64,
            detail: "size smaller than header",
        });
    }
    if size > max_box_size {
        return Err(Mp4Error::OversizeBox {
            fourcc: String::from_utf8_lossy(&fourcc).into_owned(),
            size,
            limit: max_box_size,
        });
    }
    if (offset as u64) + size > parent_end as u64 {
        return Err(Mp4Error::BoxTruncated {
            fourcc: String::from_utf8_lossy(&fourcc).into_owned(),
            need: size,
            have: (parent_end - offset) as u64,
        });
    }
    Ok(BoxHeader {
        fourcc,
        size,
        header_size,
        is_extends_to_eof,
    })
}

/// Write a 4cc box with size header to `buf`. The closure receives the buffer
/// and writes the payload; this helper patches the box size field afterwards.
pub fn write_box<F>(buf: &mut BytesMut, fourcc: &[u8; 4], body: F)
where
    F: FnOnce(&mut BytesMut),
{
    let start = buf.len();
    buf.put_u32(0);
    buf.extend_from_slice(fourcc);
    body(buf);
    let size = (buf.len() - start) as u32;
    buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

/// Write a "full box" (version + flags) header.
pub fn write_full_box<F>(buf: &mut BytesMut, fourcc: &[u8; 4], version: u8, flags: u32, body: F)
where
    F: FnOnce(&mut BytesMut),
{
    write_box(buf, fourcc, |buf| {
        buf.put_u8(version);
        buf.put_u8((flags >> 16) as u8);
        buf.put_u8((flags >> 8) as u8);
        buf.put_u8(flags as u8);
        body(buf);
    });
}

/// Iterate over child boxes inside a parent box payload.
///
/// Skips unknown boxes; returns the slice payload of every child box.
pub struct BoxIter<'a> {
    /// `buf` field of type `&'a [u8]`.
    /// `buf` 字段，类型为 `&'一个 [u8]`.
    pub buf: &'a [u8],
    /// `offset` field of type `usize`.
    /// `offset` 字段，类型为 `usize`.
    pub offset: usize,
    /// `parent_end` field of type `usize`.
    /// `parent_end` 字段，类型为 `usize`.
    pub parent_end: usize,
    /// `max_box_size` field of type `u64`.
    /// `max_box_size` 字段，类型为 `u64`.
    pub max_box_size: u64,
}

impl<'a> BoxIter<'a> {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(buf: &'a [u8], offset: usize, parent_end: usize, max_box_size: u64) -> Self {
        Self {
            buf,
            offset,
            parent_end,
            max_box_size,
        }
    }
}

/// `ChildBox` data structure.
/// `ChildBox` 数据结构.
#[derive(Debug, Clone)]
pub struct ChildBox<'a> {
    /// `header` field of type `BoxHeader`.
    /// `header` 字段，类型为 `BoxHeader`.
    pub header: BoxHeader,
    /// `payload` field of type `&'a [u8]`.
    /// `payload` 字段，类型为 `&'一个 [u8]`.
    pub payload: &'a [u8],
    /// `absolute_offset` field of type `u64`.
    /// `absolute_offset` 字段，类型为 `u64`.
    pub absolute_offset: u64,
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = Result<ChildBox<'a>, Mp4Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.parent_end {
            return None;
        }
        let header =
            match read_box_header(self.buf, self.offset, self.parent_end, self.max_box_size) {
                Ok(h) => h,
                Err(e) => return Some(Err(e)),
            };
        let payload_start = self.offset + header.header_size as usize;
        let box_end = self.offset + header.size as usize;
        let payload = &self.buf[payload_start..box_end];
        let abs_offset = self.offset as u64;
        self.offset = box_end;
        Some(Ok(ChildBox {
            header,
            payload,
            absolute_offset: abs_offset,
        }))
    }
}

/// Read a big-endian u32 from `buf` at `offset`. Returns `Mp4Error` on
/// underflow.
pub fn read_u32(buf: &[u8], offset: usize) -> Result<u32, Mp4Error> {
    if offset + 4 > buf.len() {
        return Err(Mp4Error::InvalidBox {
            offset: offset as u64,
            detail: "u32 underflow",
        });
    }
    Ok(u32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

/// `read_u64` function.
/// `read_u64` 函数.
pub fn read_u64(buf: &[u8], offset: usize) -> Result<u64, Mp4Error> {
    if offset + 8 > buf.len() {
        return Err(Mp4Error::InvalidBox {
            offset: offset as u64,
            detail: "u64 underflow",
        });
    }
    Ok(u64::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ]))
}

/// `read_u16` function.
/// `read_u16` 函数.
pub fn read_u16(buf: &[u8], offset: usize) -> Result<u16, Mp4Error> {
    if offset + 2 > buf.len() {
        return Err(Mp4Error::InvalidBox {
            offset: offset as u64,
            detail: "u16 underflow",
        });
    }
    Ok(u16::from_be_bytes([buf[offset], buf[offset + 1]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn parses_simple_box_header() {
        let mut buf = BytesMut::new();
        write_box(&mut buf, b"ftyp", |buf| {
            buf.extend_from_slice(b"isom");
        });
        let h = read_box_header(&buf, 0, buf.len(), DEFAULT_MAX_BOX_SIZE).expect("header");
        assert_eq!(h.fourcc, *b"ftyp");
        assert_eq!(h.size, 12);
        assert_eq!(h.header_size, 8);
        assert!(!h.is_extends_to_eof);
    }

    #[test]
    fn parses_large_box_header() {
        let mut buf = BytesMut::new();
        // size=1, type=mdat, largesize=24, payload 8 bytes
        buf.put_u32(1);
        buf.extend_from_slice(b"mdat");
        buf.put_u64(24);
        buf.extend_from_slice(b"AAAAAAAA");
        let h = read_box_header(&buf, 0, buf.len(), DEFAULT_MAX_BOX_SIZE).expect("header");
        assert_eq!(h.fourcc, *b"mdat");
        assert_eq!(h.size, 24);
        assert_eq!(h.header_size, 16);
    }

    #[test]
    fn rejects_oversized_box() {
        let mut buf = BytesMut::new();
        buf.put_u32(0xFFFF_FFFF);
        buf.extend_from_slice(b"junk");
        let err = read_box_header(&buf, 0, buf.len(), 1024).unwrap_err();
        assert!(matches!(err, Mp4Error::OversizeBox { .. }));
    }

    #[test]
    fn iter_skips_through_boxes() {
        let mut buf = BytesMut::new();
        write_box(&mut buf, b"ftyp", |buf| {
            buf.extend_from_slice(b"isom");
        });
        write_box(&mut buf, b"free", |_buf| {});
        let mut iter = BoxIter::new(&buf, 0, buf.len(), DEFAULT_MAX_BOX_SIZE);
        let first = iter.next().unwrap().unwrap();
        assert_eq!(first.header.fourcc, *b"ftyp");
        let second = iter.next().unwrap().unwrap();
        assert_eq!(second.header.fourcc, *b"free");
        assert!(iter.next().is_none());
    }
}
