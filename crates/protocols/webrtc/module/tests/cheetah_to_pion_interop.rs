//! Cheetah ↔ Pion ignored end-to-end test.
//!
//! Phase 06 (`plans-27-webrtc-zlm2/phase-06-external-interop-infra.md`).
//!
//! Drives the Pion helper binary against a running WHIP / WHEP
//! endpoint (typically the cheetah-server interop lab service) and
//! verifies the helper produced a non-empty `peer-stats.json` with
//! sane counters. Mirrors `cheetah_to_zlm_interop.rs` but with the
//! direction reversed: cheetah is the WebRTC server, Pion is the
//! client.
//!
//! Run locally:
//!
//! ```bash
//! docker compose -f dev-docs/plans-27-webrtc-zlm2/interop-docker-compose.yml \
//!   --profile cheetah --profile pion up -d
//! export WEBRTC_INTEROP_PION_BIN=$(docker compose ... ps -q pion-helper)/cheetah-pion-helper
//! export WEBRTC_INTEROP_ZLM_WHIP_URL='http://127.0.0.1:8088/whip'   # cheetah's WHIP
//! cargo test -p cheetah-webrtc-module --test cheetah_to_pion_interop \
//!   -- --ignored pion_publish_to_cheetah_whip
//! ```
//!
//! The harness skips when `WEBRTC_INTEROP_PION_BIN` is unset.

mod interop_harness;

use std::process::{Command, Stdio};
use std::time::Duration;

use interop_harness::{open_test, require_env, ENV_PION_BIN, ENV_ZLM_WHIP};
use serde_json::Value;

#[derive(Debug)]
struct PionRunResult {
    status_success: bool,
    stdout: String,
    stderr: String,
}

async fn run_pion(bin: &str, args: &[&str], timeout: Duration) -> Result<PionRunResult, String> {
    // We use blocking `std::process::Command` inside
    // `spawn_blocking` instead of pulling the `tokio/process`
    // feature into the module's dev-deps. The pion helper is a
    // one-shot binary so the convenience of `Output::wait_with_output`
    // is enough.
    let bin = bin.to_string();
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let join = tokio::task::spawn_blocking(move || {
        let child = Command::new(&bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("spawn pion helper: {err}"))?;
        let output = child
            .wait_with_output()
            .map_err(|err| format!("wait pion helper: {err}"))?;
        Ok::<PionRunResult, String>(PionRunResult {
            status_success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    });

    match tokio::time::timeout(timeout, join).await {
        Ok(Ok(Ok(res))) => Ok(res),
        Ok(Ok(Err(err))) => Err(err),
        Ok(Err(err)) => Err(format!("pion helper task panicked: {err}")),
        Err(_) => Err(format!("pion helper exceeded {:?}", timeout)),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Pion helper binary; set WEBRTC_INTEROP_PION_BIN and WEBRTC_INTEROP_ZLM_WHIP_URL to run"]
async fn pion_publish_to_cheetah_whip() {
    let Some(artifact) = open_test("pion_publish_to_cheetah_whip", Some(ENV_PION_BIN)) else {
        return;
    };
    let pion_bin = require_env(ENV_PION_BIN).unwrap();
    let url = match require_env(ENV_ZLM_WHIP) {
        Some(u) => u,
        None => {
            artifact
                .set_failure("WEBRTC_INTEROP_PION_BIN set but WEBRTC_INTEROP_ZLM_WHIP_URL is not");
            panic!("missing WEBRTC_INTEROP_ZLM_WHIP_URL");
        }
    };

    let timeout = interop_harness::timeout();
    let args = ["--mode", "whip", "--url", url.as_str()];
    let result = match run_pion(&pion_bin, &args, timeout).await {
        Ok(r) => r,
        Err(err) => {
            artifact.set_failure(format!("pion run failed: {err}"));
            panic!("{err}");
        }
    };

    artifact
        .write("peer.log", result.stdout.as_bytes())
        .expect("write peer.log");
    artifact
        .append("peer.log", "----- stderr -----")
        .expect("append separator");
    artifact
        .append("peer.log", &result.stderr)
        .expect("append stderr");

    if !result.status_success {
        artifact.set_failure(format!(
            "pion helper exited non-zero; see peer.log\nstderr: {}",
            result.stderr
        ));
        panic!("pion helper exit non-zero");
    }

    // Look for the peer-stats.json the helper wrote into the
    // artifact dir. The helper sets `WEBRTC_INTEROP_ARTIFACT_DIR`
    // when it runs under docker-compose; locally the operator has
    // to mount the artifact dir back into the container or rely on
    // the helper's `os.Getenv` falling through.
    let stats_path = artifact.dir().join("peer-stats.json");
    if !stats_path.exists() {
        artifact.set_failure(
            "peer-stats.json missing after pion run; the helper either failed silently or could not write to the artifact dir",
        );
        panic!("peer-stats.json missing");
    }
    let raw = match std::fs::read_to_string(&stats_path) {
        Ok(s) => s,
        Err(err) => {
            artifact.set_failure(format!("read peer-stats.json: {err}"));
            panic!("{err}");
        }
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => {
            artifact.set_failure(format!("peer-stats.json not JSON: {err}"));
            panic!("{err}");
        }
    };

    // Sanity check: keys we expect from main.go.
    for key in [
        "first_keyframe_ms",
        "nacks_sent",
        "nacks_received",
        "bytes_sent",
        "bytes_received",
    ] {
        assert!(
            parsed.get(key).is_some(),
            "peer-stats.json missing key {key}; raw={raw}"
        );
    }

    artifact
        .append(
            "module-events.log",
            &format!("pion run OK url={url} stats={raw}"),
        )
        .expect("append log");
}
