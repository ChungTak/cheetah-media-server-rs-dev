//! WebRTC over TCP framing (RFC 4571).
//!
//! WebRTC clients that cannot use UDP (typically because the operator's
//! NAT/firewall environment blocks it) fall back to ICE TCP candidates,
//! which carry STUN, DTLS and SRTP packets over a length-prefixed TCP
//! stream. The framing is defined in [RFC 4571]: each network packet is
//! preceded by a 16-bit big-endian length field. This module implements
//! a small Sans-I/O frame decoder so the driver can hand `WebRtcCore`
//! the same `Bytes` it would have received from UDP.
//!
//! The driver layer owns I/O. The decoder here only operates on byte
//! slices and yields complete packet payloads. The caller is expected
//! to drive `read_from` (or equivalent) until the underlying connection
//! returns EOF or an error.
//!
//! [RFC 4571]: https://datatracker.ietf.org/doc/html/rfc4571
//!
//! WebRTC-over-TCP 框架 (RFC 4571)。
//!
//! 无法使用 UDP（通常是因为运营商的 NAT/防火墙环境阻止它）的 WebRTC 客户端会回退到 ICE TCP candidates，它通过长度前缀 TCP 流携带 STUN、DTLS 和 SRTP 数据包。
//! 成帧在 [RFC 4571] 中定义：每个网络数据包前面都有一个 16 位大端长度字段。
//! 该模块实现了一个小型 Sans-I/O 帧解码器，因此 driver 可以向 `WebRtcCore` 传递与从 UDP 接收到的相同的 `Bytes` 。
//!
//! driver 层拥有 I/O。
//! 这里的解码器仅对字节片进行操作并产生完整的数据包有效负载。
//! 调用者应驱动 `read_from` （或等效项），直到底层连接返回 EOF 或错误。
//!
//! [RFC 4571]：https://datatracker.ietf.org/doc/html/rfc4571

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Maximum frame size we will accept on a single TCP connection.
///
/// 16-bit length means RFC 4571 can in principle carry up to 65535
/// bytes, but real DTLS/SRTP packets stay well below MTU. We use a
/// comfortable cap that matches our UDP `read_buffer_size` default.
///
/// 我们在单个 TCP 连接上接受的最大帧大小。
///
/// 16 位长度意味着 RFC 4571 原则上最多可以承载 65535 个字节，但真正的 DTLS/SRTP 数据包远低于 MTU。
/// 我们使用与我们的 UDP `read_buffer_size` 默认值相匹配的舒适帽子。
pub const TCP_FRAME_MAX_BYTES: usize = 65_535;

/// Streaming RFC 4571 frame decoder.
///
/// Drivers call [`Tcp4571Decoder::extend`] with whatever bytes they
/// just read off the socket and then [`Tcp4571Decoder::next_frame`] in
/// a loop until it returns `None`. Partial frames remain buffered until
/// the next `extend` call.
///
/// 流式 RFC 4571 帧解码器。
///
/// drivers 使用刚刚从套接字读取的任何字节调用 [`Tcp4571Decoder::extend`]，然后循环调用 [`Tcp4571Decoder::next_frame`]
/// ，直到返回 `None`。
/// 部分帧保持缓冲状态，直到下一次 `extend` 调用。
#[derive(Debug, Default)]
pub struct Tcp4571Decoder {
    buf: BytesMut,
    max_frame: usize,
}

impl Tcp4571Decoder {
    /// Create a decoder using the default maximum frame size.
    ///
    /// 使用默认最大帧大小创建解码器。
    pub fn new() -> Self {
        Self::with_max_frame(TCP_FRAME_MAX_BYTES)
    }
    /// Create a decoder with a custom maximum frame size.
    ///
    /// 创建具有自定义最大帧大小的解码器。
    pub fn with_max_frame(max_frame: usize) -> Self {
        Self {
            buf: BytesMut::with_capacity(4096),
            max_frame,
        }
    }

    /// Append newly read bytes from the underlying TCP socket.
    ///
    /// 附加从底层 TCP 套接字新读取的字节。
    pub fn extend(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Pop the next complete frame, if any.
    ///
    /// Returns:
    ///
    /// * `Ok(Some(payload))` when a full frame has been buffered.
    /// * `Ok(None)` when more bytes are required.
    /// * `Err(Tcp4571Error::FrameTooLarge { len })` if the next frame
    ///   header advertises more than `max_frame` bytes. The error is
    ///   terminal — the caller must close the connection because the
    ///   stream is no longer self-synchronising.
    ///
    /// 弹出下一个完整帧（如果有）。
    ///
    /// 返回：
    ///
    /// * `Ok(Some(payload))` 当缓冲完整帧时。
    /// * `Ok(None)` 当需要更多字节时。
    /// * `Err(Tcp4571Error::FrameTooLarge { len })` 如果下一帧标头通告的字节数超过 `max_frame` 字节。
    ///   该错误是致命的——调用者必须关闭连接，因为流不再自同步。
    pub fn next_frame(&mut self) -> Result<Option<Bytes>, Tcp4571Error> {
        if self.buf.len() < 2 {
            return Ok(None);
        }
        let len = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
        if len > self.max_frame {
            return Err(Tcp4571Error::FrameTooLarge { len });
        }
        if self.buf.len() < 2 + len {
            return Ok(None);
        }
        // Drop the length prefix and copy out the payload.
        self.buf.advance(2);
        let payload = self.buf.split_to(len).freeze();
        Ok(Some(payload))
    }

    /// Currently buffered byte count (for diagnostics / backpressure).
    ///
    /// 当前缓冲的字节计数（用于诊断/背压）。
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// Errors produced by [`Tcp4571Decoder::next_frame`].
///
/// [`Tcp4571Decoder::next_frame`] 产生的错误。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Tcp4571Error {
    /// The advertised RFC 4571 frame length exceeds the configured cap.
    ///
    /// 通告的 RFC 4571 帧长度超出了配置的上限。
    #[error("RFC 4571 frame length {len} exceeds configured maximum")]
    FrameTooLarge { len: usize },
}

/// Encode a single packet for transmission over a RFC 4571 TCP stream.
///
/// The output contains the 16-bit big-endian length followed by
/// `payload`. Packets larger than `u16::MAX` cannot be encoded and
/// must be dropped by the caller (a diagnostic suffices).
///
/// 对单个数据包进行编码，以便通过 RFC 4571 TCP 流进行传输。
///
/// 输出包含 16 位大端长度，后跟 `payload`。
/// 大于 `u16::MAX` 的数据包无法编码，必须由调用者丢弃（诊断就足够了）。
pub fn encode_frame(payload: &[u8]) -> Result<Bytes, Tcp4571Error> {
    if payload.len() > TCP_FRAME_MAX_BYTES {
        return Err(Tcp4571Error::FrameTooLarge { len: payload.len() });
    }
    let mut out = BytesMut::with_capacity(2 + payload.len());
    out.put_u16(payload.len() as u16);
    out.extend_from_slice(payload);
    Ok(out.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_single_complete_frame() {
        let mut dec = Tcp4571Decoder::new();
        let raw = b"hello";
        dec.extend(&[0x00, 0x05]);
        dec.extend(raw);
        let frame = dec.next_frame().unwrap().expect("frame");
        assert_eq!(frame.as_ref(), raw);
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn decode_partial_then_complete() {
        let mut dec = Tcp4571Decoder::new();
        // length-only: not a complete frame yet.
        dec.extend(&[0x00, 0x04]);
        assert!(dec.next_frame().unwrap().is_none());
        // half the payload.
        dec.extend(b"AB");
        assert!(dec.next_frame().unwrap().is_none());
        // remaining payload completes the frame.
        dec.extend(b"CD");
        let frame = dec.next_frame().unwrap().expect("frame");
        assert_eq!(frame.as_ref(), b"ABCD");
    }

    #[test]
    fn decode_back_to_back_frames() {
        let mut dec = Tcp4571Decoder::new();
        dec.extend(&[
            0x00, 0x02, b'h', b'i', 0x00, 0x05, b'w', b'o', b'r', b'l', b'd',
        ]);
        let f1 = dec.next_frame().unwrap().expect("first");
        let f2 = dec.next_frame().unwrap().expect("second");
        assert!(dec.next_frame().unwrap().is_none());
        assert_eq!(f1.as_ref(), b"hi");
        assert_eq!(f2.as_ref(), b"world");
    }

    #[test]
    fn decode_zero_length_frame_is_legal() {
        // RFC 4571 allows 0-length frames — they are STUN keepalives
        // in some implementations. The decoder must surface them so
        // the driver can either pass them along or drop them as the
        // upper layer wishes.
        let mut dec = Tcp4571Decoder::new();
        dec.extend(&[0x00, 0x00]);
        let frame = dec.next_frame().unwrap().expect("zero-length frame");
        assert_eq!(frame.as_ref(), b"");
    }

    #[test]
    fn decode_rejects_frame_larger_than_max() {
        let mut dec = Tcp4571Decoder::with_max_frame(8);
        dec.extend(&[0x00, 0x10]); // advertises 16 bytes.
        let err = dec.next_frame().unwrap_err();
        assert_eq!(err, Tcp4571Error::FrameTooLarge { len: 16 });
    }

    #[test]
    fn encode_roundtrip() {
        let payload = b"some-stun-or-dtls-bytes";
        let encoded = encode_frame(payload).unwrap();
        let mut dec = Tcp4571Decoder::new();
        dec.extend(&encoded);
        let decoded = dec.next_frame().unwrap().expect("frame");
        assert_eq!(decoded.as_ref(), payload);
    }

    #[test]
    fn encode_rejects_oversize_payload() {
        let payload = vec![0u8; TCP_FRAME_MAX_BYTES + 1];
        let err = encode_frame(&payload).unwrap_err();
        assert!(matches!(err, Tcp4571Error::FrameTooLarge { .. }));
    }
}
