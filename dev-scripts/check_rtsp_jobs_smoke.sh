#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

TEST_TIMEOUT_SECS="${TEST_TIMEOUT_SECS:-240}"
CARGO_BIN="${CARGO_BIN:-cargo}"

PULL_TEST="pull_job_remote_rtsp_source_restreams_to_local_rtsp_and_rtmp_play"
PUSH_TEST="push_job_setup_record_then_sends_interleaved_rtp_and_rtcp"

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "[rtsp-jobs-smoke] missing required command: ${cmd}" >&2
    exit 1
  fi
}

require_cmd "${CARGO_BIN}"
require_cmd timeout

run_case() {
  local test_target="$1"
  local test_name="$2"
  local log_file="$3"

  echo "[rtsp-jobs-smoke] running ${test_target}::${test_name}"

  if ! timeout "${TEST_TIMEOUT_SECS}" \
    "${CARGO_BIN}" test -p cheetah-rtsp-module --test "${test_target}" "${test_name}" -- --nocapture \
    > "${log_file}" 2>&1; then
    echo "[rtsp-jobs-smoke] failed: ${test_target}::${test_name}" >&2
    echo "[rtsp-jobs-smoke] log: ${log_file}" >&2
    tail -n 120 "${log_file}" >&2 || true
    return 1
  fi

  echo "[rtsp-jobs-smoke] passed: ${test_target}::${test_name}"
}

run_case "rtsp_pull_job" "${PULL_TEST}" "/tmp/cheetah-rtsp-jobs-smoke-pull.log"
run_case "rtsp_push_job" "${PUSH_TEST}" "/tmp/cheetah-rtsp-jobs-smoke-push.log"

echo "[rtsp-jobs-smoke] all job smoke cases passed"
