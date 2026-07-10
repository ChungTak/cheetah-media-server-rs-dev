//! SDP compatibility preprocessing.
//!
//! `str0m` is fairly strict about RFC compliance. Some real-world peers
//! (older browsers, vendor stacks, SMS-style fixtures) ship offers that
//! `str0m` rejects out of the box. We normalize the most common
//! deviations here, before the offer reaches `str0m::change::SdpOffer`.
//!
//! The preprocessor is intentionally conservative — it only repairs
//! patterns we have observed in real fixtures and emits a
//! [`SdpCompatReport`] describing what was changed so that operators have
//! visibility into vendor quirks.
//!
//! 本模块进行 SDP 兼容性预处理。
//!
//! `str0m` 对 RFC 合规性要求严格。一些真实对端（旧浏览器、厂商协议栈、
//! SMS 风格 fixture）发送的 offer 会被 `str0m` 直接拒绝。我们在 offer 进入
//! `str0m::change::SdpOffer` 之前，对最常见偏差进行归一化。
//!
//! 预处理器刻意保持保守——仅修复我们在真实 fixture 中见过的模式，并发出
//! [`SdpCompatReport`] 描述改动内容，以便运维人员了解厂商 quirks。
//!
//! ## `a=ssrc-group:SIM` without RID
//!
//! Some SDP-munging tools strip `a=rid` and `a=simulcast` lines but leave
//! `a=ssrc-group:SIM <ssrc0> <ssrc1> [<ssrc2>]`. ZLMediaKit's
//! `RtpExtContext` handles this by generating stable RID labels from SSRC
//! ordering (`r0`, `r1`, `r2`). We replicate this behaviour in
//! [`inject_rid_from_ssrc_group_sim`] so that `str0m` can negotiate
//! simulcast even when the offer has been munged.
//!
//! 一些 SDP 篡改工具会剥离 `a=rid` 和 `a=simulcast` 行，但保留
//! `a=ssrc-group:SIM <ssrc0> <ssrc1> [<ssrc2>]`。ZLMediaKit 的
//! `RtpExtContext` 通过按 SSRC 顺序生成稳定 RID 标签（`r0`、`r1`、`r2`）
//! 来处理这种情况。我们在 [`inject_rid_from_ssrc_group_sim`] 中复现该行为，
//! 使 `str0m` 即使在被篡改的 offer 上也能协商 simulcast。

use serde::{Deserialize, Serialize};

/// Outcome of running [`preprocess_remote_sdp`].
///
/// Each field tracks a specific class of repair so the module can report
/// vendor compatibility metrics and reproduce the original SDP shape if
/// needed.
///
/// [`preprocess_remote_sdp`] 的运行结果。
///
/// 每个字段跟踪特定修复类别，模块可据此上报厂商兼容性指标，并在需要时
/// 还原原始 SDP 形态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdpCompatReport {
    pub trimmed_trailing_whitespace: bool,
    pub normalized_line_endings: bool,
    pub appended_missing_terminator: bool,
    /// True when `a=ssrc-group:SIM` was present without `a=rid` lines and
    /// the preprocessor injected synthetic `a=rid:r0/r1/r2` + `a=simulcast`.
    ///
    /// 当存在 `a=ssrc-group:SIM` 但无 `a=rid` 行时，预处理器注入合成的
    /// `a=rid:r0/r1/r2` + `a=simulcast`，此字段为 true。
    pub ssrc_group_sim_rid_generated: bool,
    /// True when `a=extmap-allow-mixed` was observed in the session level.
    ///
    /// 在会话级别观察到 `a=extmap-allow-mixed` 时为 true。
    pub extmap_allow_mixed_observed: bool,
}

impl SdpCompatReport {
    /// Returns true if any repair was applied.
    ///
    /// 若应用了任何修复则返回 true。
    pub fn is_modified(&self) -> bool {
        self.trimmed_trailing_whitespace
            || self.normalized_line_endings
            || self.appended_missing_terminator
            || self.ssrc_group_sim_rid_generated
    }
}

/// RTP extension type enumeration aligned with ZLM `RTP_EXT_MAP`.
///
/// Used by [`RtpExtensionMapping`] to provide a stable, str0m-independent
/// view of the negotiated extension set for module-layer observability.
///
/// 与 ZLM `RTP_EXT_MAP` 对齐的 RTP 扩展类型枚举。
///
/// 用于 [`RtpExtensionMapping`]，为模块层可观测性提供稳定的、不依赖 `str0m` 的
/// 协商扩展集合视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RtpExtensionType {
    /// `urn:ietf:params:rtp-hdrext:ssrc-audio-level` (RFC 6464)
    ///
    /// `urn:ietf:params:rtp-hdrext:ssrc-audio-level`（RFC 6464）
    AudioLevel,
    /// `http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time`
    ///
    /// `http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time`
    AbsSendTime,
    /// `http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01`
    ///
    /// `http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01`
    TransportWideCc,
    /// `urn:ietf:params:rtp-hdrext:sdes:mid`
    ///
    /// `urn:ietf:params:rtp-hdrext:sdes:mid`
    Mid,
    /// `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id`
    ///
    /// `urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id`
    Rid,
    /// `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id`
    ///
    /// `urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id`
    RepairedRid,
    /// `urn:3gpp:video-orientation` (RFC 7742)
    ///
    /// `urn:3gpp:video-orientation`（RFC 7742）
    VideoOrientation,
    /// `http://www.webrtc.org/experiments/rtp-hdrext/video-timing`
    ///
    /// `http://www.webrtc.org/experiments/rtp-hdrext/video-timing`
    VideoTiming,
    /// `http://www.webrtc.org/experiments/rtp-hdrext/playout-delay`
    ///
    /// `http://www.webrtc.org/experiments/rtp-hdrext/playout-delay`
    PlayoutDelay,
    /// `urn:ietf:params:rtp-hdrext:toffset`
    ///
    /// `urn:ietf:params:rtp-hdrext:toffset`
    TransmissionOffset,
    /// `http://www.webrtc.org/experiments/rtp-hdrext/video-content-type`
    ///
    /// `http://www.webrtc.org/experiments/rtp-hdrext/video-content-type`
    VideoContentType,
    /// `http://www.webrtc.org/experiments/rtp-hdrext/color-space`
    ///
    /// `http://www.webrtc.org/experiments/rtp-hdrext/color-space`
    ColorSpace,
    /// `urn:ietf:params:rtp-hdrext:framemarking` or draft variant
    ///
    /// `urn:ietf:params:rtp-hdrext:framemarking` 或其 draft 变体
    FrameMarking,
    /// AV1 dependency descriptor
    ///
    /// AV1 依赖描述符
    Av1DependencyDescriptor,
    /// Extension URI not recognized by this enumeration.
    ///
    /// 本枚举无法识别的扩展 URI。
    Unknown,
}

impl RtpExtensionType {
    /// Map an SDP `extmap` URI to the corresponding type.
    ///
    /// Uses substring matching for draft/extension names that do not have a
    /// single canonical URI.
    ///
    /// 将 SDP `extmap` URI 映射到对应类型。
    ///
    /// 对没有单一规范 URI 的 draft/扩展名使用子串匹配。
    pub fn from_uri(uri: &str) -> Self {
        match uri {
            "urn:ietf:params:rtp-hdrext:ssrc-audio-level" => Self::AudioLevel,
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time" => Self::AbsSendTime,
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01" => {
                Self::TransportWideCc
            }
            "urn:ietf:params:rtp-hdrext:sdes:mid" => Self::Mid,
            "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id" => Self::Rid,
            "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id" => Self::RepairedRid,
            "urn:3gpp:video-orientation" => Self::VideoOrientation,
            "http://www.webrtc.org/experiments/rtp-hdrext/video-timing" => Self::VideoTiming,
            "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay" => Self::PlayoutDelay,
            "urn:ietf:params:rtp-hdrext:toffset" => Self::TransmissionOffset,
            "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type" => {
                Self::VideoContentType
            }
            "http://www.webrtc.org/experiments/rtp-hdrext/color-space" => Self::ColorSpace,
            s if s.contains("framemarking") => Self::FrameMarking,
            s if s.contains("dependency-descriptor") => Self::Av1DependencyDescriptor,
            _ => Self::Unknown,
        }
    }

    /// Return the canonical URI for known extension types.
    ///
    /// `Unknown` returns an empty string because there is no canonical URI.
    ///
    /// 返回已知扩展类型的规范 URI。
    ///
    /// `Unknown` 返回空字符串，因为没有规范 URI。
    pub fn uri(&self) -> &'static str {
        match self {
            Self::AudioLevel => "urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            Self::AbsSendTime => "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            Self::TransportWideCc => {
                "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01"
            }
            Self::Mid => "urn:ietf:params:rtp-hdrext:sdes:mid",
            Self::Rid => "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id",
            Self::RepairedRid => "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id",
            Self::VideoOrientation => "urn:3gpp:video-orientation",
            Self::VideoTiming => "http://www.webrtc.org/experiments/rtp-hdrext/video-timing",
            Self::PlayoutDelay => "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay",
            Self::TransmissionOffset => "urn:ietf:params:rtp-hdrext:toffset",
            Self::VideoContentType => {
                "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type"
            }
            Self::ColorSpace => "http://www.webrtc.org/experiments/rtp-hdrext/color-space",
            Self::FrameMarking => "urn:ietf:params:rtp-hdrext:framemarking",
            Self::Av1DependencyDescriptor => {
                "https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension"
            }
            Self::Unknown => "",
        }
    }
}

/// A single RTP extension mapping extracted from an SDP `a=extmap` line.
///
/// The `id` is the numeric RTP header extension id, `ext_type` is the
/// normalized type, and `direction` is the optional SDP direction qualifier.
///
/// 从 SDP `a=extmap` 行提取的单个 RTP 扩展映射。
///
/// `id` 是 RTP 头扩展数字 id，`ext_type` 是归一化类型，`direction` 是可选的
/// SDP 方向限定符。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpExtensionMapping {
    /// Numeric extension id (1–14 for one-byte, 1–255 for two-byte).
    ///
    /// 数字扩展 id（one-byte 为 1–14，two-byte 为 1–255）。
    pub id: u8,
    /// Parsed extension type.
    ///
    /// 解析后的扩展类型。
    pub ext_type: RtpExtensionType,
    /// Raw URI from the SDP.
    ///
    /// SDP 中的原始 URI。
    pub uri: String,
    /// Direction qualifier if present (`sendonly`, `recvonly`, `sendrecv`).
    ///
    /// 方向限定符（如果存在），如 `sendonly`、`recvonly`、`sendrecv`。
    pub direction: Option<String>,
}

/// Extract all `a=extmap` mappings from an SDP string.
///
/// This is a lightweight line-based parser that does not require a full SDP
/// parse. It returns mappings in document order, including duplicates across
/// m-lines. The caller can group by media section if needed.
///
/// 从 SDP 字符串提取所有 `a=extmap` 映射。
///
/// 这是一个轻量级基于行的解析器，无需完整 SDP 解析。它按文档顺序返回映射，
/// 包括跨 m-line 的重复项。调用方可按需按媒体段分组。
pub fn extract_rtp_extension_mappings(sdp: &str) -> Vec<RtpExtensionMapping> {
    let mut mappings = Vec::new();
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("a=extmap:") {
            // Format: <id>[/<direction>] <uri> [<extensionattributes>]
            let mut parts = rest.splitn(2, ' ');
            let id_part = match parts.next() {
                Some(p) => p,
                None => continue,
            };
            let uri = match parts.next() {
                Some(p) => p.split_whitespace().next().unwrap_or(""),
                None => continue,
            };
            if uri.is_empty() {
                continue;
            }
            let (id_str, direction) = if let Some(slash_pos) = id_part.find('/') {
                (
                    &id_part[..slash_pos],
                    Some(id_part[slash_pos + 1..].to_string()),
                )
            } else {
                (id_part, None)
            };
            let id: u8 = match id_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            mappings.push(RtpExtensionMapping {
                id,
                ext_type: RtpExtensionType::from_uri(uri),
                uri: uri.to_string(),
                direction,
            });
        }
    }
    mappings
}

/// Ensure each video m-line contains the playout-delay RTP header
/// extension mapping.
///
/// This repairs offers that omit the extension but expect the receiver to
/// accept it. Existing playout-delay mappings are preserved.
///
/// 确保每个 video m-line 都包含 playout-delay RTP 头扩展映射。
///
/// 用于修复省略了该扩展但期望接收方接受的 offer。已存在的 playout-delay 映射
/// 会被保留。
///
/// Returns a possibly rewritten SDP string.
///
/// 返回可能改写后的 SDP 字符串。
pub fn ensure_playout_delay_extmap(sdp: &str) -> String {
    const URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";
    let mut lines: Vec<String> = sdp
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    if lines.is_empty() {
        return sdp.to_string();
    }
    let mut i = 0usize;
    while i < lines.len() {
        if !lines[i].starts_with("m=video ") {
            i += 1;
            continue;
        }
        let section_start = i;
        let mut section_end = lines.len();
        let mut probe = i + 1;
        while probe < lines.len() {
            if lines[probe].starts_with("m=") {
                section_end = probe;
                break;
            }
            probe += 1;
        }
        let mut has_playout = false;
        let mut used_ids = std::collections::BTreeSet::new();
        let mut last_extmap_line = None;
        for (idx, line) in lines
            .iter()
            .enumerate()
            .take(section_end)
            .skip(section_start + 1)
        {
            let line = line.trim();
            if line.contains(URI) {
                has_playout = true;
            }
            if let Some(id) = parse_extmap_id(line) {
                used_ids.insert(id);
                last_extmap_line = Some(idx);
            }
        }
        if !has_playout {
            let Some(extmap_id) = (1u8..=14u8).find(|id| !used_ids.contains(id)) else {
                i = section_end;
                continue;
            };
            let insert_at = last_extmap_line.unwrap_or(section_start) + 1;
            lines.insert(insert_at, format!("a=extmap:{extmap_id} {URI}"));
            i = section_end + 1;
        } else {
            i = section_end;
        }
    }
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// Remove RED/ULPFEC payload types from local SDP.
///
/// Useful for deployments where these repair codecs are not wired in
/// the packet pipeline and should not be negotiated.
///
/// 从本地 SDP 中移除 RED/ULPFEC payload type。
///
/// 用于这些修复编解码器未接入包管线且不应协商的部署。
///
/// Returns a possibly rewritten SDP string.
///
/// 返回可能改写后的 SDP 字符串。
pub fn strip_red_ulpfec_payloads(sdp: &str) -> String {
    if sdp.is_empty() {
        return sdp.to_string();
    }

    let mut out = String::with_capacity(sdp.len());
    let mut section = Vec::<String>::new();
    let mut in_media_section = false;
    for raw in sdp.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with("m=") {
            if in_media_section {
                append_sdp_lines(&mut out, process_red_ulpfec_media_section(&section));
            } else {
                append_sdp_lines(&mut out, section.iter());
                in_media_section = true;
            }
            section.clear();
        }
        section.push(line.to_string());
    }
    if in_media_section {
        append_sdp_lines(&mut out, process_red_ulpfec_media_section(&section));
    } else {
        append_sdp_lines(&mut out, section.iter());
    }
    out
}

/// Append SDP lines to a string with CRLF terminators.
///
/// 将 SDP 行附加到字符串，使用 CRLF 终止符。
fn append_sdp_lines<I, S>(out: &mut String, lines: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for line in lines {
        out.push_str(line.as_ref());
        out.push_str("\r\n");
    }
}

/// Remove RED/ULPFEC lines from a single media section.
///
/// Also drops RTX payloads whose `apt` points to a disabled payload.
///
/// 从单个媒体段移除 RED/ULPFEC 行。
///
/// 同时删除 `apt` 指向被禁用 payload 的 RTX payload。
fn process_red_ulpfec_media_section(lines: &[String]) -> Vec<String> {
    let disabled_pts = repair_payloads_to_disable_in_section(lines);
    if disabled_pts.is_empty() {
        return lines.to_vec();
    }
    let Some(mline) = lines.first() else {
        return Vec::new();
    };
    let Some(rewritten_mline) = rewrite_mline_without_payloads(mline, &disabled_pts) else {
        // Avoid creating an invalid media section with no payload
        // formats. A RED/ULPFEC-only m-line is unusual, but preserving
        // it is safer than advertising an empty section.
        return lines.to_vec();
    };

    let mut processed = Vec::with_capacity(lines.len());
    processed.push(rewritten_mline);
    for line in lines.iter().skip(1) {
        if !should_drop_payload_line(line, &disabled_pts) {
            processed.push(line.clone());
        }
    }
    processed
}

/// Find RED/ULPFEC payload types and their dependent RTX payloads in a section.
///
/// Iterates until a fixed point is reached because disabling a RED/ULPFEC
/// payload may cause a dependent RTX payload to become disabled, which may
/// in turn affect another RTX payload.
///
/// 在单个媒体段中查找 RED/ULPFEC payload type 及其依赖的 RTX payload。
///
/// 迭代直到不动点，因为禁用 RED/ULPFEC payload 可能使依赖的 RTX payload
/// 被禁用，进而影响其他 RTX payload。
fn repair_payloads_to_disable_in_section(lines: &[String]) -> std::collections::BTreeSet<String> {
    let codecs = payload_codecs_in_section(lines);
    let mut disabled_pts = std::collections::BTreeSet::<String>::new();
    for (pt, codec) in &codecs {
        if codec == "red" || codec == "ulpfec" {
            disabled_pts.insert(pt.clone());
        }
    }

    loop {
        let mut changed = false;
        for line in lines {
            let Some((pt, apt)) = parse_fmtp_apt(line) else {
                continue;
            };
            if codecs.get(&pt).is_some_and(|codec| codec == "rtx")
                && disabled_pts.contains(&apt)
                && disabled_pts.insert(pt)
            {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    disabled_pts
}

/// Extract `payload-type -> codec-name` mapping from a media section.
///
/// 从媒体段提取 `payload-type -> codec-name` 映射。
fn payload_codecs_in_section(lines: &[String]) -> std::collections::BTreeMap<String, String> {
    let mut codecs = std::collections::BTreeMap::<String, String>::new();
    for line in lines {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("a=rtpmap:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        if let (Some(pt), Some(codec_spec)) = (parts.next(), parts.next()) {
            let codec = codec_spec
                .split('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            codecs.insert(pt.to_string(), codec);
        }
    }
    codecs
}

/// Parse `a=fmtp:<pt> apt=<associated-pt>` into `(pt, apt)`.
///
/// 解析 `a=fmtp:<pt> apt=<associated-pt>` 为 `(pt, apt)`。
fn parse_fmtp_apt(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("a=fmtp:")?;
    let (pt, params) = rest.split_once(char::is_whitespace)?;
    for param in params.split(';') {
        let param = param.trim();
        let Some(value) = param.strip_prefix("apt=") else {
            continue;
        };
        let apt = value.split_whitespace().next().unwrap_or("");
        if !apt.is_empty() {
            return Some((pt.to_string(), apt.to_string()));
        }
    }
    None
}

/// Parse the numeric id from an `a=extmap:` line, ignoring direction.
///
/// 从 `a=extmap:` 行解析数字 id，忽略方向。
fn parse_extmap_id(line: &str) -> Option<u8> {
    let rest = line.strip_prefix("a=extmap:")?;
    let id_part = rest.split_whitespace().next()?;
    let id_token = id_part.split('/').next()?;
    id_token.parse::<u8>().ok()
}

/// Returns true if the line is an rtpmap/fmtp/rtcp-fb for a disabled payload.
///
/// 若该行是被禁用 payload 的 rtpmap/fmtp/rtcp-fb，则返回 true。
fn should_drop_payload_line(line: &str, disabled_pts: &std::collections::BTreeSet<String>) -> bool {
    for prefix in ["a=rtpmap:", "a=fmtp:", "a=rtcp-fb:"] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let pt = rest.split_whitespace().next().unwrap_or("");
            if disabled_pts.contains(pt) {
                return true;
            }
        }
    }
    false
}

/// Rewrite an `m=` line, removing the listed payload types.
///
/// Returns `None` if the resulting m-line would have no payload formats.
///
/// 重写 `m=` 行，移除列出的 payload type。
///
/// 若结果 m-line 没有 payload 格式，则返回 `None`。
fn rewrite_mline_without_payloads(
    line: &str,
    disabled_pts: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if !line.starts_with("m=") {
        return None;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() <= 3 {
        return None;
    }
    let mut rebuilt = vec![parts[0], parts[1], parts[2]];
    for payload in &parts[3..] {
        if !disabled_pts.contains(*payload) {
            rebuilt.push(payload);
        }
    }
    if rebuilt.len() <= 3 {
        return None;
    }
    Some(rebuilt.join(" "))
}

/// Inject `a=rid` and `a=simulcast` lines for media sections that have
/// `a=ssrc-group:SIM` but no `a=rid` lines.
///
/// ZLMediaKit and some SDP-munging tools strip RID/simulcast lines but
/// leave the SSRC group. Without RID lines, `str0m` cannot negotiate
/// simulcast. This function generates stable labels `r0`, `r1`, `r2` from
/// the SSRC ordering in the group, matching ZLM `RtpExtContext` behaviour.
///
/// Returns `true` if any injection was performed.
///
/// 为存在 `a=ssrc-group:SIM` 但无 `a=rid` 行的媒体段注入 `a=rid` 和
/// `a=simulcast` 行。
///
/// ZLMediaKit 与一些 SDP 篡改工具会剥离 RID/simulcast 行但保留 SSRC group。
/// 没有 RID 行时 `str0m` 无法协商 simulcast。本函数按 group 中的 SSRC 顺序
/// 生成稳定标签 `r0`、`r1`、`r2`，与 ZLM `RtpExtContext` 行为一致。
///
/// 若执行了任何注入则返回 `true`。
pub fn inject_rid_from_ssrc_group_sim(sdp: &mut String) -> bool {
    // We work on a line-by-line basis. For each m= section, check if it
    // has `a=ssrc-group:SIM` but no `a=rid:` lines. If so, inject
    // synthetic RID and simulcast lines before the next m= or at EOF.
    let lines: Vec<&str> = sdp.lines().collect();
    let mut result = String::with_capacity(sdp.len() + 256);
    let mut injected = false;

    // Track per-section state
    let mut in_media_section = false;
    let mut has_rid = false;
    let mut ssrc_group_sim_ssrcs: Vec<String> = Vec::new();
    let mut section_lines: Vec<&str> = Vec::new();

    let flush_section = |section_lines: &[&str],
                         has_rid: bool,
                         ssrc_group_sim_ssrcs: &[String],
                         result: &mut String,
                         injected: &mut bool| {
        // Write all lines of this section
        for &line in section_lines {
            result.push_str(line);
            result.push_str("\r\n");
        }
        // If we have SIM group but no RID, inject
        if !has_rid && ssrc_group_sim_ssrcs.len() >= 2 {
            let count = ssrc_group_sim_ssrcs.len().min(3);
            for i in 0..count {
                result.push_str(&format!("a=rid:r{i} send\r\n"));
            }
            let rids: Vec<String> = (0..count).map(|i| format!("r{i}")).collect();
            result.push_str(&format!("a=simulcast:send {}\r\n", rids.join(";")));
            *injected = true;
        }
    };

    for &line in lines.iter() {
        if line.starts_with("m=") {
            if in_media_section {
                // Flush previous section
                flush_section(
                    &section_lines,
                    has_rid,
                    &ssrc_group_sim_ssrcs,
                    &mut result,
                    &mut injected,
                );
                section_lines.clear();
            }
            in_media_section = true;
            has_rid = false;
            ssrc_group_sim_ssrcs.clear();
            section_lines.push(line);
        } else if in_media_section {
            section_lines.push(line);
            let trimmed = line.trim();
            if trimmed.starts_with("a=rid:") {
                has_rid = true;
            } else if let Some(after_sim) = trimmed.strip_prefix("a=ssrc-group:SIM ") {
                // Parse SSRCs from the group line
                ssrc_group_sim_ssrcs = after_sim
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
            }
        } else {
            // Session-level lines before first m=
            result.push_str(line);
            result.push_str("\r\n");
        }
    }

    // Flush last section
    if in_media_section {
        flush_section(
            &section_lines,
            has_rid,
            &ssrc_group_sim_ssrcs,
            &mut result,
            &mut injected,
        );
    }

    if injected {
        *sdp = result;
    }
    injected
}

/// Apply minimal compatibility transformations to a remote SDP.
///
/// Safe transformations only:
/// * Convert any of `\r`, `\n`, `\r\n` to `\r\n`.
/// * Trim trailing whitespace from each line.
/// * Ensure the SDP ends with `\r\n`.
/// * Inject `a=rid` + `a=simulcast` when `a=ssrc-group:SIM` is present
///   without RID lines (ZLM/SDP-munging compatibility).
///
/// We do **not** rewrite codec lines, ICE attributes, or fingerprint here —
/// those need to be preserved verbatim for `str0m` to validate the peer.
///
/// 对远端 SDP 应用最小兼容性转换。
///
/// 仅安全转换：
/// - 将 `\r`、`\n`、`\r\n` 统一为 `\r\n`。
/// - 去掉每行尾部空白。
/// - 确保 SDP 以 `\r\n` 结尾。
/// - 当存在 `a=ssrc-group:SIM` 但无 RID 行时注入 `a=rid` + `a=simulcast`
///   （ZLM/SDP 篡改兼容性）。
///
/// 我们**不**在此重写 codec 行、ICE 属性或 fingerprint——这些需要原样保留，
/// 供 `str0m` 验证对端。
///
/// Returns the normalized SDP and a report describing the changes.
///
/// 返回归一化后的 SDP 与描述变化的报告。
pub fn preprocess_remote_sdp(input: &str) -> (String, SdpCompatReport) {
    let mut report = SdpCompatReport::default();

    // Step 1: normalise line endings to a `\n`-only intermediate form,
    // detecting any mismatched terminator.
    let mut intermediate = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if matches!(chars.peek(), Some('\n')) {
                    // CRLF — the canonical form. Consume the LF.
                    let _ = chars.next();
                    intermediate.push('\n');
                } else {
                    // Lone CR.
                    report.normalized_line_endings = true;
                    intermediate.push('\n');
                }
            }
            '\n' => {
                // Lone LF (the only branch where this fires given the
                // CR arm consumes its companion).
                report.normalized_line_endings = true;
                intermediate.push('\n');
            }
            other => intermediate.push(other),
        }
    }

    // Step 2: trim trailing whitespace from each line and emit CRLF
    // separators. Always re-emit CRLF between lines; the trailing
    // empty segment that comes from a final `\n` is dropped here so we
    // don't double-emit terminators.
    let mut trimmed = String::with_capacity(intermediate.len());
    let lines: Vec<&str> = intermediate.split('\n').collect();
    let last_idx = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        let trimmed_line = line.trim_end_matches([' ', '\t']);
        if trimmed_line.len() != line.len() {
            report.trimmed_trailing_whitespace = true;
        }
        trimmed.push_str(trimmed_line);
        if i != last_idx {
            trimmed.push_str("\r\n");
        }
    }

    // Step 3: ensure the document ends with CRLF.
    let mut final_text = if trimmed.is_empty() {
        String::new()
    } else if trimmed.ends_with("\r\n") {
        trimmed
    } else {
        report.appended_missing_terminator = true;
        format!("{trimmed}\r\n")
    };

    // Step 4: detect `a=extmap-allow-mixed` for observability.
    if final_text.contains("a=extmap-allow-mixed") {
        report.extmap_allow_mixed_observed = true;
    }

    // Step 5: inject RID from `a=ssrc-group:SIM` when RID lines are absent.
    if inject_rid_from_ssrc_group_sim(&mut final_text) {
        report.ssrc_group_sim_rid_generated = true;
    }

    (final_text, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_normalizes_lone_lf_to_crlf() {
        let raw = "v=0\no=- 0 0 IN IP4 127.0.0.1\n";
        let (out, report) = preprocess_remote_sdp(raw);
        assert!(out.starts_with("v=0\r\n"));
        assert!(out.ends_with("\r\n"));
        assert!(report.normalized_line_endings);
        // already terminated with CRLF after normalization.
        assert!(!report.appended_missing_terminator);
    }

    #[test]
    fn preprocess_keeps_already_normalized_sdp() {
        let raw = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n";
        let (out, report) = preprocess_remote_sdp(raw);
        assert_eq!(out, raw);
        assert!(!report.is_modified());
    }

    #[test]
    fn preprocess_strips_trailing_whitespace_per_line() {
        let raw = "v=0   \r\no=- 0 0 IN IP4 127.0.0.1\r\n";
        let (out, report) = preprocess_remote_sdp(raw);
        assert!(out.starts_with("v=0\r\n"));
        assert!(report.trimmed_trailing_whitespace);
    }

    #[test]
    fn preprocess_appends_missing_terminator() {
        let raw = "v=0\r\no=- 0 0 IN IP4 127.0.0.1";
        let (out, report) = preprocess_remote_sdp(raw);
        assert!(out.ends_with("\r\n"));
        assert!(report.appended_missing_terminator);
    }

    #[test]
    fn preprocess_does_not_flag_canonical_input() {
        // Already well-formed CRLF input must not set
        // `normalized_line_endings` (regression: an earlier impl set
        // the flag on every `\n` and tried to revert it via a buggy
        // self-comparison).
        let raw = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
        let (out, report) = preprocess_remote_sdp(raw);
        assert_eq!(out, raw);
        assert!(!report.normalized_line_endings);
        assert!(!report.trimmed_trailing_whitespace);
        assert!(!report.appended_missing_terminator);
    }

    #[test]
    fn preprocess_handles_lone_cr_terminators() {
        // Old Mac-style `\r`-only terminators: rare but seen in some
        // pre-WebRTC SDP fixtures.
        let raw = "v=0\ro=- 0 0 IN IP4 127.0.0.1\r";
        let (out, report) = preprocess_remote_sdp(raw);
        assert!(out.starts_with("v=0\r\n"));
        assert!(out.ends_with("\r\n"));
        assert!(report.normalized_line_endings);
    }

    #[test]
    fn inject_rid_from_ssrc_group_sim_generates_r0_r1_r2() {
        let mut sdp = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111 2222 3333\r\n",
            "a=ssrc:1111 cname:test\r\n",
            "a=ssrc:2222 cname:test\r\n",
            "a=ssrc:3333 cname:test\r\n",
        )
        .to_string();
        let injected = inject_rid_from_ssrc_group_sim(&mut sdp);
        assert!(injected);
        assert!(sdp.contains("a=rid:r0 send\r\n"));
        assert!(sdp.contains("a=rid:r1 send\r\n"));
        assert!(sdp.contains("a=rid:r2 send\r\n"));
        assert!(sdp.contains("a=simulcast:send r0;r1;r2\r\n"));
    }

    #[test]
    fn inject_rid_does_not_modify_when_rid_already_present() {
        let mut sdp = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111 2222 3333\r\n",
            "a=rid:q send\r\n",
            "a=rid:h send\r\n",
            "a=rid:f send\r\n",
            "a=simulcast:send q;h;f\r\n",
        )
        .to_string();
        let original = sdp.clone();
        let injected = inject_rid_from_ssrc_group_sim(&mut sdp);
        assert!(!injected);
        assert_eq!(sdp, original);
    }

    #[test]
    fn inject_rid_handles_two_ssrc_sim_group() {
        let mut sdp = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111 2222\r\n",
        )
        .to_string();
        let injected = inject_rid_from_ssrc_group_sim(&mut sdp);
        assert!(injected);
        assert!(sdp.contains("a=rid:r0 send\r\n"));
        assert!(sdp.contains("a=rid:r1 send\r\n"));
        assert!(!sdp.contains("a=rid:r2 send\r\n"));
        assert!(sdp.contains("a=simulcast:send r0;r1\r\n"));
    }

    #[test]
    fn inject_rid_ignores_single_ssrc_sim_group() {
        let mut sdp = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111\r\n",
        )
        .to_string();
        let injected = inject_rid_from_ssrc_group_sim(&mut sdp);
        assert!(!injected);
    }

    #[test]
    fn inject_rid_only_affects_media_section_without_rid() {
        // Two m= sections: first has SIM without RID, second has RID already.
        let mut sdp = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111 2222 3333\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 97\r\n",
            "a=ssrc-group:SIM 4444 5555\r\n",
            "a=rid:q send\r\n",
            "a=rid:h send\r\n",
            "a=simulcast:send q;h\r\n",
        )
        .to_string();
        let injected = inject_rid_from_ssrc_group_sim(&mut sdp);
        assert!(injected);
        // First section should get r0/r1/r2
        assert!(sdp.contains("a=rid:r0 send\r\n"));
        assert!(sdp.contains("a=rid:r1 send\r\n"));
        assert!(sdp.contains("a=rid:r2 send\r\n"));
        // Second section should keep its original RIDs
        assert!(sdp.contains("a=rid:q send\r\n"));
        assert!(sdp.contains("a=rid:h send\r\n"));
    }

    #[test]
    fn preprocess_detects_extmap_allow_mixed() {
        let raw = "v=0\r\na=extmap-allow-mixed\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
        let (_out, report) = preprocess_remote_sdp(raw);
        assert!(report.extmap_allow_mixed_observed);
    }

    #[test]
    fn preprocess_does_not_flag_extmap_allow_mixed_when_absent() {
        let raw = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
        let (_out, report) = preprocess_remote_sdp(raw);
        assert!(!report.extmap_allow_mixed_observed);
    }

    #[test]
    fn preprocess_injects_rid_from_ssrc_group_sim() {
        let raw = concat!(
            "v=0\r\n",
            "o=- 0 0 IN IP4 127.0.0.1\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=ssrc-group:SIM 1111 2222 3333\r\n",
        );
        let (out, report) = preprocess_remote_sdp(raw);
        assert!(report.ssrc_group_sim_rid_generated);
        assert!(report.is_modified());
        assert!(out.contains("a=rid:r0 send"));
        assert!(out.contains("a=simulcast:send r0;r1;r2"));
    }

    // --- RTP extension mapping tests ---

    #[test]
    fn rtp_extension_type_from_uri_roundtrip() {
        let known_uris = [
            "urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
            "urn:ietf:params:rtp-hdrext:sdes:mid",
            "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id",
            "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id",
            "urn:3gpp:video-orientation",
            "http://www.webrtc.org/experiments/rtp-hdrext/video-timing",
            "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay",
            "urn:ietf:params:rtp-hdrext:toffset",
            "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type",
            "http://www.webrtc.org/experiments/rtp-hdrext/color-space",
        ];
        for uri in known_uris {
            let ext_type = RtpExtensionType::from_uri(uri);
            assert_ne!(
                ext_type,
                RtpExtensionType::Unknown,
                "URI {uri} should be recognized"
            );
            // The canonical URI should map back to the same type
            let canonical = ext_type.uri();
            let roundtrip = RtpExtensionType::from_uri(canonical);
            assert_eq!(ext_type, roundtrip, "roundtrip failed for {uri}");
        }
    }

    #[test]
    fn rtp_extension_type_unknown_for_unrecognized_uri() {
        assert_eq!(
            RtpExtensionType::from_uri("http://example.com/custom-ext"),
            RtpExtensionType::Unknown
        );
    }

    #[test]
    fn extract_rtp_extension_mappings_from_zlm_simulcast_fixture() {
        let sdp = include_str!("../tests/fixtures/zlm_offer_simulcast.sdp");
        let mappings = extract_rtp_extension_mappings(sdp);
        // The fixture has extmaps in both audio and video m-lines
        assert!(!mappings.is_empty());
        // Check that audio-level is found
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == RtpExtensionType::AudioLevel),
            "should find audio-level extension"
        );
        // Check that transport-wide-cc is found
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == RtpExtensionType::TransportWideCc),
            "should find transport-wide-cc extension"
        );
        // Check that mid is found
        assert!(
            mappings.iter().any(|m| m.ext_type == RtpExtensionType::Mid),
            "should find mid extension"
        );
        // Check that rid is found
        assert!(
            mappings.iter().any(|m| m.ext_type == RtpExtensionType::Rid),
            "should find rid extension"
        );
        // Check that repaired-rid is found
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == RtpExtensionType::RepairedRid),
            "should find repaired-rid extension"
        );
        // Check that video-orientation is found
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == RtpExtensionType::VideoOrientation),
            "should find video-orientation extension"
        );
        // Check that playout-delay is found
        assert!(
            mappings
                .iter()
                .any(|m| m.ext_type == RtpExtensionType::PlayoutDelay),
            "should find playout-delay extension"
        );
    }

    #[test]
    fn extract_rtp_extension_mappings_handles_direction_qualifier() {
        let sdp = "v=0\r\na=extmap:1/sendonly urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n";
        let mappings = extract_rtp_extension_mappings(sdp);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].id, 1);
        assert_eq!(mappings[0].ext_type, RtpExtensionType::AudioLevel);
        assert_eq!(mappings[0].direction.as_deref(), Some("sendonly"));
    }

    #[test]
    fn extract_rtp_extension_mappings_skips_malformed_lines() {
        let sdp = concat!(
            "a=extmap:abc urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n",
            "a=extmap:2\r\n",
            "a=extmap:3 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\n",
        );
        let mappings = extract_rtp_extension_mappings(sdp);
        // Only the valid line (id=3) should be parsed
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].id, 3);
        assert_eq!(mappings[0].ext_type, RtpExtensionType::AbsSendTime);
    }

    #[test]
    fn ensure_playout_delay_extmap_injects_for_video_sections() {
        let sdp = concat!(
            "v=0\r\n",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            "a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=extmap:2 urn:ietf:params:rtp-hdrext:sdes:mid\r\n",
        );
        let out = ensure_playout_delay_extmap(sdp);
        assert!(out.contains("http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"));
    }

    #[test]
    fn ensure_playout_delay_extmap_does_not_duplicate_exhausted_ids() {
        let mut sdp = String::from("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n");
        for id in 1u8..=14 {
            sdp.push_str(&format!("a=extmap:{id} http://example.com/ext/{id}\r\n"));
        }

        let out = ensure_playout_delay_extmap(&sdp);

        assert!(!out.contains("http://www.webrtc.org/experiments/rtp-hdrext/playout-delay"));
        assert_eq!(out.matches("a=extmap:14 ").count(), 1);
    }

    #[test]
    fn ensure_playout_delay_extmap_does_not_skip_adjacent_video_sections() {
        let sdp = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            "a=extmap:1 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 97\r\n",
            "a=extmap:2 urn:ietf:params:rtp-hdrext:sdes:mid\r\n",
        );

        let out = ensure_playout_delay_extmap(sdp);

        assert_eq!(
            out.matches("http://www.webrtc.org/experiments/rtp-hdrext/playout-delay")
                .count(),
            2
        );
    }

    #[test]
    fn strip_red_ulpfec_payloads_removes_payload_and_mline_ids() {
        let sdp = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 116 117\r\n",
            "a=rtpmap:96 VP8/90000\r\n",
            "a=rtpmap:116 red/90000\r\n",
            "a=rtpmap:117 ulpfec/90000\r\n",
            "a=fmtp:116 apt=96\r\n",
            "a=rtcp-fb:117 nack\r\n",
        );
        let out = strip_red_ulpfec_payloads(sdp);
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n"));
        assert!(!out.contains("a=rtpmap:116 red/90000"));
        assert!(!out.contains("a=rtpmap:117 ulpfec/90000"));
        assert!(!out.contains("a=fmtp:116"));
        assert!(!out.contains("a=rtcp-fb:117"));
    }

    #[test]
    fn strip_red_ulpfec_payloads_keeps_payload_scope_per_media_section() {
        let sdp = concat!(
            "v=0\r\n",
            "m=audio 9 UDP/TLS/RTP/SAVPF 116\r\n",
            "a=rtpmap:116 opus/48000/2\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 116\r\n",
            "a=rtpmap:96 VP8/90000\r\n",
            "a=rtpmap:116 red/90000\r\n",
        );
        let out = strip_red_ulpfec_payloads(sdp);
        assert!(out.contains("m=audio 9 UDP/TLS/RTP/SAVPF 116\r\n"));
        assert!(out.contains("a=rtpmap:116 opus/48000/2\r\n"));
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n"));
        assert!(!out.contains("a=rtpmap:116 red/90000"));
    }

    #[test]
    fn strip_red_ulpfec_payloads_preserves_red_only_section() {
        let sdp = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 116\r\n",
            "a=rtpmap:116 red/90000\r\n",
        );
        let out = strip_red_ulpfec_payloads(sdp);
        assert_eq!(out, sdp);
    }

    #[test]
    fn strip_red_ulpfec_payloads_removes_rtx_apt_to_disabled_payload() {
        let sdp = concat!(
            "v=0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 116 118\r\n",
            "a=rtpmap:96 VP8/90000\r\n",
            "a=rtpmap:116 red/90000\r\n",
            "a=rtpmap:118 rtx/90000\r\n",
            "a=fmtp:118 apt=116\r\n",
            "a=rtcp-fb:118 nack\r\n",
        );
        let out = strip_red_ulpfec_payloads(sdp);
        assert!(out.contains("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n"));
        assert!(!out.contains("a=rtpmap:116 red/90000"));
        assert!(!out.contains("a=rtpmap:118 rtx/90000"));
        assert!(!out.contains("a=fmtp:118 apt=116"));
        assert!(!out.contains("a=rtcp-fb:118 nack"));
    }
}
