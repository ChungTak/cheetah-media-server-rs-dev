#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${ROOT_DIR}/dev-scripts/cross_protocol_matrix_regression.sh"

fail() {
  echo "[test] $*" >&2
  exit 1
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if [[ "${haystack}" != *"${needle}"* ]]; then
    fail "expected output to contain '${needle}', got: ${haystack}"
  fi
}

assert_fails_with() {
  local expected="$1"
  shift

  local out
  if out="$($@ 2>&1)"; then
    fail "expected command to fail, but succeeded: $*"
  fi

  assert_contains "${out}" "${expected}"
}

write_acceptance_matrix() {
  local path="$1"
  cat > "${path}" <<'MATRIX'
# check_key|required|threshold|unit|description
startup_latency|yes|800|ms|first frame should arrive quickly
first_keyframe_delay_ms|no|2000|ms|first keyframe delay check disabled in stub test without ffprobe
continuous_play|yes|1|seconds|playback should continue for at least one second
freeze_events|yes|0|count|freeze logs should stay zero
dts_out_of_order|yes|0|count|dts out of order should stay zero
invalid_timestamps|yes|0|count|invalid timestamps must stay zero
non_increasing_dts|yes|0|count|non increasing dts must stay zero
negative_cts|yes|0|count|negative cts must stay zero
ffprobe_first_video_keyframe|no|1|bool|ffprobe first packet keyframe check disabled in stub test
ffprobe_first_video_pts_near_zero|no|1|bool|ffprobe first packet pts near zero check disabled in stub test
ffprobe_video_dts_monotonic|no|1|bool|ffprobe dts monotonic check disabled in stub test
source_repair_events|no|0|count|source repair count tracking only
canonical_repair_events|no|0|count|canonical repair count tracking only
egress_repair_events|no|0|count|egress repair count tracking only
repair_warn_high_frequency|yes|0|bool|canonical and egress repair warns should not be high frequency
repair_context_complete|yes|1|bool|repair logs should include source and canonical context
MATRIX
}

write_matrix_script_success() {
  local path="$1"
  cat > "${path}" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

command="${1:-}"

  case "${command}" in
  list)
    cat <<'LIST'
rtsp-tcp-loopback
LIST
    ;;
  list-inputs)
    cat <<'INPUTS'
non-b-h264|b_frames=no|path=/tmp/non-b.flv|description=non b sample
b-frame-h264|b_frames=yes|path=/tmp/b.flv|description=b frame sample
INPUTS
    ;;
  show)
    scenario="${2:-}"
    if [[ "${scenario}" != "rtsp-tcp-loopback" ]]; then
      echo "[stub] unknown scenario" >&2
      exit 1
    fi
    cat <<'SHOW'
### rtsp-tcp-loopback
# Terminal A (push):
printf 'push-start\n'; sleep 5
# Terminal B (pull):
printf 'pull-first-frame\n'; sleep 2
SHOW
    ;;
  doctor)
    echo "[stub] doctor ok"
    ;;
  *)
    echo "[stub] unsupported command '${command}'" >&2
    exit 1
    ;;
esac
STUB
  chmod +x "${path}"
}

write_matrix_script_bad_show() {
  local path="$1"
  cat > "${path}" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

command="${1:-}"

case "${command}" in
  list)
    echo "rtsp-tcp-loopback"
    ;;
  show)
    cat <<'SHOW'
### rtsp-tcp-loopback
# Terminal A (push):
printf 'push-only\n'; sleep 1
SHOW
    ;;
  doctor)
    echo "[stub] doctor ok"
    ;;
  *)
    echo "[stub] unsupported command '${command}'" >&2
    exit 1
    ;;
esac
STUB
  chmod +x "${path}"
}

write_matrix_script_dts_failure() {
  local path="$1"
  cat > "${path}" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

command="${1:-}"

case "${command}" in
  list)
    echo "rtsp-tcp-loopback"
    ;;
  show)
    cat <<'SHOW'
### rtsp-tcp-loopback
# Terminal A (push):
printf 'push-start\n'; sleep 2
# Terminal B (pull):
printf 'DTS out of order\n'; sleep 2
SHOW
    ;;
  doctor)
    echo "[stub] doctor ok"
    ;;
  *)
    echo "[stub] unsupported command '${command}'" >&2
    exit 1
    ;;
esac
STUB
  chmod +x "${path}"
}

write_matrix_script_invalid_timestamp_failure() {
  local path="$1"
  cat > "${path}" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail

command="${1:-}"

case "${command}" in
  list)
    echo "rtsp-tcp-loopback"
    ;;
  show)
    cat <<'SHOW'
### rtsp-tcp-loopback
# Terminal A (push):
printf 'push-start\n'; sleep 2
# Terminal B (pull):
printf 'Invalid timestamps stream=live\n'; sleep 2
SHOW
    ;;
  doctor)
    echo "[stub] doctor ok"
    ;;
  *)
    echo "[stub] unsupported command '${command}'" >&2
    exit 1
    ;;
esac
STUB
  chmod +x "${path}"
}

run_test() {
  local temp_dir
  temp_dir="$(mktemp -d)"
  trap 'rm -rf "${temp_dir}"' RETURN

  local acceptance_file="${temp_dir}/acceptance.tsv"
  write_acceptance_matrix "${acceptance_file}"

  local matrix_success="${temp_dir}/matrix-success.sh"
  write_matrix_script_success "${matrix_success}"

  local report_root_success="${temp_dir}/reports-success"
  local out
  out="$(MATRIX_SCRIPT="${matrix_success}" MATRIX_ACCEPTANCE_FILE="${acceptance_file}" REPORT_ROOT="${report_root_success}" SCENARIO_DURATION_SECONDS=1 ENABLE_FFPROBE_CHECKS=0 "${SCRIPT}" run-all)"
  assert_contains "${out}" "run-all summary: passed=2, failed=0"

  local summary_count
  summary_count="$(find "${report_root_success}" -name summary.txt | wc -l | tr -d ' ')"
  [[ "${summary_count}" == "2" ]] || fail "expected two summary files, got ${summary_count}"

  local summary_file
  summary_file="$(find "${report_root_success}" -name summary.txt | head -n 1)"
  [[ -n "${summary_file}" ]] || fail "summary file path is empty"
  local summary_content
  summary_content="$(cat "${summary_file}")"
  assert_contains "${summary_content}" "result=PASS"
  assert_contains "${summary_content}" "first_second_avg_frame_interval_ms="
  assert_contains "${summary_content}" "average_playback_rate_x="
  assert_contains "${summary_content}" "invalid_timestamps=0"
  assert_contains "${summary_content}" "non_increasing_dts=0"
  assert_contains "${summary_content}" "negative_cts=0"
  assert_contains "${summary_content}" "first_keyframe_delay_ms="
  assert_contains "${summary_content}" "source_repair_events="
  assert_contains "${summary_content}" "canonical_repair_events="
  assert_contains "${summary_content}" "egress_repair_events="
  assert_contains "${summary_content}" "repair_warn_high_frequency="
  assert_contains "${summary_content}" "repair_context_complete="

  local matrix_bad_show="${temp_dir}/matrix-bad-show.sh"
  write_matrix_script_bad_show "${matrix_bad_show}"
  assert_fails_with "failed to parse pull command" \
    env MATRIX_SCRIPT="${matrix_bad_show}" MATRIX_ACCEPTANCE_FILE="${acceptance_file}" REPORT_ROOT="${temp_dir}/reports-bad-show" SCENARIO_DURATION_SECONDS=1 ENABLE_FFPROBE_CHECKS=0 "${SCRIPT}" run rtsp-tcp-loopback

  local matrix_dts_failure="${temp_dir}/matrix-dts-failure.sh"
  write_matrix_script_dts_failure "${matrix_dts_failure}"
  assert_fails_with "one or more scenarios failed" \
    env MATRIX_SCRIPT="${matrix_dts_failure}" MATRIX_ACCEPTANCE_FILE="${acceptance_file}" REPORT_ROOT="${temp_dir}/reports-dts-failure" SCENARIO_DURATION_SECONDS=1 ENABLE_FFPROBE_CHECKS=0 "${SCRIPT}" run-all

  local matrix_invalid_timestamp_failure="${temp_dir}/matrix-invalid-timestamp-failure.sh"
  write_matrix_script_invalid_timestamp_failure "${matrix_invalid_timestamp_failure}"
  assert_fails_with "one or more scenarios failed" \
    env MATRIX_SCRIPT="${matrix_invalid_timestamp_failure}" MATRIX_ACCEPTANCE_FILE="${acceptance_file}" REPORT_ROOT="${temp_dir}/reports-invalid-timestamp" SCENARIO_DURATION_SECONDS=1 ENABLE_FFPROBE_CHECKS=0 "${SCRIPT}" run-all
}

run_test
echo "[test] cross_protocol_matrix_regression_test passed"
