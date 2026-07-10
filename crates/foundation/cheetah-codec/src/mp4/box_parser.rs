//! Generic ISO BMFF box parser/serializer helpers.
//!
//! 通用 ISO BMFF Box 解析/序列化辅助函数。

use crate::prelude::*;
use bytes::{BufMut, BytesMut};

use super::Mp4Error;

/// Maximum default box size enforced when parsing untrusted content.
///
/// 解析不可信内容时默认允许的最大 Box 大小。
pub const DEFAULT_MAX_BOX_SIZE: u64 = 256 * 1024 * 1024;

/// A parsed box header (size + 4cc).
///
/// 已解析的 Box 头部（大小 + 4cc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxHeader {
    pub fourcc: [u8; 4],
    pub size: u64,
    pub header_size: u8,
    pub is_extends_to_eof: bool,
}

impl BoxHeader {
    /// 4cc as a printable string for diagnostics.
    ///
    /// 4cc 的可打印字符串，用于诊断。
    pub fn type_str(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }

    /// Payload size in bytes (total size minus header size).
    ///
    /// 负载字节大小（总大小减去头部大小）。
    pub fn payload_size(&self) -> u64 {
        self.size.saturating_sub(self.header_size as u64)
    }
}

/// Parse a box header at `offset` inside the parent `[0, parent_end)` range.
///
/// Handles compact 32-bit size, extended 64-bit size (`size == 1`), and
/// end-of-parent size (`size == 0`). `uuid` boxes are reported with a 16-byte
/// header; callers must account for the extra bytes.
///
/// 在父范围 `[0, parent_end)` 的 `offset` 处解析 Box 头部。
///
/// 处理紧凑 32 位大小、扩展 64 位大小（`size == 1`）以及父容器结尾大小（`size == 0`）。
/// `uuid` Box 的头部大小为 16 字节；调用方需额外处理。
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
/// Write a 4cc box to `buf`, with `body` filling the payload.
///
/// The size field is patched after the body is written.
///
/// 向 `buf` 写入 4cc Box，由 `body` 填充负载。
///
/// 在 `body` 写入后回填大小字段。
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
/// Write a full box (version + 24-bit flags) to `buf`.
///
/// 写入 full box（版本 + 24 位标志）。
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
///
/// 迭代父 Box 负载内部的子 Box。
///
/// 跳过未知 Box，返回每个子 Box 的切片负载。
pub struct BoxIter<'a> {
    pub buf: &'a [u8],
    pub offset: usize,
    pub parent_end: usize,
    pub max_box_size: u64,
}

impl<'a> BoxIter<'a> {
    /// Create an iterator over the children of the parent range.
    ///
    /// 创建父范围子 Box 迭代器。
    pub fn new(buf: &'a [u8], offset: usize, parent_end: usize, max_box_size: u64) -> Self {
        Self {
            buf,
            offset,
            parent_end,
            max_box_size,
        }
    }
}

/// A child box with its parsed header and payload slice.
///
/// 包含已解析头部与负载切片的子 Box。
#[derive(Debug, Clone)]
pub struct ChildBox<'a> {
    pub header: BoxHeader,
    pub payload: &'a [u8],
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
/// Read a big-endian u32 at `offset` or return an underflow error.
///
/// 在 `offset` 处读取大端 u32，越界时返回 underflow 错误。
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

/// Read a big-endian u64 at `offset` or return an underflow error.
///
/// 在 `offset` 处读取大端 u64，越界时返回 underflow 错误。
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

/// Read a big-endian u16 at `offset` or return an underflow error.
///
/// 在 `offset` 处读取大端 u16，越界时返回 underflow 错误。
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
