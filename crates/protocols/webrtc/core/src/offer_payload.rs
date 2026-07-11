//! Pure-function payload type extraction from SDP offers.
//!
//! ABL fixed a bug (2025-06-12) where H264 payload was hardcoded instead of
//! extracted from the browser SDP; 2025-12-01 fixed Opus payload extraction.
//! This module provides a deterministic, Sans-I/O function that parses
//! `a=rtpmap:` lines and returns the negotiated payload type numbers for
//! codecs the server cares about.
//!
//! The function is case-insensitive on codec names (e.g. "h264" == "H264")
//! and returns a structured error when a required codec is not found.
//!
//! 本模块从 SDP offer 中纯函数式提取 payload type。
//!
//! ABL 曾修复一个 bug（2025-06-12）：H264 payload 被硬编码而非从浏览器 SDP 提取；
//! 2025-12-01 修复了 Opus payload 提取。本模块提供确定性的、无 I/O 的函数，
//! 解析 `a=rtpmap:` 行并返回服务器关心的编解码器协商后的 payload type 数字。
//!
//! 函数对编解码器名称大小写不敏感（例如 "h264" == "H264"），并在找不到必需
//! 编解码器时返回结构化错误。

use std::fmt;

use serde::{Deserialize, Serialize};

/// Codec identifiers that the server needs to extract from an SDP offer.
///
/// The set is intentionally small: these are the audio/video codecs that the
/// downstream engine can currently ingest. Other codecs are ignored during
/// payload extraction.
///
/// 服务器需要从 SDP offer 中提取的编解码器标识符。
///
/// 集合刻意保持较小：这些是下游引擎当前可接入的音频/视频编解码器。
/// 其他编解码器在 payload 提取过程中被忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OfferCodec {
    /// H.264 video at 90 kHz clock rate.
    ///
    /// 90 kHz 时钟的 H.264 视频。
    H264,
    /// H.265 (HEVC) video at 90 kHz clock rate.
    ///
    /// 90 kHz 时钟的 H.265（HEVC）视频。
    H265,
    /// Opus audio at 48 kHz clock rate.
    ///
    /// 48 kHz 时钟的 Opus 音频。
    Opus,
}

impl fmt::Display for OfferCodec {
    /// Format as `<codec>/<clock-rate>` for diagnostic messages.
    ///
    /// 格式化为 `<codec>/<clock-rate>` 供诊断消息使用。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::H264 => write!(f, "H264/90000"),
            Self::H265 => write!(f, "H265/90000"),
            Self::Opus => write!(f, "opus/48000"),
        }
    }
}

/// Successfully extracted payload type numbers from an SDP offer.
///
/// Fields are `Option` because not every offer contains every codec.
/// Use [`extract_offer_payloads`] to parse, then check which codecs
/// are present for your use case.
///
/// 从 SDP offer 成功提取的 payload type 数字。
///
/// 字段为 `Option`，因为并非每个 offer 都包含所有编解码器。使用
/// [`extract_offer_payloads`] 解析后，再检查当前用例包含哪些编解码器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferPayloads {
    /// Payload type for H264/90000 (first match).
    ///
    /// H264/90000 的 payload type（首个匹配）。
    pub h264: Option<u8>,
    /// Payload type for H265/90000 (first match).
    ///
    /// H265/90000 的 payload type（首个匹配）。
    pub h265: Option<u8>,
    /// Payload type for opus/48000 (first match).
    ///
    /// opus/48000 的 payload type（首个匹配）。
    pub opus: Option<u8>,
}

impl OfferPayloads {
    /// Returns the payload type for the given codec, or `None` if not found.
    ///
    /// 返回指定编解码器的 payload type，若未找到则返回 `None`。
    pub fn get(&self, codec: OfferCodec) -> Option<u8> {
        match codec {
            OfferCodec::H264 => self.h264,
            OfferCodec::H265 => self.h265,
            OfferCodec::Opus => self.opus,
        }
    }

    /// Require that specific codecs are present, returning a structured error
    /// listing all missing codecs if any are absent.
    ///
    /// 要求特定编解码器必须存在；若缺少则返回列出所有缺失编解码器的结构化错误。
    pub fn require(&self, codecs: &[OfferCodec]) -> Result<(), PayloadNotFound> {
        let missing: Vec<OfferCodec> = codecs
            .iter()
            .filter(|c| self.get(**c).is_none())
            .copied()
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(PayloadNotFound { missing })
        }
    }
}

/// Error returned when one or more required codecs are not found in the offer.
///
/// The `missing` vector preserves the order of the requested codecs so the
/// caller can build a single error message covering all absent codecs.
///
/// offer 中未找到必需编解码器时返回的错误。
///
/// `missing` 向量保留请求编解码器的顺序，调用方可据此构建覆盖所有缺失编解码器
/// 的单一错误消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadNotFound {
    /// The codecs that were required but not present in the SDP offer.
    ///
    /// 要求但 SDP offer 中不存在的编解码器。
    pub missing: Vec<OfferCodec>,
}

impl fmt::Display for PayloadNotFound {
    /// Human-readable list of missing codecs.
    ///
    /// 人类可读的缺失编解码器列表。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "required codec payload not found in offer: ")?;
        for (i, codec) in self.missing.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{codec}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PayloadNotFound {}

/// Extract payload type numbers from an SDP offer string.
///
/// Parses `a=rtpmap:<pt> <codec>/<clock>[/<channels>]` lines and matches
/// against the known codec set. Codec name matching is **case-insensitive**.
///
/// When multiple `a=rtpmap` lines match the same codec (e.g. two H264
/// entries with different profiles), the **first** match wins. This aligns
/// with browser preference ordering in the m-line.
///
/// This is a pure function with no I/O, no allocation beyond the result,
/// and no dependency on external state.
///
/// 从 SDP offer 字符串中提取 payload type 数字。
///
/// 解析 `a=rtpmap:<pt> <codec>/<clock>[/<channels>]` 行并与已知编解码器集合匹配。
/// 编解码器名称匹配 **大小写不敏感**。
///
/// 当多个 `a=rtpmap` 行匹配同一编解码器（例如两个不同 profile 的 H264 项）时，
/// **首个**匹配获胜。这与 m-line 中浏览器的偏好排序一致。
///
/// 这是一个纯函数：无 I/O，除结果外无额外分配，且不依赖外部状态。
///
/// # Examples
///
/// ```
/// use cheetah_webrtc_core::offer_payload::{extract_offer_payloads, OfferCodec};
///
/// let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n";
/// let payloads = extract_offer_payloads(sdp);
/// assert_eq!(payloads.opus, Some(111));
/// assert_eq!(payloads.h264, None);
/// payloads.require(&[OfferCodec::Opus]).unwrap();
/// ```
pub fn extract_offer_payloads(sdp: &str) -> OfferPayloads {
    let mut result = OfferPayloads {
        h264: None,
        h265: None,
        opus: None,
    };

    for line in sdp.lines() {
        let trimmed = line.trim();
        let rest = match trimmed.strip_prefix("a=rtpmap:") {
            Some(r) => r,
            None => continue,
        };

        // Format: <payload-type> <codec-name>/<clock-rate>[/<channels>]
        let mut parts = rest.splitn(2, ' ');
        let pt_str = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let codec_clock = match parts.next() {
            Some(s) => s,
            None => continue,
        };

        let pt: u8 = match pt_str.parse() {
            Ok(v) if v <= 127 => v,
            Ok(_) => continue,
            Err(_) => continue,
        };

        // Split codec/clock/channels — we only need codec and clock.
        let mut codec_parts = codec_clock.splitn(3, '/');
        let codec_name = match codec_parts.next() {
            Some(s) => s,
            None => continue,
        };
        let clock_rate = match codec_parts.next() {
            Some(s) => s,
            None => continue,
        };

        // Case-insensitive matching on codec name.
        let codec_upper = codec_name.to_ascii_uppercase();
        match (codec_upper.as_str(), clock_rate) {
            ("H264", "90000") if result.h264.is_none() => {
                result.h264 = Some(pt);
            }
            ("H265" | "HEVC", "90000") if result.h265.is_none() => {
                result.h265 = Some(pt);
            }
            ("OPUS", "48000") if result.opus.is_none() => {
                result.opus = Some(pt);
            }
            _ => {}
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_payload_parser_uses_browser_h264_payload() {
        // Chrome uses H264 at PT 100
        let chrome_sdp = include_str!("../tests/fixtures/offer_from_chrome.sdp");
        let payloads = extract_offer_payloads(chrome_sdp);
        assert_eq!(payloads.h264, Some(100));

        // Firefox uses H264 at PT 121
        let firefox_sdp = include_str!("../tests/fixtures/offer_from_firefox.sdp");
        let payloads = extract_offer_payloads(firefox_sdp);
        assert_eq!(payloads.h264, Some(121));

        // Safari uses H264 at PT 96
        let safari_sdp = include_str!("../tests/fixtures/offer_from_safari.sdp");
        let payloads = extract_offer_payloads(safari_sdp);
        assert_eq!(payloads.h264, Some(96));
    }

    #[test]
    fn offer_payload_parser_uses_browser_opus_payload() {
        // Chrome uses opus at PT 111
        let chrome_sdp = include_str!("../tests/fixtures/offer_from_chrome.sdp");
        let payloads = extract_offer_payloads(chrome_sdp);
        assert_eq!(payloads.opus, Some(111));

        // Firefox uses opus at PT 109
        let firefox_sdp = include_str!("../tests/fixtures/offer_from_firefox.sdp");
        let payloads = extract_offer_payloads(firefox_sdp);
        assert_eq!(payloads.opus, Some(109));

        // Safari uses opus at PT 111
        let safari_sdp = include_str!("../tests/fixtures/offer_from_safari.sdp");
        let payloads = extract_offer_payloads(safari_sdp);
        assert_eq!(payloads.opus, Some(111));
    }

    #[test]
    fn codec_name_matching_is_case_insensitive() {
        let sdp = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=rtpmap:96 h264/90000\r\n",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            "a=rtpmap:111 OPUS/48000/2\r\n",
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, Some(96));
        assert_eq!(payloads.opus, Some(111));
    }

    #[test]
    fn mixed_case_codec_names_are_recognized() {
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:42 H264/90000\r\n",
            "a=rtpmap:77 Opus/48000/2\r\n",
            "a=rtpmap:55 h265/90000\r\n",
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, Some(42));
        assert_eq!(payloads.opus, Some(77));
        assert_eq!(payloads.h265, Some(55));
    }

    #[test]
    fn hevc_alias_is_recognized_for_h265() {
        let sdp = "v=0\r\na=rtpmap:108 HEVC/90000\r\n";
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h265, Some(108));
    }

    #[test]
    fn first_match_wins_for_duplicate_codecs() {
        // Two H264 entries — first one (PT 96) should win
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:96 H264/90000\r\n",
            "a=rtpmap:102 H264/90000\r\n",
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, Some(96));
    }

    #[test]
    fn missing_codec_returns_none() {
        let sdp = concat!("v=0\r\n", "a=rtpmap:96 VP8/90000\r\n",);
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, None);
        assert_eq!(payloads.h265, None);
        assert_eq!(payloads.opus, None);
    }

    #[test]
    fn require_returns_error_listing_missing_codecs() {
        let sdp = concat!("v=0\r\n", "a=rtpmap:111 opus/48000/2\r\n",);
        let payloads = extract_offer_payloads(sdp);
        let err = payloads
            .require(&[OfferCodec::H264, OfferCodec::Opus])
            .unwrap_err();
        assert_eq!(err.missing, vec![OfferCodec::H264]);
        assert!(err.to_string().contains("H264/90000"));
    }

    #[test]
    fn require_succeeds_when_all_codecs_present() {
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:96 H264/90000\r\n",
            "a=rtpmap:111 opus/48000/2\r\n",
        );
        let payloads = extract_offer_payloads(sdp);
        payloads
            .require(&[OfferCodec::H264, OfferCodec::Opus])
            .unwrap();
    }

    #[test]
    fn malformed_rtpmap_lines_are_skipped() {
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:abc H264/90000\r\n", // non-numeric PT
            "a=rtpmap:96\r\n",             // missing codec
            "a=rtpmap:97 H264\r\n",        // missing clock rate
            "a=rtpmap:98 H264/90000\r\n",  // valid
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, Some(98));
    }

    #[test]
    fn payload_types_above_127_are_rejected() {
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:200 H264/90000\r\n",
            "a=rtpmap:128 opus/48000/2\r\n",
            "a=rtpmap:127 HEVC/90000\r\n",
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, None);
        assert_eq!(payloads.opus, None);
        assert_eq!(payloads.h265, Some(127));
    }

    #[test]
    fn wrong_clock_rate_is_not_matched() {
        let sdp = concat!(
            "v=0\r\n",
            "a=rtpmap:96 H264/48000\r\n",   // wrong clock for H264
            "a=rtpmap:111 opus/8000/2\r\n", // wrong clock for opus
        );
        let payloads = extract_offer_payloads(sdp);
        assert_eq!(payloads.h264, None);
        assert_eq!(payloads.opus, None);
    }

    #[test]
    fn empty_sdp_returns_all_none() {
        let payloads = extract_offer_payloads("");
        assert_eq!(payloads.h264, None);
        assert_eq!(payloads.h265, None);
        assert_eq!(payloads.opus, None);
    }

    #[test]
    fn payload_not_found_error_display() {
        let err = PayloadNotFound {
            missing: vec![OfferCodec::H264, OfferCodec::H265, OfferCodec::Opus],
        };
        let msg = err.to_string();
        assert!(msg.contains("H264/90000"));
        assert!(msg.contains("H265/90000"));
        assert!(msg.contains("opus/48000"));
    }
}
