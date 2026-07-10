use alloc::string::String;
use alloc::vec::Vec;

use crate::error::Error;

/// `BytesReader` trait.
/// `BytesReader` trait。
pub trait BytesReader {
    fn read_u8(&mut self) -> Result<u8, Error>;
    fn read_u16(&mut self) -> Result<u16, Error>;
    fn read_u24(&mut self) -> Result<u32, Error>;
    fn read_i24(&mut self) -> Result<i32, Error>;
    fn read_u32(&mut self) -> Result<u32, Error>;
    fn read_i32(&mut self) -> Result<i32, Error>;
    fn read_f64(&mut self) -> Result<f64, Error>;
    fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, Error>;
    fn read_utf8(&mut self, len: usize) -> Result<String, Error>;
}

impl BytesReader for &[u8] {
    #[track_caller]
    fn read_u8(&mut self) -> Result<u8, Error> {
        Error::check_buffer_size(1, self)?;
        let v = self[0];
        *self = &self[1..];
        Ok(v)
    }

    #[track_caller]
    fn read_u16(&mut self) -> Result<u16, Error> {
        Error::check_buffer_size(2, self)?;
        let bytes = [self[0], self[1]];
        *self = &self[2..];
        Ok(u16::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_u24(&mut self) -> Result<u32, Error> {
        Error::check_buffer_size(3, self)?;
        let bytes = [0, self[0], self[1], self[2]];
        *self = &self[3..];
        Ok(u32::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_i24(&mut self) -> Result<i32, Error> {
        Error::check_buffer_size(3, self)?;
        let bytes = [
            if self[0] & 0x80 != 0 { 0xFF } else { 0x00 },
            self[0],
            self[1],
            self[2],
        ];
        *self = &self[3..];
        Ok(i32::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_u32(&mut self) -> Result<u32, Error> {
        Error::check_buffer_size(4, self)?;
        let bytes = [self[0], self[1], self[2], self[3]];
        *self = &self[4..];
        Ok(u32::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_i32(&mut self) -> Result<i32, Error> {
        Error::check_buffer_size(4, self)?;
        let bytes = [self[0], self[1], self[2], self[3]];
        *self = &self[4..];
        Ok(i32::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_f64(&mut self) -> Result<f64, Error> {
        Error::check_buffer_size(8, self)?;
        let bytes = [
            self[0], self[1], self[2], self[3], self[4], self[5], self[6], self[7],
        ];
        *self = &self[8..];
        Ok(f64::from_be_bytes(bytes))
    }

    #[track_caller]
    fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        Error::check_buffer_size(len, self)?;
        let buf = self[..len].to_vec();
        *self = &self[len..];
        Ok(buf)
    }

    #[track_caller]
    fn read_utf8(&mut self, len: usize) -> Result<String, Error> {
        let buf = self.read_bytes(len)?;
        String::from_utf8(buf).map_err(|e| Error::invalid_data(format!("invalid UTF-8 bytes: {e}")))
    }
}

/// `BytesWriter` trait.
/// `BytesWriter` trait。
pub trait BytesWriter {
    fn write_u8(&mut self, v: u8);
    fn write_u16(&mut self, v: u16);
    fn write_u24(&mut self, v: u32);
    fn write_i24(&mut self, v: i32);
    fn write_u32(&mut self, v: u32);
    fn write_i32(&mut self, v: i32);
    fn write_f64(&mut self, v: f64);
    fn write_bytes(&mut self, v: &[u8]);
}

impl BytesWriter for Vec<u8> {
    fn write_u8(&mut self, v: u8) {
        self.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn write_u24(&mut self, v: u32) {
        let bytes = v.to_be_bytes();
        self.extend_from_slice(&bytes[1..4]);
    }

    fn write_i24(&mut self, v: i32) {
        let bytes = v.to_be_bytes();
        self.extend_from_slice(&bytes[1..4]);
    }

    fn write_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn write_i32(&mut self, v: i32) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn write_f64(&mut self, v: f64) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn write_bytes(&mut self, v: &[u8]) {
        self.extend_from_slice(v);
    }
}

/// `Buf` data structure.
/// `Buf` 数据结构。
#[derive(Debug, Default)]
pub struct Buf {
    bytes: Vec<u8>,
    offset: usize,
}

impl Buf {
    /// Returns the value.
    /// 返回值。
    pub fn get(&self) -> &[u8] {
        &self.bytes[self.offset..]
    }

    /// `inner_mut` function of `Buf`.
    /// `Buf` 的 `inner_mut` 函数。
    pub fn inner_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }

    /// `feed` function of `Buf`.
    /// `Buf` 的 `feed` 函数。
    pub fn feed(&mut self, buf: &[u8]) {
        self.bytes.extend_from_slice(buf);

        // 为了防止 bytes 的尺寸在时机不好时无限增长，
        // 超过 1 MB 时，将数据移动到开头并将 offset 重置为 0
        // （基本上这种情况不会发生，但作为保险处理）
        const MAX_SIZE: usize = 1024 * 1024; // 1 MB
        if self.offset > 0 && self.bytes.len() > MAX_SIZE {
            let remaining = self.bytes.len() - self.offset;
            self.bytes.copy_within(self.offset.., 0);
            self.bytes.truncate(remaining);
            self.offset = 0;
        }
    }

    // [NOTE]
    // 当调用指定了大于 `Buf::get().len()` 的值作为 n 时，
    // 后续处理可能会 panic
    // （`Buf` 是内部结构体，正确使用该方法是调用方的责任，因此此处不做错误检查）
    /// `advance` function of `Buf`.
    /// `Buf` 的 `advance` 函数。
    pub fn advance(&mut self, n: usize) {
        debug_assert!(
            n <= self.bytes.len() - self.offset,
            "Buf::advance({n}) exceeds remaining bytes (offset={}, len={})",
            self.offset,
            self.bytes.len()
        );
        self.offset += n;
        if self.offset == self.bytes.len() {
            self.offset = 0;
            self.bytes.clear(); // [NOTE] bytes 本身的 capacity 不会改变
        }
    }
}
