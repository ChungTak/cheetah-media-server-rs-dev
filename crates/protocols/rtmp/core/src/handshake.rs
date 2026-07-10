use alloc::vec::Vec;

use crate::bytes::{BytesReader, BytesWriter};
use crate::error::Error;

// [NOTE]
// 在 Flash Player 已被废弃的今天，
// 细致控制握手参数已没有意义，
// 在出现问题之前先使用固定值
const RTMP_VERSION: u8 = 3;
const HANDSHAKE_PACKET_SIZE: usize = 1536;
const APP_VERSION: [u8; 4] = [0, 0, 0, 0]; // 使用最小值，从而无需支持后来引入的摘要格式
const RANDOM_DATA: [u8; HANDSHAKE_PACKET_SIZE - 8] = [0; HANDSHAKE_PACKET_SIZE - 8]; // 固定值不会造成问题，因此使用固定值
const TIMESTAMP: u32 = 0; // 同上

#[derive(Debug, Clone)]
struct RtmpHandshakeOptions {
    app_version: [u8; 4],
    timestamp: u32,
    random_data: [u8; HANDSHAKE_PACKET_SIZE - 8],
}

impl RtmpHandshakeOptions {
    fn phase1_packet(&self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(HANDSHAKE_PACKET_SIZE);

        packet.write_u32(self.timestamp);
        packet.write_u32(u32::from_be_bytes(self.app_version));
        packet.write_bytes(&self.random_data);

        packet
    }
}

impl Default for RtmpHandshakeOptions {
    fn default() -> Self {
        Self {
            app_version: APP_VERSION,
            timestamp: TIMESTAMP,
            random_data: RANDOM_DATA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerHandshakeMode {
    Strict,
    LenientSeededS1,
}

/// Mode selecting `Client Handshake` behavior.
/// 选择 `Client Handshake` 行为的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientHandshakeMode {
    Strict,
    Lenient,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    P0,
    P1,
    P2,
    Complete,
}

/// `RtmpServerHandshake` data structure.
/// `RtmpServerHandshake` 数据结构。
#[derive(Debug)]
pub struct RtmpServerHandshake {
    options: RtmpHandshakeOptions,
    mode: ServerHandshakeMode,
    phase: Phase,
    recv_buf: Vec<u8>,
    send_buf: Vec<u8>,
    s1_packet: Vec<u8>,
}

impl RtmpServerHandshake {
    /// Creates a new `RtmpServerHandshake` instance.
    /// 创建新的 `RtmpServerHandshake` 实例。
    pub fn new() -> Self {
        Self::new_with_mode(ServerHandshakeMode::Strict)
    }

    /// Creates a new `lenient seeded s 1` instance.
    /// 创建新的 `lenient seeded s 1` 实例。
    pub fn new_lenient_seeded_s1() -> Self {
        Self::new_with_mode(ServerHandshakeMode::LenientSeededS1)
    }

    fn new_with_mode(mode: ServerHandshakeMode) -> Self {
        Self {
            options: RtmpHandshakeOptions::default(),
            mode,
            phase: Phase::P0,
            recv_buf: Vec::new(),
            send_buf: Vec::new(),
            s1_packet: Vec::new(),
        }
    }

    /// `feed_recv_buf` function of `RtmpServerHandshake`.
    /// `RtmpServerHandshake` 的 `feed_recv_buf` 函数。
    pub fn feed_recv_buf(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.recv_buf.extend_from_slice(buf);
        self.handle_recv_buf()?;
        Ok(())
    }

    fn handle_recv_buf(&mut self) -> Result<(), Error> {
        match self.phase {
            Phase::P0 => self.handle_phase_p0(),
            Phase::P1 => self.handle_phase_p1(),
            Phase::P2 => self.handle_phase_p2(),
            Phase::Complete => Ok(()),
        }
    }

    fn handle_phase_p0(&mut self) -> Result<(), Error> {
        if self.recv_buf.is_empty() {
            return Ok(());
        }

        let mut buf: &[u8] = &self.recv_buf;
        let client_rtmp_version = buf.read_u8()?;
        let consumed = self.recv_buf.len() - buf.len();
        self.recv_buf.drain(..consumed);

        if client_rtmp_version != RTMP_VERSION {
            return Err(Error::invalid_data(format!(
                "invalid RTMP version: expected {RTMP_VERSION}, got {client_rtmp_version}"
            )));
        }

        // SEND: S0
        self.send_buf.write_u8(RTMP_VERSION);

        self.phase = Phase::P1;
        self.handle_phase_p1()
    }

    fn handle_phase_p1(&mut self) -> Result<(), Error> {
        if self.recv_buf.len() < HANDSHAKE_PACKET_SIZE {
            return Ok(());
        }

        let mut buf: &[u8] = &self.recv_buf;
        let c1_packet = buf.read_bytes(HANDSHAKE_PACKET_SIZE)?;
        let consumed = self.recv_buf.len() - buf.len();
        self.recv_buf.drain(..consumed);

        // Try complex handshake detection (feature-gated)
        #[cfg(feature = "complex-handshake")]
        if let Some(scheme) = crate::handshake_complex::detect_client_scheme(&c1_packet) {
            let s1 = crate::handshake_complex::build_complex_s1(&c1_packet, scheme);
            let s2 = crate::handshake_complex::build_complex_s2(&c1_packet, scheme);
            self.s1_packet = s1.to_vec();
            self.send_buf.write_bytes(&s1);
            self.send_buf.write_bytes(&s2);
            self.phase = Phase::P2;
            return self.handle_phase_p2();
        }

        // SEND: S1, S2 (simple handshake)
        self.s1_packet = match self.mode {
            ServerHandshakeMode::Strict => self.options.phase1_packet(),
            ServerHandshakeMode::LenientSeededS1 => build_lenient_seeded_s1(&c1_packet).to_vec(),
        };
        self.send_buf.write_bytes(&self.s1_packet);
        self.send_buf.write_bytes(&c1_packet);

        self.phase = Phase::P2;
        self.handle_phase_p2()
    }

    fn handle_phase_p2(&mut self) -> Result<(), Error> {
        if self.recv_buf.len() < HANDSHAKE_PACKET_SIZE {
            return Ok(());
        }

        let mut buf: &[u8] = &self.recv_buf;
        let c2_packet = buf.read_bytes(HANDSHAKE_PACKET_SIZE)?;
        let consumed = self.recv_buf.len() - buf.len();
        self.recv_buf.drain(..consumed);

        if self.mode == ServerHandshakeMode::Strict && self.s1_packet != c2_packet {
            return Err(Error::invalid_data("C2 packet does not match S1 packet"));
        }

        self.phase = Phase::Complete;
        Ok(())
    }

    /// Takes `recv buf`, replacing it with the default.
    /// 获取 `recv buf`，并用默认值替换。
    pub fn take_recv_buf(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.recv_buf)
    }

    /// Sends `buf` to the peer.
    /// 向对端发送 `buf`。
    pub fn send_buf(&self) -> &[u8] {
        &self.send_buf
    }

    /// `advance_send_buf` function of `RtmpServerHandshake`.
    /// `RtmpServerHandshake` 的 `advance_send_buf` 函数。
    pub fn advance_send_buf(&mut self, n: usize) {
        let n = n.min(self.send_buf.len());
        self.send_buf.drain(..n); // NOTE: 效率不高，但不是需要关注性能的地方，因此优先简洁
    }

    /// Returns `true` if `recv complete` is true.
    /// 当 `recv complete` 为真时返回 `true`。
    pub fn is_recv_complete(&self) -> bool {
        self.phase == Phase::Complete
    }

    /// Returns `true` if `send complete` is true.
    /// 当 `send complete` 为真时返回 `true`。
    pub fn is_send_complete(&self) -> bool {
        self.phase == Phase::Complete && self.send_buf.is_empty()
    }
}

impl Default for RtmpServerHandshake {
    fn default() -> Self {
        Self::new()
    }
}

/// `RtmpClientHandshake` data structure.
/// `RtmpClientHandshake` 数据结构。
#[derive(Debug)]
pub struct RtmpClientHandshake {
    options: RtmpHandshakeOptions,
    mode: ClientHandshakeMode,
    phase: Phase,
    recv_buf: Vec<u8>,
    send_buf: Vec<u8>,
}

impl RtmpClientHandshake {
    /// Creates a new `RtmpClientHandshake` instance.
    /// 创建新的 `RtmpClientHandshake` 实例。
    pub fn new() -> Self {
        Self::new_with_mode(ClientHandshakeMode::Strict)
    }

    /// Creates a new `lenient` instance.
    /// 创建新的 `lenient` 实例。
    pub fn new_lenient() -> Self {
        Self::new_with_mode(ClientHandshakeMode::Lenient)
    }

    fn new_with_mode(mode: ClientHandshakeMode) -> Self {
        let options = RtmpHandshakeOptions::default();
        let mut send_buf = Vec::new();

        // SEND: C0, C1
        send_buf.push(RTMP_VERSION);
        send_buf.extend_from_slice(&options.phase1_packet());

        Self {
            options,
            mode,
            phase: Phase::P0,
            recv_buf: Vec::new(),
            send_buf,
        }
    }

    /// `feed_recv_buf` function of `RtmpClientHandshake`.
    /// `RtmpClientHandshake` 的 `feed_recv_buf` 函数。
    pub fn feed_recv_buf(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.recv_buf.extend_from_slice(buf);
        self.handle_recv_buf()?;
        Ok(())
    }

    fn handle_recv_buf(&mut self) -> Result<(), Error> {
        match self.phase {
            Phase::P0 => self.handle_phase_p0(),
            Phase::P1 => self.handle_phase_p1(),
            Phase::P2 | Phase::Complete => Ok(()),
        }
    }

    fn handle_phase_p0(&mut self) -> Result<(), Error> {
        if self.recv_buf.is_empty() {
            return Ok(());
        }

        let mut buf: &[u8] = &self.recv_buf;
        let server_rtmp_version = buf.read_u8()?;
        let consumed = self.recv_buf.len() - buf.len();
        self.recv_buf.drain(..consumed);

        if server_rtmp_version != RTMP_VERSION {
            return Err(Error::invalid_data(format!(
                "invalid RTMP version: expected {RTMP_VERSION}, got {server_rtmp_version}"
            )));
        }

        self.phase = Phase::P1;
        self.handle_phase_p1()
    }

    fn handle_phase_p1(&mut self) -> Result<(), Error> {
        if self.recv_buf.len() < HANDSHAKE_PACKET_SIZE * 2 {
            return Ok(());
        }

        let mut buf: &[u8] = &self.recv_buf;
        let s1_packet = buf.read_bytes(HANDSHAKE_PACKET_SIZE)?;
        let s2_packet = buf.read_bytes(HANDSHAKE_PACKET_SIZE)?;
        let consumed = self.recv_buf.len() - buf.len();
        self.recv_buf.drain(..consumed);

        if self.mode == ClientHandshakeMode::Strict && s2_packet != self.options.phase1_packet() {
            return Err(Error::invalid_data("S2 packet does not match C1 packet"));
        }

        // SEND: C2
        self.send_buf.extend_from_slice(&s1_packet);

        self.phase = Phase::Complete;
        Ok(())
    }

    /// Takes `recv buf`, replacing it with the default.
    /// 获取 `recv buf`，并用默认值替换。
    pub fn take_recv_buf(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.recv_buf)
    }

    /// Sends `buf` to the peer.
    /// 向对端发送 `buf`。
    pub fn send_buf(&self) -> &[u8] {
        &self.send_buf
    }

    /// `advance_send_buf` function of `RtmpClientHandshake`.
    /// `RtmpClientHandshake` 的 `advance_send_buf` 函数。
    pub fn advance_send_buf(&mut self, n: usize) {
        let n = n.min(self.send_buf.len());
        self.send_buf.drain(..n); // NOTE: 效率不高，但不是需要关注性能的地方，因此优先简洁
    }

    /// Returns `true` if `recv complete` is true.
    /// 当 `recv complete` 为真时返回 `true`。
    pub fn is_recv_complete(&self) -> bool {
        self.phase == Phase::Complete
    }

    /// Returns `true` if `send complete` is true.
    /// 当 `send complete` 为真时返回 `true`。
    pub fn is_send_complete(&self) -> bool {
        self.phase == Phase::Complete && self.send_buf.is_empty()
    }
}

impl Default for RtmpClientHandshake {
    fn default() -> Self {
        Self::new()
    }
}

fn build_lenient_seeded_s1(c1: &[u8]) -> [u8; HANDSHAKE_PACKET_SIZE] {
    // Keep first 8 bytes zeroed and derive the remainder from C1 without runtime randomness.
    let mut s1 = [0u8; HANDSHAKE_PACKET_SIZE];
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    for &byte in c1.iter().take(128) {
        seed ^= (byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15_u64);
        seed = seed.rotate_left(13).wrapping_mul(0xbf58_476d_1ce4_e5b9_u64);
    }
    for out in &mut s1[8..] {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *out = seed as u8;
    }
    s1
}
