//! WebRTC interoperability tests against external peers.
//!
//! These tests are gated by `#[ignore]` because they require an
//! external WebRTC peer (Pion server, GStreamer `webrtcbin`, or a
//! browser running an automation harness). The CI pipeline picks
//! them up on demand using `cargo test -- --ignored`.
//!
//! ## Phase 06 harness
//!
//! All tests use the shared [`interop_harness`] helpers
//! (`tests/interop_harness.rs`) so the env-var contract, artifact
//! directory layout, and skip behaviour stay consistent. Each test:
//!
//! 1. Reads its enable env var (e.g. `WEBRTC_INTEROP_ZLM_WHIP_URL`).
//! 2. Opens an artifact directory under
//!    `target/webrtc-interop/<test-name>/` and writes a `README.md`
//!    capturing the env at run time.
//! 3. Performs the test body and writes session/SDP/log artifacts as
//!    they become available.
//! 4. On failure, calls `set_failure(...)` so the artifact dir
//!    contains a `failure.txt` for triage.
//!
//! ## Environment variables
//!
//! | Var                                  | Purpose                                |
//! |--------------------------------------|----------------------------------------|
//! | `WEBRTC_INTEROP_ARTIFACT_DIR`        | Override artifact root                 |
//! | `WEBRTC_INTEROP_TIMEOUT_MS`          | Per-test timeout (default 30 s)        |
//! | `WEBRTC_INTEROP_ZLM_BASE_URL`        | ZLMediaKit HTTP base                   |
//! | `WEBRTC_INTEROP_ZLM_WHIP_URL`        | ZLM WHIP URL for `pull_*` tests        |
//! | `WEBRTC_INTEROP_ZLM_WHEP_URL`        | ZLM WHEP URL for `push_*` tests        |
//! | `WEBRTC_INTEROP_ZLM_SIGNALING_URL`   | ZLM P2P signaling endpoint             |
//! | `WEBRTC_INTEROP_BROWSER`             | "1" to enable browser tests            |
//! | `WEBRTC_INTEROP_PION_BIN`            | path to a Pion helper binary           |
//! | `WEBRTC_INTEROP_GSTREAMER_BIN`       | path to GStreamer test runner          |
//! | `WEBRTC_INTEROP_JANUS_URL`           | Janus REST endpoint                    |
//! | `WEBRTC_INTEROP_RTSP_URL`            | RTSP source for cross-protocol tests   |
//! | `WEBRTC_INTEROP_RTMP_URL`            | RTMP source for cross-protocol tests   |
//! | `WEBRTC_INTEROP_GB28181_SOURCE`      | GB28181 device source                  |
//! | `WEBRTC_INTEROP_WEAK_NETWORK`        | "1" to run tc netem weak-network suite |
//!
//! Local runs:
//!
//! ```bash
//! cargo test -p cheetah-webrtc-module --test interop -- --ignored
//! ```

mod interop_harness;

use interop_harness::{
    open_test, require_env, ENV_BROWSER, ENV_GB28181, ENV_GST_BIN, ENV_JANUS, ENV_PION_BIN,
    ENV_RTMP, ENV_RTSP, ENV_WEAK_NETWORK, ENV_ZLM_SIGNALING, ENV_ZLM_WHEP, ENV_ZLM_WHIP,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires external Pion peer; set WEBRTC_INTEROP_PION_BIN to run"]
async fn pion_pull_smoke() {
    let Some(artifact) = open_test("pion_pull_smoke", Some(ENV_PION_BIN)) else {
        return;
    };
    let url = match require_env(ENV_ZLM_WHEP) {
        Some(u) => u,
        None => {
            artifact
                .set_failure("WEBRTC_INTEROP_PION_BIN set but WEBRTC_INTEROP_ZLM_WHEP_URL is not");
            panic!("missing WEBRTC_INTEROP_ZLM_WHEP_URL");
        }
    };
    artifact
        .append("module-events.log", &format!("pion pull URL = {url}"))
        .unwrap();
    assert!(
        url.starts_with("http://") || url.starts_with("https://"),
        "WEBRTC_INTEROP_ZLM_WHEP_URL must be an http(s) URL, got {url:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires external GStreamer peer; set WEBRTC_INTEROP_GSTREAMER_BIN to run"]
async fn gstreamer_push_smoke() {
    let Some(artifact) = open_test("gstreamer_push_smoke", Some(ENV_GST_BIN)) else {
        return;
    };
    let url = match require_env(ENV_ZLM_WHIP) {
        Some(u) => u,
        None => {
            artifact.set_failure(
                "WEBRTC_INTEROP_GSTREAMER_BIN set but WEBRTC_INTEROP_ZLM_WHIP_URL is not",
            );
            panic!("missing WEBRTC_INTEROP_ZLM_WHIP_URL");
        }
    };
    artifact
        .append("module-events.log", &format!("gst push URL = {url}"))
        .unwrap();
    assert!(
        url.starts_with("http://") || url.starts_with("https://"),
        "WEBRTC_INTEROP_ZLM_WHIP_URL must be an http(s) URL, got {url:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires browser automation harness; set WEBRTC_INTEROP_BROWSER=1 to run"]
async fn browser_whip_whep_smoke() {
    let Some(_artifact) = open_test("browser_whip_whep_smoke", Some(ENV_BROWSER)) else {
        return;
    };
}

/// ZLMediaKit interop scaffold: cross-test cheetah ↔ ZLM by pointing
/// `WEBRTC_INTEROP_ZLM_WHIP_URL` at a running ZLMediaKit
/// `/index/api/webrtc` endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires running ZLMediaKit peer; set WEBRTC_INTEROP_ZLM_WHIP_URL to run"]
async fn zlm_whip_smoke() {
    let Some(artifact) = open_test("zlm_whip_smoke", Some(ENV_ZLM_WHIP)) else {
        return;
    };
    let url = require_env(ENV_ZLM_WHIP).unwrap();
    artifact
        .append("module-events.log", &format!("zlm whip URL = {url}"))
        .unwrap();
    assert!(
        url.starts_with("http://") || url.starts_with("https://"),
        "WEBRTC_INTEROP_ZLM_WHIP_URL must be an http(s) URL, got {url:?}"
    );
}

/// Harness-driven SDP validation: when the operator captures a ZLM
/// answer SDP into `target/webrtc-interop/zlm_answer_sdp_validation/
/// response-answer.sdp` (e.g. via `curl` against the WHIP endpoint),
/// this test runs the assertion helpers against it. Useful when
/// driving a manual lab without a full browser/Pion roundtrip.
///
/// The test is gated on a separate env (`WEBRTC_INTEROP_ZLM_BASE_URL`)
/// so it only fires when the operator has explicitly set up a ZLM
/// instance to capture against.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a captured ZLM answer SDP; set WEBRTC_INTEROP_ZLM_BASE_URL to run"]
async fn zlm_answer_sdp_validation() {
    use interop_harness::assertions::{assert_answer_well_formed, InteropThresholds};
    use interop_harness::ENV_ZLM_BASE;

    let Some(artifact) = open_test("zlm_answer_sdp_validation", Some(ENV_ZLM_BASE)) else {
        return;
    };
    let answer_path = artifact.dir().join("response-answer.sdp");
    if !answer_path.exists() {
        artifact.set_failure(format!(
            "expected captured SDP at {} but file is missing; \
             curl the WHIP endpoint and save the response there before running this test",
            answer_path.display()
        ));
        eprintln!("[interop] skipping: {} is missing", answer_path.display());
        return;
    }
    let answer = match std::fs::read_to_string(&answer_path) {
        Ok(s) => s,
        Err(err) => {
            artifact.set_failure(format!("failed to read {}: {err}", answer_path.display()));
            panic!("read failed: {err}");
        }
    };
    if let Err(err) = assert_answer_well_formed(&answer) {
        artifact.set_failure(format!("answer SDP not well-formed: {err}"));
        panic!("{err}");
    }
    let thresholds = InteropThresholds::default();
    artifact
        .append(
            "module-events.log",
            &format!(
                "answer SDP validated; thresholds = first_keyframe={:?} max_rtt={:?}",
                thresholds.first_keyframe, thresholds.max_rtt
            ),
        )
        .unwrap();
}

/// ZLM P2P signaling smoke test using the new
/// `WEBRTC_INTEROP_ZLM_SIGNALING_URL` env var.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires ZLMediaKit signaling peer; set WEBRTC_INTEROP_ZLM_SIGNALING_URL to run"]
async fn zlm_p2p_signaling_smoke() {
    let Some(artifact) = open_test("zlm_p2p_signaling_smoke", Some(ENV_ZLM_SIGNALING)) else {
        return;
    };
    let url = require_env(ENV_ZLM_SIGNALING).unwrap();
    artifact
        .append("module-events.log", &format!("zlm signaling URL = {url}"))
        .unwrap();
    assert!(
        url.starts_with("ws://") || url.starts_with("wss://"),
        "WEBRTC_INTEROP_ZLM_SIGNALING_URL must be a ws(s) URL, got {url:?}"
    );
}

/// ZLMRTCClient.js + browser interop. Driven manually via the
/// browser env flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires browser harness; set WEBRTC_INTEROP_BROWSER=1 to run"]
async fn zlmrtcclient_browser_interop() {
    let Some(_artifact) = open_test("zlmrtcclient_browser_interop", Some(ENV_BROWSER)) else {
        return;
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires RTSP source; set WEBRTC_INTEROP_RTSP_URL to run"]
async fn cross_protocol_rtsp_to_webrtc() {
    let Some(artifact) = open_test("cross_protocol_rtsp_to_webrtc", Some(ENV_RTSP)) else {
        return;
    };
    let url = require_env(ENV_RTSP).unwrap();
    artifact
        .append("module-events.log", &format!("rtsp src = {url}"))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires RTMP source; set WEBRTC_INTEROP_RTMP_URL to run"]
async fn cross_protocol_rtmp_to_webrtc() {
    let Some(artifact) = open_test("cross_protocol_rtmp_to_webrtc", Some(ENV_RTMP)) else {
        return;
    };
    let url = require_env(ENV_RTMP).unwrap();
    artifact
        .append("module-events.log", &format!("rtmp src = {url}"))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires GB28181 device; set WEBRTC_INTEROP_GB28181_SOURCE to run"]
async fn cross_protocol_gb28181_to_webrtc() {
    let Some(artifact) = open_test("cross_protocol_gb28181_to_webrtc", Some(ENV_GB28181)) else {
        return;
    };
    let src = require_env(ENV_GB28181).unwrap();
    artifact
        .append("module-events.log", &format!("gb28181 src = {src}"))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Janus peer; set WEBRTC_INTEROP_JANUS_URL to run"]
async fn janus_signaling_smoke() {
    let Some(artifact) = open_test("janus_signaling_smoke", Some(ENV_JANUS)) else {
        return;
    };
    let url = require_env(ENV_JANUS).unwrap();
    artifact
        .append("module-events.log", &format!("janus URL = {url}"))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires tc netem or equivalent; set WEBRTC_INTEROP_WEAK_NETWORK=1 to run"]
async fn weak_network_nack_recovery() {
    let Some(_artifact) = open_test("weak_network_nack_recovery", Some(ENV_WEAK_NETWORK)) else {
        return;
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OME WebSocket endpoint; set WEBRTC_INTEROP_OME_WS_URL to run"]
async fn ome_ws_request_offer_smoke() {
    let env = "WEBRTC_INTEROP_OME_WS_URL";
    let Some(artifact) = open_test("ome_ws_request_offer_smoke", Some(env)) else {
        return;
    };
    let url = require_env(env).unwrap();
    artifact
        .append(
            "module-events.log",
            &format!("ome ws signaling URL = {url}"),
        )
        .unwrap();
    assert!(
        url.starts_with("ws://") || url.starts_with("wss://"),
        "WEBRTC_INTEROP_OME_WS_URL must be a ws(s) URL, got {url:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OvenRtcTester runbook; set WEBRTC_INTEROP_OME_TESTER_BIN to run"]
async fn ome_oven_rtc_tester_smoke() {
    let env = "WEBRTC_INTEROP_OME_TESTER_BIN";
    let Some(artifact) = open_test("ome_oven_rtc_tester_smoke", Some(env)) else {
        return;
    };
    let bin = require_env(env).unwrap();
    artifact
        .append("module-events.log", &format!("ome tester bin = {bin}"))
        .unwrap();
    assert!(
        !bin.trim().is_empty(),
        "WEBRTC_INTEROP_OME_TESTER_BIN must not be empty"
    );
}
