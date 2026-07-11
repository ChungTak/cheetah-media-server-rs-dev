use alloc::string::String;
use alloc::vec::Vec;

use crate::error::Error;

/// Trait for reading fixed-width big-endian values from a byte slice.
/// 从字节切片读取固定宽度大端值。
pub trait BytesReader {
    /// Reads an unsigned 8-bit value.
    /// 读取 8 位无符号值。
    fn read_u8(&mut self) -> Result<u8, Error>;
    /// Reads an unsigned 16-bit big-endian value.
    /// 读取 16 位无符号大端值。
    fn read_u16(&mut self) -> Result<u16, Error>;
    /// Reads an unsigned 24-bit big-endian value.
    /// 读取 24 位无符号大端值。
    fn read_u24(&mut self) -> Result<u32, Error>;
    /// Reads a signed 24-bit big-endian value, sign-extended to 32 bits.
    /// 读取 24 位有符号大端值，并符号扩展为 32 位。
    fn read_i24(&mut self) -> Result<i32, Error>;
    /// Reads an unsigned 32-bit big-endian value.
    /// 读取 32 位无符号大端值。
    fn read_u32(&mut self) -> Result<u32, Error>;
    /// Reads a signed 32-bit big-endian value.
    /// 读取 32 位有符号大端值。
    fn read_i32(&mut self) -> Result<i32, Error>;
    /// Reads an IEEE-754 double-precision big-endian value.
    /// 读取 IEEE-754 双精度大端值。
    fn read_f64(&mut self) -> Result<f64, Error>;
    /// Reads `len` bytes and returns them as a `Vec<u8)`.
    /// 读取 `len` 字节并返回为 `Vec<u8>`。
    fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, Error>;
    /// Reads `len` bytes and decodes them as a UTF-8 string.
    /// 读取 `len` 字节并解码为 UTF-8 字符串。
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

/// Trait for writing fixed-width big-endian values into a byte buffer.
/// 将固定宽度大端值写入字节缓冲区。
pub trait BytesWriter {
    /// Writes an unsigned 8-bit value.
    /// 写入 8 位无符号值。
    fn write_u8(&mut self, v: u8);
    /// Writes an unsigned 16-bit big-endian value.
    /// 写入 16 位无符号大端值。
    fn write_u16(&mut self, v: u16);
    /// Writes the lower 24 bits of an unsigned 32-bit value as big-endian.
    /// 写入 32 位无符号值的低 24 位大端值。
    fn write_u24(&mut self, v: u32);
    /// Writes the lower 24 bits of a signed 32-bit value as big-endian.
    /// 写入 32 位有符号值的低 24 位大端值。
    fn write_i24(&mut self, v: i32);
    /// Writes an unsigned 32-bit big-endian value.
    /// 写入 32 位无符号大端值。
    fn write_u32(&mut self, v: u32);
    /// Writes a signed 32-bit big-endian value.
    /// 写入 32 位有符号大端值。
    fn write_i32(&mut self, v: i32);
    /// Writes an IEEE-754 double-precision big-endian value.
    /// 写入 IEEE-754 双精度大端值。
    fn write_f64(&mut self, v: f64);
    /// Writes a raw byte slice.
    /// 写入原始字节切片。
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

/// A growing byte buffer that tracks consumed bytes to support incremental decoding.
/// 支持增量解码的增长型字节缓冲区，追踪已消费字节。
#[derive(Debug, Default)]
pub struct Buf {
    bytes: Vec<u8>,
    offset: usize,
}

impl Buf {
    /// Returns the unconsumed bytes starting from the current offset.
    /// 返回从当前 offset 开始的未消费字节。
    pub fn get(&self) -> &[u8] {
        &self.bytes[self.offset..]
    }

    /// Returns mutable access to the underlying storage.
    /// 返回对底层存储的可变访问。
    pub fn inner_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }

    /// Appends bytes to the buffer and may compact the storage when it exceeds 1 MiB.
    /// 将字节追加到缓冲区，并在超过 1 MiB 时压缩存储。
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
    /// Advances the consumed offset by `n` bytes, resetting the buffer when fully consumed.
    /// 将已消费偏移量前进 `n` 字节，当完全消费后重置缓冲区。
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
