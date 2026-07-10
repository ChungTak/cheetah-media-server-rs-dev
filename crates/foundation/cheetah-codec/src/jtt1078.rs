//! JT/T 1078 RTP payload parser and packetizer.
//!
//! Supports the 2013/2016 and 2019 variants of the JT/T 1078 vehicle surveillance
//! standard. Provides frame assembling, bounded caching, frame-rate learning from
//! `frame_interval`, and media PT mappings.
//!
//! # Payload types (standard)
//! - Video: PT 98 (H.264), PT 99 (H.265)
//! - Audio: PT 6 (G.711A, PCMA), PT 7 (G.711U, PCMU), PT 19 (G.726)

use crate::prelude::*;
use bytes::{Bytes, BytesMut};

/// JT/T 1078 protocol version variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jtt1078Version {
    /// JT/T 1078-2013 / 2016 (26-byte fixed header).
    V2013,
    /// JT/T 1078-2019 (extended header, 30 bytes minimum).
    V2019,
}

/// JT/T 1078 keep-open semantic for a session. This mirrors ABLMediaServer's
/// `kt1078_keep_mode` field that selects between continuous live streaming, on-demand playback,
/// bidirectional voice talk, and sub-stream lookup. The codec layer carries the variant as
/// metadata; the actual networking lifecycle lives in the GB28181 / JTT1078 module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Jtt1078KeepOpenMode {
    /// Single-shot session — close after the response or after BYE.
    #[default]
    Single,
    /// Live streaming session — keep the port open for continuous frames.
    Live,
    /// On-demand playback — keep open while the seek window has frames.
    Playback,
    /// Bidirectional voice talk — keep open until the talk session is torn down.
    Talk,
    /// Sub-stream / lower-bitrate negotiation — keep open while the sub stream is active.
    Sub,
}

/// Frame type flags from the JTT1078 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jtt1078FrameType {
    /// I-frame (key frame).
    IFrame,
    /// P-frame (predicted frame).
    PFrame,
    /// B-frame.
    BFrame,
    /// Audio frame.
    Audio,
    /// Pass-through / transparent transmission (`dataType == 4`).
    Passthrough,
    /// Other / unknown.
    Other(u8),
}

impl Jtt1078FrameType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x0F {
            0x00 => Jtt1078FrameType::IFrame,
            0x01 => Jtt1078FrameType::PFrame,
            0x02 => Jtt1078FrameType::BFrame,
            0x03 => Jtt1078FrameType::Audio,
            0x04 => Jtt1078FrameType::Passthrough,
            other => Jtt1078FrameType::Other(other),
        }
    }
}

/// Sub-package handling mark from the JT/T 1078 RTP header (lower nibble of byte 15
/// in 2013/2016, byte 19 in 2019). Reflects how a frame is fragmented across packets:
/// `Atomic` means the whole frame fits in a single packet; the others form a
/// `First → Intermediate* → Last` sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jtt1078SubPackage {
    /// 0 — single packet contains a complete frame.
    Atomic,
    /// 1 — first packet of a fragmented frame.
    First,
    /// 2 — last packet of a fragmented frame.
    Last,
    /// 3 — middle packet of a fragmented frame.
    Intermediate,
}

impl Jtt1078SubPackage {
    fn from_bits(bits: u8) -> Option<Self> {
        match bits & 0x0F {
            0 => Some(Self::Atomic),
            1 => Some(Self::First),
            2 => Some(Self::Last),
            3 => Some(Self::Intermediate),
            _ => None,
        }
    }
}

/// Parsed header of a single JT/T 1078 RTP packet.
#[derive(Debug, Clone)]
pub struct Jtt1078Header {
    pub version: Jtt1078Version,
    /// Payload type as carried in the M/PT byte (lower 7 bits).
    pub payload_type: u8,
    /// RTP `marker` bit from the M/PT byte (bit 7).
    pub marker: bool,
    /// SIM card number (BCD encoded, 6 or 10 bytes → 12 / 20 ASCII digits).
    pub sim: String,
    /// Channel number.
    pub channel: u8,
    /// Data type (4-bit `dataType` field).
    pub frame_type: Jtt1078FrameType,
    /// Sub-package handling mark (4-bit `subPackageHandleMark` field).
    pub sub_package: Jtt1078SubPackage,
    /// RTP sequence number from the header (bytes 6-7 / BE u16).
    pub packet_seq: u16,
    /// Frame timestamp in milliseconds (only present for video and audio frames).
    pub timestamp_ms: u64,
    /// Last I-frame interval in milliseconds (video frames only).
    pub last_iframe_interval_ms: Option<u16>,
    /// Frame interval in milliseconds (video frames only).
    pub last_frame_interval_ms: Option<u16>,
    /// Length of the payload in bytes (`bodyLen`).
    pub payload_len: u16,
}

impl Jtt1078Header {
    /// Minimum size of a JT/T 1078-2013/2016 packet that we can verify the magic for.
    /// The actual header length depends on `dataType`:
    /// - Video (I/P/B): 30 bytes
    /// - Audio: 26 bytes
    /// - Pass-through: 18 bytes
    pub const MIN_SIZE_2013: usize = 18;
    /// Minimum size of a JT/T 1078-2019 packet (sim is 10 bytes, so add 4):
    /// - Video: 34 bytes
    /// - Audio: 30 bytes
    /// - Pass-through: 22 bytes
    pub const MIN_SIZE_2019: usize = 22;

    /// Parse a JT/T 1078-2013/2016 RTP packet.
    pub fn parse(data: &[u8]) -> Option<(Jtt1078Header, usize)> {
        Self::parse_with_version(data, Jtt1078Version::V2013)
    }

    /// Parse a JT/T 1078-2019 RTP packet (10-byte SIM).
    pub fn parse_v2019(data: &[u8]) -> Option<(Jtt1078Header, usize)> {
        Self::parse_with_version(data, Jtt1078Version::V2019)
    }

    fn parse_with_version(data: &[u8], version: Jtt1078Version) -> Option<(Jtt1078Header, usize)> {
        let min_size = match version {
            Jtt1078Version::V2013 => Self::MIN_SIZE_2013,
            Jtt1078Version::V2019 => Self::MIN_SIZE_2019,
        };
        if data.len() < min_size {
            return None;
        }

        // Magic: 0x30 0x31 0x63 0x64
        if data[0] != 0x30 || data[1] != 0x31 || data[2] != 0x63 || data[3] != 0x64 {
            return None;
        }

        // Bytes 4-5: V/P/X/CC + M/PT.
        // We treat the wire format permissively (vendor stacks sometimes set V=2, sometimes
        // ignore the field); we only extract `marker` and `pt` from byte 5 and `cc` from
        // byte 4 (lowest 4 bits) to keep round-trip semantics.
        let mpt = data[5];
        let marker = (mpt & 0x80) != 0;
        let payload_type = mpt & 0x7F;

        // Bytes 6-7: sequence number.
        let packet_seq = u16::from_be_bytes([data[6], data[7]]);

        // SIM occupies bytes 8..(8 + sim_len). 2013/2016 = 6 bytes, 2019 = 10 bytes.
        let sim_len = match version {
            Jtt1078Version::V2013 => 6,
            Jtt1078Version::V2019 => 10,
        };
        let sim = bcd_to_str(&data[8..8 + sim_len]);

        // Byte (8 + sim_len): channel.
        let ch_off = 8 + sim_len;
        let channel = data[ch_off];

        // Byte (9 + sim_len): dataType(4) | subPackageHandleMark(4).
        let dtype_byte = data[ch_off + 1];
        let frame_type = Jtt1078FrameType::from_bits(dtype_byte >> 4);
        let sub_package = Jtt1078SubPackage::from_bits(dtype_byte & 0x0F)?;

        // Variable tail layout (per JT/T 1078 §5.4):
        // - I/P/B (video): timestamp(8) + iframe_interval(2) + frame_interval(2) + body_len(2)
        // - Audio:         timestamp(8) + body_len(2)
        // - Passthrough:   body_len(2)
        let tail_off = ch_off + 2;
        let (timestamp_ms, last_iframe_interval_ms, last_frame_interval_ms, payload_offset) =
            match frame_type {
                Jtt1078FrameType::IFrame | Jtt1078FrameType::PFrame | Jtt1078FrameType::BFrame => {
                    let need = tail_off + 8 + 2 + 2 + 2;
                    if data.len() < need {
                        return None;
                    }
                    let ts = u64::from_be_bytes([
                        data[tail_off],
                        data[tail_off + 1],
                        data[tail_off + 2],
                        data[tail_off + 3],
                        data[tail_off + 4],
                        data[tail_off + 5],
                        data[tail_off + 6],
                        data[tail_off + 7],
                    ]);
                    let ifi = u16::from_be_bytes([data[tail_off + 8], data[tail_off + 9]]);
                    let fi = u16::from_be_bytes([data[tail_off + 10], data[tail_off + 11]]);
                    (ts, Some(ifi), Some(fi), need)
                }
                Jtt1078FrameType::Audio => {
                    let need = tail_off + 8 + 2;
                    if data.len() < need {
                        return None;
                    }
                    let ts = u64::from_be_bytes([
                        data[tail_off],
                        data[tail_off + 1],
                        data[tail_off + 2],
                        data[tail_off + 3],
                        data[tail_off + 4],
                        data[tail_off + 5],
                        data[tail_off + 6],
                        data[tail_off + 7],
                    ]);
                    (ts, None, None, need)
                }
                Jtt1078FrameType::Passthrough | Jtt1078FrameType::Other(_) => {
                    let need = tail_off + 2;
                    if data.len() < need {
                        return None;
                    }
                    (0, None, None, need)
                }
            };

        // body_len is the last 2 bytes before the payload.
        let body_len_off = payload_offset - 2;
        let payload_len = u16::from_be_bytes([data[body_len_off], data[body_len_off + 1]]);

        if data.len() < payload_offset + payload_len as usize {
            return None;
        }

        Some((
            Jtt1078Header {
                version,
                payload_type,
                marker,
                sim,
                channel,
                frame_type,
                sub_package,
                packet_seq,
                timestamp_ms,
                last_iframe_interval_ms,
                last_frame_interval_ms,
                payload_len,
            },
            payload_offset,
        ))
    }
}

/// A fully assembled JTT1078 media frame (after de-fragmentation).
#[derive(Debug, Clone)]
pub struct Jtt1078Frame {
    pub payload_type: u8,
    pub sim: String,
    pub channel: u8,
    pub frame_type: Jtt1078FrameType,
    pub timestamp_ms: u64,
    /// Estimated frame interval in milliseconds (learned from header or gap).
    pub frame_interval_ms: u32,
    pub data: Bytes,
}

impl Jtt1078Frame {
    /// Whether this frame is a video keyframe (I-frame).
    pub fn is_key(&self) -> bool {
        matches!(self.frame_type, Jtt1078FrameType::IFrame)
    }

    /// Whether this frame carries audio payload.
    pub fn is_audio(&self) -> bool {
        matches!(self.frame_type, Jtt1078FrameType::Audio)
            || matches!(self.payload_type, 6 | 7 | 19)
    }
}

/// Diagnostics emitted by the frame assembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jtt1078Diagnostic {
    /// A fragment reassembly buffer exceeded the configured limit.
    CacheOverflow {
        sim: String,
        channel: u8,
        dropped_bytes: usize,
    },
    /// A packet arrived with an unexpected sequence number.
    SequenceGap { expected: u16, got: u16 },
    /// A fragment arrived after the frame was already completed.
    LateFragment { packet_seq: u16 },
    /// Header parse failed (bad magic or truncated data).
    BadHeader,
}

/// Key identifying an assembly buffer (one per SIM+channel pair).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AssemblyKey {
    sim: String,
    channel: u8,
}

struct AssemblyBuffer {
    data: BytesMut,
    first_timestamp_ms: u64,
    last_packet_seq: u16,
    frame_type: Jtt1078FrameType,
    payload_type: u8,
}

/// JT/T 1078 frame assembler.
///
/// Accepts individual RTP packets from a JTT1078 device and reassembles them
/// into complete media frames. The assembly cache has a bounded maximum size per
/// channel (`max_cache_bytes`), ensuring that a misbehaving sender cannot exhaust
/// memory.
///
/// Frame-rate is learned from `last_frame_interval_ms` in the 2019 header, or
/// estimated from the delta between successive complete frames.
pub struct Jtt1078FrameAssembler {
    /// Per-channel assembly buffers.
    buffers: HashMap<AssemblyKey, AssemblyBuffer>,
    /// Maximum number of bytes allowed per assembly buffer.
    max_cache_bytes: usize,
    /// Last completed frame timestamp per channel (for interval estimation).
    last_frame_ts: HashMap<AssemblyKey, u64>,
    /// Learned frame interval in milliseconds (rolling average, per channel).
    frame_interval_ms: HashMap<AssemblyKey, u32>,
}

impl Jtt1078FrameAssembler {
    /// Create a new assembler. `max_cache_bytes` is the upper bound on each
    /// channel's reassembly buffer; typical values are 512 KiB–4 MiB.
    pub fn new(max_cache_bytes: usize) -> Self {
        Self {
            buffers: HashMap::new(),
            max_cache_bytes,
            last_frame_ts: HashMap::new(),
            frame_interval_ms: HashMap::new(),
        }
    }

    /// Push one raw JTT1078 RTP packet. Returns completed frames and any
    /// diagnostics generated during processing.
    pub fn push(&mut self, data: &[u8]) -> (Vec<Jtt1078Frame>, Vec<Jtt1078Diagnostic>) {
        let mut frames = Vec::new();
        let mut diags = Vec::new();

        let Some((hdr, payload_offset)) = Jtt1078Header::parse(data) else {
            diags.push(Jtt1078Diagnostic::BadHeader);
            return (frames, diags);
        };

        // Bound the payload by `payload_len` from the header so trailing/garbage
        // bytes (e.g. transport-layer padding) don't leak into the reassembly buffer.
        let body_len = hdr.payload_len as usize;
        let end = (payload_offset + body_len).min(data.len());
        let payload = Bytes::copy_from_slice(&data[payload_offset..end]);
        let key = AssemblyKey {
            sim: hdr.sim.clone(),
            channel: hdr.channel,
        };

        // Learn frame interval from 2019 header if available.
        if let Some(fi) = hdr.last_frame_interval_ms {
            if fi > 0 {
                self.update_frame_interval(&key, fi as u32);
            }
        }

        if hdr.sub_package == Jtt1078SubPackage::Atomic {
            // Single-packet frame — skip assembly buffer entirely.
            let interval = self.current_frame_interval(&key, hdr.timestamp_ms);
            self.last_frame_ts.insert(key, hdr.timestamp_ms);
            frames.push(Jtt1078Frame {
                payload_type: hdr.payload_type,
                sim: hdr.sim,
                channel: hdr.channel,
                frame_type: hdr.frame_type,
                timestamp_ms: hdr.timestamp_ms,
                frame_interval_ms: interval,
                data: payload,
            });
            return (frames, diags);
        }

        if hdr.sub_package == Jtt1078SubPackage::First {
            // Start new assembly buffer, discarding any leftover.
            let buf = AssemblyBuffer {
                data: BytesMut::from(payload.as_ref()),
                first_timestamp_ms: hdr.timestamp_ms,
                last_packet_seq: hdr.packet_seq,
                frame_type: hdr.frame_type,
                payload_type: hdr.payload_type,
            };
            self.buffers.insert(key, buf);
            return (frames, diags);
        }

        // Intermediate or Last fragment.
        if let Some(buf) = self.buffers.get_mut(&key) {
            let expected_seq = buf.last_packet_seq.wrapping_add(1);
            if hdr.packet_seq != expected_seq {
                diags.push(Jtt1078Diagnostic::SequenceGap {
                    expected: expected_seq,
                    got: hdr.packet_seq,
                });
                // Discard the stale buffer and start fresh.
                self.buffers.remove(&key);
                return (frames, diags);
            }

            // Enforce cache limit.
            let incoming_len = payload.len();
            if buf.data.len() + incoming_len > self.max_cache_bytes {
                let dropped = buf.data.len() + incoming_len;
                diags.push(Jtt1078Diagnostic::CacheOverflow {
                    sim: hdr.sim.clone(),
                    channel: hdr.channel,
                    dropped_bytes: dropped,
                });
                self.buffers.remove(&key);
                return (frames, diags);
            }

            buf.data.extend_from_slice(&payload);
            buf.last_packet_seq = hdr.packet_seq;

            if hdr.sub_package == Jtt1078SubPackage::Last {
                let assembled_data = buf.data.split().freeze();
                let ts = buf.first_timestamp_ms;
                let ft = buf.frame_type;
                let pt = buf.payload_type;
                self.buffers.remove(&key);

                let interval = self.current_frame_interval(&key, ts);
                self.last_frame_ts.insert(key, ts);

                frames.push(Jtt1078Frame {
                    payload_type: pt,
                    sim: hdr.sim,
                    channel: hdr.channel,
                    frame_type: ft,
                    timestamp_ms: ts,
                    frame_interval_ms: interval,
                    data: assembled_data,
                });
            }
        } else {
            // Late fragment with no active buffer — drop.
            diags.push(Jtt1078Diagnostic::LateFragment {
                packet_seq: hdr.packet_seq,
            });
        }

        (frames, diags)
    }

    /// Return the current best-estimate frame interval for `key`.
    fn current_frame_interval(&self, key: &AssemblyKey, current_ts: u64) -> u32 {
        if let Some(&fi) = self.frame_interval_ms.get(key) {
            return fi;
        }
        if let Some(&last_ts) = self.last_frame_ts.get(key) {
            if current_ts > last_ts {
                let delta = (current_ts - last_ts).min(u32::MAX as u64) as u32;
                if delta > 0 && delta < 10_000 {
                    return delta;
                }
            }
        }
        40 // default: 25 fps fallback
    }

    fn update_frame_interval(&mut self, key: &AssemblyKey, new_val: u32) {
        let entry = self.frame_interval_ms.entry(key.clone()).or_insert(new_val);
        // Rolling average: weight 3:1 toward the existing estimate.
        *entry = (*entry / 4) * 3 + new_val / 4;
    }
}

/// Maximum number of bytes in a single JTT1078 RTP packet payload.
const DEFAULT_MAX_PACKET_PAYLOAD: usize = 1400;

/// JT/T 1078 packetizer — splits a large frame into multiple RTP packets with
/// correct sub-package handling marks (`Atomic` / `First` / `Intermediate` / `Last`).
///
/// The output matches the JT/T 1078-2013/2016 wire format. Each packet has the form:
///
/// `magic(4) | V/P/X/CC | M/PT | seq(2) | SIM(6 BCD) | channel | dataType<<4|subPkg`
///
/// followed by the variable tail (timestamp + intervals + body_len for video,
/// timestamp + body_len for audio, body_len for pass-through), and finally the payload.
pub struct Jtt1078Packetizer {
    sim: [u8; 6],
    channel: u8,
    max_payload_bytes: usize,
    next_seq: u16,
}

impl Jtt1078Packetizer {
    /// Create a new packetizer.
    ///
    /// `sim_digits` must be a 12-character ASCII decimal string (BCD encoded as
    /// 6 bytes in the output header). `channel` is the 1-byte channel number.
    pub fn new(sim_digits: &str, channel: u8) -> Self {
        let mut sim = [0u8; 6];
        str_to_bcd(sim_digits, &mut sim);
        Self {
            sim,
            channel,
            max_payload_bytes: DEFAULT_MAX_PACKET_PAYLOAD,
            next_seq: 0,
        }
    }

    /// Override the maximum payload bytes per packet (default 1400).
    pub fn set_max_payload_bytes(&mut self, max: usize) {
        // Keep at least 1 to avoid empty packets, but allow small sizes for tests
        // that exercise fragmentation behaviour.
        self.max_payload_bytes = max.max(1);
    }

    /// Packetize `frame_data` into one or more JTT1078 RTP byte buffers.
    ///
    /// `payload_type` is the RTP PT (e.g. 98 for H.264 video, 6 for G.711A).
    /// `timestamp_ms` is the frame timestamp in milliseconds.
    /// `frame_type` controls the `dataType` field in the header.
    pub fn packetize(
        &mut self,
        payload_type: u8,
        timestamp_ms: u64,
        frame_type: Jtt1078FrameType,
        frame_data: &[u8],
    ) -> Vec<Bytes> {
        let chunks: Vec<&[u8]> = frame_data.chunks(self.max_payload_bytes).collect();
        let total = chunks.len();
        let mut packets = Vec::with_capacity(total);

        let ft_bits: u8 = match frame_type {
            Jtt1078FrameType::IFrame => 0x00,
            Jtt1078FrameType::PFrame => 0x01,
            Jtt1078FrameType::BFrame => 0x02,
            Jtt1078FrameType::Audio => 0x03,
            Jtt1078FrameType::Passthrough => 0x04,
            Jtt1078FrameType::Other(v) => v & 0x0F,
        };
        let is_video = matches!(
            frame_type,
            Jtt1078FrameType::IFrame | Jtt1078FrameType::PFrame | Jtt1078FrameType::BFrame
        );
        // The wire-format tail layout depends solely on `dataType`:
        //   0..=2 (video)    -> ts + iframe_interval + frame_interval + body_len
        //   3 (audio)        -> ts + body_len
        //   any other value  -> body_len (parser treats as pass-through / unknown)
        // We key off `ft_bits` directly so callers using `Other(v)` (or `Passthrough`) emit
        // bytes that round-trip through `Jtt1078Header::parse`.
        let is_audio = ft_bits == 0x03;

        for (i, chunk) in chunks.iter().enumerate() {
            // 0=Atomic, 1=First, 2=Last, 3=Intermediate (per JT/T 1078 §5.4 table).
            let sub_pkg_bits: u8 = if total == 1 {
                0
            } else if i == 0 {
                1
            } else if i == total - 1 {
                2
            } else {
                3
            };
            let dtype_byte = (ft_bits << 4) | sub_pkg_bits;

            let chunk_len_u16 = u16::try_from(chunk.len()).unwrap_or(u16::MAX);

            let mut pkt = BytesMut::with_capacity(34 + chunk.len());
            // Magic
            pkt.extend_from_slice(&[0x30, 0x31, 0x63, 0x64]);
            // V=2, P=0, X=0, CC=1 → 0b10_0_0_0001 = 0x81. The low nibble is `cc`, fixed at 1.
            pkt.extend_from_slice(&[0x81]);
            // M=0 (no marker), PT in low 7 bits.
            pkt.extend_from_slice(&[payload_type & 0x7F]);
            // Sequence number (u16 BE)
            pkt.extend_from_slice(&self.next_seq.to_be_bytes());
            // SIM (6 BCD bytes)
            pkt.extend_from_slice(&self.sim);
            // Logical channel number
            pkt.extend_from_slice(&[self.channel]);
            // dataType<<4 | subPackageHandleMark
            pkt.extend_from_slice(&[dtype_byte]);

            if is_video {
                pkt.extend_from_slice(&timestamp_ms.to_be_bytes()); // 8 bytes
                pkt.extend_from_slice(&0u16.to_be_bytes()); // i_frame_interval (unknown)
                pkt.extend_from_slice(&0u16.to_be_bytes()); // frame_interval (unknown)
                pkt.extend_from_slice(&chunk_len_u16.to_be_bytes());
            } else if is_audio {
                pkt.extend_from_slice(&timestamp_ms.to_be_bytes());
                pkt.extend_from_slice(&chunk_len_u16.to_be_bytes());
            } else {
                // Pass-through (`dataType == 4`) and any other unknown `dataType` follow the
                // body_len-only layout that `Jtt1078Header::parse` expects for those values.
                pkt.extend_from_slice(&chunk_len_u16.to_be_bytes());
            }

            pkt.extend_from_slice(chunk);

            packets.push(pkt.freeze());
            self.next_seq = self.next_seq.wrapping_add(1);
        }

        packets
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Decode a 6-byte BCD-encoded SIM number to a 12-character ASCII string.
fn bcd_to_str(bcd: &[u8]) -> String {
    let mut s = String::with_capacity(bcd.len() * 2);
    for &b in bcd {
        s.push(char::from_digit((b >> 4) as u32, 10).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0F) as u32, 10).unwrap_or('0'));
    }
    s
}

/// Encode a decimal ASCII string into BCD bytes (up to `out.len() * 2` digits).
fn str_to_bcd(digits: &str, out: &mut [u8]) {
    let mut chars = digits.chars().filter(|c| c.is_ascii_digit());
    for byte in out.iter_mut() {
        let hi = chars.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
        let lo = chars.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
        *byte = (hi << 4) | lo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a JT/T 1078-2013/2016 packet for the assembler tests. Layout:
    /// `magic(4) | V/P/X/CC(0x81) | M/PT | seq(2) | SIM(6 BCD) | channel | dataType<<4|subPkg`,
    /// then the variable tail (timestamp + intervals + body_len for video, timestamp +
    /// body_len for audio, body_len for pass-through), then the payload.
    #[allow(clippy::too_many_arguments)]
    fn make_packet(
        payload_type: u8,
        seq: u16,
        sim: &str,
        channel: u8,
        frame_type_bits: u8,
        sub_pkg_bits: u8,
        ts: u64,
        payload: &[u8],
    ) -> Vec<u8> {
        let dtype_byte = (frame_type_bits << 4) | (sub_pkg_bits & 0x0F);
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[0x30, 0x31, 0x63, 0x64]);
        pkt.push(0x81); // V=2, P=0, X=0, CC=1
        pkt.push(payload_type & 0x7F);
        pkt.extend_from_slice(&seq.to_be_bytes());
        let mut sim_bcd = [0u8; 6];
        str_to_bcd(sim, &mut sim_bcd);
        pkt.extend_from_slice(&sim_bcd);
        pkt.push(channel);
        pkt.push(dtype_byte);

        let body_len = payload.len() as u16;
        match frame_type_bits {
            0x00..=0x02 => {
                // Video: ts(8) + iframe_interval(2) + frame_interval(2) + body_len(2)
                pkt.extend_from_slice(&ts.to_be_bytes());
                pkt.extend_from_slice(&0u16.to_be_bytes());
                pkt.extend_from_slice(&0u16.to_be_bytes());
                pkt.extend_from_slice(&body_len.to_be_bytes());
            }
            0x03 => {
                // Audio: ts(8) + body_len(2)
                pkt.extend_from_slice(&ts.to_be_bytes());
                pkt.extend_from_slice(&body_len.to_be_bytes());
            }
            _ => {
                // Pass-through: body_len(2)
                pkt.extend_from_slice(&body_len.to_be_bytes());
            }
        }
        pkt.extend_from_slice(payload);
        pkt
    }

    #[test]
    fn single_packet_frame_assembled_immediately() {
        let mut assembler = Jtt1078FrameAssembler::new(1024 * 1024);
        // sub_pkg=0 (Atomic), I-frame
        let pkt = make_packet(98, 1, "123456789012", 1, 0x00, 0, 1000, b"videoidr");
        let (frames, diags) = assembler.push(&pkt);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        assert_eq!(frames.len(), 1);
        assert_eq!(&*frames[0].data, b"videoidr");
        assert!(frames[0].is_key());
    }

    #[test]
    fn multi_packet_frame_assembled_after_last_fragment() {
        let mut assembler = Jtt1078FrameAssembler::new(1024 * 1024);
        // First (sub_pkg=1) → Last (sub_pkg=2). Use P-frame so the assembler
        // doesn't classify the joined output as a keyframe.
        let pkt1 = make_packet(98, 1, "123456789012", 1, 0x01, 1, 2000, b"aaaa");
        let pkt2 = make_packet(98, 2, "123456789012", 1, 0x01, 2, 2000, b"bbbb");

        let (frames, diags) = assembler.push(&pkt1);
        assert!(diags.is_empty());
        assert!(frames.is_empty());

        let (frames, diags) = assembler.push(&pkt2);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        assert_eq!(frames.len(), 1);
        assert_eq!(&*frames[0].data, b"aaaabbbb");
    }

    #[test]
    fn cache_overflow_drops_buffer_and_emits_diagnostic() {
        let mut assembler = Jtt1078FrameAssembler::new(8); // tiny limit
        let pkt1 = make_packet(98, 1, "000000000001", 1, 0x01, 1, 3000, b"12345");
        let pkt2 = make_packet(98, 2, "000000000001", 1, 0x01, 2, 3000, b"678901234"); // 9 > limit
        let (_, _) = assembler.push(&pkt1);
        let (frames, diags) = assembler.push(&pkt2);
        assert!(frames.is_empty());
        assert!(
            diags
                .iter()
                .any(|d| matches!(d, Jtt1078Diagnostic::CacheOverflow { .. })),
            "expected CacheOverflow diagnostic"
        );
    }

    #[test]
    fn sequence_gap_discards_buffer() {
        let mut assembler = Jtt1078FrameAssembler::new(1024 * 1024);
        let pkt1 = make_packet(98, 10, "000000000002", 2, 0x01, 1, 4000, b"first");
        // Wrong seq — should be 11, send 99 — and we mark it as Last so we hit the
        // gap branch on the assembly path (not the `First` reset path).
        let pkt2 = make_packet(98, 99, "000000000002", 2, 0x01, 2, 4000, b"bad");
        let (_, _) = assembler.push(&pkt1);
        let (frames, diags) = assembler.push(&pkt2);
        assert!(frames.is_empty());
        assert!(diags
            .iter()
            .any(|d| matches!(d, Jtt1078Diagnostic::SequenceGap { .. })));
    }

    #[test]
    fn packetizer_splits_large_frame() {
        let mut pkt = Jtt1078Packetizer::new("123456789012", 1);
        pkt.set_max_payload_bytes(8);
        let data: Vec<u8> = (0u8..20).collect(); // 20 bytes → 3 packets
        let packets = pkt.packetize(98, 5000, Jtt1078FrameType::IFrame, &data);
        assert_eq!(packets.len(), 3);
        // dataType<<4 | subPackageHandleMark — subPkg lives in the low nibble.
        // Packetizer header offset for the dataType byte is 15 (V/P/X/CC + M/PT + seq(2) +
        // SIM(6) + channel + dataType).
        assert_eq!(packets[0][15] & 0x0F, 1, "first sub_pkg");
        assert_eq!(packets[1][15] & 0x0F, 3, "intermediate sub_pkg");
        assert_eq!(packets[2][15] & 0x0F, 2, "last sub_pkg");
    }

    #[test]
    fn packetizer_single_packet_frame() {
        let mut pkt = Jtt1078Packetizer::new("000000000000", 0);
        let packets = pkt.packetize(6, 100, Jtt1078FrameType::Audio, b"audio");
        assert_eq!(packets.len(), 1);
        // Atomic: sub_pkg = 0
        assert_eq!(packets[0][15] & 0x0F, 0, "atomic sub_pkg");
    }

    #[test]
    fn packetizer_round_trips_through_assembler() {
        // End-to-end: pack a multi-fragment video frame, feed each packet to the
        // assembler, expect one reassembled frame matching the source bytes.
        let mut pkt = Jtt1078Packetizer::new("123456789012", 3);
        pkt.set_max_payload_bytes(7);
        let data: Vec<u8> = (0u8..30).collect();
        let packets = pkt.packetize(98, 9000, Jtt1078FrameType::IFrame, &data);
        assert!(packets.len() >= 4);

        let mut assembler = Jtt1078FrameAssembler::new(64 * 1024);
        let mut last_frame = None;
        for p in &packets {
            let (frames, diags) = assembler.push(p);
            assert!(diags.is_empty(), "unexpected diags: {diags:?}");
            if let Some(f) = frames.into_iter().next() {
                last_frame = Some(f);
            }
        }
        let frame = last_frame.expect("reassembled frame");
        assert_eq!(&*frame.data, data.as_slice());
        assert_eq!(frame.timestamp_ms, 9000);
        assert!(frame.is_key());
    }

    #[test]
    fn audio_packet_round_trips_through_assembler() {
        let mut pkt = Jtt1078Packetizer::new("000000000003", 1);
        let audio_payload = b"g711a-frame".to_vec();
        let packets = pkt.packetize(6, 1234, Jtt1078FrameType::Audio, &audio_payload);
        assert_eq!(packets.len(), 1);

        let mut assembler = Jtt1078FrameAssembler::new(8 * 1024);
        let (frames, diags) = assembler.push(&packets[0]);
        assert!(diags.is_empty(), "unexpected diags: {diags:?}");
        assert_eq!(frames.len(), 1);
        assert_eq!(&*frames[0].data, audio_payload.as_slice());
        assert_eq!(frames[0].timestamp_ms, 1234);
        assert!(frames[0].is_audio());
    }

    #[test]
    fn header_parses_with_marker_and_payload_type() {
        // Build a custom audio packet with marker=1 and PT=6 (PCMA) and verify the
        // header parser recovers both fields correctly.
        let payload = b"abcd";
        let mut data = Vec::new();
        data.extend_from_slice(&[0x30, 0x31, 0x63, 0x64]);
        data.push(0x81); // V/P/X/CC
        data.push(0x80 | 6); // M=1, PT=6
        data.extend_from_slice(&7u16.to_be_bytes()); // seq
        let mut sim = [0u8; 6];
        str_to_bcd("000000000001", &mut sim);
        data.extend_from_slice(&sim);
        data.push(2); // channel
                      // dataType=0x03 (audio), subPackageHandleMark=0 (atomic).
        data.push(0x03 << 4);
        data.extend_from_slice(&5000u64.to_be_bytes()); // ts
        data.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        data.extend_from_slice(payload);

        let (hdr, off) = Jtt1078Header::parse(&data).expect("parse header");
        assert!(hdr.marker);
        assert_eq!(hdr.payload_type, 6);
        assert_eq!(hdr.packet_seq, 7);
        assert_eq!(hdr.channel, 2);
        assert_eq!(hdr.frame_type, Jtt1078FrameType::Audio);
        assert_eq!(hdr.sub_package, Jtt1078SubPackage::Atomic);
        assert_eq!(hdr.timestamp_ms, 5000);
        assert_eq!(hdr.payload_len, payload.len() as u16);
        assert_eq!(&data[off..off + payload.len()], payload);
    }

    #[test]
    fn passthrough_packet_round_trips_through_parser() {
        // Pass-through frames have `dataType == 4` and only a 2-byte body_len in the tail
        // (no timestamp, no interval fields). The packetizer used to emit an audio-style
        // tail for both `Passthrough` and any `Other(_)` variant, which made the resulting
        // bytes unparseable. This regression ensures the packetizer and parser agree on the
        // layout for both spellings.
        let mut pkt = Jtt1078Packetizer::new("000000000004", 5);
        let payload = b"raw-bytes".to_vec();

        for variant in [
            Jtt1078FrameType::Passthrough,
            Jtt1078FrameType::Other(4),
            Jtt1078FrameType::Other(7),
        ] {
            let packets = pkt.packetize(0, 9999, variant, &payload);
            assert_eq!(
                packets.len(),
                1,
                "variant {variant:?} should produce 1 packet"
            );
            let (hdr, off) = Jtt1078Header::parse(&packets[0]).expect("parse round-tripped packet");
            // Wire `dataType` matches the requested variant.
            let expected_ft_bits: u8 = match variant {
                Jtt1078FrameType::Passthrough => 4,
                Jtt1078FrameType::Other(v) => v & 0x0F,
                _ => unreachable!(),
            };
            // `Jtt1078FrameType::from_bits` maps 4 → Passthrough, others → Other(_).
            let expected_frame_type = Jtt1078FrameType::from_bits(expected_ft_bits);
            assert_eq!(hdr.frame_type, expected_frame_type);
            assert_eq!(hdr.payload_len, payload.len() as u16);
            assert_eq!(&packets[0][off..], payload.as_slice());
        }
    }

    #[test]
    fn rejects_invalid_sub_package_handle_mark() {
        // sub_pkg=15 is not a valid JT/T 1078 sub-package value.
        let pkt = make_packet(98, 1, "000000000099", 0, 0x00, 0x0F, 100, b"x");
        // Note `make_packet` builds video tail because frame_type=0x00 (I-frame); the
        // parser must reject the bad sub-package field rather than panic.
        let result = Jtt1078Header::parse(&pkt);
        assert!(result.is_none());
    }

    #[test]
    fn bcd_roundtrip() {
        let input = "123456789012";
        let mut bcd = [0u8; 6];
        str_to_bcd(input, &mut bcd);
        let decoded = bcd_to_str(&bcd);
        assert_eq!(decoded, input);
    }
}
