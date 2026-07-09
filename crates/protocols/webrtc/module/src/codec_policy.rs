//! Codec negotiation policy.
//!
//! Phase 03 implements the high-level codec gate: which codecs the module
//! is willing to negotiate at all. Per-request `preferVideoCodec` and
//! `preferAudioCodec` filtering happens here so that incompatible
//! combinations return `422 Unprocessable Entity` to the caller rather
//! than getting silently dropped at the SDP level.
//!
//! Phase 02 Task 02 adds audio output strategy: G711 passthrough, AAC/MP3
//! to Opus transcoding for Browser profile, and clear error reporting when
//! transcoding is unavailable.

use serde::{Deserialize, Serialize};

use cheetah_codec::CodecId;

use crate::config::CodecProfileWire;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcVideoCodecPreference {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcAudioCodecPreference {
    Opus,
    G711a,
    G711u,
    Aac,
    Any,
}

impl WebRtcVideoCodecPreference {
    pub fn from_str_lossy(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "h264" | "avc" => Self::H264,
            "h265" | "hevc" => Self::H265,
            "vp8" => Self::Vp8,
            "vp9" => Self::Vp9,
            "av1" => Self::Av1,
            _ => Self::Any,
        }
    }

    /// Returns whether the requested video codec is permissible under the
    /// configured profile.
    pub fn is_allowed(self, profile: CodecProfileWire) -> bool {
        !matches!((profile, self), (CodecProfileWire::Browser, Self::H265))
    }
}

impl WebRtcAudioCodecPreference {
    pub fn from_str_lossy(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "opus" => Self::Opus,
            "pcma" | "g711a" => Self::G711a,
            "pcmu" | "g711u" => Self::G711u,
            "aac" => Self::Aac,
            _ => Self::Any,
        }
    }

    pub fn is_allowed(self, profile: CodecProfileWire) -> bool {
        !matches!((profile, self), (CodecProfileWire::Browser, Self::Aac))
    }
}

// ─── Audio output strategy ──────────────────────────────────────────────────

/// Canonical Opus output parameters for WebRTC browser playback.
///
/// These match the mandatory-to-implement Opus profile in RFC 7587 / WebRTC:
/// - Clock rate: 48000 Hz
/// - Channels: 2 (stereo)
/// - Samples per frame: 960 (20ms at 48kHz)
pub const OPUS_CLOCK_RATE: u32 = 48_000;
pub const OPUS_CHANNELS: u8 = 2;
pub const OPUS_SAMPLES_PER_FRAME: u16 = 960;

/// G711A uses static RTP payload type 8 (RFC 3551).
pub const G711A_STATIC_PAYLOAD_TYPE: u8 = 8;

/// G711U uses static RTP payload type 0 (RFC 3551).
pub const G711U_STATIC_PAYLOAD_TYPE: u8 = 0;

/// Audio output strategy configuration.
///
/// Controls how the module handles audio codec mismatches between the
/// source stream and the WebRTC client's capabilities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioOutputStrategy {
    /// Prefer transcoding to Opus when the source codec is not directly
    /// supported by the client. This is the default for Browser profile.
    TranscodeToOpus,
    /// Pass through the source codec unchanged. G711A/G711U use their
    /// static payload types. If the client does not support the source
    /// codec, the session fails with a clear error.
    Passthrough,
    /// Automatically select: G711 passes through when client supports it;
    /// AAC/MP3 transcode to Opus for Browser profile; fall back to error
    /// if transcoding is unavailable.
    #[default]
    Auto,
}

impl AudioOutputStrategy {
    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "transcode_to_opus" | "transcode-to-opus" | "opus" => Self::TranscodeToOpus,
            "passthrough" | "pass" => Self::Passthrough,
            "auto" | "" => Self::Auto,
            _ => Self::Auto,
        }
    }
}

/// The resolved audio output decision for a given source codec and profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioOutputDecision {
    /// Pass through the source codec directly. Includes the static RTP
    /// payload type for G711 variants.
    Passthrough {
        codec: CodecId,
        payload_type: u8,
        clock_rate: u32,
    },
    /// Transcode to Opus output with canonical WebRTC parameters.
    TranscodeToOpus {
        clock_rate: u32,
        channels: u8,
        samples_per_frame: u16,
    },
    /// Cannot produce audio output — transcoding required but unavailable.
    Unavailable {
        source_codec: CodecId,
        reason: AudioOutputError,
    },
}

/// Error describing why audio output cannot be negotiated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioOutputError {
    #[error(
        "audio codec {source_codec:?} requires transcoding to Opus for browser playback, \
         but transcoding capability is not available; cannot negotiate audio output"
    )]
    TranscodingUnavailable { source_codec: CodecId },
    #[error(
        "audio codec {source_codec:?} is not supported by the client and passthrough \
         is not possible; codec negotiation failed"
    )]
    CodecNotNegotiable { source_codec: CodecId },
}

/// Determines the audio output decision given the source codec, target
/// profile, configured strategy, and whether transcoding is available.
///
/// # Arguments
/// - `source_codec`: The codec of the incoming audio stream.
/// - `profile`: The configured codec profile (Browser, Device, Passthrough).
/// - `strategy`: The configured audio output strategy.
/// - `transcode_available`: Whether an Opus encoder is available at runtime.
/// - `client_supports_g711`: Whether the client's SDP offer includes G711.
pub fn resolve_audio_output(
    source_codec: CodecId,
    profile: CodecProfileWire,
    strategy: AudioOutputStrategy,
    transcode_available: bool,
    client_supports_g711: bool,
) -> AudioOutputDecision {
    match strategy {
        AudioOutputStrategy::Passthrough => resolve_passthrough(source_codec, client_supports_g711),
        AudioOutputStrategy::TranscodeToOpus => {
            resolve_transcode_to_opus(source_codec, transcode_available)
        }
        AudioOutputStrategy::Auto => resolve_auto(
            source_codec,
            profile,
            transcode_available,
            client_supports_g711,
        ),
    }
}

/// Passthrough strategy: use static payload types for G711, fail for
/// codecs that cannot be passed through.
fn resolve_passthrough(source_codec: CodecId, client_supports_g711: bool) -> AudioOutputDecision {
    match source_codec {
        CodecId::G711A if client_supports_g711 => AudioOutputDecision::Passthrough {
            codec: CodecId::G711A,
            payload_type: G711A_STATIC_PAYLOAD_TYPE,
            clock_rate: 8_000,
        },
        CodecId::G711U if client_supports_g711 => AudioOutputDecision::Passthrough {
            codec: CodecId::G711U,
            payload_type: G711U_STATIC_PAYLOAD_TYPE,
            clock_rate: 8_000,
        },
        CodecId::Opus => AudioOutputDecision::Passthrough {
            codec: CodecId::Opus,
            // Opus uses dynamic payload; 0 here means "use negotiated value"
            payload_type: 0,
            clock_rate: OPUS_CLOCK_RATE,
        },
        other => AudioOutputDecision::Unavailable {
            source_codec: other,
            reason: AudioOutputError::CodecNotNegotiable {
                source_codec: other,
            },
        },
    }
}

/// TranscodeToOpus strategy: always output Opus regardless of source.
fn resolve_transcode_to_opus(
    source_codec: CodecId,
    transcode_available: bool,
) -> AudioOutputDecision {
    // If source is already Opus, pass through directly
    if source_codec == CodecId::Opus {
        return AudioOutputDecision::TranscodeToOpus {
            clock_rate: OPUS_CLOCK_RATE,
            channels: OPUS_CHANNELS,
            samples_per_frame: OPUS_SAMPLES_PER_FRAME,
        };
    }

    if transcode_available {
        AudioOutputDecision::TranscodeToOpus {
            clock_rate: OPUS_CLOCK_RATE,
            channels: OPUS_CHANNELS,
            samples_per_frame: OPUS_SAMPLES_PER_FRAME,
        }
    } else {
        AudioOutputDecision::Unavailable {
            source_codec,
            reason: AudioOutputError::TranscodingUnavailable { source_codec },
        }
    }
}

/// Auto strategy: the default intelligent routing.
///
/// - G711A/G711U: pass through when client supports it (static payload 8/0).
/// - AAC/MP3 with Browser profile: prefer Opus output (requires transcoding).
/// - Opus source: pass through directly.
/// - Other combinations: fail with clear error.
fn resolve_auto(
    source_codec: CodecId,
    profile: CodecProfileWire,
    transcode_available: bool,
    client_supports_g711: bool,
) -> AudioOutputDecision {
    match source_codec {
        // G711 can pass through directly when client supports it
        CodecId::G711A if client_supports_g711 => AudioOutputDecision::Passthrough {
            codec: CodecId::G711A,
            payload_type: G711A_STATIC_PAYLOAD_TYPE,
            clock_rate: 8_000,
        },
        CodecId::G711U if client_supports_g711 => AudioOutputDecision::Passthrough {
            codec: CodecId::G711U,
            payload_type: G711U_STATIC_PAYLOAD_TYPE,
            clock_rate: 8_000,
        },
        // G711 without client support → transcode to Opus if available
        CodecId::G711A | CodecId::G711U => {
            if transcode_available {
                AudioOutputDecision::TranscodeToOpus {
                    clock_rate: OPUS_CLOCK_RATE,
                    channels: OPUS_CHANNELS,
                    samples_per_frame: OPUS_SAMPLES_PER_FRAME,
                }
            } else {
                AudioOutputDecision::Unavailable {
                    source_codec,
                    reason: AudioOutputError::TranscodingUnavailable { source_codec },
                }
            }
        }
        // Opus source passes through directly
        CodecId::Opus => AudioOutputDecision::Passthrough {
            codec: CodecId::Opus,
            payload_type: 0,
            clock_rate: OPUS_CLOCK_RATE,
        },
        // AAC/MP3 targeting Browser profile → prefer Opus
        CodecId::AAC | CodecId::MP3
            if profile == CodecProfileWire::Browser || profile == CodecProfileWire::Device =>
        {
            if transcode_available {
                AudioOutputDecision::TranscodeToOpus {
                    clock_rate: OPUS_CLOCK_RATE,
                    channels: OPUS_CHANNELS,
                    samples_per_frame: OPUS_SAMPLES_PER_FRAME,
                }
            } else {
                AudioOutputDecision::Unavailable {
                    source_codec,
                    reason: AudioOutputError::TranscodingUnavailable { source_codec },
                }
            }
        }
        // AAC/MP3 with Passthrough profile → cannot pass through to WebRTC
        CodecId::AAC | CodecId::MP3 => AudioOutputDecision::Unavailable {
            source_codec,
            reason: AudioOutputError::CodecNotNegotiable { source_codec },
        },
        // Any other codec
        other => AudioOutputDecision::Unavailable {
            source_codec: other,
            reason: AudioOutputError::CodecNotNegotiable {
                source_codec: other,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h265_blocked_by_browser_profile() {
        assert!(!WebRtcVideoCodecPreference::H265.is_allowed(CodecProfileWire::Browser));
        assert!(WebRtcVideoCodecPreference::H265.is_allowed(CodecProfileWire::Device));
        assert!(WebRtcVideoCodecPreference::H265.is_allowed(CodecProfileWire::Passthrough));
    }

    #[test]
    fn aac_blocked_by_browser_profile() {
        assert!(!WebRtcAudioCodecPreference::Aac.is_allowed(CodecProfileWire::Browser));
        assert!(WebRtcAudioCodecPreference::Aac.is_allowed(CodecProfileWire::Device));
    }

    #[test]
    fn unknown_codec_treated_as_any_and_allowed() {
        assert_eq!(
            WebRtcVideoCodecPreference::from_str_lossy("unknown"),
            WebRtcVideoCodecPreference::Any
        );
        assert!(WebRtcVideoCodecPreference::Any.is_allowed(CodecProfileWire::Browser));
    }

    // ─── Audio output strategy tests ────────────────────────────────────────

    #[test]
    fn g711_passthrough_keeps_static_payload() {
        // G711A → payload 8
        let decision = resolve_audio_output(
            CodecId::G711A,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            false, // no transcoding
            true,  // client supports G711
        );
        assert_eq!(
            decision,
            AudioOutputDecision::Passthrough {
                codec: CodecId::G711A,
                payload_type: G711A_STATIC_PAYLOAD_TYPE,
                clock_rate: 8_000,
            }
        );

        // G711U → payload 0
        let decision = resolve_audio_output(
            CodecId::G711U,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            false,
            true,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::Passthrough {
                codec: CodecId::G711U,
                payload_type: G711U_STATIC_PAYLOAD_TYPE,
                clock_rate: 8_000,
            }
        );
    }

    #[test]
    fn aac_browser_profile_requires_opus_output() {
        // AAC with Browser profile and transcoding available → Opus
        let decision = resolve_audio_output(
            CodecId::AAC,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            true, // transcoding available
            true,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::TranscodeToOpus {
                clock_rate: OPUS_CLOCK_RATE,
                channels: OPUS_CHANNELS,
                samples_per_frame: OPUS_SAMPLES_PER_FRAME,
            }
        );

        // MP3 with Browser profile and transcoding available → Opus
        let decision = resolve_audio_output(
            CodecId::MP3,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            true,
            true,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::TranscodeToOpus {
                clock_rate: OPUS_CLOCK_RATE,
                channels: OPUS_CHANNELS,
                samples_per_frame: OPUS_SAMPLES_PER_FRAME,
            }
        );
    }

    #[test]
    fn aac_browser_profile_no_transcoding_returns_clear_error() {
        let decision = resolve_audio_output(
            CodecId::AAC,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            false, // no transcoding
            true,
        );
        match decision {
            AudioOutputDecision::Unavailable {
                source_codec,
                reason,
            } => {
                assert_eq!(source_codec, CodecId::AAC);
                let msg = reason.to_string();
                assert!(
                    msg.contains("transcoding"),
                    "error should mention transcoding: {msg}"
                );
                assert!(
                    msg.contains("not available"),
                    "error should say not available: {msg}"
                );
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn opus_source_passes_through_directly() {
        let decision = resolve_audio_output(
            CodecId::Opus,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            false,
            false,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::Passthrough {
                codec: CodecId::Opus,
                payload_type: 0,
                clock_rate: OPUS_CLOCK_RATE,
            }
        );
    }

    #[test]
    fn opus_output_uses_canonical_webrtc_params() {
        // Verify the canonical Opus output parameters
        assert_eq!(OPUS_CLOCK_RATE, 48_000);
        assert_eq!(OPUS_CHANNELS, 2);
        assert_eq!(OPUS_SAMPLES_PER_FRAME, 960);
    }

    #[test]
    fn g711_without_client_support_transcodes_to_opus() {
        let decision = resolve_audio_output(
            CodecId::G711A,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            true,  // transcoding available
            false, // client does NOT support G711
        );
        assert_eq!(
            decision,
            AudioOutputDecision::TranscodeToOpus {
                clock_rate: OPUS_CLOCK_RATE,
                channels: OPUS_CHANNELS,
                samples_per_frame: OPUS_SAMPLES_PER_FRAME,
            }
        );
    }

    #[test]
    fn g711_without_client_support_no_transcoding_returns_error() {
        let decision = resolve_audio_output(
            CodecId::G711A,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Auto,
            false, // no transcoding
            false, // client does NOT support G711
        );
        match decision {
            AudioOutputDecision::Unavailable {
                source_codec,
                reason,
            } => {
                assert_eq!(source_codec, CodecId::G711A);
                let msg = reason.to_string();
                assert!(msg.contains("transcoding"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn passthrough_strategy_forces_passthrough() {
        let decision = resolve_audio_output(
            CodecId::G711A,
            CodecProfileWire::Browser,
            AudioOutputStrategy::Passthrough,
            true,
            true,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::Passthrough {
                codec: CodecId::G711A,
                payload_type: G711A_STATIC_PAYLOAD_TYPE,
                clock_rate: 8_000,
            }
        );
    }

    #[test]
    fn transcode_strategy_forces_opus_output() {
        let decision = resolve_audio_output(
            CodecId::G711U,
            CodecProfileWire::Passthrough,
            AudioOutputStrategy::TranscodeToOpus,
            true,
            true,
        );
        assert_eq!(
            decision,
            AudioOutputDecision::TranscodeToOpus {
                clock_rate: OPUS_CLOCK_RATE,
                channels: OPUS_CHANNELS,
                samples_per_frame: OPUS_SAMPLES_PER_FRAME,
            }
        );
    }

    #[test]
    fn audio_output_strategy_parses_known_values() {
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("auto"),
            AudioOutputStrategy::Auto
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("passthrough"),
            AudioOutputStrategy::Passthrough
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("transcode_to_opus"),
            AudioOutputStrategy::TranscodeToOpus
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("transcode-to-opus"),
            AudioOutputStrategy::TranscodeToOpus
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("opus"),
            AudioOutputStrategy::TranscodeToOpus
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy(""),
            AudioOutputStrategy::Auto
        );
        assert_eq!(
            AudioOutputStrategy::from_str_lossy("unknown"),
            AudioOutputStrategy::Auto
        );
    }
}
