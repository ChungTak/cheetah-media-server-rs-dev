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
/// finally the payload is sniffed. The resolver itself is stateless; the caller decides
/// how many times to call `resolve` for a given stream.
///
/// RTP PT 解析器。调用方绑定优先，其次静态表，最后 payload 探测。解析器本身无状态，
/// 由调用方决定针对单个流调用多少次 `resolve`。
#[derive(Debug, Clone, Default)]
pub struct RtpPtResolver {
    bindings: HashMap<u8, RtpPayloadProfile>,
}

impl RtpPtResolver {
    /// Create a resolver with no built-in sniff budget.
    ///
    /// 创建解析器。探测预算由调用方自行维护。
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
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
    /// 解析 PT。返回 `None` 表示当前 payload 不足以确定格式，调用方可继续等待更多包。
    pub fn resolve(&self, payload_type: u8, payload: &[u8]) -> Option<RtpPayloadProfile> {
        // 1. External binding.
        if let Some(profile) = self.bindings.get(&payload_type) {
            return Some(*profile);
        }

        // 2. Static profile mapping for well-known payload types.
        if let Some(profile) = static_profile(payload_type) {
            return Some(profile);
        }

        // 3. Payload sniff for dynamic (and other unknown) payload types.
        sniff_profile(payload)
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
    // Disambiguate from MPEG program-stream PES/system headers, which also begin
    // with `00 00 01` but use start-code IDs >= 0xB0. Annex-B NAL headers have
    // the forbidden-zero bit clear, so the first NAL byte is always < 0x80.
    let is_annexb_start_code = (payload.len() >= 4
        && payload[0] == 0x00
        && payload[1] == 0x00
        && payload[2] == 0x00
        && payload[3] == 0x01)
        || (payload.len() >= 3 && payload[0] == 0x00 && payload[1] == 0x00 && payload[2] == 0x01);
    if is_annexb_start_code {
        let nal_or_start_code_id = if payload.len() >= 5 && payload[2] == 0x00 {
            payload[4]
        } else {
            payload[3]
        };
        if nal_or_start_code_id < 0x80 {
            return Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000));
        }
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
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(8, &[]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 8_000))
        );
    }

    #[test]
    fn static_pt_mp2t_is_ts_90k() {
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(33, &[]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ts, 90_000))
        );
    }

    #[test]
    fn external_binding_overrides_static_table() {
        let mut resolver = RtpPtResolver::new();
        resolver.bind(96, RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000));
        assert_eq!(
            resolver.resolve(96, &[0x00, 0x00, 0x01, 0xBA]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000))
        );
    }

    #[test]
    fn sniff_detects_ps_from_pack_header() {
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(96, &[0x00, 0x00, 0x01, 0xBA]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ps, 90_000))
        );
    }

    #[test]
    fn sniff_detects_ts_from_sync_byte() {
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(97, &[0x47, 0x00, 0x01]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Ts, 90_000))
        );
    }

    #[test]
    fn sniff_detects_h26x_annexb_start_code() {
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(98, &[0x00, 0x00, 0x00, 0x01, 0x09]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::Es, 90_000))
        );
    }

    #[test]
    fn sniff_detects_aac_adts_sync_word() {
        let resolver = RtpPtResolver::new();
        assert_eq!(
            resolver.resolve(99, &[0xFF, 0xF1, 0x00, 0x00]),
            Some(RtpPayloadProfile::new(RtpPayloadMode::RawAudio, 90_000))
        );
    }

    #[test]
    fn unknown_dynamic_payload_returns_none() {
        let resolver = RtpPtResolver::new();
        assert!(resolver.resolve(100, &[0xAB, 0xCD]).is_none());
    }

    #[test]
    fn ps_pes_start_code_is_not_misidentified_as_annexb() {
        // A PS video PES starts with `00 00 01 E0`; the start-code ID is >= 0x80,
        // so it must not be classified as raw H.26x elementary video.
        let resolver = RtpPtResolver::new();
        assert_eq!(resolver.resolve(96, &[0x00, 0x00, 0x01, 0xE0, 0x00]), None);
    }
}
