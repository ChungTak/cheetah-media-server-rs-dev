#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="${ROOT_DIR}/dev-scripts/cross_protocol_matrix_command_templates.sh"

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
  if out="$("$@" 2>&1)"; then
    fail "expected command to fail, but succeeded: $*"
  fi

  assert_contains "${out}" "${expected}"
}

run_test() {
  local temp_dir
  temp_dir="$(mktemp -d)"
  trap 'rm -rf "${temp_dir}"' RETURN

  local non_b_file="${temp_dir}/non-b.flv"
  local b_file="${temp_dir}/b-frame.flv"
  : > "${non_b_file}"
  : > "${b_file}"

  local matrix_file="${temp_dir}/inputs.tsv"
  cat > "${matrix_file}" <<EOF
# profile|b_frames|relative_path|description
non-b-h264|no|${non_b_file}|h264 baseline style sample
b-frame-h264|yes|${b_file}|h264 b-frame style sample
EOF
  local acceptance_file="${temp_dir}/acceptance.tsv"
  cat > "${acceptance_file}" <<'EOF'
# check_key|required|threshold|unit|description
startup_latency|yes|3000|ms|first frame latency must stay within 3 seconds
first_keyframe_delay_ms|yes|2000|ms|first keyframe delay must stay within 2 seconds
continuous_play|yes|300|seconds|continuous playback must last at least 5 minutes
freeze_events|yes|0|count|playback freeze events must be zero
dts_out_of_order|yes|0|count|ffmpeg logs must not contain DTS out of order
invalid_timestamps|yes|0|count|ffmpeg logs must not contain invalid timestamps
non_increasing_dts|yes|0|count|ffmpeg logs must not contain non-increasing dts
negative_cts|yes|0|count|ffmpeg logs must not contain negative cts
ffprobe_first_video_keyframe|yes|1|bool|ffprobe first video packet must be keyframe
ffprobe_first_video_pts_near_zero|yes|1|bool|ffprobe first video packet pts should be near zero
ffprobe_video_dts_monotonic|yes|1|bool|ffprobe sampled video dts should be monotonic
source_repair_events|no|0|count|source repair count tracking only
canonical_repair_events|no|0|count|canonical repair count tracking only
egress_repair_events|no|0|count|egress repair count tracking only
repair_warn_high_frequency|yes|0|bool|canonical and egress repair warns should not be high frequency
repair_context_complete|yes|1|bool|repair logs must include source and canonical context
EOF

  local out
  out="$(MATRIX_INPUT_FILE="${matrix_file}" "${SCRIPT}" list-inputs)"
  assert_contains "${out}" "non-b-h264"
  assert_contains "${out}" "b-frame-h264"

  out="$("${SCRIPT}" list)"
  assert_contains "${out}" "rtsp-tcp-to-rtsp-udp"
  assert_contains "${out}" "rtsp-udp-to-rtsp-tcp"

  out="$(MATRIX_INPUT_FILE="${matrix_file}" INPUT_PROFILE="non-b-h264" "${SCRIPT}" show rtsp-tcp-to-rtsp-udp)"
  assert_contains "${out}" "-rtsp_transport tcp -f rtsp"
  assert_contains "${out}" "-rtsp_transport udp rtsp://"

  out="$(MATRIX_INPUT_FILE="${matrix_file}" "${SCRIPT}" show-input b-frame-h264)"
  assert_contains "${out}" "${b_file}"
  assert_contains "${out}" "b_frames: yes"

  out="$(MATRIX_INPUT_FILE="${matrix_file}" INPUT_PROFILE="b-frame-h264" "${SCRIPT}" show rtmp-loopback)"
  assert_contains "${out}" "${b_file}"

  out="$(MATRIX_INPUT_FILE="${matrix_file}" "${SCRIPT}" doctor-inputs)"
  assert_contains "${out}" "doctor-inputs check passed"

  out="$(MATRIX_ACCEPTANCE_FILE="${acceptance_file}" "${SCRIPT}" list-acceptance)"
  assert_contains "${out}" "startup_latency"
  assert_contains "${out}" "dts_out_of_order"
  assert_contains "${out}" "invalid_timestamps"

  out="$(MATRIX_ACCEPTANCE_FILE="${acceptance_file}" "${SCRIPT}" show-acceptance)"
  assert_contains "${out}" "threshold: <= 3000 ms"
  assert_contains "${out}" "threshold: == 0 count"

  out="$(MATRIX_INPUT_FILE="${matrix_file}" MATRIX_ACCEPTANCE_FILE="${acceptance_file}" "${SCRIPT}" doctor)"
  assert_contains "${out}" "doctor-acceptance check passed"
  assert_contains "${out}" "doctor check passed"

  local invalid_acceptance_file="${temp_dir}/invalid_acceptance.tsv"
  cat > "${invalid_acceptance_file}" <<'EOF'
# check_key|required|threshold|unit|description
startup_latency|yes|3000|ms|ok
continuous_play|yes|300|seconds|ok
freeze_events|yes|0|count|ok
EOF
  assert_fails_with "must contain first_keyframe_delay_ms check" \
    env MATRIX_ACCEPTANCE_FILE="${invalid_acceptance_file}" "${SCRIPT}" doctor-acceptance
}

run_test
echo "[test] cross_protocol_matrix_command_templates_test passed"
