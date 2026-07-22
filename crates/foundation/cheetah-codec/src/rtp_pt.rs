//! RTP payload-type resolver.
//!
//! Resolves an RTP payload type (PT) into a concrete [`RtpPayloadProfile`]. Resolution
//! follows the order mandated by the implementation plan:
//!
//! 1. External payload bindings supplied by the caller (e.g. SDP / API).
//! 2. Static RFC/profile mapping for well-known PT values.
//! 3. Bounded payload sniff when the PT is dynamic or no binding exists.
//!
//! RTP payload-type 解析器。

use crate::prelude::*;
use crate::rtp::{probe_rtp_payload, RtpPayloadMode};

/// Resolved payload profile for a single RTP payload type.
///
/// 单个 RTP payload type 解析后的配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpPayloadProfile {
    pub mode: RtpPayloadMode,
    pub clock_rate_hz: u32,
}

impl RtpPayloadProfile {
    pub const fn new(mode: RtpPayloadMode, clock_rate_hz: u32) -> Self {
        Self {
            mode,
            clock_rate_hz,
        }
    }
}

fn default_clock_rate_for_mode(mode: RtpPayloadMode) -> u32 {
    match mode {
        RtpPayloadMode::RawAudio => 8_000,
        _ => 90_000,
    }
}

/// Resolver that turns an RTP PT into a [`RtpPayloadProfile`].
///
/// Bindings provided by the caller take precedence; otherwise a static table is used;
/// finally a bounded number of payloads are sniffed before giving up.
///
/// RTP PT 解析器。调用方绑定优先，其次静态表，最后进行有界探测。
#[derive(Debug, Clone)]
pub struct RtpPtResolver {
    bindings: HashMap<u8, RtpPayloadProfile>,
    max_probe_packets: u8,
    probe_counts: HashMap<u8, u8>,
}

impl Default for RtpPtResolver {
    fn default() -> Self {
        Self::new(8)
    }
}

impl RtpPtResolver {
    /// Create a resolver with a per-PT payload-sniff budget.
    ///
    /// `max_probe_packets` controls how many packets with the same PT are inspected
    /// before the resolver stops trying to resolve that PT from payload bytes alone.
    pub fn new(max_probe_packets: u8) -> Self {
        Self {
            bindings: HashMap::new(),
            max_probe_packets,
            probe_counts: HashMap::new(),
        }
    }

    /// Register an externally supplied binding for a PT. This always wins.
    ///
    /// 注册外部提供的 PT 绑定。
    pub fn bind(&mut self, payload_type: u8, profile: RtpPayloadProfile) {
        self.bindings.insert(payload_type, profile);
    }

    /// Resolve a PT using the configured bindings, static table and/or payload sniff.
    ///
    /// 解析 PT。如果本次无法解析，返回 `None` 并让调用方继续等待更多包。
    pub fn resolve(&mut self, payload_type: u8, payload: &[u8]) -> Option<RtpPayloadProfile> {
        // 1. External binding.
        if let Some(profile) = self.bindings.get(&payload_type) {
            return Some(*profile);
        }

        // 2. Static profile mapping for well-known payload types.
        if let Some(profile) = static_profile(payload_type) {
            return Some(profile);
        }

        // 3. Bounded sniff for dynamic (and other unknown) payload types.
        let count = self.probe_counts.entry(payload_type).or_insert(0);
        if *count >= self.max_probe_packets {
            return None;
        }
        *count += 1;

        sniff_profile(payload)
    }

    /// Reset any probe state for a PT, e.g. after a mid-stream format change.
    ///
    /// 重置某个 PT 的探测状态，例如在流中途格式变化后使用。
    pub fn reset_probe(&mut self, payload_type: u8) {
        self.probe_counts.remove(&payload_type);
    }
}

fn static_profile(payload_type: u8) -> Option<RtpPayloadProfile> {
    match payload_type {
        // Audio
        0 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // PCMU
        3 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // GSM
        4 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // G723
        8 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // PCMA
        9 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // G722
        10 | 11 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 44_100)), // L16
        12 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // QCELP
        13 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // CN
        15 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // G728
        18 => Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000)), // G729
        // Video / container
        31 => Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000)), // H261
        32 => Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000)), // MPV
        33 => Some(RtpPayloadProfile::new(RtpPayloadMode::Ts, 90_000)), // MP2T
        34 => Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000)), // H263
        // Static-mapped dynamic range used by some vendors for PS/TS.
        96..=127 => None, // dynamic; rely on sniff or external binding
        _ => None,
    }
}

fn sniff_profile(payload: &[u8]) -> Option<RtpPayloadProfile> {
    if payload.is_empty() {
        return None;
    }

    // Use the existing encapsulation probe (JTT/Ehome/PS/TS).
    let mode = probe_rtp_payload(payload);
    if mode != RtpPayloadMode::Unknown {
        return Some(RtpPayloadProfile::new(
            mode,
            default_clock_rate_for_mode(mode),
        ));
    }

    // Elementary video stream: H.264/H.265/H.266 Annex-B start code.
    if (payload.len() >= 4
        && payload[0] == 0x00
        && payload[1] == 0x00
        && payload[2] == 0x00
        && payload[3] == 0x01)
        || (payload.len() >= 3 && payload[0] == 0x00 && payload[1] == 0x00 && payload[2] == 0x01)
    {
        return Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000));
    }

    // AAC ADTS: sync word 0xFF with upper nibble of next byte set to 0xF.
    if payload.len() >= 2 && payload[0] == 0xFF && (payload[1] & 0xF0) == 0xF0 {
        return Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 90_000));
    }

    // Give up this packet; the caller may try again with the next one.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_pt_pcma_is_raw_audio_8k() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(8, &[]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000))
        );
    }

    #[test]
    fn static_pt_mp2t_is_ts_90k() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(33, &[]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ts, 90_000))
        );
    }

    #[test]
    fn external_binding_overrides_static_table() {
        let mut resolver = RtpPtResolver::new(8);
        resolver.bind(96, RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000));
        assert_eq!(
            resolver.resolve(96, &[0x00, 0x00, 0x01, 0xBA]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000))
        );
    }

    #[test]
    fn sniff_detects_ps_from_pack_header() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(96, &[0x00, 0x00, 0x01, 0xBA]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000))
        );
    }

    #[test]
    fn sniff_detects_ts_from_sync_byte() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(97, &[0x47, 0x00, 0x01]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ts, 90_000))
        );
    }

    #[test]
    fn sniff_detects_h26x_annexb_start_code() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(98, &[0x00, 0x00, 0x00, 0x01, 0x09]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000))
        );
    }

    #[test]
    fn sniff_detects_aac_adts_sync_word() {
        let mut resolver = RtpPtResolver::new(8);
        assert_eq!(
            resolver.resolve(99, &[0xFF, 0xF1, 0x00, 0x00]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 90_000))
        );
    }

    #[test]
    fn unknown_dynamic_returns_none_until_budget_exhausted() {
        let mut resolver = RtpPtResolver::new(2);
        assert!(resolver.resolve(100, &[0xAB, 0xCD]).is_none());
        assert!(resolver.resolve(100, &[0xAB, 0xCD]).is_none());
        assert!(resolver.resolve(100, &[0xAB, 0xCD]).is_none()); // budget exhausted
    }
}
