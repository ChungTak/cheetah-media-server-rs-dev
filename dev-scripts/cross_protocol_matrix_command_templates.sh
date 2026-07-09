#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CHEETAH_HOST="${CHEETAH_HOST:-127.0.0.1}"
RTSP_PORT="${RTSP_PORT:-8554}"
RTMP_PORT="${RTMP_PORT:-1935}"
APP_NAME="${APP_NAME:-live}"
STREAM_NAME="${STREAM_NAME:-test}"
INPUT_FILE="${INPUT_FILE:-}"
INPUT_PROFILE="${INPUT_PROFILE:-b-frame-h264}"
MATRIX_INPUT_FILE="${MATRIX_INPUT_FILE:-${ROOT_DIR}/dev-scripts/cross_protocol_matrix_input_matrix.tsv}"
MATRIX_ACCEPTANCE_FILE="${MATRIX_ACCEPTANCE_FILE:-${ROOT_DIR}/dev-scripts/cross_protocol_matrix_acceptance_matrix.tsv}"
FFMPEG_BIN="${FFMPEG_BIN:-ffmpeg}"
FFPLAY_BIN="${FFPLAY_BIN:-ffplay}"
FFMPEG_LOG_LEVEL="${FFMPEG_LOG_LEVEL:-debug}"
FFPLAY_LOG_LEVEL="${FFPLAY_LOG_LEVEL:-debug}"
# Extra flags / sink for the pull command. For headless ffmpeg sink mode set:
#   FFPLAY_BIN=ffmpeg FFPLAY_PULL_EXTRA_FLAGS=-debug_ts FFPLAY_PULL_SINK="-f null -"
FFPLAY_PULL_EXTRA_FLAGS="${FFPLAY_PULL_EXTRA_FLAGS:-}"
FFPLAY_PULL_SINK="${FFPLAY_PULL_SINK:-}"

log() {
  echo "[cross-protocol-matrix] $*"
}

fail() {
  log "error: $*" >&2
  exit 1
}

usage() {
  cat <<USAGE
Usage:
  ${SCRIPT_NAME} list
  ${SCRIPT_NAME} show <scenario>
  ${SCRIPT_NAME} show-all
  ${SCRIPT_NAME} doctor
  ${SCRIPT_NAME} list-inputs
  ${SCRIPT_NAME} show-input <profile>
  ${SCRIPT_NAME} doctor-inputs
  ${SCRIPT_NAME} list-acceptance
  ${SCRIPT_NAME} show-acceptance
  ${SCRIPT_NAME} doctor-acceptance

Scenarios:
  rtsp-tcp-loopback
  rtsp-udp-loopback
  rtsp-tcp-to-rtsp-udp
  rtsp-udp-to-rtsp-tcp
  rtmp-loopback
  bridge-rtsp-tcp-to-rtmp
  bridge-rtsp-udp-to-rtmp
  bridge-rtmp-to-rtsp-tcp
  bridge-rtmp-to-rtsp-udp

Environment overrides:
  CHEETAH_HOST, RTSP_PORT, RTMP_PORT, APP_NAME, STREAM_NAME, INPUT_FILE,
  INPUT_PROFILE, MATRIX_INPUT_FILE, MATRIX_ACCEPTANCE_FILE, FFMPEG_BIN, FFPLAY_BIN,
  FFMPEG_LOG_LEVEL, FFPLAY_LOG_LEVEL
USAGE
}

list_scenarios() {
  cat <<'LIST'
rtsp-tcp-loopback
rtsp-udp-loopback
rtsp-tcp-to-rtsp-udp
rtsp-udp-to-rtsp-tcp
rtmp-loopback
bridge-rtsp-tcp-to-rtmp
bridge-rtsp-udp-to-rtmp
bridge-rtmp-to-rtsp-tcp
bridge-rtmp-to-rtsp-udp
LIST
}

rtsp_url() {
  echo "rtsp://${CHEETAH_HOST}:${RTSP_PORT}/${APP_NAME}/${STREAM_NAME}"
}

rtmp_url() {
  echo "rtmp://${CHEETAH_HOST}:${RTMP_PORT}/${APP_NAME}/${STREAM_NAME}"
}

iter_matrix_records() {
  [[ -f "${MATRIX_INPUT_FILE}" ]] || fail "matrix input file not found: ${MATRIX_INPUT_FILE}"

  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ -n "${line}" ]] || continue
    [[ "${line}" == \#* ]] && continue
    echo "${line}"
  done < "${MATRIX_INPUT_FILE}"
}

iter_acceptance_records() {
  [[ -f "${MATRIX_ACCEPTANCE_FILE}" ]] || fail "matrix acceptance file not found: ${MATRIX_ACCEPTANCE_FILE}"

  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ -n "${line}" ]] || continue
    [[ "${line}" == \#* ]] && continue
    echo "${line}"
  done < "${MATRIX_ACCEPTANCE_FILE}"
}

resolve_matrix_path() {
  local matrix_path="$1"
  if [[ "${matrix_path}" = /* ]]; then
    echo "${matrix_path}"
    return 0
  fi

  echo "${ROOT_DIR}/${matrix_path}"
}

resolve_input_file() {
  if [[ -n "${INPUT_FILE}" ]]; then
    echo "${INPUT_FILE}"
    return 0
  fi

  local record
  while IFS= read -r record; do
    IFS='|' read -r profile _b_frames relative_path _description <<< "${record}"
    if [[ "${profile}" == "${INPUT_PROFILE}" ]]; then
      resolve_matrix_path "${relative_path}"
      return 0
    fi
  done < <(iter_matrix_records)

  fail "input profile '${INPUT_PROFILE}' not found in matrix: ${MATRIX_INPUT_FILE}"
}

list_inputs() {
  local record
  while IFS= read -r record; do
    IFS='|' read -r profile b_frames relative_path description <<< "${record}"
    [[ -n "${profile}" && -n "${b_frames}" && -n "${relative_path}" ]] || fail "invalid matrix row: ${record}"
    local resolved_path
    resolved_path="$(resolve_matrix_path "${relative_path}")"
    echo "${profile}|b_frames=${b_frames}|path=${resolved_path}|description=${description}"
  done < <(iter_matrix_records)
}

show_input() {
  local target_profile="$1"
  local record

  while IFS= read -r record; do
    IFS='|' read -r profile b_frames relative_path description <<< "${record}"
    [[ -n "${profile}" && -n "${b_frames}" && -n "${relative_path}" ]] || fail "invalid matrix row: ${record}"
    if [[ "${profile}" == "${target_profile}" ]]; then
      local resolved_path
      resolved_path="$(resolve_matrix_path "${relative_path}")"
      echo "profile: ${profile}"
      echo "b_frames: ${b_frames}"
      echo "path: ${resolved_path}"
      echo "description: ${description}"
      return 0
    fi
  done < <(iter_matrix_records)

  fail "input profile '${target_profile}' not found in matrix: ${MATRIX_INPUT_FILE}"
}

doctor_inputs() {
  local record_count=0
  local b_frame_count=0
  local non_b_frame_count=0
  local record

  while IFS= read -r record; do
    IFS='|' read -r profile b_frames relative_path description <<< "${record}"
    [[ -n "${profile}" && -n "${b_frames}" && -n "${relative_path}" ]] || fail "invalid matrix row: ${record}"
    [[ -n "${description}" ]] || fail "empty description in matrix row: ${record}"

    case "${b_frames}" in
      yes)
        b_frame_count=$((b_frame_count + 1))
        ;;
      no)
        non_b_frame_count=$((non_b_frame_count + 1))
        ;;
      *)
        fail "invalid b_frames flag '${b_frames}' for profile '${profile}', expected 'yes' or 'no'"
        ;;
    esac

    local resolved_path
    resolved_path="$(resolve_matrix_path "${relative_path}")"
    [[ -f "${resolved_path}" ]] || fail "matrix input profile '${profile}' file not found: ${resolved_path}"
    record_count=$((record_count + 1))
  done < <(iter_matrix_records)

  (( record_count > 0 )) || fail "matrix input file has no data rows: ${MATRIX_INPUT_FILE}"
  (( b_frame_count > 0 )) || fail "matrix input file must contain at least one b_frames=yes sample"
  (( non_b_frame_count > 0 )) || fail "matrix input file must contain at least one b_frames=no sample"

  log "matrix input file: ${MATRIX_INPUT_FILE}"
  log "matrix profiles: ${record_count} (b_frames=yes: ${b_frame_count}, b_frames=no: ${non_b_frame_count})"
  log "doctor-inputs check passed"
}

acceptance_threshold_operator() {
  local check_key="$1"

  case "${check_key}" in
    startup_latency)
      echo "<="
      ;;
    first_keyframe_delay_ms)
      echo "<="
      ;;
    continuous_play)
      echo ">="
      ;;
    freeze_events|dts_out_of_order|invalid_timestamps|non_increasing_dts|negative_cts)
      echo "=="
      ;;
    ffprobe_first_video_keyframe|ffprobe_first_video_pts_near_zero|ffprobe_video_dts_monotonic)
      echo "=="
      ;;
    source_repair_events|canonical_repair_events|egress_repair_events|repair_warn_high_frequency|repair_context_complete)
      echo "=="
      ;;
    *)
      fail "unknown acceptance check key '${check_key}'"
      ;;
  esac
}

acceptance_threshold_display() {
  local check_key="$1"
  local threshold="$2"
  local unit="$3"
  local operator
  operator="$(acceptance_threshold_operator "${check_key}")"
  echo "${operator} ${threshold} ${unit}"
}

list_acceptance() {
  local record
  while IFS= read -r record; do
    IFS='|' read -r check_key required threshold unit description <<< "${record}"
    [[ -n "${check_key}" && -n "${required}" && -n "${threshold}" && -n "${unit}" ]] || fail "invalid acceptance row: ${record}"
    [[ -n "${description}" ]] || fail "empty description in acceptance row: ${record}"
    local threshold_display
    threshold_display="$(acceptance_threshold_display "${check_key}" "${threshold}" "${unit}")"
    echo "${check_key}|required=${required}|threshold=${threshold_display}|description=${description}"
  done < <(iter_acceptance_records)
}

show_acceptance() {
  local record
  while IFS= read -r record; do
    IFS='|' read -r check_key required threshold unit description <<< "${record}"
    [[ -n "${check_key}" && -n "${required}" && -n "${threshold}" && -n "${unit}" ]] || fail "invalid acceptance row: ${record}"
    [[ -n "${description}" ]] || fail "empty description in acceptance row: ${record}"
    echo "check: ${check_key}"
    echo "required: ${required}"
    echo "threshold: $(acceptance_threshold_display "${check_key}" "${threshold}" "${unit}")"
    echo "description: ${description}"
    echo
  done < <(iter_acceptance_records)
}

doctor_acceptance() {
  local record_count=0
  local startup_latency_count=0
  local first_keyframe_delay_ms_count=0
  local continuous_play_count=0
  local freeze_events_count=0
  local dts_out_of_order_count=0
  local invalid_timestamps_count=0
  local non_increasing_dts_count=0
  local negative_cts_count=0
  local ffprobe_first_video_keyframe_count=0
  local ffprobe_first_video_pts_near_zero_count=0
  local ffprobe_video_dts_monotonic_count=0
  local source_repair_events_count=0
  local canonical_repair_events_count=0
  local egress_repair_events_count=0
  local repair_warn_high_frequency_count=0
  local repair_context_complete_count=0
  local record

  while IFS= read -r record; do
    IFS='|' read -r check_key required threshold unit description <<< "${record}"
    [[ -n "${check_key}" && -n "${required}" && -n "${threshold}" && -n "${unit}" ]] || fail "invalid acceptance row: ${record}"
    [[ -n "${description}" ]] || fail "empty description in acceptance row: ${record}"
    [[ "${threshold}" =~ ^[0-9]+$ ]] || fail "invalid threshold '${threshold}' for acceptance check '${check_key}', expected non-negative integer"

    case "${required}" in
      yes|no)
        ;;
      *)
        fail "invalid required flag '${required}' for acceptance check '${check_key}', expected 'yes' or 'no'"
        ;;
    esac

    case "${check_key}" in
      startup_latency)
        [[ "${required}" == "yes" ]] || fail "startup_latency check must set required=yes"
        [[ "${unit}" == "ms" ]] || fail "startup_latency unit must be 'ms', got '${unit}'"
        startup_latency_count=$((startup_latency_count + 1))
        ;;
      first_keyframe_delay_ms)
        [[ "${required}" == "yes" ]] || fail "first_keyframe_delay_ms check must set required=yes"
        [[ "${unit}" == "ms" ]] || fail "first_keyframe_delay_ms unit must be 'ms', got '${unit}'"
        first_keyframe_delay_ms_count=$((first_keyframe_delay_ms_count + 1))
        ;;
      continuous_play)
        [[ "${required}" == "yes" ]] || fail "continuous_play check must set required=yes"
        [[ "${unit}" == "seconds" ]] || fail "continuous_play unit must be 'seconds', got '${unit}'"
        continuous_play_count=$((continuous_play_count + 1))
        ;;
      freeze_events)
        [[ "${required}" == "yes" ]] || fail "freeze_events check must set required=yes"
        [[ "${unit}" == "count" ]] || fail "freeze_events unit must be 'count', got '${unit}'"
        freeze_events_count=$((freeze_events_count + 1))
        ;;
      dts_out_of_order)
        [[ "${required}" == "yes" ]] || fail "dts_out_of_order check must set required=yes"
        [[ "${unit}" == "count" ]] || fail "dts_out_of_order unit must be 'count', got '${unit}'"
        dts_out_of_order_count=$((dts_out_of_order_count + 1))
        ;;
      invalid_timestamps)
        [[ "${required}" == "yes" ]] || fail "invalid_timestamps check must set required=yes"
        [[ "${unit}" == "count" ]] || fail "invalid_timestamps unit must be 'count', got '${unit}'"
        invalid_timestamps_count=$((invalid_timestamps_count + 1))
        ;;
      non_increasing_dts)
        [[ "${required}" == "yes" ]] || fail "non_increasing_dts check must set required=yes"
        [[ "${unit}" == "count" ]] || fail "non_increasing_dts unit must be 'count', got '${unit}'"
        non_increasing_dts_count=$((non_increasing_dts_count + 1))
        ;;
      negative_cts)
        [[ "${required}" == "yes" ]] || fail "negative_cts check must set required=yes"
        [[ "${unit}" == "count" ]] || fail "negative_cts unit must be 'count', got '${unit}'"
        negative_cts_count=$((negative_cts_count + 1))
        ;;
      ffprobe_first_video_keyframe)
        [[ "${required}" == "yes" ]] || fail "ffprobe_first_video_keyframe check must set required=yes"
        [[ "${unit}" == "bool" ]] || fail "ffprobe_first_video_keyframe unit must be 'bool', got '${unit}'"
        ffprobe_first_video_keyframe_count=$((ffprobe_first_video_keyframe_count + 1))
        ;;
      ffprobe_first_video_pts_near_zero)
        [[ "${required}" == "yes" ]] || fail "ffprobe_first_video_pts_near_zero check must set required=yes"
        [[ "${unit}" == "bool" ]] || fail "ffprobe_first_video_pts_near_zero unit must be 'bool', got '${unit}'"
        ffprobe_first_video_pts_near_zero_count=$((ffprobe_first_video_pts_near_zero_count + 1))
        ;;
      ffprobe_video_dts_monotonic)
        [[ "${required}" == "yes" ]] || fail "ffprobe_video_dts_monotonic check must set required=yes"
        [[ "${unit}" == "bool" ]] || fail "ffprobe_video_dts_monotonic unit must be 'bool', got '${unit}'"
        ffprobe_video_dts_monotonic_count=$((ffprobe_video_dts_monotonic_count + 1))
        ;;
      source_repair_events)
        [[ "${unit}" == "count" ]] || fail "source_repair_events unit must be 'count', got '${unit}'"
        source_repair_events_count=$((source_repair_events_count + 1))
        ;;
      canonical_repair_events)
        [[ "${unit}" == "count" ]] || fail "canonical_repair_events unit must be 'count', got '${unit}'"
        canonical_repair_events_count=$((canonical_repair_events_count + 1))
        ;;
      egress_repair_events)
        [[ "${unit}" == "count" ]] || fail "egress_repair_events unit must be 'count', got '${unit}'"
        egress_repair_events_count=$((egress_repair_events_count + 1))
        ;;
      repair_warn_high_frequency)
        [[ "${required}" == "yes" ]] || fail "repair_warn_high_frequency check must set required=yes"
        [[ "${unit}" == "bool" ]] || fail "repair_warn_high_frequency unit must be 'bool', got '${unit}'"
        repair_warn_high_frequency_count=$((repair_warn_high_frequency_count + 1))
        ;;
      repair_context_complete)
        [[ "${required}" == "yes" ]] || fail "repair_context_complete check must set required=yes"
        [[ "${unit}" == "bool" ]] || fail "repair_context_complete unit must be 'bool', got '${unit}'"
        repair_context_complete_count=$((repair_context_complete_count + 1))
        ;;
      *)
        fail "unsupported acceptance check key '${check_key}'"
        ;;
    esac

    record_count=$((record_count + 1))
  done < <(iter_acceptance_records)

  (( record_count > 0 )) || fail "matrix acceptance file has no data rows: ${MATRIX_ACCEPTANCE_FILE}"
  (( startup_latency_count > 0 )) || fail "matrix acceptance file must contain startup_latency check"
  (( first_keyframe_delay_ms_count > 0 )) || fail "matrix acceptance file must contain first_keyframe_delay_ms check"
  (( continuous_play_count > 0 )) || fail "matrix acceptance file must contain continuous_play check"
  (( freeze_events_count > 0 )) || fail "matrix acceptance file must contain freeze_events check"
  (( dts_out_of_order_count > 0 )) || fail "matrix acceptance file must contain dts_out_of_order check"
  (( invalid_timestamps_count > 0 )) || fail "matrix acceptance file must contain invalid_timestamps check"
  (( non_increasing_dts_count > 0 )) || fail "matrix acceptance file must contain non_increasing_dts check"
  (( negative_cts_count > 0 )) || fail "matrix acceptance file must contain negative_cts check"
  (( ffprobe_first_video_keyframe_count > 0 )) || fail "matrix acceptance file must contain ffprobe_first_video_keyframe check"
  (( ffprobe_first_video_pts_near_zero_count > 0 )) || fail "matrix acceptance file must contain ffprobe_first_video_pts_near_zero check"
  (( ffprobe_video_dts_monotonic_count > 0 )) || fail "matrix acceptance file must contain ffprobe_video_dts_monotonic check"
  (( source_repair_events_count > 0 )) || fail "matrix acceptance file must contain source_repair_events check"
  (( canonical_repair_events_count > 0 )) || fail "matrix acceptance file must contain canonical_repair_events check"
  (( egress_repair_events_count > 0 )) || fail "matrix acceptance file must contain egress_repair_events check"
  (( repair_warn_high_frequency_count > 0 )) || fail "matrix acceptance file must contain repair_warn_high_frequency check"
  (( repair_context_complete_count > 0 )) || fail "matrix acceptance file must contain repair_context_complete check"

  log "matrix acceptance file: ${MATRIX_ACCEPTANCE_FILE}"
  log "matrix acceptance checks: ${record_count}"
  log "doctor-acceptance check passed"
}

print_header() {
  local scenario="$1"
  echo "### ${scenario}"
  echo "# Terminal A (push):"
}

show_scenario() {
  local scenario="$1"
  local rtsp
  local rtmp
  local selected_input_file

  rtsp="$(rtsp_url)"
  rtmp="$(rtmp_url)"
  selected_input_file="$(resolve_input_file)"

  case "${scenario}" in
    rtsp-tcp-loopback)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport tcp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport tcp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    rtsp-udp-loopback)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport udp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport udp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    rtsp-tcp-to-rtsp-udp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport tcp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport udp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    rtsp-udp-to-rtsp-tcp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport udp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport tcp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    rtmp-loopback)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -f flv ${rtmp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -i ${rtmp} ${FFPLAY_PULL_SINK}"
      ;;
    bridge-rtsp-tcp-to-rtmp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport tcp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -i ${rtmp} ${FFPLAY_PULL_SINK}"
      ;;
    bridge-rtsp-udp-to-rtmp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -rtsp_transport udp -f rtsp ${rtsp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -i ${rtmp} ${FFPLAY_PULL_SINK}"
      ;;
    bridge-rtmp-to-rtsp-tcp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -f flv ${rtmp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport tcp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    bridge-rtmp-to-rtsp-udp)
      print_header "${scenario}"
      echo "${FFMPEG_BIN} -v ${FFMPEG_LOG_LEVEL} -debug_ts -re -stream_loop -1 -i \"${selected_input_file}\" -c copy -f flv ${rtmp}"
      echo "# Terminal B (pull):"
      echo "${FFPLAY_BIN} -v ${FFPLAY_LOG_LEVEL} ${FFPLAY_PULL_EXTRA_FLAGS} -stats -fflags nobuffer -flags low_delay -rtsp_transport udp -i ${rtsp} ${FFPLAY_PULL_SINK}"
      ;;
    *)
      fail "unknown scenario '${scenario}'. Run '${SCRIPT_NAME} list' to inspect supported scenarios."
      ;;
  esac
}

doctor() {
  doctor_inputs
  doctor_acceptance

  command -v "${FFMPEG_BIN}" >/dev/null 2>&1 || fail "missing binary: ${FFMPEG_BIN}"
  command -v "${FFPLAY_BIN}" >/dev/null 2>&1 || fail "missing binary: ${FFPLAY_BIN}"

  local selected_input_file
  selected_input_file="$(resolve_input_file)"
  [[ -f "${selected_input_file}" ]] || fail "selected input file not found: ${selected_input_file}"

  log "ffmpeg binary: ${FFMPEG_BIN}"
  log "ffplay binary: ${FFPLAY_BIN}"
  log "ffmpeg log level: ${FFMPEG_LOG_LEVEL}"
  log "ffplay log level: ${FFPLAY_LOG_LEVEL}"
  if [[ -n "${INPUT_FILE}" ]]; then
    log "input file mode: direct (INPUT_FILE override)"
  else
    log "input file mode: matrix profile (${INPUT_PROFILE})"
  fi
  log "input file: ${selected_input_file}"
  log "rtsp url: $(rtsp_url)"
  log "rtmp url: $(rtmp_url)"
  log "doctor check passed"
}

main() {
  local command="${1:-}"

  case "${command}" in
    list)
      list_scenarios
      ;;
    show)
      [[ $# -eq 2 ]] || fail "'show' requires one scenario argument"
      show_scenario "$2"
      ;;
    show-all)
      [[ $# -eq 1 ]] || fail "'show-all' takes no extra arguments"
      while IFS= read -r scenario; do
        show_scenario "${scenario}"
        echo
      done < <(list_scenarios)
      ;;
    doctor)
      [[ $# -eq 1 ]] || fail "'doctor' takes no extra arguments"
      doctor
      ;;
    list-inputs)
      [[ $# -eq 1 ]] || fail "'list-inputs' takes no extra arguments"
      list_inputs
      ;;
    show-input)
      [[ $# -eq 2 ]] || fail "'show-input' requires one profile argument"
      show_input "$2"
      ;;
    doctor-inputs)
      [[ $# -eq 1 ]] || fail "'doctor-inputs' takes no extra arguments"
      doctor_inputs
      ;;
    list-acceptance)
      [[ $# -eq 1 ]] || fail "'list-acceptance' takes no extra arguments"
      list_acceptance
      ;;
    show-acceptance)
      [[ $# -eq 1 ]] || fail "'show-acceptance' takes no extra arguments"
      show_acceptance
      ;;
    doctor-acceptance)
      [[ $# -eq 1 ]] || fail "'doctor-acceptance' takes no extra arguments"
      doctor_acceptance
      ;;
    -h|--help|help)
      usage
      ;;
    "")
      usage
      exit 1
      ;;
    *)
      fail "unknown command '${command}'. Run '${SCRIPT_NAME} --help' for usage."
      ;;
  esac
}

main "$@"
