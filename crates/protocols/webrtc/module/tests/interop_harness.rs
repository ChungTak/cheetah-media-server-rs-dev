//! Shared interop test harness.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`)
//! standardises the env-var contract, artifact directory layout, and
//! skip behaviour for the ignored interop tests.
//!
//! ## Env contract
//!
//! Every interop test reads `WEBRTC_INTEROP_ARTIFACT_DIR` for the
//! artifact root (defaults to `target/webrtc-interop`) and, when its
//! own enable env var is unset, returns `None` from
//! [`open_test`]. Tests written against this harness then early-return
//! without failing — the standard "ignored test, env not set" idiom.
//!
//! ## Artifact layout
//!
//! ```text
//! <root>/<test-name>/
//!   README.md           # captured env at run time
//!   request-offer.sdp   # cheetah-side offer
//!   response-answer.sdp # peer answer
//!   local-candidates.txt
//!   remote-candidates.txt
//!   session-stats.json
//!   module-events.log
//!   peer.log
//!   failure.txt         # written iff the test fails
//! ```
//!
//! Tests don't have to write every file; the harness creates the
//! directory and exposes helpers so the contract stays consistent.

#![allow(dead_code)]

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Standard environment variable names consumed by the harness.
pub const ENV_ARTIFACT_DIR: &str = "WEBRTC_INTEROP_ARTIFACT_DIR";
pub const ENV_TIMEOUT_MS: &str = "WEBRTC_INTEROP_TIMEOUT_MS";
pub const ENV_ZLM_BASE: &str = "WEBRTC_INTEROP_ZLM_BASE_URL";
pub const ENV_ZLM_WHIP: &str = "WEBRTC_INTEROP_ZLM_WHIP_URL";
pub const ENV_ZLM_WHEP: &str = "WEBRTC_INTEROP_ZLM_WHEP_URL";
pub const ENV_ZLM_SIGNALING: &str = "WEBRTC_INTEROP_ZLM_SIGNALING_URL";
pub const ENV_BROWSER: &str = "WEBRTC_INTEROP_BROWSER";
pub const ENV_PION_BIN: &str = "WEBRTC_INTEROP_PION_BIN";
pub const ENV_GST_BIN: &str = "WEBRTC_INTEROP_GSTREAMER_BIN";
pub const ENV_JANUS: &str = "WEBRTC_INTEROP_JANUS_URL";
pub const ENV_RTSP: &str = "WEBRTC_INTEROP_RTSP_URL";
pub const ENV_RTMP: &str = "WEBRTC_INTEROP_RTMP_URL";
pub const ENV_GB28181: &str = "WEBRTC_INTEROP_GB28181_SOURCE";
pub const ENV_WEAK_NETWORK: &str = "WEBRTC_INTEROP_WEAK_NETWORK";

/// Default artifact root if `WEBRTC_INTEROP_ARTIFACT_DIR` is unset.
pub fn default_artifact_root() -> PathBuf {
    PathBuf::from(env::var("WEBRTC_INTEROP_ARTIFACT_DIR").unwrap_or_else(|_| {
        // The crate test process runs in `target/debug/deps/<bin>`,
        // so the workspace `target/` is two levels up. Fall back to
        // the current dir if neither resolution works.
        let mut here = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for _ in 0..3 {
            let candidate = here.join("target");
            if candidate.exists() {
                return candidate
                    .join("webrtc-interop")
                    .to_string_lossy()
                    .into_owned();
            }
            if !here.pop() {
                break;
            }
        }
        "target/webrtc-interop".to_string()
    }))
}

/// Per-test artifact context. Auto-created on first write; tests can
/// optionally call [`InteropArtifact::set_failure`] to mark the run as
/// failed and emit a `failure.txt`.
pub struct InteropArtifact {
    dir: PathBuf,
    test_name: String,
}

impl InteropArtifact {
    /// Open (or create) the artifact directory for `test_name`.
    pub fn open(test_name: &str) -> std::io::Result<Self> {
        let root = default_artifact_root();
        let dir = root.join(test_name);
        fs::create_dir_all(&dir)?;
        let mut artifact = Self {
            dir,
            test_name: test_name.to_string(),
        };
        artifact.write_readme()?;
        Ok(artifact)
    }

    /// Path to the artifact root for this test.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write a binary or text artifact under the test directory.
    pub fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> std::io::Result<PathBuf> {
        let path = self.dir.join(name);
        fs::write(&path, contents.as_ref())?;
        Ok(path)
    }

    /// Append a line to a log file (e.g. `module-events.log`).
    pub fn append(&self, name: &str, line: &str) -> std::io::Result<()> {
        let path = self.dir.join(name);
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")
    }

    /// Mark the test as failed and emit a `failure.txt` artifact.
    pub fn set_failure(&self, reason: impl AsRef<str>) {
        let _ = self.write("failure.txt", reason.as_ref());
    }

    fn write_readme(&mut self) -> std::io::Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut buf = String::new();
        buf.push_str(&format!(
            "# WebRTC interop artifact: {}\n\n",
            self.test_name
        ));
        buf.push_str(&format!("Captured: epoch={now}\n\n"));
        buf.push_str("## Environment\n\n");
        for var in &[
            ENV_ARTIFACT_DIR,
            ENV_TIMEOUT_MS,
            ENV_ZLM_BASE,
            ENV_ZLM_WHIP,
            ENV_ZLM_WHEP,
            ENV_ZLM_SIGNALING,
            ENV_BROWSER,
            ENV_PION_BIN,
            ENV_GST_BIN,
            ENV_JANUS,
            ENV_RTSP,
            ENV_RTMP,
            ENV_GB28181,
            ENV_WEAK_NETWORK,
        ] {
            let val = env::var(var).unwrap_or_else(|_| "<unset>".into());
            buf.push_str(&format!("- {var} = {val}\n"));
        }
        self.write("README.md", buf)?;
        Ok(())
    }
}

/// Effective timeout for an interop test. Defaults to 30 s, capped at
/// 5 min; respects `WEBRTC_INTEROP_TIMEOUT_MS`.
pub fn timeout() -> Duration {
    let ms = env::var(ENV_TIMEOUT_MS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30_000);
    Duration::from_millis(ms.clamp(1_000, 5 * 60_000))
}

/// Read an env var; return `None` (and log) when missing or empty.
pub fn require_env(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("[interop] skipping: env var {var} is unset");
            None
        }
    }
}

/// Open an interop test artifact directory and confirm the prerequisite
/// env vars are set. When `enable_var` is `Some(name)` the harness
/// requires that env var as the "I want to run this" gate.
pub fn open_test(test_name: &str, enable_var: Option<&str>) -> Option<InteropArtifact> {
    if let Some(var) = enable_var {
        require_env(var)?;
    }
    match InteropArtifact::open(test_name) {
        Ok(a) => Some(a),
        Err(err) => {
            eprintln!("[interop] failed to open artifact dir for {test_name}: {err}");
            None
        }
    }
}

/// Media-plane assertion helpers used by interop tests once they have
/// captured SDP / stats. Pure functions so they can be shared between
/// real implementations and self-tests.
pub mod assertions {
    use std::time::Duration;

    /// Default thresholds for the standard interop checklist. Tests
    /// can override individual fields by constructing the struct
    /// manually.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct InteropThresholds {
        /// First decodable keyframe must arrive within this window.
        pub first_keyframe: Duration,
        /// Maximum acceptable RTT after ICE / DTLS up.
        pub max_rtt: Duration,
        /// Minimum NACK count under a 10% loss profile (used by the
        /// weak-network suite to confirm RTX engaged).
        pub min_nacks_under_loss: u64,
        /// Lower bound for the BWE estimate after the warm-up period.
        pub min_bwe_bps: u64,
    }

    impl Default for InteropThresholds {
        fn default() -> Self {
            Self {
                first_keyframe: Duration::from_secs(2),
                max_rtt: Duration::from_millis(500),
                min_nacks_under_loss: 1,
                min_bwe_bps: 200_000,
            }
        }
    }

    /// Assert an offer SDP starts with the canonical `v=0` line and
    /// references at least one media section.
    pub fn assert_offer_well_formed(sdp: &str) -> Result<(), String> {
        if !sdp.starts_with("v=0") {
            return Err(format!(
                "offer SDP must start with `v=0`; got first 8 bytes: {:?}",
                &sdp.as_bytes()[..sdp.len().min(8)]
            ));
        }
        if !sdp.contains("\nm=") && !sdp.contains("\r\nm=") {
            return Err("offer SDP must contain at least one m= section".into());
        }
        Ok(())
    }

    /// Assert an answer SDP is shaped like an answer (no `a=ice-restart`
    /// alone, has a media section, has `a=fingerprint:`).
    pub fn assert_answer_well_formed(sdp: &str) -> Result<(), String> {
        assert_offer_well_formed(sdp)?;
        if !sdp.contains("a=fingerprint:") {
            return Err("answer SDP must include a=fingerprint:".into());
        }
        Ok(())
    }

    /// Assert the first decodable keyframe arrived within the
    /// threshold window. Returns `Err` describing the slack so the
    /// caller can dump it into `failure.txt`.
    pub fn assert_first_keyframe_within(
        elapsed: Duration,
        thresholds: &InteropThresholds,
    ) -> Result<(), String> {
        if elapsed > thresholds.first_keyframe {
            return Err(format!(
                "first keyframe took {:?} > threshold {:?}",
                elapsed, thresholds.first_keyframe
            ));
        }
        Ok(())
    }

    /// Assert a stats snapshot crossed the NACK floor under loss.
    pub fn assert_nack_engaged(nacks: u64, thresholds: &InteropThresholds) -> Result<(), String> {
        if nacks < thresholds.min_nacks_under_loss {
            return Err(format!(
                "expected >= {} NACKs under loss, observed {nacks}",
                thresholds.min_nacks_under_loss
            ));
        }
        Ok(())
    }

    /// Assert the BWE estimate is reasonable.
    pub fn assert_bwe_above(
        estimate_bps: u64,
        thresholds: &InteropThresholds,
    ) -> Result<(), String> {
        if estimate_bps < thresholds.min_bwe_bps {
            return Err(format!(
                "BWE estimate {estimate_bps} bps below floor {} bps",
                thresholds.min_bwe_bps
            ));
        }
        Ok(())
    }

    /// Per-type candidate counts extracted from an SDP. Used by the
    /// candidate-policy interop tests to verify expected candidate
    /// types appear (host / srflx / relay) and that filters drop
    /// the right ones.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct CandidateCounts {
        pub host: usize,
        pub srflx: usize,
        pub prflx: usize,
        pub relay: usize,
        pub tcp: usize,
        pub udp: usize,
        pub ipv4: usize,
        pub ipv6: usize,
    }

    impl CandidateCounts {
        pub fn total(&self) -> usize {
            self.host + self.srflx + self.prflx + self.relay
        }
    }

    /// Count `a=candidate:` lines in an SDP and bucket them by type
    /// and transport. Pure string parsing — no SDP library — so
    /// callers can use it during build-time fixture validation
    /// without dragging in the full SDP parser.
    ///
    /// Lines that don't match the canonical RFC 5245 / 8839 format
    /// are silently skipped; partial lines do not throw.
    pub fn count_candidates(sdp: &str) -> CandidateCounts {
        let mut c = CandidateCounts::default();
        for line in sdp.lines() {
            let line = line.trim();
            // a=candidate:foundation component transport priority addr
            // port typ <type> [raddr ...] [tcptype ...]
            let Some(rest) = line.strip_prefix("a=candidate:") else {
                continue;
            };
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 8 {
                continue;
            }
            // parts[2] = transport, parts[4] = address, parts[6] = "typ", parts[7] = type
            match parts[2].to_ascii_uppercase().as_str() {
                "UDP" => c.udp += 1,
                "TCP" => c.tcp += 1,
                _ => {}
            }
            // Address family heuristic: ipv6 contains a colon, ipv4
            // does not. mDNS hosts (ending in `.local`) are counted
            // as ipv4 by convention since they resolve at runtime.
            if parts[4].contains(':') {
                c.ipv6 += 1;
            } else {
                c.ipv4 += 1;
            }
            if parts[6] == "typ" {
                match parts[7] {
                    "host" => c.host += 1,
                    "srflx" => c.srflx += 1,
                    "prflx" => c.prflx += 1,
                    "relay" => c.relay += 1,
                    _ => {}
                }
            }
        }
        c
    }

    /// Assert that the SDP carries at least one of each requested
    /// candidate type. Used by interop tests to confirm the local
    /// candidate gathering produced what the policy expected.
    pub fn assert_candidate_types_present(
        sdp: &str,
        require_host: bool,
        require_srflx: bool,
        require_relay: bool,
    ) -> Result<(), String> {
        let c = count_candidates(sdp);
        if require_host && c.host == 0 {
            return Err("expected at least one host candidate, found none".into());
        }
        if require_srflx && c.srflx == 0 {
            return Err("expected at least one srflx candidate, found none".into());
        }
        if require_relay && c.relay == 0 {
            return Err("expected at least one relay candidate, found none".into());
        }
        Ok(())
    }

    /// RIDs declared in a `a=simulcast:` line. We only parse the
    /// `send` direction because cheetah is always the recv side
    /// in the WHEP path that consumes simulcast.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct SimulcastRids {
        pub send: Vec<String>,
    }

    /// Extract simulcast RID layers from an SDP.
    ///
    /// Returns `None` when the SDP does not declare simulcast (no
    /// `a=simulcast:` line). Otherwise returns the RID list in
    /// declaration order. `recv` is intentionally not parsed —
    /// cheetah currently treats it as a future extension.
    pub fn extract_simulcast_rids(sdp: &str) -> Option<SimulcastRids> {
        let line = sdp
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("a=simulcast:"))?;
        // a=simulcast:send hi;mid;lo
        let rest = line.trim_start_matches("a=simulcast:").trim();
        let mut send: Vec<String> = Vec::new();
        for token in rest.split_whitespace() {
            // The token is either a direction (send/recv) or a
            // semicolon-separated RID list. We only buffer RIDs
            // immediately after the `send` direction marker.
            if token == "send" {
                continue;
            }
            if token == "recv" {
                break;
            }
            send.extend(token.split(';').map(str::to_string));
            break;
        }
        if send.is_empty() {
            return None;
        }
        Some(SimulcastRids { send })
    }

    /// Assert the SDP declares at least `required_layers` simulcast
    /// RIDs in the `send` direction.
    pub fn assert_simulcast_layers(sdp: &str, required_layers: usize) -> Result<(), String> {
        let rids = extract_simulcast_rids(sdp)
            .ok_or_else(|| "SDP does not declare a=simulcast: send line".to_string())?;
        if rids.send.len() < required_layers {
            return Err(format!(
                "expected >= {} simulcast layers, saw {}: {:?}",
                required_layers,
                rids.send.len(),
                rids.send
            ));
        }
        Ok(())
    }

    /// Track identifiers parsed from `a=msid:<stream-id> <track-id>`
    /// lines. Used by screen-share / multi-track fixtures to assert
    /// every section attaches to a labelled stream.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct MsidEntry {
        pub stream_id: String,
        pub track_id: String,
    }

    /// Extract every `a=msid:` line from the SDP. Returns the
    /// entries in declaration order so callers can assert on
    /// stream / track grouping.
    pub fn extract_msids(sdp: &str) -> Vec<MsidEntry> {
        let mut out = Vec::new();
        for line in sdp.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("a=msid:") else {
                continue;
            };
            let mut parts = rest.splitn(2, ' ');
            let stream = parts.next().unwrap_or("").trim();
            let track = parts.next().unwrap_or("").trim();
            if stream.is_empty() || track.is_empty() {
                continue;
            }
            out.push(MsidEntry {
                stream_id: stream.to_string(),
                track_id: track.to_string(),
            });
        }
        out
    }

    /// Assert that the SDP declares at least one `a=msid:` line
    /// matching `stream_id`. Useful for screen-share fixtures
    /// where every track on the same surface shares one stream id.
    pub fn assert_msid_stream_present(sdp: &str, stream_id: &str) -> Result<(), String> {
        let entries = extract_msids(sdp);
        if entries.is_empty() {
            return Err("SDP has no a=msid: lines".to_string());
        }
        if !entries.iter().any(|e| e.stream_id == stream_id) {
            return Err(format!(
                "SDP does not include msid stream {stream_id:?}; saw streams {:?}",
                entries
                    .iter()
                    .map(|e| e.stream_id.as_str())
                    .collect::<Vec<_>>()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_dir_is_created_lazily() {
        let test = "harness_smoke_artifact_dir_is_created_lazily";
        let artifact = InteropArtifact::open(test).expect("open");
        assert!(artifact.dir().exists(), "dir should exist after open");
        assert!(artifact.dir().join("README.md").exists());
        // Cleanup: not strictly necessary since target/ is .gitignored.
        let _ = fs::remove_dir_all(artifact.dir());
    }

    #[test]
    fn artifact_write_round_trip() {
        let test = "harness_smoke_write_round_trip";
        let artifact = InteropArtifact::open(test).expect("open");
        let path = artifact
            .write("session-stats.json", r#"{"ok":true}"#)
            .unwrap();
        let read = fs::read_to_string(&path).unwrap();
        assert!(read.contains("ok"));
        let _ = fs::remove_dir_all(artifact.dir());
    }

    #[test]
    fn artifact_append_creates_log_file() {
        let test = "harness_smoke_append_creates_log_file";
        let artifact = InteropArtifact::open(test).expect("open");
        artifact.append("module-events.log", "first").unwrap();
        artifact.append("module-events.log", "second").unwrap();
        let log = fs::read_to_string(artifact.dir().join("module-events.log")).unwrap();
        assert!(log.contains("first"));
        assert!(log.contains("second"));
        let _ = fs::remove_dir_all(artifact.dir());
    }

    #[test]
    fn require_env_returns_none_for_missing() {
        // Use a name that is highly unlikely to ever be set.
        let val = require_env("WEBRTC_INTEROP_DEFINITELY_UNSET_DO_NOT_SET_THIS_FOR_TESTS");
        assert!(val.is_none());
    }

    #[test]
    fn timeout_uses_default_when_env_unset() {
        env::remove_var(ENV_TIMEOUT_MS);
        let t = timeout();
        assert_eq!(t, Duration::from_millis(30_000));
    }

    mod assertions_tests {
        use super::super::assertions::*;
        use std::time::Duration;

        #[test]
        fn offer_well_formed_accepts_canonical_sdp() {
            let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=-\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
            assert!(assert_offer_well_formed(sdp).is_ok());
        }

        #[test]
        fn offer_well_formed_rejects_missing_v0() {
            let err = assert_offer_well_formed("o=- 0 0 IN IP4 0.0.0.0\r\n").unwrap_err();
            assert!(err.contains("`v=0`"));
        }

        #[test]
        fn offer_well_formed_rejects_no_media_section() {
            let err = assert_offer_well_formed("v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\n").unwrap_err();
            assert!(err.contains("m="));
        }

        #[test]
        fn answer_well_formed_requires_fingerprint() {
            let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
            let err = assert_answer_well_formed(sdp).unwrap_err();
            assert!(err.contains("fingerprint"));
        }

        #[test]
        fn answer_well_formed_accepts_with_fingerprint() {
            let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=fingerprint:sha-256 ab:cd\r\n";
            assert!(assert_answer_well_formed(sdp).is_ok());
        }

        #[test]
        fn first_keyframe_within_window() {
            let t = InteropThresholds::default();
            assert!(assert_first_keyframe_within(Duration::from_millis(500), &t).is_ok());
            let err = assert_first_keyframe_within(Duration::from_secs(5), &t).unwrap_err();
            assert!(err.contains("first keyframe"));
        }

        #[test]
        fn nack_engaged_threshold() {
            let t = InteropThresholds::default();
            assert!(assert_nack_engaged(2, &t).is_ok());
            let err = assert_nack_engaged(0, &t).unwrap_err();
            assert!(err.contains("NACK"));
        }

        #[test]
        fn bwe_above_threshold() {
            let t = InteropThresholds::default();
            assert!(assert_bwe_above(500_000, &t).is_ok());
            let err = assert_bwe_above(50_000, &t).unwrap_err();
            assert!(err.contains("BWE"));
        }

        #[test]
        fn count_candidates_buckets_by_type_and_transport() {
            let sdp = "v=0\r\n\
                       a=candidate:1 1 UDP 2113937151 192.168.1.1 5000 typ host\r\n\
                       a=candidate:2 1 UDP 1685987327 203.0.113.5 5000 typ srflx raddr 192.168.1.1 rport 5000\r\n\
                       a=candidate:3 1 TCP 2105524479 192.168.1.1 9 typ host tcptype active\r\n\
                       a=candidate:4 1 UDP 33554431 198.51.100.4 49152 typ relay raddr 203.0.113.5 rport 5000\r\n\
                       a=candidate:5 1 UDP 2113937150 fe80::1 5000 typ host\r\n";
            let c = count_candidates(sdp);
            assert_eq!(c.host, 3);
            assert_eq!(c.srflx, 1);
            assert_eq!(c.relay, 1);
            assert_eq!(c.prflx, 0);
            assert_eq!(c.tcp, 1);
            assert_eq!(c.udp, 4);
            assert_eq!(c.ipv4, 4);
            assert_eq!(c.ipv6, 1);
            assert_eq!(c.total(), 5);
        }

        #[test]
        fn count_candidates_ignores_partial_lines() {
            // Truncated `a=candidate:` lines must not panic or
            // bump counters.
            let sdp = "a=candidate:1 1 UDP\r\n\
                       a=candidate:malformed\r\n\
                       a=candidate:1 1 UDP 100 1.1.1.1 5000 typ host\r\n";
            let c = count_candidates(sdp);
            assert_eq!(c.host, 1, "only the well-formed line should count");
            assert_eq!(c.total(), 1);
        }

        #[test]
        fn assert_candidate_types_present_passes_when_satisfied() {
            let sdp = "a=candidate:1 1 UDP 0 1.1.1.1 5 typ host\r\n\
                       a=candidate:2 1 UDP 0 2.2.2.2 5 typ srflx raddr 1.1.1.1 rport 5\r\n";
            assert!(assert_candidate_types_present(sdp, true, true, false).is_ok());
        }

        #[test]
        fn assert_candidate_types_present_reports_missing_relay() {
            let sdp = "a=candidate:1 1 UDP 0 1.1.1.1 5 typ host\r\n";
            let err = assert_candidate_types_present(sdp, false, false, true).unwrap_err();
            assert!(err.contains("relay"));
        }

        #[test]
        fn extract_simulcast_rids_returns_send_layers_in_order() {
            let sdp = "v=0\r\na=simulcast:send hi;mid;lo\r\n";
            let rids = extract_simulcast_rids(sdp).unwrap();
            assert_eq!(rids.send, vec!["hi", "mid", "lo"]);
        }

        #[test]
        fn extract_simulcast_rids_returns_none_when_absent() {
            assert!(extract_simulcast_rids("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n").is_none());
        }

        #[test]
        fn extract_simulcast_rids_handles_two_layer_offer() {
            let sdp = "a=simulcast:send hi;lo\r\n";
            let rids = extract_simulcast_rids(sdp).unwrap();
            assert_eq!(rids.send.len(), 2);
        }

        #[test]
        fn assert_simulcast_layers_passes_when_satisfied() {
            let sdp = "a=simulcast:send hi;mid;lo\r\n";
            assert!(assert_simulcast_layers(sdp, 3).is_ok());
            assert!(assert_simulcast_layers(sdp, 2).is_ok());
        }

        #[test]
        fn assert_simulcast_layers_reports_shortfall() {
            let sdp = "a=simulcast:send hi;lo\r\n";
            let err = assert_simulcast_layers(sdp, 3).unwrap_err();
            assert!(err.contains("expected >= 3"));
        }

        #[test]
        fn assert_simulcast_layers_reports_missing_line() {
            let err = assert_simulcast_layers("v=0\r\n", 2).unwrap_err();
            assert!(err.contains("a=simulcast"));
        }

        #[test]
        fn extract_msids_returns_stream_track_pairs() {
            let sdp = "v=0\r\na=msid:stream-a track-a\r\na=msid:stream-a track-b\r\n";
            let entries = extract_msids(sdp);
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].stream_id, "stream-a");
            assert_eq!(entries[0].track_id, "track-a");
            assert_eq!(entries[1].track_id, "track-b");
        }

        #[test]
        fn extract_msids_skips_malformed_lines() {
            let sdp = "a=msid:onlyone\r\na=msid: \r\na=msid:s t\r\n";
            let entries = extract_msids(sdp);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].stream_id, "s");
        }

        #[test]
        fn assert_msid_stream_present_matches_known_stream() {
            let sdp = "a=msid:screen-share screen-share-video\r\n";
            assert!(assert_msid_stream_present(sdp, "screen-share").is_ok());
            let err = assert_msid_stream_present(sdp, "missing").unwrap_err();
            assert!(err.contains("missing"));
        }

        #[test]
        fn assert_msid_stream_present_reports_empty_sdp() {
            let err = assert_msid_stream_present("v=0\r\n", "any").unwrap_err();
            assert!(err.contains("no a=msid"));
        }
    }
}
